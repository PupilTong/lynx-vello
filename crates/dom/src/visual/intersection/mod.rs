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
//! - **An owner tag, not a callback.** Every observer carries an [`IntersectionObserverOwner`]:
//!   `dom` only queues entries, and never invokes anything on its own.
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
//! # Observers and their owners
//!
//! An observer is created with [`Document::create_intersection_observer`]
//! for one root (the viewport, or an element), one root margin and a sorted
//! threshold list, and named by an [`IntersectionObserverId`] that is never
//! reissued. What it observes is a list of targets in observe order, each
//! with the spec's previous threshold index and intersecting state; what it
//! has to say is a queue of [`IntersectionObserverEntry`] records.
//!
//! Its owner decides who hears the queue and how long it lives:
//! - [`IntersectionObserverOwner::Element`] is a constructed custom element. It hears its entries
//!   through [`CustomElement::intersections_changed`], and its observers go with it when it is
//!   freed.
//! - [`IntersectionObserverOwner::Host`] is the embedder's — a realm's observer, once there is a
//!   binding. It lives until [`Document::drop_intersection_observer`].
//!
//! # The update and the delivery
//!
//! [`Document::update_intersection_observations`] is §3.2.10 for every
//! observer in creation order: one [`RootGeometry`] per observer, one
//! [`IntersectionGeometry`] per target, and an entry queued wherever the
//! pair (threshold index, intersecting) moved. It invokes nothing. The host
//! drains the queues with [`Document::take_intersection_notifications`] —
//! §3.2.5's "notify intersection observers" loop minus the call — and routes
//! each by its owner: an `Element` owner's through
//! [`Document::deliver_intersections_to_element`], which calls the element's
//! hook in its own `[CEReactions]` scope; a `Host` owner's to wherever the
//! embedder keeps its own callbacks.
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
//! registry: observers owned by the node are dropped, the node leaves every
//! target list, queued entries naming it are dropped (so every entry handed
//! out names a live node), and a root that was the node turns `Freed`, whose
//! targets report one leave entry and then nothing. A node that is unlinked
//! but alive stays observed: the unlink dirties layout, the render that
//! follows sets the stale bit, and the target, no longer rendered, reports
//! leaving.
//!
//! # Cost
//!
//! A document with no observers pays one `is_empty` test per freed node and
//! one `bool` store per render and per moved scroll; an update with nothing
//! stale is two tests.
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

/// Who an observer's entries are for, and what ends its life.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntersectionObserverOwner {
    /// A constructed custom element. Its entries are delivered through
    /// [`Document::deliver_intersections_to_element`], and the observer is
    /// dropped when the element is freed.
    Element(NodeId),
    /// The embedder. The observer lives until
    /// [`Document::drop_intersection_observer`], and its entries go
    /// wherever the embedder routes them.
    Host,
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

/// One observer's queued entries, as the host drains them.
#[derive(Debug, Clone, PartialEq)]
pub struct IntersectionNotification {
    pub observer: IntersectionObserverId,
    pub owner: IntersectionObserverOwner,
    /// In the order the updates queued them: by update, then by observe
    /// order within one.
    pub entries: Vec<IntersectionObserverEntry>,
}

/// An observer's root.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ObserverRoot {
    /// The viewport.
    Implicit,
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

#[derive(Debug)]
struct Observer {
    id: IntersectionObserverId,
    owner: IntersectionObserverOwner,
    root: ObserverRoot,
    root_margin: RootMargin,
    /// Ascending, never empty.
    thresholds: Box<[f64]>,
    /// In observe order.
    targets: Vec<Registration>,
    queue: Vec<IntersectionObserverEntry>,
}

/// The document's observers, in creation order — the order the spec notifies
/// them in.
#[derive(Debug)]
pub(crate) struct IntersectionObservers {
    observers: Vec<Observer>,
    next: NonZeroU32,
    /// Something may have moved an observation since the last update ran.
    stale: bool,
}

impl Default for IntersectionObservers {
    fn default() -> Self {
        Self {
            observers: Vec::new(),
            next: NonZeroU32::MIN,
            stale: false,
        }
    }
}

impl IntersectionObservers {
    pub(crate) fn is_empty(&self) -> bool {
        self.observers.is_empty()
    }

    /// The index of a live observer. An id that names none is a caller bug,
    /// the way a stale `NodeId` passed to a mutation is.
    fn index(&self, id: IntersectionObserverId) -> usize {
        self.observers
            .iter()
            .position(|observer| observer.id == id)
            .unwrap_or_else(|| panic!("{id:?} names no live intersection observer"))
    }

    fn get_mut(&mut self, id: IntersectionObserverId) -> &mut Observer {
        let index = self.index(id);
        &mut self.observers[index]
    }

    /// `node` is being freed: no observer may outlive its owner, observe it,
    /// queue an entry naming it, or keep it as a root.
    pub(crate) fn forget_node(&mut self, node: NodeId) {
        self.observers
            .retain(|observer| observer.owner != IntersectionObserverOwner::Element(node));
        for observer in &mut self.observers {
            observer
                .targets
                .retain(|registration| registration.target != node);
            observer.queue.retain(|entry| entry.target != node);
            if observer.root == ObserverRoot::Element(node) {
                observer.root = ObserverRoot::Freed;
                // Its targets owe a leave entry whether or not anything
                // renders again.
                self.stale = true;
            }
        }
    }
}

impl<T> Document<T> {
    /// Creates an intersection observer and answers its id.
    ///
    /// `root` is the intersection root: `None` for the implicit root (the
    /// viewport), or a live element. `thresholds` are sorted ascending; an
    /// empty list is the spec's `[0]`. Creating observes nothing, so it
    /// leaves the update with nothing to do.
    ///
    /// # Panics
    ///
    /// When a threshold is not a finite number in `[0, 1]`, when `root` is
    /// not a live element, or when an [`IntersectionObserverOwner::Element`]
    /// owner is not a live, constructed custom element — create one from its
    /// `connected_callback` or later, not from its constructor.
    pub fn create_intersection_observer(
        &mut self,
        owner: IntersectionObserverOwner,
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
        if let IntersectionObserverOwner::Element(element) = owner {
            assert!(
                self.custom_element_handler(element).is_some(),
                "an intersection observer's element owner {element:?} must be a live, \
                 constructed custom element"
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
        intersections.observers.push(Observer {
            id,
            owner,
            root: root.map_or(ObserverRoot::Implicit, ObserverRoot::Element),
            root_margin,
            thresholds: thresholds.into_boxed_slice(),
            targets: Vec::new(),
            queue: Vec::new(),
        });
        id
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

    /// Drops an observer, its targets and its queue. Its id is never
    /// reissued. An [`IntersectionObserverOwner::Host`] observer ends only
    /// here; an `Element` one may end here too, before its element does.
    ///
    /// # Panics
    ///
    /// When `observer` names no live observer.
    pub fn drop_intersection_observer(&mut self, observer: IntersectionObserverId) {
        let index = self.intersections.index(observer);
        self.intersections.observers.remove(index);
    }

    /// The observers `node` owns as an [`IntersectionObserverOwner::Element`],
    /// in creation order, so a component holding several tells them apart.
    pub fn intersection_observers_owned_by(
        &self,
        node: NodeId,
    ) -> impl Iterator<Item = IntersectionObserverId> + '_ {
        self.intersections
            .observers
            .iter()
            .filter(move |observer| observer.owner == IntersectionObserverOwner::Element(node))
            .map(|observer| observer.id)
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
                ObserverRoot::Implicit => self.root_geometry(None, &observer.root_margin),
                ObserverRoot::Element(element) => {
                    self.root_geometry(Some(element), &observer.root_margin)
                }
                ObserverRoot::Freed => None,
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

    /// §3.2.5 "notify intersection observers" minus the call: every observer
    /// with entries queued, in creation order, each with its queue taken.
    ///
    /// Nothing queued hands back an empty `Vec` with no allocation. `dom`
    /// calls nothing itself: the host routes each notification by its
    /// owner — an [`IntersectionObserverOwner::Element`] one through
    /// [`Self::deliver_intersections_to_element`].
    #[must_use]
    pub fn take_intersection_notifications(&mut self) -> Vec<IntersectionNotification> {
        if !self.has_pending_intersection_notifications() {
            return Vec::new();
        }
        self.intersections
            .observers
            .iter_mut()
            .filter(|observer| !observer.queue.is_empty())
            .map(|observer| IntersectionNotification {
                observer: observer.id,
                owner: observer.owner,
                entries: std::mem::take(&mut observer.queue),
            })
            .collect()
    }

    /// Delivers one notification to the [`IntersectionObserverOwner::Element`]
    /// that owns it: [`CustomElement::intersections_changed`] on `element`,
    /// in its own `[CEReactions]` scope, as an event dispatch wraps each
    /// handler — what the hook's mutations raise runs before this returns.
    ///
    /// An element that has been freed since the notification was taken, or
    /// that is not a constructed custom element, hears nothing: the entries
    /// are dropped.
    ///
    /// [`CustomElement::intersections_changed`]: crate::CustomElement::intersections_changed
    pub fn deliver_intersections_to_element(
        &mut self,
        element: NodeId,
        observer: IntersectionObserverId,
        entries: Vec<IntersectionObserverEntry>,
    ) {
        let Some(handler) = self.custom_element_handler(element) else {
            return;
        };
        let base = self.begin_reactions();
        handler.intersections_changed(self, element, observer, entries);
        self.drain_reactions(base);
    }
}
