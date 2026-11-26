//! The two Lynx layout events: every element's `layoutchange`
//! (`bindlayoutchange`) and `<text>`'s `layout` (`bindlayout`)
//! (`docs/layout-observer-design.md`).
//!
//! Neither has a standards counterpart with its event shape, so both live
//! here rather than in `terminus`, over two primitives `terminus` has: the W3C
//! `ResizeObserver` ([`terminus::link::ResizeEventHandler`]) for `layoutchange`, and the
//! per-pass text layout report list
//! ([`terminus::link::Document::set_text_layout_reporting`],
//! [`terminus::link::Document::take_text_layout_reports`]) for `layout`.
//!
//! # `layoutchange` (the user's rulings of 2026-10-20, §5 of the design)
//!
//! **Size changes only.** An element hears `layoutchange` when its border
//! box changes size, and once on the first observation after it registered,
//! with the size it has then: web-core's `ResizeObserver` on the element
//! (`CommonEventsAndMethods.ts:23-58`), ruling 1. An element that moves
//! without changing size, or changes an edge without changing its border-box
//! size, hears nothing; native fires there too (`layout_object.cc:300-322`),
//! which `docs/tracking/deviations.md` records.
//!
//! **One observer for the document** (ruling 5): every registered element is
//! one `border-box` target of it. The entry names its target, and there are
//! no per-element options to keep apart, so nothing is gained from one
//! observer per element.
//!
//! **Delivery is a posted entry after the frame** (ruling 2): the epilogue
//! runs `terminus`'s update after its commit and posts the page's resize delivery
//! entry when the update queued something
//! ([`crate::main::runtime::MainThreadRuntime::notify_resize_observers`]).
//! A handler's writes land in the next commit.
//!
//! **The detail's rect is [`terminus::link::Document::bounding_client_rect`]** (rulings
//! 3 and 8), read when the event is delivered: the transform-free walk the
//! `boundingClientRect` UI method answers from, in the Lynx view's own CSS
//! px, with nothing subtracted. A target that has no box at delivery —
//! `display: none`, which the primitive reports as a `0 × 0` entry, or a
//! detached element — reports `0` for all four numbers.
//!
//! # Which box is observed
//!
//! An element's own border box, except an `overlay`'s or `x-overlay-ng`'s,
//! which generates no box: there the box observed is its shadow `#dialog`'s,
//! web-core's `layoutChangeTarget` (`XOverlayNg.ts:21-23`), and the event is
//! dispatched at the overlay with the dialog's rect
//! (`CommonEventsAndMethods.ts:31-50`). [`layout_change_target`] answers
//! which node that is; [`LayoutChangeObserver`] keeps the retargeted pairs
//! (observed node to overlay) for the delivery to dispatch at the overlay.
//!
//! # How a registration is held
//!
//! An element is observed while the document's listener table has a
//! `layoutchange` row on it, of any realm and any form —
//! `__AddEventListener`, and `__AddEvent`'s `bindEvent`, `catchEvent`,
//! `capture-bind`, `capture-catch` and `global-bindEvent`, string handler or
//! worklet — which is web-core's
//! `registerEventEnableStatusChangeHandler('layoutchange')`. On each change
//! of that answer (`listenerOpened`, `listenerClosed`) the runtime adds or
//! removes the element's [`LayoutEventRegistration::ResizeObservation`]:
//! [`LayoutChangeObserver::observe`] or [`LayoutChangeObserver::unobserve`].
//! The observer is created on the first observation and kept for the
//! document's life.
//!
//! The handler, [`LayoutChangeHandler`], is bound to no node and only
//! records which targets resized; the realm half runs after `terminus`'s loop,
//! with the document released, as the exposure handler's does
//! (`crate::main::exposure`). The record is taken in the same delivery
//! entry, so no target outlives it.
//!
//! Nothing here holds a node per registration. A freed element leaves the
//! observer through the primitive (`Document::free_node` drops its
//! registration and its queued entries), and a realm that is forgotten
//! (`forget_realm`) takes its rows with it without a `listenerClosed`: the
//! delivery finds such an element with no `layoutchange` row and unobserves
//! it there, dispatching nothing.
//!
//! # `<text>` `layout`
//!
//! A `text` element reports while the listener table has a `layout` row on
//! it: on each change of that answer the runtime adds or removes the
//! element's [`LayoutEventRegistration::TextLayoutReport`], turning `terminus`'s
//! text layout report on or off, as for `layoutchange`. A `layout` row on any
//! other element is ignored. After every commit the epilogue takes `terminus`'s report list and
//! queues one component event per reporting paragraph a committing layout run
//! recomputed ([`super::tree::ComponentEvent::TextLayout`]).
//!
//! Turning the report on lists nothing by itself: a listener filed after the
//! paragraph laid out hears nothing until the paragraph is laid out again,
//! which is native iOS's per-renderer latch (`LynxUIText.m:359-365`; the
//! latch resets when the text renderer object changes, `:412-415`).
//!
//! # Cost
//!
//! A page with no `layoutchange` registration holds no observer, and `terminus`'s
//! update answers at its first `is_empty` test. A page with no `layout`
//! registration pays one `is_empty` test per commit for the report list.

use std::cell::RefCell;
use std::rc::Rc;

use rustc_hash::FxHashMap;
use terminus::link::{
    NodeId, ObservedBox, ResizeEventHandler, ResizeObserverEntry, ResizeObserverId,
};

use super::tree::{LynxDocument, layout_change_target};

pub(crate) const LAYOUT_CHANGE_EVENT: &str = "layoutchange";

pub(crate) const TEXT_LAYOUT_EVENT: &str = "layout";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LayoutEventRegistration {
    ResizeObservation,
    TextLayoutReport,
}

impl LayoutEventRegistration {
    pub(crate) fn of(name: &str) -> Option<Self> {
        match name {
            LAYOUT_CHANGE_EVENT => Some(Self::ResizeObservation),
            TEXT_LAYOUT_EVENT => Some(Self::TextLayoutReport),
            _ => None,
        }
    }

    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::ResizeObservation => LAYOUT_CHANGE_EVENT,
            Self::TextLayoutReport => TEXT_LAYOUT_EVENT,
        }
    }
}

#[derive(Debug)]
struct LayoutChangeHandler {
    resized: Rc<RefCell<Vec<NodeId>>>,
}

impl ResizeEventHandler<()> for LayoutChangeHandler {
    fn notify(
        &mut self,
        _document: &mut LynxDocument,
        _observer: ResizeObserverId,
        entries: Vec<ResizeObserverEntry>,
    ) {
        self.resized
            .borrow_mut()
            .extend(entries.iter().map(|entry| entry.target));
    }
}

#[derive(Debug, Default)]
pub(crate) struct LayoutChangeObserver {
    observer: Option<ResizeObserverId>,
    resized: Rc<RefCell<Vec<NodeId>>>,
    hosts: FxHashMap<NodeId, NodeId>,
}

impl LayoutChangeObserver {
    pub(crate) fn observe(&mut self, document: &mut LynxDocument, node: NodeId) {
        let target = layout_change_target(document, node);
        self.hosts.retain(|target, host| {
            document.get(*target).is_some() && document.get(*host).is_some()
        });
        if target != node {
            self.hosts.insert(target, node);
        }
        let observer = *self.observer.get_or_insert_with(|| {
            document.create_resize_observer(Box::new(LayoutChangeHandler {
                resized: Rc::clone(&self.resized),
            }))
        });
        document.observe_resize(observer, target, ObservedBox::BorderBox);
    }

    pub(crate) fn unobserve(&mut self, document: &mut LynxDocument, node: NodeId) {
        let target = layout_change_target(document, node);
        self.hosts.remove(&target);
        if let Some(observer) = self.observer {
            document.unobserve_resize(observer, target);
        }
    }

    pub(crate) fn take_resized(&mut self, document: &LynxDocument) -> Vec<(NodeId, NodeId)> {
        let resized = std::mem::take(&mut *self.resized.borrow_mut());
        resized
            .into_iter()
            .filter_map(|target| {
                let host = self.hosts.get(&target).copied().unwrap_or(target);
                if document.get(target).is_some() && document.get(host).is_some() {
                    Some((target, host))
                } else {
                    self.hosts.remove(&target);
                    None
                }
            })
            .collect()
    }

    #[cfg(test)]
    pub(crate) const fn observer(&self) -> Option<ResizeObserverId> {
        self.observer
    }

    #[cfg(test)]
    pub(crate) fn is_observed(&self, document: &LynxDocument, node: NodeId) -> bool {
        let target = layout_change_target(document, node);
        self.observer
            .and_then(|observer| document.resize_observer(observer))
            .is_some_and(|observer| observer.observed_box(target).is_some())
    }
}
