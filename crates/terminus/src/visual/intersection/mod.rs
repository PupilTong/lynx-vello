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
//! - **A target margin beside the root margin**, a Lynx-motivated extension: it dilates every
//!   target's border box in the target's own space, before its transform and before any clip, and
//!   the entry's rects and ratio are over the dilated rect. Lynx native's exposure grows the
//!   element's own rect and tests it against each scroll container, which reaches an element a
//!   scrollport or a clipping page has cut; a root margin, applied after every clip, cannot. The
//!   spec has no `targetMargin`. Its `scrollMargin` is not the same thing: it dilates scroll
//!   containers' clip rects only, so a non-scrolling `overflow: clip` ancestor such as the page
//!   still cuts, and it keeps the undilated ratio basis where native's ratio is over the grown
//!   rect. It has no JavaScript surface; the MTS binding, once it exists, passes the zero target
//!   margin.
//! - **Rust only.** No JavaScript surface exists yet. A W3C-shaped MTS constructor and a
//!   Lynx-shaped `lynx.createIntersectionObserver` are adapters over this primitive, not part of
//!   it.
//! - **A handler per observer, as a trait object.** Every observer carries a `Box<dyn
//!   IntersectionEventHandler<T>>`, and `terminus` runs §3.2.5's whole notify loop, call included.
//!   `dyn` is the user's ruling (2026-10-10), chosen for simplicity: one trait object per observer,
//!   so one document holds observers of any number of handler types — an engine component's
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
//! for one handler and one [`IntersectionObserverInit`] — one root (the
//! viewport, or an element), one root margin, one target margin and a
//! sorted threshold list — and named by an [`IntersectionObserverId`]
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
//! naming it are dropped (so every entry handed out names a live node), the
//! handler of each observer it left hears of it
//! ([`IntersectionEventHandler::target_freed`]), and
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
//! [`CustomElement::intersections_changed`]: crate::tree::custom::CustomElement::intersections_changed

mod geometry;

use std::num::NonZeroU32;

use euclid::default::Rect;

pub use self::geometry::{IntersectionGeometry, IntersectionMargin, MarginLength, RootGeometry};
use crate::tree::document::{Document, NodeId};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct IntersectionObserverId(NonZeroU32);

impl IntersectionObserverId {
    #[must_use]
    pub const fn get(self) -> u32 {
        self.0.get()
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct IntersectionObserverEntry {
    pub time: f64,
    pub root_bounds: Rect<f32>,
    pub bounding_client_rect: Rect<f32>,
    pub intersection_rect: Rect<f32>,
    pub is_intersecting: bool,
    pub intersection_ratio: f64,
    pub target: NodeId,
}

pub trait IntersectionEventHandler<T> {
    fn notify(
        &mut self,
        document: &mut Document<T>,
        observer: IntersectionObserverId,
        entries: Vec<IntersectionObserverEntry>,
    );

    fn bound_to(&self) -> Option<NodeId> {
        None
    }

    fn target_freed(&mut self, _node: NodeId) {}
}

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

#[derive(Debug, Clone, Default, PartialEq)]
pub struct IntersectionObserverInit {
    pub root: Option<NodeId>,
    pub root_margin: IntersectionMargin,
    pub target_margin: IntersectionMargin,
    pub thresholds: Vec<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntersectionObserverRoot {
    Implicit,
    Element(NodeId),
    Freed,
}

#[derive(Debug)]
struct Registration {
    target: NodeId,
    previous: Option<(usize, bool)>,
}

pub struct IntersectionObserver<T> {
    id: IntersectionObserverId,
    root: IntersectionObserverRoot,
    root_margin: IntersectionMargin,
    target_margin: IntersectionMargin,
    thresholds: Box<[f64]>,
    targets: Vec<Registration>,
    queue: Vec<IntersectionObserverEntry>,
    bound_to: Option<NodeId>,
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
    pub const fn root_margin(&self) -> &IntersectionMargin {
        &self.root_margin
    }

    #[must_use]
    pub const fn target_margin(&self) -> &IntersectionMargin {
        &self.target_margin
    }

    #[must_use]
    pub fn thresholds(&self) -> &[f64] {
        &self.thresholds
    }

    #[must_use]
    pub const fn bound_to(&self) -> Option<NodeId> {
        self.bound_to
    }

    #[must_use]
    pub fn handler(&self) -> Option<&dyn IntersectionEventHandler<T>> {
        self.handler.as_deref()
    }

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
            .field("target_margin", &self.target_margin)
            .field("thresholds", &self.thresholds)
            .field("targets", &self.targets)
            .field("queue", &self.queue)
            .field("bound_to", &self.bound_to)
            .field("notifying", &self.handler.is_none())
            .finish_non_exhaustive()
    }
}

pub(crate) struct IntersectionObservers<T> {
    observers: Vec<IntersectionObserver<T>>,
    next: NonZeroU32,
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

    fn position(&self, id: IntersectionObserverId) -> Option<usize> {
        self.observers
            .binary_search_by_key(&id, |observer| observer.id)
            .ok()
    }

    fn index(&self, id: IntersectionObserverId) -> usize {
        self.position(id)
            .unwrap_or_else(|| panic!("{id:?} names no live intersection observer"))
    }

    fn get_mut(&mut self, id: IntersectionObserverId) -> &mut IntersectionObserver<T> {
        let index = self.index(id);
        &mut self.observers[index]
    }

    pub(crate) fn forget_node(&mut self, node: NodeId) {
        self.observers
            .retain(|observer| observer.bound_to != Some(node));
        for observer in &mut self.observers {
            let observed = observer.targets.len();
            observer
                .targets
                .retain(|registration| registration.target != node);
            observer.queue.retain(|entry| entry.target != node);
            if observer.targets.len() != observed
                && let Some(handler) = &mut observer.handler
            {
                handler.target_freed(node);
            }
            if observer.root == IntersectionObserverRoot::Element(node) {
                observer.root = IntersectionObserverRoot::Freed;
                self.stale = true;
            }
        }
    }
}

impl<T> Document<T> {
    pub fn create_intersection_observer(
        &mut self,
        handler: Box<dyn IntersectionEventHandler<T>>,
        init: IntersectionObserverInit,
    ) -> IntersectionObserverId {
        let IntersectionObserverInit {
            root,
            root_margin,
            target_margin,
            mut thresholds,
        } = init;
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
                self.get(root)
                    .is_some_and(crate::tree::node::Node::is_element),
                "an intersection observer's root {root:?} must be a live element"
            );
        }
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
            target_margin,
            thresholds: thresholds.into_boxed_slice(),
            targets: Vec::new(),
            queue: Vec::new(),
            bound_to,
            handler: Some(handler),
        });
        id
    }

    #[must_use]
    pub fn intersection_observer(
        &self,
        observer: IntersectionObserverId,
    ) -> &IntersectionObserver<T> {
        &self.intersections.observers[self.intersections.index(observer)]
    }

    #[must_use]
    pub fn intersection_observer_mut(
        &mut self,
        observer: IntersectionObserverId,
    ) -> &mut IntersectionObserver<T> {
        self.intersections.get_mut(observer)
    }

    pub fn observe_intersection(&mut self, observer: IntersectionObserverId, target: NodeId) {
        assert!(
            self.get(target)
                .is_some_and(crate::tree::node::Node::is_element),
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

    pub fn unobserve_intersection(&mut self, observer: IntersectionObserverId, target: NodeId) {
        self.intersections
            .get_mut(observer)
            .targets
            .retain(|registration| registration.target != target);
    }

    pub fn forget_intersection_target(&mut self, observer: IntersectionObserverId, target: NodeId) {
        let observer = self.intersections.get_mut(observer);
        observer
            .targets
            .retain(|registration| registration.target != target);
        observer.queue.retain(|entry| entry.target != target);
    }

    pub fn disconnect_intersection_observer(&mut self, observer: IntersectionObserverId) {
        self.intersections.get_mut(observer).targets.clear();
    }

    #[must_use]
    pub fn take_intersection_records(
        &mut self,
        observer: IntersectionObserverId,
    ) -> Vec<IntersectionObserverEntry> {
        std::mem::take(&mut self.intersections.get_mut(observer).queue)
    }

    pub fn drop_intersection_observer(&mut self, observer: IntersectionObserverId) {
        let index = self.intersections.index(observer);
        self.intersections.observers.remove(index);
    }

    pub(crate) fn note_intersections_stale(&mut self) {
        self.intersections.stale = true;
    }

    pub fn update_intersection_observations(&mut self, time: f64) -> bool {
        if self.intersections.observers.is_empty()
            || !self.intersections.stale
            || self.painter.borrow().frame().is_none()
        {
            return false;
        }
        self.intersections.stale = false;
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
            let target_margin = observer.target_margin;
            for registration in &mut observer.targets {
                let geometry = root
                    .as_ref()
                    .map_or(IntersectionGeometry::UNRENDERED, |root| {
                        self.intersection_geometry_in(registration.target, root, &target_margin)
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

    #[must_use]
    pub fn has_pending_intersection_notifications(&self) -> bool {
        self.intersections
            .observers
            .iter()
            .any(|observer| !observer.queue.is_empty())
    }

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
