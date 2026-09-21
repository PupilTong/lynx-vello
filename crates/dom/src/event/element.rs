//! Dispatch to the engine's own components: the one walk this crate runs
//! itself, and the one that never reaches script.
//!
//! [`Document::event_steps`](crate::Document::event_steps) exists because a
//! *script* listener lives in a realm this crate knows nothing about, so the
//! path is computed here and delivered above. An engine component is the
//! other case entirely: a [`CustomElement`](crate::CustomElement) handler is Rust, it is owned by
//! this document, and it is already called from inside mutations this crate
//! runs ([`connected_callback`](crate::CustomElement::connected_callback) and its
//! siblings). Handing the path *up* so the layer above can call back *down*
//! per step would buy nothing and cost a crossing per node.
//!
//! So [`Document::dispatch_element_event`] is the whole dispatch:
//! [`Document::event_steps`] builds the standard's path, the walk runs here,
//! and [`CustomElement::handle_event`](crate::CustomElement::handle_event) is invoked on every step
//! whose node is a defined custom element. A `<list>`, an `<image>` or any other engine
//! component can therefore hear an event the engine itself decides.
//!
//! # What this path is not
//!
//! It is **not** the script event system, and nothing on it reaches a realm.
//! An `addEventListener` registration lives in the realm, the host keeps no
//! index of it, and this walk consults nothing but the definition table — so
//! a page that listens for the same event name in JavaScript hears nothing
//! from here, deliberately. An event that script must see crosses the boundary
//! the routed-input path already owns (`bobcat-core`'s
//! `__BobcatDispatchEvent`), and an event both must see would have to be
//! dispatched on both, once each.
//!
//! # The walk
//!
//! - **Capture pass, root inward, then bubble pass, target outward**, exactly the order
//!   [`EventSteps`](super::EventSteps) lists, with `bubbles` and `composed` deciding the path as
//!   they do for any other event.
//! - **The target hears it once.** The standard visits the target in both passes because each pass
//!   runs a different registration set — the capture one, then the bubble one. A component has one
//!   `handle_event`, not a set per phase, so the at-target capture step is skipped rather than
//!   delivered twice; [`EventPhase::AtTarget`] is what that one delivery reports. Every
//!   shadow-adjusted target on the path follows the same rule.
//! - **Liveness is re-checked per step, not per dispatch.** The path is taken once, up front, and
//!   every hook runs with `&mut Document`, so a handler may remove or free nodes the rest of the
//!   path names. A step whose node is gone — or is no longer a constructed custom element —
//!   delivers nothing and the walk continues, which is the same "resolves to nothing rather than to
//!   whatever took its storage" rule a script path already has.
//! - **Propagation stops where a handler says.** [`ElementEvent::stop_propagation`] is read after
//!   each call, so a handler that stops during the capture pass keeps the target itself from
//!   hearing the event.
//!
//! # Where the reaction queue drains
//!
//! Each hook call is its own `[CEReactions]` scope: the queue is opened
//! before the call and drained the moment it returns, before the next step.
//! That is where a browser's boundary is — every event-listener invocation is
//! wrapped in one, so lifecycle callbacks run *between* listeners rather than
//! after all of them.
//!
//! In this crate that scope is empty today, and stating why is the point of
//! having it: every public mutation opens and drains a scope of its own, so a
//! handler that appends a child has already seen that child's
//! `connected_callback` run inside its own `append_child`, before the handler
//! even returned. The scope here is what makes "nothing a handler raised
//! outlives its step" true by construction rather than by inspecting every
//! mutation method, and it is where a reaction this walk itself ever raises
//! would drain. It costs two integer operations when nothing is queued.
//!
//! # Cost when nothing listens
//!
//! A document with no definitions returns before the path is built, so a
//! dispatch costs one `is_empty` check. A document that *has* definitions
//! pays the path — the same `SmallVec` walk a routed event pays — plus one
//! load per step for the node's definition pointer.

use crate::tree::document::{Document, NodeId};

/// Which pass of the walk a delivery belongs to — DOM's `Event.eventPhase`,
/// minus the `NONE` an event that is not being dispatched reports.
///
/// There is no `NONE` here because an [`ElementEvent`] exists only for the
/// length of one dispatch: it is created by the walk and borrowed out to each
/// handler, and nothing can hold one past the end.
///
/// <https://dom.spec.whatwg.org/#dom-event-eventphase>
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EventPhase {
    /// Root inward, above the target.
    Capturing = 1,
    /// The target itself, or a host standing in for a node inside its shadow
    /// tree. Delivered once, whatever `bubbles` says.
    AtTarget = 2,
    /// Target outward, above the target. Absent when the event does not
    /// bubble.
    Bubbling = 3,
}

impl EventPhase {
    /// The DOM constant, for a consumer that reports the number.
    #[must_use]
    pub fn value(self) -> u16 {
        self as u16
    }
}

/// What an [`ElementEvent`] is, and the payload that goes with it.
///
/// One variant today. The enum is the extension point: an engine-decided
/// event that a component must hear adds a variant here, and every handler
/// that does not match it keeps compiling — which is what keeps the hook one
/// method rather than one method per event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ElementEventKind {
    /// [css-contain-2 §4.4](https://drafts.csswg.org/css-contain-2/#content-visibility-auto-state-change-event):
    /// a `content-visibility: auto` element started or stopped skipping its
    /// contents, decided by the commit that determined relevance.
    ContentVisibilityAutoStateChange {
        /// The state the element changed **to**: `true` when it now skips.
        skipped: bool,
    },
}

impl ElementEventKind {
    /// The event's type, as the web platform spells it.
    ///
    /// Nothing in this crate routes on the string — the walk matches the
    /// variant — but a handler that logs or forwards one needs the name the
    /// specification gives it.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::ContentVisibilityAutoStateChange { .. } => "contentvisibilityautostatechange",
        }
    }
}

/// One in-flight dispatch, as the handler on each step sees it.
///
/// The read half is the DOM event interface this crate can answer — `type`,
/// `target`, `currentTarget`, `eventPhase` — and the write half is the two
/// stop methods. There is no `preventDefault`: nothing on this path is
/// cancelable, because the engine decides the event *after* the fact it
/// reports has already happened.
#[derive(Debug)]
pub struct ElementEvent {
    kind: ElementEventKind,
    target: NodeId,
    current_target: NodeId,
    phase: EventPhase,
    stopped: bool,
}

impl ElementEvent {
    /// What happened, and its payload.
    #[must_use]
    pub fn kind(&self) -> ElementEventKind {
        self.kind
    }

    /// The node the event was dispatched at, as this step may see it: the
    /// dispatch target, or the shadow host standing in for it above a
    /// boundary the event crossed.
    #[must_use]
    pub fn target(&self) -> NodeId {
        self.target
    }

    /// The node whose handler is running — always the `element` argument of
    /// the call it is running in.
    #[must_use]
    pub fn current_target(&self) -> NodeId {
        self.current_target
    }

    #[must_use]
    pub fn phase(&self) -> EventPhase {
        self.phase
    }

    /// Ends the walk after this handler returns: no node further along the
    /// path hears the event.
    pub fn stop_propagation(&mut self) {
        self.stopped = true;
    }

    /// The same, plus the rest of this node's own handlers — of which there
    /// are none.
    ///
    /// A node's local name resolves to at most one definition, so "the other
    /// listeners on this node" is empty by construction and the two methods
    /// have one effect. Both exist because a handler written against the DOM
    /// vocabulary should not have to know that, and because the day a node
    /// can carry more than one engine listener is the day they diverge.
    pub fn stop_immediate_propagation(&mut self) {
        self.stopped = true;
    }

    /// Whether a handler has ended the walk.
    #[must_use]
    pub fn propagation_stopped(&self) -> bool {
        self.stopped
    }
}

impl<T> Document<T> {
    /// Fires one engine event at `target` and runs the standard's two passes
    /// over the path, calling [`CustomElement::handle_event`](crate::CustomElement::handle_event)
    /// on every step whose node is a defined custom element.
    ///
    /// Nothing about this reaches script: the realm's own listener
    /// registrations are not consulted, and a realm is not entered. This
    /// module's own doc has the walk, the per-step liveness re-check and the
    /// reaction boundary.
    ///
    /// A target that is no longer live dispatches nothing — the rule a stale
    /// path already follows — and so does a document that defines no
    /// components at all, which is the check the whole call costs when
    /// nobody can listen.
    pub fn dispatch_element_event(
        &mut self,
        target: NodeId,
        kind: ElementEventKind,
        bubbles: bool,
        composed: bool,
    ) {
        if !self.has_custom_definitions() || self.get(target).is_none() {
            return;
        }
        // Computed once, owning no borrow: every handler below takes the
        // document mutably, and one of them may free a node this list names.
        let steps = self.event_steps(target, bubbles, composed);
        let mut event = ElementEvent {
            kind,
            target,
            current_target: target,
            phase: EventPhase::AtTarget,
            stopped: false,
        };
        for step in steps.steps() {
            let at_target = step.node == step.target;
            if at_target && step.capture {
                // One delivery per node per dispatch: the standard visits
                // the target in both passes only because each pass runs a
                // different registration set, and a component has one
                // handler rather than a set per phase.
                continue;
            }
            let Some(handler) = self.custom_element_handler(step.node) else {
                continue;
            };
            event.target = step.target;
            event.current_target = step.node;
            event.phase = if at_target {
                EventPhase::AtTarget
            } else if step.capture {
                EventPhase::Capturing
            } else {
                EventPhase::Bubbling
            };
            // One `[CEReactions]` scope per handler, as a browser wraps one
            // listener invocation: what this handler's mutations raised runs
            // before the next step, not after the walk.
            let base = self.begin_reactions();
            handler.handle_event(self, step.node, &mut event);
            self.drain_reactions(base);
            if event.stopped {
                break;
            }
        }
    }
}
