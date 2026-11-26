//! Dispatch to the engine's own components: the walk this crate runs
//! itself, one step at a time, over the same path a script dispatch uses.
//!
//! A [`CustomElement`](crate::tree::custom::CustomElement) handler is Rust, it is owned by
//! this document, and it is already called from inside mutations this crate
//! runs ([`connected_callback`](crate::tree::custom::CustomElement::connected_callback) and its
//! siblings). Handing the path *up* so the layer above can call back *down*
//! per step would buy nothing and cost a crossing per node. A *script*
//! listener is the other case: its callback lives in a realm, and the host
//! must release the document before it calls one.
//!
//! So the walk is here and the host drives it:
//!
//! - [`Document::event_walk`] starts an [`EventWalk`] over steps the caller computed, and
//!   [`Document::advance`] visits one step — runs that step's component handler and reports the
//!   step ([`StepOutcome`]) so the host can run what it has there, with the document released in
//!   between. The walk holds the dispatch's one stop flag: a component's
//!   [`ElementEvent::stop_propagation`] ends it, and every later step, the host's included, is not
//!   reported.
//! - [`Document::dispatch_element_event_over`] drives that walk to its end, for a caller with
//!   nothing of its own per step.
//! - [`Document::dispatch_element_event`] computes the path itself and walks it, for an event the
//!   engine fires to its components alone: `slotchange`, and `contentvisibilityautostatechange`
//!   when a host does not also take it to script (`bobcat-core` does, through its own dispatch).
//!
//! [`CustomElement::handle_event`](crate::tree::custom::CustomElement::handle_event) is invoked on
//! every step whose node is a defined custom element. An event is identified by its W3C type name
//! ([`ElementEvent::type_name`]) and carries one [`EventDetail`] shape ([`ElementEvent::detail`]).
//!
//! # What this walk is not
//!
//! It does not call script. Which registrants listen where is the
//! listener table's ([`Document::add_listener`] and its siblings); a host
//! reads that table to decide what it runs at a reported step, and this walk
//! itself consults nothing but the definition table. The host's per-step
//! work and the components share one path, one order and one stop flag, the
//! one a component sets. Script has no channel to set it.
//!
//! # Which names are walked
//!
//! A definition says which event types it handles, in
//! [`CustomElement::observed_events`](crate::tree::custom::CustomElement::observed_events), the
//! analogue of `observed_attributes`; [`Document::define`](crate::tree::document::Document::define)
//! unions them into one set for the document. A walk of a type no definition observes looks no
//! component up on any step, and whether an element's component hears an event is its definition's
//! answer alone — a script registration on the element changes nothing about it. A step whose
//! definition does not name the type calls nothing and opens no reaction scope.
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
//!   path names. A step whose node is gone is not reported and the walk continues; one whose node
//!   is no longer a constructed custom element delivers to no component but is still reported. That
//!   is the same "resolves to nothing rather than to whatever took its storage" rule a script path
//!   already has.
//! - **A shadow root delivers to its host.** In the platform a component hears the events of its
//!   own shadow tree on `this.shadowRoot.addEventListener(…)`; that is how it hears a non-composed
//!   event such as `slotchange`, whose path ends at the shadow root. A shadow root has no handler
//!   here, so the step at which a non-composed event's path ends — its target's own shadow root —
//!   calls the host's [`handle_event`](crate::tree::custom::CustomElement::handle_event) instead,
//!   in both passes as any defined element on the path is called, with `element` and
//!   `currentTarget` both the host: a shadow root as `currentTarget` is not representable when the
//!   call is the host's. Only that step, so a host never hears one event twice: a shadow root the
//!   path crosses is followed by its host, which is called as itself.
//! - **A non-bubbling event reaches a host's boundary step in the capture pass only.** The bubble
//!   pass of a non-bubbling event keeps its at-target steps alone, so for a non-composed,
//!   non-bubbling event fired inside a UA shadow tree (the `scroll` of a container in a swiper's
//!   shadow tree) the host's one delivery is [`EventPhase::Capturing`]. A handler that filters on
//!   [`EventPhase::Bubbling`] never sees it; filter on the phase only for an event that bubbles.
//! - **Propagation stops where a handler says.** [`ElementEvent::stop_propagation`] is read after
//!   each call, so a handler that stops during the capture pass keeps the target itself from
//!   hearing the event. The step it stopped at is still reported, since the standard runs the rest
//!   of the current node's listeners; [`ElementEvent::stop_immediate_propagation`] withholds that
//!   step too.
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
//! A type nothing hears — no registration and no observing definition,
//! [`Document::hears`] — costs that check, and a host builds neither a path
//! nor a walk. A walk of a type no definition observes costs one liveness
//! check per step and no definition lookup; [`Document::dispatch_element_event`]
//! and [`Document::dispatch_element_event_over`] return after one set lookup
//! for such a type. An observed type pays one definition lookup per step and
//! one call per defined element on the path.

use super::{EventDetail, EventSteps};
use crate::tree::document::{Document, NodeId};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EventPhase {
    Capturing = 1,
    AtTarget = 2,
    Bubbling = 3,
}

impl EventPhase {
    #[must_use]
    pub fn value(self) -> u16 {
        self as u16
    }
}

#[derive(Debug)]
pub struct ElementEvent<'a> {
    type_name: &'static str,
    detail: &'a EventDetail,
    timestamp: f64,
    target: NodeId,
    current_target: NodeId,
    phase: EventPhase,
    stopped: bool,
    immediate: bool,
}

impl ElementEvent<'_> {
    #[must_use]
    pub fn type_name(&self) -> &'static str {
        self.type_name
    }

    #[must_use]
    pub fn detail(&self) -> &EventDetail {
        self.detail
    }

    #[must_use]
    pub fn timestamp(&self) -> f64 {
        self.timestamp
    }

    #[must_use]
    pub fn target(&self) -> NodeId {
        self.target
    }

    #[must_use]
    pub fn current_target(&self) -> NodeId {
        self.current_target
    }

    #[must_use]
    pub fn phase(&self) -> EventPhase {
        self.phase
    }

    pub fn stop_propagation(&mut self) {
        self.stopped = true;
    }

    pub fn stop_immediate_propagation(&mut self) {
        self.stopped = true;
        self.immediate = true;
    }

    #[must_use]
    pub fn propagation_stopped(&self) -> bool {
        self.stopped
    }
}

#[derive(Debug)]
pub struct EventWalk<'a> {
    steps: &'a EventSteps,
    at: usize,
    type_name: Option<&'static str>,
    detail: &'a EventDetail,
    timestamp: f64,
    stopped: bool,
}

impl EventWalk<'_> {
    #[must_use]
    pub fn propagation_stopped(&self) -> bool {
        self.stopped
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct StepOutcome {
    pub node: NodeId,
    pub target: NodeId,
    pub capture: bool,
}

impl<T> Document<T> {
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
        let steps = self.event_steps(target, bubbles, composed);
        self.dispatch_element_event_over(&steps, type_name, detail, timestamp);
    }

    pub fn dispatch_element_event_over(
        &mut self,
        steps: &EventSteps,
        type_name: &str,
        detail: &EventDetail,
        timestamp: f64,
    ) {
        if !self.observes_event(type_name) {
            return;
        }
        let mut walk = self.event_walk(steps, type_name, detail, timestamp);
        while self.advance(&mut walk).is_some() {}
    }

    #[must_use]
    pub fn event_walk<'a>(
        &self,
        steps: &'a EventSteps,
        type_name: &str,
        detail: &'a EventDetail,
        timestamp: f64,
    ) -> EventWalk<'a> {
        EventWalk {
            steps,
            at: 0,
            type_name: self.custom_elements_observed_event(type_name).copied(),
            detail,
            timestamp,
            stopped: false,
        }
    }

    pub fn advance(&mut self, walk: &mut EventWalk<'_>) -> Option<StepOutcome> {
        while !walk.stopped {
            let step = *walk.steps.steps().get(walk.at)?;
            walk.at += 1;
            if self.get(step.node).is_none() {
                continue;
            }
            if let Some(type_name) = walk.type_name
                && self.deliver_to_component(walk, type_name, step.node, step.target, step.capture)
            {
                return None;
            }
            return Some(StepOutcome {
                node: step.node,
                target: step.target,
                capture: step.capture,
            });
        }
        None
    }

    fn deliver_to_component(
        &mut self,
        walk: &mut EventWalk<'_>,
        type_name: &'static str,
        node: NodeId,
        target: NodeId,
        capture: bool,
    ) -> bool {
        let at_target = node == target;
        if at_target && capture {
            return false;
        }
        let (element, handler) = match self.custom_element_hearing(node, type_name) {
            Some(handler) => (node, handler),
            None if walk.steps.boundary == Some(node) => {
                let Some((host, handler)) = self.shadow_host(node).and_then(|host| {
                    self.custom_element_hearing(host, type_name)
                        .map(|handler| (host, handler))
                }) else {
                    return false;
                };
                (host, handler)
            }
            None => return false,
        };
        let mut event = ElementEvent {
            type_name,
            detail: walk.detail,
            timestamp: walk.timestamp,
            target,
            current_target: element,
            phase: if at_target {
                EventPhase::AtTarget
            } else if capture {
                EventPhase::Capturing
            } else {
                EventPhase::Bubbling
            },
            stopped: false,
            immediate: false,
        };
        let base = self.begin_reactions();
        handler.handle_event(self, element, &mut event);
        self.drain_reactions(base);
        walk.stopped |= event.stopped;
        event.immediate
    }
}
