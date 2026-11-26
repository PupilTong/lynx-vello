//! DOM events: the ordered set of node visits one event resolves to, the
//! table of who listens where, and the one walk over a path that this crate
//! runs and a host drives.
//!
//! This crate does not call *script*. It cannot: a listener's callback is a
//! JavaScript value in a realm this crate knows nothing about, and reaching
//! one means releasing the document first. What it does is everything around
//! that call that needs the tree, and that no caller could do correctly from
//! outside.
//!
//! **The path.** [`Document::event_steps`] works out which nodes an event
//! visits, in which order, and which target each may see: the capture pass
//! root-inward followed by the bubble pass target-outward, one [`EventStep`]
//! per visit, computed in a single borrow and owning no reference to the
//! document afterwards.
//!
//! ```
//! # use terminus::link::{Document, NodeId};
//! # fn f(document: &Document<()>, button: NodeId) {
//! for step in document.event_steps(button, true, false).steps() {
//!     println!("{} sees target {}", step.node, step.target);
//!     let _ = step.capture;
//! }
//! # }
//! ```
//!
//! **The listener table.** [`Document::add_listener`] and
//! [`Document::remove_listener`] record the standard's event listener list
//! minus the callbacks: per node, which registrant ([`ListenerKey`], a realm
//! to the host) listens for which type in which passes ([`ListenerKinds`]).
//! From it a component learns whether anything listens on it
//! ([`Document::has_listener`]) or would hear an event it fires
//! ([`Document::would_reach_listener`]), a host learns which registrants one
//! dispatch reaches ([`Document::listeners_on_path`],
//! [`Document::global_registrants_on_path`]) and whether anything hears a type at all
//! ([`Document::hears`]), and a router learns when a type's registration
//! count crosses zero ([`RegistrationChange`]). A freed node's registrations
//! leave in the document's one free path; `listeners`' module doc says why
//! the table sits beside the tree.
//!
//! **The walk.** [`Document::event_walk`] starts an [`EventWalk`] over a
//! path, and [`Document::advance`] visits one step at a time: it runs the
//! engine component on that step, if any, and reports the step
//! ([`StepOutcome`]) so the host can run what it has there — a realm call —
//! with the document released in between. The walk holds the dispatch's one
//! stop flag. That is what lets a listener mutate the tree, and what keeps a
//! walk from holding a borrow across a call into script.
//!
//! # The engine's own listeners
//!
//! An engine component is a [`CustomElement`](crate::tree::custom::CustomElement) whose
//! handler is Rust owned by this document, so the walk calls it directly:
//! [`CustomElement::handle_event`](crate::tree::custom::CustomElement::handle_event) on
//! every defined element along the path, before [`Document::advance`]
//! reports the step. [`Document::dispatch_element_event_over`] drives a walk
//! to its end for a caller with nothing of its own per step, and
//! [`Document::dispatch_element_event`] computes the steps itself, for an
//! event fired to the components alone (`slotchange`, and
//! `contentvisibilityautostatechange` for a host that does not take it to
//! script). [`ElementEvent`] is what a handler is
//! given: the W3C type name, an [`EventDetail`] (one variant per payload
//! shape, owned by this crate and carried unchanged by every layer above),
//! the timestamp, and the stop methods, which end the walk — the host's
//! later steps with it. A type no definition lists in
//! [`CustomElement::observed_events`](crate::tree::custom::CustomElement::observed_events)
//! looks no component up; a type nothing hears at all is one
//! [`Document::hears`] check and builds no walk.
//!
//! A shadow root is not an element and has no handler, yet it is where a
//! component listens for its own shadow tree's non-composed events
//! (`this.shadowRoot.addEventListener('slotchange', …)`). So the shadow root
//! a non-composed path ends at is delivered to its host's handler, with the
//! host as `element` and `currentTarget`: the shadow root itself as
//! `currentTarget` is not representable in a call that is the host's. A
//! shadow root the path crosses delivers nothing, because the host that
//! follows it on the path is called as itself.
//!
//! **A trap for component authors.** For a non-composed event fired inside a
//! component's own shadow tree, that boundary step is the *only* step at
//! which the host hears it. When the event also does not bubble — the W3C
//! `scroll` of a scroll container inside a swiper's shadow tree, say — the
//! bubble pass has no step at the shadow root at all, so the host's one
//! delivery is the **capture**-phase one ([`EventPhase::Capturing`]). A
//! handler that filters on [`EventPhase::Bubbling`], as one written for
//! `slotchange` (which bubbles) may, never sees such an event.
//!
//! # Shadow trees
//!
//! Path construction is the standard's, including its two shadow rules. The
//! parent of a slotted node is its assigned slot, so an event on light content
//! walks up through the shadow tree that displays it. Crossing out of a shadow
//! tree to its host **retargets**: every step from the host outward reports
//! the host as its [`EventStep::target`], because the outer tree may not learn
//! the identity of a node inside a shadow tree. A non-composed event does not
//! cross out of the tree its target is in at all.
//!
//! # Recorded limits
//!
//! - **The path is a snapshot, and needs no staleness protocol.** It names nodes by [`NodeId`],
//!   which names one node for the life of the document: freeing retires the id, so a step that
//!   outlived its node resolves to nothing rather than to whatever took its storage. A host may
//!   therefore hold a path across a thread hand-off and deliver it later, and the worst a free can
//!   do is make a step reach no one. Detaching changes nothing at all, which is what a re-render
//!   actually does.
//! - **No `composedPath()`.** Reporting it requires the standard's *root-of-closed-tree* and
//!   *slot-in-closed-tree* bookkeeping, whose only consumer is that one method. A closed shadow
//!   root is still honored where it changes the path — retargeting — because that part is not
//!   optional.
//! - **No `relatedTarget`, and no touch target lists.** Both exist for event interfaces this crate
//!   does not model; their retargeting steps are absent with them.
//! - **No *script* event object, no callbacks, no `preventDefault`.** The listener table records
//!   who listens where and nothing a listener runs: for an event whose listeners are script's, the
//!   event object and whether anything cancels are above this boundary. Lynx dispatches no
//!   cancelable event in any case, and suppressing a user-agent default action arrives on
//!   [`InputEvent::default_prevented`](crate::input::InputEvent::default_prevented) instead. The
//!   engine-component path has an event object of its own ([`ElementEvent`]) because there is no
//!   layer above it to build one; it is not cancelable either.
//! - **The path ends at the node with no event parent** — the document node for a connected target,
//!   the topmost ancestor for a detached one. A detached target still produces a path, exactly as
//!   the standard specifies.

mod detail;
mod element;
pub(crate) mod listeners;
mod queued;

use smallvec::SmallVec;

pub use self::detail::{
    EventDetail, TOUCH_ACTIVE, TOUCH_CHANGED, TOUCH_TARGET, TouchPoint, TouchPoints,
};
pub use self::element::{ElementEvent, EventPhase, EventWalk, StepOutcome};
pub use self::listeners::{ListenerKey, ListenerKinds, RegistrationChange};
pub use self::queued::QueuedEventSink;
use crate::tree::document::{Document, NodeId};
use crate::tree::node::Node;

const INLINE_STEPS: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct EventStep {
    pub node: NodeId,
    pub target: NodeId,
    pub capture: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventSteps {
    steps: SmallVec<[EventStep; INLINE_STEPS]>,
    target: NodeId,
    boundary: Option<NodeId>,
    bubbles: bool,
}

impl EventSteps {
    #[must_use]
    pub fn steps(&self) -> &[EventStep] {
        &self.steps
    }

    #[must_use]
    pub fn target(&self) -> NodeId {
        self.target
    }
}

#[derive(Debug, Clone, Copy)]
struct PathEntry {
    node: NodeId,
    target: NodeId,
    at_target: bool,
}

impl<T> Document<T> {
    #[must_use]
    pub fn event_steps(&self, target: NodeId, bubbles: bool, composed: bool) -> EventSteps {
        assert!(
            self.contains_node(target),
            "Document::event_steps: stale target NodeId"
        );
        let (path, target_root) = self.event_path(target, composed);
        let boundary = (!composed && self.is_shadow_root(target_root)).then_some(target_root);
        let mut steps = SmallVec::new();

        for entry in path.iter().rev() {
            steps.push(EventStep {
                node: entry.node,
                target: entry.target,
                capture: true,
            });
        }
        for entry in &path {
            if !entry.at_target && !bubbles {
                continue;
            }
            steps.push(EventStep {
                node: entry.node,
                target: entry.target,
                capture: false,
            });
        }

        EventSteps {
            steps,
            target,
            boundary,
            bubbles,
        }
    }

    fn event_path(
        &self,
        target: NodeId,
        composed: bool,
    ) -> (SmallVec<[PathEntry; INLINE_STEPS]>, NodeId) {
        let mut path = SmallVec::new();
        path.push(PathEntry {
            node: target,
            target,
            at_target: true,
        });

        let target_root = self.tree_root(target);
        let mut current_target = target;
        let mut current_root = target_root;
        let mut node = target;

        while let Some(parent) = self.event_parent(node, composed, target_root) {
            let at_target = node == current_root && self.is_shadow_root(node);
            if at_target {
                current_target = parent;
                current_root = self.tree_root(parent);
            }
            path.push(PathEntry {
                node: parent,
                target: current_target,
                at_target,
            });
            node = parent;
        }
        (path, target_root)
    }

    #[must_use]
    pub fn nearest_common_event_ancestor(&self, a: NodeId, b: NodeId) -> Option<NodeId> {
        if !self.contains_node(a) || !self.contains_node(b) {
            return None;
        }
        let mut ancestors: SmallVec<[NodeId; INLINE_STEPS]> = SmallVec::new();
        let mut node = Some(a);
        while let Some(current) = node {
            ancestors.push(current);
            node = self.event_parent(current, true, current);
        }
        let mut node = Some(b);
        while let Some(current) = node {
            if ancestors.contains(&current) {
                return Some(current);
            }
            node = self.event_parent(current, true, current);
        }
        None
    }

    fn is_shadow_root(&self, node: NodeId) -> bool {
        self.get(node).is_some_and(Node::is_shadow_root)
    }

    fn event_parent(&self, node: NodeId, composed: bool, target_root: NodeId) -> Option<NodeId> {
        let live = self.get(node)?;
        if live.is_shadow_root() {
            if !composed && node == target_root {
                return None;
            }
            return self.shadow_host(node);
        }
        if let Some(slot) = self.assigned_slot(node) {
            return Some(slot);
        }
        live.parent_id()
    }

    fn tree_root(&self, node: NodeId) -> NodeId {
        if self.is_shadow_root(node) {
            return node;
        }
        if let Some(root) = self.containing_shadow_root(node) {
            return root;
        }
        let mut current = node;
        while let Some(parent) = self.get(current).and_then(Node::parent_id) {
            current = parent;
        }
        current
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests;
