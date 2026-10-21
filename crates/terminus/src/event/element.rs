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
//! - [`Document::dispatch_element_event`] computes the path itself and walks it, for an event only
//!   the engine hears: `slotchange` and `contentvisibilityautostatechange`, which never reach a
//!   realm.
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
//! answer alone — a script registration on the element changes nothing about it.
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
    /// Copied into the walk's one stop flag when the handler returns: an
    /// event object lives for one handler call, the flag for the dispatch.
    stopped: bool,
    /// Set by [`Self::stop_immediate_propagation`] alone.
    immediate: bool,
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
    /// path hears the event. The node whose handler stopped it still runs
    /// the rest of its listeners — a host's, at that step.
    pub fn stop_propagation(&mut self) {
        self.stopped = true;
    }

    /// The same, plus the rest of this node's listeners: the walk does not
    /// report the step this handler ran at, so a host runs nothing of its
    /// own there.
    ///
    /// A node's local name resolves to at most one definition, so the other
    /// listeners on the node are never another component's; they are the
    /// host's, which [`Document::advance`] reports a step for after the
    /// component handler has run.
    pub fn stop_immediate_propagation(&mut self) {
        self.stopped = true;
        self.immediate = true;
    }

    /// Whether a handler has ended the walk.
    #[must_use]
    pub fn propagation_stopped(&self) -> bool {
        self.stopped
    }
}

/// One dispatch in progress over a path the caller computed, advanced one
/// step at a time by [`Document::advance`].
///
/// Built by [`Document::event_walk`]. It borrows the steps and the detail but
/// not the document, so a host releases the document between two steps and
/// calls into script with nothing borrowed — the reason the walk is driven
/// from outside rather than run to the end in one call.
///
/// It holds the dispatch's one stop flag. A component's
/// [`ElementEvent::stop_propagation`] sets it, and from then on
/// [`Document::advance`] reports no further step, so nothing the host runs
/// per step runs for a node further along the path either.
#[derive(Debug)]
pub struct EventWalk<'a> {
    steps: &'a EventSteps,
    /// The next step to visit.
    at: usize,
    /// The observed-event set's `'static` spelling of the type, or `None`
    /// when no definition observes it: then no step looks a component up.
    type_name: Option<&'static str>,
    detail: &'a EventDetail,
    timestamp: f64,
    stopped: bool,
}

impl EventWalk<'_> {
    /// Whether a component has stopped the dispatch.
    #[must_use]
    pub fn propagation_stopped(&self) -> bool {
        self.stopped
    }
}

/// The identity of one step [`Document::advance`] visited: which node's
/// listeners run, the target that node may see, and which pass this is —
/// the fields of the [`EventStep`](super::EventStep) it came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct StepOutcome {
    pub node: NodeId,
    pub target: NodeId,
    pub capture: bool,
}

impl<T> Document<T> {
    /// Fires one engine event at `target` and runs the standard's two passes
    /// over the path, calling
    /// [`CustomElement::handle_event`](crate::tree::custom::CustomElement::handle_event)
    /// on every step whose node is a defined custom element — and, for a
    /// non-composed event, on the host of the shadow root its path ends at.
    ///
    /// The entry point of an event only the engine hears (`slotchange`,
    /// `contentvisibilityautostatechange`): nothing about this reaches
    /// script. `timestamp` is [`ElementEvent::timestamp`].
    ///
    /// A name no definition observes
    /// ([`CustomElement::observed_events`](crate::tree::custom::CustomElement::observed_events))
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

    /// The component walk over a path the caller already computed with
    /// [`Self::event_steps`], for the event `type_name` carrying `detail`,
    /// run to its end: [`Self::event_walk`] advanced until it reports no
    /// step.
    ///
    /// For a caller with nothing of its own to run per step. One that does —
    /// a host calling a realm at a step — drives the walk itself with
    /// [`Self::advance`].
    ///
    /// Gated on the definitions: when no
    /// [`CustomElement::observed_events`](crate::tree::custom::CustomElement::observed_events)
    /// names `type_name`, this is one set lookup and returns. Otherwise each
    /// step costs one definition lookup and, on a defined element, one call.
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

    /// Starts a dispatch of `type_name` carrying `detail` over `steps`,
    /// which the caller computed with [`Self::event_steps`] and owns.
    ///
    /// The type's observed-event spelling is looked up here, once, so a walk
    /// of a type no definition observes visits its steps without a
    /// definition lookup per step. Whether to build a walk at all is the
    /// caller's question: a type nothing hears ([`Self::hears`] is `false` —
    /// no registration and no observing definition) costs that one check and
    /// no path.
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

    /// Visits the next step of `walk`: runs that step's component handler,
    /// if any, and reports the step, or answers `None` once the walk has
    /// ended — at the last step, or for good once a component stopped it.
    ///
    /// The component rules are the walk's of the module doc: the at-target
    /// capture step delivers to no component (the target hears it once, on
    /// its bubble step) but is still reported, because a host's own
    /// listeners there are a different set; the shadow root a non-composed
    /// path ends at delivers to its host's handler; each handler call is its
    /// own `[CEReactions]` scope. A step whose node is no longer live is
    /// skipped without being reported.
    ///
    /// A handler's [`ElementEvent::stop_propagation`] still reports the step
    /// it stopped at — the standard runs the rest of the current node's
    /// listeners — and ends the walk after it.
    /// [`ElementEvent::stop_immediate_propagation`] reports nothing for that
    /// step, since what the host would run there are the rest of the node's
    /// listeners.
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

    /// Runs one step's component handler, if the step has one, recording a
    /// stop in `walk`. Answers whether the stop was immediate.
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
            // One delivery per node per dispatch: the standard visits the
            // target in both passes only because each pass runs a different
            // registration set, and a component has one handler rather than a
            // set per phase.
            return false;
        }
        let (element, handler) = match self.custom_element_handler(node) {
            Some(handler) => (node, handler),
            // The shadow root a non-composed path ends at, whose host hears
            // the event in its place; see the module doc.
            None if walk.steps.boundary == Some(node) => {
                let Some((host, handler)) = self.shadow_host(node).and_then(|host| {
                    self.custom_element_handler(host)
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
        // One `[CEReactions]` scope per handler, as a browser wraps one
        // listener invocation: what this handler's mutations raised runs
        // before the next step, not after the walk.
        let base = self.begin_reactions();
        handler.handle_event(self, element, &mut event);
        self.drain_reactions(base);
        walk.stopped |= event.stopped;
        event.immediate
    }
}
