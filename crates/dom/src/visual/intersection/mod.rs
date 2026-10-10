//! W3C [Intersection Observer](https://w3c.github.io/IntersectionObserver/)
//! as an engine primitive: the observers a document holds, the spec's
//! "update intersection observations" steps over them, and the geometry
//! those steps read.
//!
//! The rulings this primitive is built on:
//! - **Transforms are included**, as browsers include them: a box moved out of the viewport by a
//!   `transform` is not intersecting. This is deliberately not [`Document::bounding_client_rect`],
//!   which stays transform-free (Lynx's own engine-side conversion).
//! - **Intersection is Chromium's**: edge-inclusive at every clipping ancestor on the target's
//!   containing-block chain and at the root, not the spec's literal "target rect against root
//!   bounds".
//! - **`rootMargin` percentages** resolve top/bottom against the undilated root rect's height and
//!   left/right against its width, as Chromium, `WebKit` and Gecko do (WPT `root-margin.html`),
//!   where the spec's sentence says width for all four.
//! - **Rust only.** No JavaScript surface exists yet. A W3C-shaped MTS constructor and a
//!   Lynx-shaped `lynx.createIntersectionObserver` are adapters over this primitive, not part of
//!   it.
//! - **A handler per observer, as a trait object.** Every observer carries a `Box<dyn
//!   IntersectionEventHandler<T>>`, and `dom` runs §3.2.5's whole notify loop, call included. `dyn`
//!   is the user's ruling (2026-10-10), chosen for simplicity: one trait object per observer, so
//!   one document holds observers of any number of handler types — an engine component's
//!   [`ElementHandler`] beside a realm binding's — and no host type is threaded through
//!   `Document<T>`. The cost is one `Box` per observer and one indirect call per delivery.
//!
//! # Geometry
//!
//! [`Document::root_geometry`] and [`Document::intersection_geometry_in`]
//! compute what the update steps need of one target against one root — its
//! bounding box, the root intersection rectangle, the intersection rectangle
//! and whether the two intersect — from the last completed layout and the
//! live scroll offsets, with no pass run. The `geometry` submodule's doc is
//! the algorithm and every choice it makes.
//!
//! # Observers and their handlers
//!
//! An observer is created with [`Document::create_intersection_observer`]
//! for one handler, one root (the viewport, or an element), one root margin
//! and a sorted threshold list, and named by an [`IntersectionObserverId`]
//! that is never reissued. What it observes is a list of targets in observe
//! order, each with the spec's previous threshold index and intersecting
//! state; what it has to say is a queue of [`IntersectionObserverEntry`]
//! records. [`Document::intersection_observer`] reads it back as an
//! [`IntersectionObserver`].
//!
//! The handler decides who hears the queue and how long the observer lives:
//! - [`ElementHandler`] is an engine component's: it calls the element's
//!   [`CustomElement::intersections_changed`] in its own `[CEReactions]` scope, and it is [bound
//!   to](IntersectionEventHandler::bound_to) the element, so the observer goes when the element is
//!   freed.
//! - A realm's observer — the MTS `IntersectionObserver` binding's, once it exists — cannot call
//!   the realm from inside this loop, because the realm is not the document's to lend. Its handler
//!   queues `(observer, entries)` on a queue of the host's, and the host drains that queue into the
//!   realm after the loop returns. Bound to no node, it lives until
//!   [`Document::drop_intersection_observer`].
//!
//! # The update and the delivery
//!
//! [`Document::update_intersection_observations`] is §3.2.10 for every
//! observer in creation order: one [`RootGeometry`] per observer, one
//! [`IntersectionGeometry`] per target, and an entry queued wherever the
//! pair (threshold index, intersecting) moved. It invokes nothing.
//! [`Document::notify_intersection_observers`] is §3.2.5, the whole loop:
//! the host calls it from a task of its own, and each observer with entries
//! queued, in creation order, has its queue handed to its handler. The list
//! never leaves the document while a handler runs, so a handler may mutate
//! the tree and create, observe through, disconnect or drop any observer,
//! its own included; each observer is looked up again by id when its turn
//! comes, and its handler is out of it only while its own notification
//! runs. A loop run from inside a notification skips the observer whose
//! handler is out and leaves its new entries queued for the next loop.
//!
//! The update runs only when something could have moved an observation. A
//! **stale bit** is set by every render that built a frame (viewport and
//! device-pixel-ratio changes, mutations, animation ticks — anything that
//! moves layout goes through one), by every scroll that moved an offset —
//! whether or not it commits anything, because a scroll inside the encode
//! window does not — by every observe, and by an element root's free. Only
//! an update that ran clears it, and none runs before the first render: a
//! document that has never committed a frame has no geometry to read.
//!
//! # Node lifetime
//!
//! `Document::free_node` — the one place an id retires — tells the
//! registry: observers whose handler is bound to the node are dropped,
//! handler and all (one whose notification is running loses its handler
//! when that returns), the node leaves every target list, queued entries
//! naming it are dropped (so every entry handed out names a live node), and
//! a root that was the node turns `Freed`, whose targets report one leave
//! entry and then nothing. A node that is unlinked but alive stays observed:
//! the unlink dirties layout, the render that follows sets the stale bit,
//! and the target, no longer rendered, reports leaving.
//!
//! # Cost
//!
//! A document with no observers pays one `is_empty` test per freed node and
//! one `bool` store per render and per moved scroll; an update with nothing
//! stale is two tests. An observer costs one `Box` for its handler, and a
//! delivery one indirect call.
//!
//! [`CustomElement::intersections_changed`]: crate::CustomElement::intersections_changed

mod geometry;

use std::num::NonZeroU32;

use euclid::default::Rect;

pub use self::geometry::{IntersectionGeometry, MarginLength, RootGeometry, RootMargin};
use crate::tree::document::{Document, NodeId};

/// Names one intersection observer for as long as it exists. Ids are
/// monotone per document and never reissued, so an id that outlived its
/// observer names nothing rather than another one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct IntersectionObserverId(NonZeroU32);

impl IntersectionObserverId {
    /// The id as a number, for an embedder that hands it across a boundary.
    #[must_use]
    pub const fn get(self) -> u32 {
        self.0.get()
    }
}

/// One `IntersectionObserverEntry`: a target's geometry at the update that
/// queued it. Every rect is in viewport CSS px.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct IntersectionObserverEntry {
    /// The time the update was run with, in the host's clock.
    pub time: f64,
    /// The root intersection rectangle. Zero when the target or an element
    /// root is not rendered.
    pub root_bounds: Rect<f32>,
    /// The bounding box of the target's transformed border box. Zero when it
    /// is not rendered or not a descendant of the root.
    pub bounding_client_rect: Rect<f32>,
    /// What of the target is visible through every clip and the root.
    pub intersection_rect: Rect<f32>,
    pub is_intersecting: bool,
    pub intersection_ratio: f64,
    pub target: NodeId,
}

/// What an observer delivers its queued entries to.
///
/// Every observer holds its own, as a `Box<dyn IntersectionEventHandler<T>>`,
/// so one document holds observers of any number of handler types and
/// [`Document::notify_intersection_observers`] calls each through one
/// indirect call. Nothing of `T` is asked for: the handler is `'static`,
/// and the document is lent to it for the call.
pub trait IntersectionEventHandler<T> {
    /// §3.2.5's call. `document` is the one the observer belongs to; the
    /// handler may observe, unobserve, disconnect, create or drop observers
    /// (its own included) and mutate the tree.
    ///
    /// `entries` are in the order the updates queued them — by update, then
    /// by observe order within one — and never empty. While this runs, its
    /// own observer's [`IntersectionObserver::handler`] is `None`, and a
    /// loop run from inside it leaves that observer's new entries queued.
    fn notify(
        &mut self,
        document: &mut Document<T>,
        observer: IntersectionObserverId,
        entries: Vec<IntersectionObserverEntry>,
    );

    /// The node this handler lives with: the observer is dropped when that
    /// node is freed. `None` → it lives until
    /// [`Document::drop_intersection_observer`]. Asked once, when the
    /// observer is created.
    fn bound_to(&self) -> Option<NodeId> {
        None
    }
}

/// `dom`'s handler for an engine component: the element's
/// [`CustomElement::intersections_changed`] hook in one `[CEReactions]`
/// scope, as an event dispatch wraps each handler — what the hook's
/// mutations raise runs before the notification returns. Bound to that
/// element, so its observer goes when the element is freed.
///
/// An element that is not a constructed custom element hears nothing: the
/// entries are dropped. So are they for a freed one, which only a direct
/// call can name, since its free dropped the observer.
///
/// [`CustomElement::intersections_changed`]: crate::CustomElement::intersections_changed
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ElementHandler(pub NodeId);

impl<T> IntersectionEventHandler<T> for ElementHandler {
    fn notify(
        &mut self,
        document: &mut Document<T>,
        observer: IntersectionObserverId,
        entries: Vec<IntersectionObserverEntry>,
    ) {
        let element = self.0;
        let Some(hook) = document.custom_element_handler(element) else {
            return;
        };
        let base = document.begin_reactions();
        hook.intersections_changed(document, element, observer, entries);
        document.drain_reactions(base);
    }

    fn bound_to(&self) -> Option<NodeId> {
        Some(self.0)
    }
}

/// An intersection observer's root.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntersectionObserverRoot {
    /// The implicit root: the viewport.
    Implicit,
    /// An element root.
    Element(NodeId),
    /// An element root that was freed: every target reports not rendered.
    Freed,
}

/// One observed target.
#[derive(Debug)]
struct Registration {
    target: NodeId,
    /// The spec's `previousThresholdIndex` and `previousIsIntersecting`;
    /// `None` is its initial `-1` and `false`, which no update produces, so
    /// the first update after observe always queues.
    previous: Option<(usize, bool)>,
}

/// The W3C object, one per [`Document::create_intersection_observer`]:
/// named by its [`IntersectionObserverId`] and read through
/// [`Document::intersection_observer`]. Its targets and its queue are the
/// document's to change, through the `Document` methods the spec's calls
/// map to.
pub struct IntersectionObserver<T> {
    id: IntersectionObserverId,
    root: IntersectionObserverRoot,
    root_margin: RootMargin,
    /// Ascending, never empty.
    thresholds: Box<[f64]>,
    /// In observe order.
    targets: Vec<Registration>,
    queue: Vec<IntersectionObserverEntry>,
    /// The handler's [`IntersectionEventHandler::bound_to`], asked once at
    /// creation, so a free finds the observers it ends without asking a
    /// handler that may be out on a notification.
    bound_to: Option<NodeId>,
    /// `None` only while its own notification runs.
    handler: Option<Box<dyn IntersectionEventHandler<T>>>,
}

impl<T> IntersectionObserver<T> {
    #[must_use]
    pub const fn id(&self) -> IntersectionObserverId {
        self.id
    }

    #[must_use]
    pub const fn root(&self) -> IntersectionObserverRoot {
        self.root
    }

    #[must_use]
    pub const fn root_margin(&self) -> &RootMargin {
        &self.root_margin
    }

    /// The thresholds, ascending; the spec's `[0]` for a list created empty.
    #[must_use]
    pub fn thresholds(&self) -> &[f64] {
        &self.thresholds
    }

    /// The node whose free drops this observer, as its handler named it at
    /// creation.
    #[must_use]
    pub const fn bound_to(&self) -> Option<NodeId> {
        self.bound_to
    }

    /// The handler, or `None` while its own notification runs — it holds
    /// itself then.
    #[must_use]
    pub fn handler(&self) -> Option<&dyn IntersectionEventHandler<T>> {
        self.handler.as_deref()
    }

    /// The handler, mutably, or `None` while its own notification runs.
    #[must_use]
    pub fn handler_mut(&mut self) -> Option<&mut dyn IntersectionEventHandler<T>> {
        match &mut self.handler {
            Some(handler) => Some(&mut **handler),
            None => None,
        }
    }
}

impl<T> std::fmt::Debug for IntersectionObserver<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IntersectionObserver")
            .field("id", &self.id)
            .field("root", &self.root)
            .field("root_margin", &self.root_margin)
            .field("thresholds", &self.thresholds)
            .field("targets", &self.targets)
            .field("queue", &self.queue)
            .field("bound_to", &self.bound_to)
            .field("notifying", &self.handler.is_none())
            .finish_non_exhaustive()
    }
}

/// The document's observers, in creation order — the order the spec notifies
/// them in, and ascending by id, since ids are monotone and nothing reorders
/// the list.
pub(crate) struct IntersectionObservers<T> {
    observers: Vec<IntersectionObserver<T>>,
    next: NonZeroU32,
    /// Something may have moved an observation since the last update ran.
    stale: bool,
}

impl<T> Default for IntersectionObservers<T> {
    fn default() -> Self {
        Self {
            observers: Vec::new(),
            next: NonZeroU32::MIN,
            stale: false,
        }
    }
}

impl<T> IntersectionObservers<T> {
    pub(crate) fn is_empty(&self) -> bool {
        self.observers.is_empty()
    }

    /// Where a live observer is, if `id` still names one.
    fn position(&self, id: IntersectionObserverId) -> Option<usize> {
        self.observers
            .binary_search_by_key(&id, |observer| observer.id)
            .ok()
    }

    /// The index of a live observer. An id that names none is a caller bug,
    /// the way a stale `NodeId` passed to a mutation is.
    fn index(&self, id: IntersectionObserverId) -> usize {
        self.position(id)
            .unwrap_or_else(|| panic!("{id:?} names no live intersection observer"))
    }

    fn get_mut(&mut self, id: IntersectionObserverId) -> &mut IntersectionObserver<T> {
        let index = self.index(id);
        &mut self.observers[index]
    }

    /// `node` is being freed: no observer may outlive the node it is bound
    /// to, observe it, queue an entry naming it, or keep it as a root.
    pub(crate) fn forget_node(&mut self, node: NodeId) {
        self.observers
            .retain(|observer| observer.bound_to != Some(node));
        for observer in &mut self.observers {
            observer
                .targets
                .retain(|registration| registration.target != node);
            observer.queue.retain(|entry| entry.target != node);
            if observer.root == IntersectionObserverRoot::Element(node) {
                observer.root = IntersectionObserverRoot::Freed;
                // Its targets owe a leave entry whether or not anything
                // renders again.
                self.stale = true;
            }
        }
    }
}

impl<T> Document<T> {
    /// Creates an intersection observer that delivers to `handler`, and
    /// answers its id.
    ///
    /// `root` is the intersection root: `None` for the implicit root (the
    /// viewport), or a live element. `thresholds` are sorted ascending; an
    /// empty list is the spec's `[0]`. Creating observes nothing, so it
    /// leaves the update with nothing to do. An engine component passes
    /// `Box::new(ElementHandler(element))`.
    ///
    /// # Panics
    ///
    /// When a threshold is not a finite number in `[0, 1]`, when `root` is
    /// not a live element, or when the node the handler is
    /// [bound to](IntersectionEventHandler::bound_to) is not live — an
    /// observer bound to a node already freed could never be dropped by its
    /// free.
    pub fn create_intersection_observer(
        &mut self,
        handler: Box<dyn IntersectionEventHandler<T>>,
        root: Option<NodeId>,
        root_margin: RootMargin,
        thresholds: Vec<f64>,
    ) -> IntersectionObserverId {
        assert!(
            thresholds
                .iter()
                .all(|threshold| threshold.is_finite() && (0.0..=1.0).contains(threshold)),
            "intersection observer thresholds must be finite and in [0, 1], got {thresholds:?}"
        );
        let bound_to = handler.bound_to();
        if let Some(node) = bound_to {
            assert!(
                self.get(node).is_some(),
                "an intersection observer's handler is bound to {node:?}, which must be a live \
                 node"
            );
        }
        if let Some(root) = root {
            assert!(
                self.get(root).is_some_and(crate::Node::is_element),
                "an intersection observer's root {root:?} must be a live element"
            );
        }
        let mut thresholds = thresholds;
        thresholds.sort_by(f64::total_cmp);
        if thresholds.is_empty() {
            thresholds.push(0.0);
        }
        let intersections = &mut self.intersections;
        let id = IntersectionObserverId(intersections.next);
        intersections.next = intersections
            .next
            .checked_add(1)
            .expect("a document cannot create u32::MAX intersection observers");
        intersections.observers.push(IntersectionObserver {
            id,
            root: root.map_or(
                IntersectionObserverRoot::Implicit,
                IntersectionObserverRoot::Element,
            ),
            root_margin,
            thresholds: thresholds.into_boxed_slice(),
            targets: Vec::new(),
            queue: Vec::new(),
            bound_to,
            handler: Some(handler),
        });
        id
    }

    /// The observer `observer` names.
    ///
    /// # Panics
    ///
    /// When `observer` names no live observer.
    #[must_use]
    pub fn intersection_observer(
        &self,
        observer: IntersectionObserverId,
    ) -> &IntersectionObserver<T> {
        &self.intersections.observers[self.intersections.index(observer)]
    }

    /// The observer `observer` names, mutably — for its handler.
    ///
    /// # Panics
    ///
    /// When `observer` names no live observer.
    #[must_use]
    pub fn intersection_observer_mut(
        &mut self,
        observer: IntersectionObserverId,
    ) -> &mut IntersectionObserver<T> {
        self.intersections.get_mut(observer)
    }

    /// Starts observing `target`. A target the observer already observes is
    /// left as it is; a new one is reported by the next update, whatever its
    /// state.
    ///
    /// # Panics
    ///
    /// When `observer` names no live observer, or `target` is not a live
    /// element.
    pub fn observe_intersection(&mut self, observer: IntersectionObserverId, target: NodeId) {
        assert!(
            self.get(target).is_some_and(crate::Node::is_element),
            "an intersection observer's target {target:?} must be a live element"
        );
        let intersections = &mut self.intersections;
        let observer = intersections.get_mut(observer);
        if observer
            .targets
            .iter()
            .any(|registration| registration.target == target)
        {
            return;
        }
        observer.targets.push(Registration {
            target,
            previous: None,
        });
        intersections.stale = true;
    }

    /// Stops observing `target`; a target it does not observe is a no-op.
    /// Entries already queued for it stay queued, as the spec's `unobserve`
    /// leaves them.
    ///
    /// # Panics
    ///
    /// When `observer` names no live observer.
    pub fn unobserve_intersection(&mut self, observer: IntersectionObserverId, target: NodeId) {
        self.intersections
            .get_mut(observer)
            .targets
            .retain(|registration| registration.target != target);
    }

    /// The spec's `disconnect()`: stops observing every target. Entries
    /// already queued stay queued, as they do after
    /// [`Self::unobserve_intersection`] — the steps touch the targets alone —
    /// so `takeRecords()` or the next notification still hands them out.
    /// The observer itself lives on, and may observe again.
    ///
    /// # Panics
    ///
    /// When `observer` names no live observer.
    pub fn disconnect_intersection_observer(&mut self, observer: IntersectionObserverId) {
        self.intersections.get_mut(observer).targets.clear();
    }

    /// The spec's `takeRecords()`: the observer's queued entries, leaving its
    /// queue empty.
    ///
    /// # Panics
    ///
    /// When `observer` names no live observer.
    #[must_use]
    pub fn take_intersection_records(
        &mut self,
        observer: IntersectionObserverId,
    ) -> Vec<IntersectionObserverEntry> {
        std::mem::take(&mut self.intersections.get_mut(observer).queue)
    }

    /// Drops an observer, its targets, its queue and its handler — or, when
    /// called from inside the observer's own notification, the handler once
    /// that returns. Its id is never reissued. An observer bound to no node
    /// ends only here; a bound one may end here too, before its node does.
    ///
    /// # Panics
    ///
    /// When `observer` names no live observer.
    pub fn drop_intersection_observer(&mut self, observer: IntersectionObserverId) {
        let index = self.intersections.index(observer);
        self.intersections.observers.remove(index);
    }

    /// Something may have moved an observation: the next
    /// [`Self::update_intersection_observations`] runs.
    pub(crate) fn note_intersections_stale(&mut self) {
        self.intersections.stale = true;
    }

    /// The spec's "run the update intersection observations steps"
    /// (§3.2.10), for every observer in creation order, with `time` as every
    /// queued entry's time. Answers whether any observer has entries queued
    /// afterwards — the host's cue to post a delivery.
    ///
    /// Per observer, the root is resolved once; a freed root or an element
    /// root that is not rendered leaves every target
    /// [not rendered](IntersectionGeometry::UNRENDERED). Per target, in
    /// observe order: its [`IntersectionGeometry`], the ratio, and the
    /// threshold index — the index of the first threshold greater than the
    /// ratio, or the list's length. An entry is queued when that index or
    /// the intersecting state differs from the previous update's, and the
    /// pair is remembered.
    ///
    /// Nothing runs when there are no observers, when nothing went stale
    /// since the last update that ran, or before the first render committed
    /// a frame — and that leaves the stale bit as it was, so the first
    /// update after the first render still runs. Reads the last completed
    /// layout and the live scroll offsets; runs no pass and invokes nothing.
    pub fn update_intersection_observations(&mut self, time: f64) -> bool {
        if self.intersections.observers.is_empty()
            || !self.intersections.stale
            || self.painter.borrow().frame().is_none()
        {
            return false;
        }
        self.intersections.stale = false;
        // Out of the document for the pass, so the geometry can read it while
        // the queues are written; nothing in between can reach the registry.
        let mut observers = std::mem::take(&mut self.intersections.observers);
        let mut queued = false;
        for observer in &mut observers {
            let root = match observer.root {
                IntersectionObserverRoot::Implicit => {
                    self.root_geometry(None, &observer.root_margin)
                }
                IntersectionObserverRoot::Element(element) => {
                    self.root_geometry(Some(element), &observer.root_margin)
                }
                IntersectionObserverRoot::Freed => None,
            };
            for registration in &mut observer.targets {
                let geometry = root
                    .as_ref()
                    .map_or(IntersectionGeometry::UNRENDERED, |root| {
                        self.intersection_geometry_in(registration.target, root)
                    });
                let ratio = geometry.intersection_ratio();
                let threshold_index = observer
                    .thresholds
                    .partition_point(|&threshold| threshold <= ratio);
                let state = (threshold_index, geometry.is_intersecting);
                if registration.previous == Some(state) {
                    continue;
                }
                registration.previous = Some(state);
                observer.queue.push(IntersectionObserverEntry {
                    time,
                    root_bounds: geometry.root_bounds,
                    bounding_client_rect: geometry.target_rect,
                    intersection_rect: geometry.intersection_rect,
                    is_intersecting: geometry.is_intersecting,
                    intersection_ratio: ratio,
                    target: registration.target,
                });
            }
            queued |= !observer.queue.is_empty();
        }
        debug_assert!(
            self.intersections.observers.is_empty(),
            "nothing creates an observer during an update"
        );
        self.intersections.observers = observers;
        queued
    }

    /// Whether any observer has entries queued — the cheap question a host
    /// asks before it posts a delivery.
    #[must_use]
    pub fn has_pending_intersection_notifications(&self) -> bool {
        self.intersections
            .observers
            .iter()
            .any(|observer| !observer.queue.is_empty())
    }

    /// §3.2.5 "notify intersection observers", the whole loop, call
    /// included: every observer with entries queued when the loop starts, in
    /// creation order, has its queue taken and handed to its handler's
    /// [`IntersectionEventHandler::notify`].
    ///
    /// Re-entrant. Each observer is looked up again by id when its turn
    /// comes, and skipped when it no longer exists, when its queue is empty
    /// by then (a handler took its records, or a loop run from inside a
    /// notification delivered them), or when its handler is out — its own
    /// notification is running further up, and what it queued since waits
    /// for the next loop. The handler is taken out of its observer for the
    /// call and put back after, unless the observer was dropped meanwhile,
    /// in which case the handler is dropped with it.
    ///
    /// The loop opens no `[CEReactions]` scope of its own:
    /// [`ElementHandler`] opens one per call, and a handler that does not
    /// call into a custom element needs none.
    pub fn notify_intersection_observers(&mut self) {
        let notify_list: Vec<IntersectionObserverId> = self
            .intersections
            .observers
            .iter()
            .filter(|observer| !observer.queue.is_empty())
            .map(|observer| observer.id)
            .collect();
        for id in notify_list {
            let Some(index) = self.intersections.position(id) else {
                continue;
            };
            let observer = &mut self.intersections.observers[index];
            if observer.queue.is_empty() {
                continue;
            }
            let Some(mut handler) = observer.handler.take() else {
                continue;
            };
            let entries = std::mem::take(&mut observer.queue);
            handler.notify(self, id, entries);
            if let Some(index) = self.intersections.position(id) {
                self.intersections.observers[index].handler = Some(handler);
            }
        }
    }
}
