//! Dispatch to the engine's own components: the walk this crate runs
//! itself, over the same path a script dispatch is handed.
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
//! So the walk is here, in two entry points over one loop:
//!
//! - [`Document::dispatch_element_event_over`] walks steps the caller already computed. The host's
//!   one dispatch entry uses it for every event script hears as well (routed input, component
//!   events, exposure): it computes the path once, runs this walk over it first, and then hands the
//!   same steps to the realm. A handler's [`ElementEvent::stop_propagation`] ends this walk and
//!   nothing else; the script dispatch that follows runs whatever happened here.
//! - [`Document::dispatch_element_event`] computes the path itself and walks it, for an event only
//!   the engine hears: `slotchange` and `contentvisibilityautostatechange`, which never reach a
//!   realm.
//!
//! [`CustomElement::handle_event`](crate::CustomElement::handle_event) is invoked on every step
//! whose node is a defined custom element. An event is identified by its W3C type name
//! ([`ElementEvent::type_name`]) and carries one [`EventDetail`] shape ([`ElementEvent::detail`]).
//!
//! # What this path is not
//!
//! It is **not** the script event system. An `addEventListener` registration
//! lives in the realm, the host keeps no index of it, and this walk consults
//! nothing but the definition table. The two walks share a path and an order
//! (this one first) and nothing else: no stop, no event object and no
//! listener set crosses between them.
//!
//! # Which names are walked
//!
//! A definition says which event types it handles, in
//! [`CustomElement::observed_events`](crate::CustomElement::observed_events), the analogue of
//! `observed_attributes`; [`Document::define`](crate::Document::define) unions them into one set
//! for the document. A dispatch of a type no definition observes is one set lookup and returns
//! before any path work, and there is no per-element registration anywhere: whether an element
//! hears an event is its definition's answer.
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
//! - **A shadow root delivers to its host.** In the platform a component hears the events of its
//!   own shadow tree on `this.shadowRoot.addEventListener(…)`; that is how it hears a non-composed
//!   event such as `slotchange`, whose path ends at the shadow root. A shadow root has no handler
//!   here, so the step at which a non-composed event's path ends — its target's own shadow root —
//!   calls the host's [`handle_event`](crate::CustomElement::handle_event) instead, in both passes
//!   as any defined element on the path is called, with `element` and `currentTarget` both the
//!   host: a shadow root as `currentTarget` is not representable when the call is the host's. Only
//!   that step, so a host never hears one event twice: a shadow root the path crosses is followed
//!   by its host, which is called as itself.
//! - **A non-bubbling event reaches a host's boundary step in the capture pass only.** The bubble
//!   pass of a non-bubbling event keeps its at-target steps alone, so for a non-composed,
//!   non-bubbling event fired inside a UA shadow tree (the `scroll` of a container in a swiper's
//!   shadow tree) the host's one delivery is [`EventPhase::Capturing`]. A handler that filters on
//!   [`EventPhase::Bubbling`] never sees it; filter on the phase only for an event that bubbles.
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
//! A type no definition observes costs one hash-set lookup, before the path
//! is built in [`Document::dispatch_element_event`] and before any step is
//! visited in [`Document::dispatch_element_event_over`]. An observed type
//! pays one definition lookup per step and one call per defined element on
//! the path.

use super::{EventDetail, EventSteps};
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

/// One in-flight dispatch, as the handler on each step sees it.
///
/// The read half is the DOM event interface this crate can answer — `type`,
/// `target`, `currentTarget`, `eventPhase` — and the write half is the two
/// stop methods. There is no `preventDefault`: nothing on this path is
/// cancelable, because the engine decides the event *after* the fact it
/// reports has already happened.
#[derive(Debug)]
pub struct ElementEvent<'a> {
    type_name: &'static str,
    detail: &'a EventDetail,
    timestamp: f64,
    target: NodeId,
    current_target: NodeId,
    phase: EventPhase,
    stopped: bool,
}

impl ElementEvent<'_> {
    /// The event's type, as the web platform spells it: `slotchange`,
    /// `pointerdown`, `contentvisibilityautostatechange`. The identity a
    /// handler matches on.
    #[must_use]
    pub fn type_name(&self) -> &'static str {
        self.type_name
    }

    /// What the event carries; see [`EventDetail`].
    #[must_use]
    pub fn detail(&self) -> &EventDetail {
        self.detail
    }

    /// When the event happened, in milliseconds on the host's timeline: the
    /// value the host's script dispatch of the same event reports as its
    /// `timestamp`. This crate owns no clock, so it is whatever the caller of
    /// the dispatch named. An engine-only event the document fires from
    /// inside a mutation, such as `slotchange`, reports `0`.
    #[must_use]
    pub fn timestamp(&self) -> f64 {
        self.timestamp
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
    /// on every step whose node is a defined custom element — and, for a
    /// non-composed event, on the host of the shadow root its path ends at.
    ///
    /// The entry point of an event only the engine hears (`slotchange`,
    /// `contentvisibilityautostatechange`): nothing about this reaches
    /// script. A dispatch that script hears too computes its path once and
    /// runs [`Self::dispatch_element_event_over`] over it before handing the
    /// same steps up. `timestamp` is [`ElementEvent::timestamp`].
    ///
    /// A name no definition observes
    /// ([`CustomElement::observed_events`](crate::CustomElement::observed_events))
    /// returns before anything else, and so does a target that is no longer
    /// live, the rule a stale path already follows.
    pub fn dispatch_element_event(
        &mut self,
        target: NodeId,
        type_name: &'static str,
        detail: &EventDetail,
        bubbles: bool,
        composed: bool,
        timestamp: f64,
    ) {
        if !self.observes_event(type_name) || self.get(target).is_none() {
            return;
        }
        // Computed once, owning no borrow: every handler below takes the
        // document mutably, and one of them may free a node this list names.
        let steps = self.event_steps(target, bubbles, composed);
        self.dispatch_element_event_over(&steps, type_name, detail, timestamp);
    }

    /// The Rust walk over a path the caller already computed with
    /// [`Self::event_steps`], for the event `type_name` carrying `detail`.
    ///
    /// The host's one dispatch entry calls this: it computes the steps once,
    /// runs this walk over them, and then hands the same steps to script.
    /// [`ElementEvent::stop_propagation`] ends this walk alone; what the
    /// caller does with the steps afterwards is the caller's.
    ///
    /// Gated on the definitions: when no
    /// [`CustomElement::observed_events`](crate::CustomElement::observed_events)
    /// names `type_name`, this is one set lookup and returns. Otherwise each
    /// step costs one definition lookup and, on a defined element, one call.
    pub fn dispatch_element_event_over(
        &mut self,
        steps: &EventSteps,
        type_name: &str,
        detail: &EventDetail,
        timestamp: f64,
    ) {
        let Some(&type_name) = self.custom_elements_observed_event(type_name) else {
            return;
        };
        // The shadow root a non-composed path ends at, whose host hears the
        // event in its place; see the module doc.
        let boundary = steps.boundary;
        let mut event = ElementEvent {
            type_name,
            detail,
            timestamp,
            target: steps.target,
            current_target: steps.target,
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
            let (element, handler) = match self.custom_element_handler(step.node) {
                Some(handler) => (step.node, handler),
                None if boundary == Some(step.node) => {
                    let Some((host, handler)) = self.shadow_host(step.node).and_then(|host| {
                        self.custom_element_handler(host)
                            .map(|handler| (host, handler))
                    }) else {
                        continue;
                    };
                    (host, handler)
                }
                None => continue,
            };
            event.target = step.target;
            event.current_target = element;
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
            handler.handle_event(self, element, &mut event);
            self.drain_reactions(base);
            if event.stopped {
                break;
            }
        }
    }
}
