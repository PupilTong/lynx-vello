//! The painting side's input router: one place that turns routed input into
//! every decision the painter executes — which scroll the user-agent performs,
//! which event is dispatched, and at which target node.
//!
//! Routing (hit testing) stays in the committed frame
//! ([`CommittedFrame::hit`](crate::visual::CommittedFrame::hit)); the propagation
//! chain stays in the event module (`Document::event_steps`) — this layer
//! decides the *type and target*, never the chain. The painter feeds each
//! routed event in and executes the returned [`InputDecision`]s in order: a
//! scroll decision drives the painter's own chain walk over the published
//! slot table (`ScrollIntents::chain`, on
//! [`drive_chain`](crate::scroll::drive_chain)), an emit decision is handed
//! back to the painter's owner, which delivers it. That order is therefore
//! the delivery order, which is how the layer guarantees a due hold gesture
//! precedes the release that follows it and a press gesture follows its own
//! `pointerup`.
//!
//! # What lives here
//!
//! - **Raw event naming.** A routed pointer phase becomes its W3C name
//!   (`pointerdown`/`pointermove`/`pointerup`/`pointercancel`), a wheel becomes `wheel`, each
//!   targeted at the routed node. The two gestures a [`GesturePolicy`] names — a press released
//!   near where it went down ([`GesturePolicy::tap_event`]) and a hold
//!   ([`GesturePolicy::long_press_event`]) — are *synthesized* from the sequence beside them.
//! - **Compatibility mouse events** (Pointer Events Level 3 §11). A [`PointerKind::Mouse`]
//!   pointer's down, move and up are followed at once by `mousedown`/`mousemove`/`mouseup` at the
//!   same target with the same detail. Touch and pen produce none (a recorded deviation), and
//!   neither does a cancel.
//! - **`click`** (UI Events), for every pointer kind: the primary button of the primary pointer
//!   pressed and released, with no movement slop, suppressed when the drag recognizer's scroll
//!   consumed the sequence or the sequence was cancelled. The release emits one decision naming the
//!   down's target and the release's, after the release's pointer, touch and mouse events and
//!   before the press gesture; the owner targets it at their nearest common inclusive ancestor, and
//!   decides `dblclick` from consecutive clicks. The press gesture keeps its own rules and fires
//!   beside it.
//! - **Key events.** A key phase becomes `keydown`/`keyup` (W3C UI Events), carrying the W3C
//!   `key`/`code` strings, `repeat` and the modifiers. A key is never hit-tested: this side has no
//!   focus, so it names the document element ([`NodeId::DOCUMENT_ELEMENT`]), and the thread that
//!   holds the document's focused element delivers the key there when there is one.
//! - **Touch events.** A pointer that [`produces_touch_events`] also produces the W3C touch name
//!   for its phase (`touchstart`/`touchmove`/`touchend`/`touchcancel`), carrying the three lists
//!   ([`TouchPoint`]) the standard gives one. Every finger is tracked from its down, and the node
//!   its down routed to is that finger's target for the finger's whole life — the standard's
//!   implicit capture, so a move that leaves the node still dispatches at it. Multi-touch is always
//!   on.
//! - **User-agent scrolling.** The drag recognizer (every pointer kind, a mouse included, latched
//!   at the down on the nearest user-scrollable, [`GesturePolicy::drag_slop`] with the slop
//!   subtracted from the first movement, per-pointer independent) and wheel scrolling (per-event
//!   nearest scrollable filtered by the CSS-pixel delta's axes). The document routes with
//!   `default_prevented` set so it performs no default action of its own, and this router is the
//!   one decision point. The scroll *primitives* (`drive_chain`'s remainder chaining, the step
//!   rule, the published chain links) stay in [`crate::scroll`]. A drag that scrolled ends with a
//!   [`InputDecision::ScrollEnd`] at its release or cancel, ahead of the release's own events, so
//!   the containers it moved settle onto their css-scroll-snap-1 positions (the executor's intents
//!   own that; the router only marks the end).
//! - **Gesture synthesis**, on the [`GesturePolicy`] the router was built with: the press gesture
//!   fires at release, targeted at the down-routed node, unless the sequence travelled past the
//!   radial [`GesturePolicy::tap_slop`], the drag recognizer's scroll consumed (reported back by
//!   the executor through [`GestureRouter::note_scroll_consumed`], so a boundary drag that moves
//!   nothing keeps its press gesture), a second pointer joined, or the hold gesture was delivered.
//!   The hold gesture fires [`GesturePolicy::long_press_seconds`] into a hold that stayed within
//!   [`GesturePolicy::long_press_move_slop`], gated on a listener existing anywhere
//!   ([`RouterHost::has_listener`] — name-level, an approximation).
//!
//! The router owns no clock and holds no document: time arrives as the
//! painter's own frame-clock reading, and every document fact arrives through
//! the borrowed [`RouterHost`] queries. That keeps the core a plain state
//! machine a test can drive without an engine.
//!
//! # What this module deliberately is not
//!
//! No fling or velocity (the scroll intents measure those), no `:active`
//! driving, no per-element gesture detectors or arena relations — recorded
//! follow-ups. No `pointerover`/`out`/`enter`/`leave`,
//! `mouseover`/`out`/`enter`/`leave`, `auxclick` or `contextmenu` (deferred).
//! Scroll *events* (`scroll`, `scrollend`) are the document's, decided from
//! the offsets this side posts.

use euclid::default::{Point2D, Vector2D};
use smallvec::SmallVec;

use crate::event::{
    EventDetail, TOUCH_ACTIVE, TOUCH_CHANGED, TOUCH_TARGET, TouchPoint, TouchPoints,
};
use crate::input::{InputEvent, InputKind, KeyPhase, PointerId, PointerKind, PointerPhase};
use crate::scroll::ScrollAxes;
use crate::tree::document::NodeId;
use crate::visual::HitTarget;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GesturePolicy {
    pub tap_slop: f32,
    pub long_press_move_slop: f32,
    pub drag_slop: f32,
    pub long_press_seconds: f64,
    pub tap_event: &'static str,
    pub long_press_event: &'static str,
}

pub(super) const CLICK_EVENT: &str = "click";

#[must_use]
pub(super) fn produces_touch_events(device: PointerKind) -> bool {
    matches!(device, PointerKind::Touch | PointerKind::Pen)
}

pub trait RouterHost {
    fn nearest_user_scrollable(&self, from: HitTarget, axes: ScrollAxes) -> Option<NodeId>;

    fn contains_node(&self, node: NodeId) -> bool;

    fn has_listener(&self, name: &str) -> bool;
}

#[derive(Clone, Debug)]
pub struct EmitEvent {
    pub name: &'static str,
    pub target: NodeId,
    pub detail: EventDetail,
    pub release: Option<NodeId>,
}

#[derive(Clone, Debug)]
pub enum InputDecision {
    Scroll {
        pointer: Option<PointerId>,
        from: NodeId,
        delta: Vector2D<f32>,
    },
    ScrollEnd {
        pointer: PointerId,
    },
    Emit(EmitEvent),
}

pub type InputDecisions = SmallVec<[InputDecision; 4]>;

fn emit(name: &'static str, target: NodeId, detail: EventDetail) -> InputDecision {
    InputDecision::Emit(EmitEvent {
        name,
        target,
        detail,
        release: None,
    })
}

fn pointer_detail(
    position: Point2D<f32>,
    id: PointerId,
    device: PointerKind,
    button: i16,
    buttons: u16,
) -> EventDetail {
    EventDetail::Pointer {
        position,
        id,
        device,
        button,
        buttons,
    }
}

fn compat_mouse_event_name(device: PointerKind, phase: PointerPhase) -> Option<&'static str> {
    if device != PointerKind::Mouse {
        return None;
    }
    match phase {
        PointerPhase::Down => Some("mousedown"),
        PointerPhase::Move => Some("mousemove"),
        PointerPhase::Up => Some("mouseup"),
        _ => None,
    }
}

fn pointer_event_name(phase: PointerPhase) -> &'static str {
    match phase {
        PointerPhase::Down => "pointerdown",
        PointerPhase::Move => "pointermove",
        PointerPhase::Up => "pointerup",
        PointerPhase::Cancel => "pointercancel",
    }
}

fn key_event_name(phase: KeyPhase) -> &'static str {
    match phase {
        KeyPhase::Down => "keydown",
        KeyPhase::Up => "keyup",
    }
}

fn touch_event_name(phase: PointerPhase) -> &'static str {
    match phase {
        PointerPhase::Down => "touchstart",
        PointerPhase::Move => "touchmove",
        PointerPhase::Up => "touchend",
        PointerPhase::Cancel => "touchcancel",
    }
}

#[derive(Clone, Copy, Debug)]
struct Drag {
    pointer: PointerId,
    scroller: NodeId,
    origin: Point2D<f32>,
    scrolling: bool,
}

#[derive(Clone, Copy, Debug)]
struct ActiveTouch {
    pointer: PointerId,
    target: NodeId,
    position: Point2D<f32>,
}

#[derive(Debug)]
struct Sequence {
    pointer: PointerId,
    device: PointerKind,
    target: NodeId,
    down_position: Point2D<f32>,
    latest_position: Point2D<f32>,
    tap_allowed: bool,
    longpress_deadline: Option<f64>,
    longpress_fired: bool,
}

impl Sequence {
    fn travelled_beyond(&self, position: Point2D<f32>, slop: f32) -> bool {
        let dx = position.x - self.down_position.x;
        let dy = position.y - self.down_position.y;
        dx * dx + dy * dy > slop * slop
    }
}

#[derive(Clone, Copy, Debug)]
struct ClickCandidate {
    pointer: PointerId,
    target: NodeId,
}

#[derive(Debug)]
pub struct GestureRouter {
    policy: GesturePolicy,
    down_pointers: Vec<PointerId>,
    sequence: Option<Sequence>,
    drags: Vec<Drag>,
    touches: SmallVec<[ActiveTouch; 2]>,
    click: Option<ClickCandidate>,
}

impl GestureRouter {
    #[must_use]
    pub fn new(policy: GesturePolicy) -> Self {
        Self {
            policy,
            down_pointers: Vec::new(),
            sequence: None,
            drags: Vec::new(),
            touches: SmallVec::new(),
            click: None,
        }
    }

    #[must_use]
    pub const fn policy(&self) -> GesturePolicy {
        self.policy
    }

    pub fn on_input(
        &mut self,
        event: &InputEvent,
        target: Option<HitTarget>,
        at: f64,
        host: &impl RouterHost,
        out: &mut InputDecisions,
    ) {
        match event.kind {
            InputKind::Pointer {
                id,
                device,
                phase,
                button,
                buttons,
            } => {
                self.fire_due(at, host, out);
                self.drag_step(
                    event,
                    target,
                    id,
                    device.drags_to_scroll(),
                    phase,
                    host,
                    out,
                );
                let detail = pointer_detail(event.position, id, device, button, buttons);
                if let Some(target) = target {
                    let name = pointer_event_name(phase);
                    out.push(emit(name, target.node, detail.clone()));
                    if let Some(compat) = compat_mouse_event_name(device, phase) {
                        out.push(emit(compat, target.node, detail.clone()));
                    }
                }
                self.touch_step(event, target, id, device, phase, out);
                self.click_step(target, id, phase, button, &detail, out);
                self.synthesize(event, target, id, device, phase, at, out);
            }
            InputKind::Wheel { delta } => {
                let Some(target) = target else {
                    return;
                };
                if !event.default_prevented {
                    Self::wheel_scroll(target, delta, host, out);
                }
                out.push(emit(
                    "wheel",
                    target.node,
                    EventDetail::Wheel {
                        position: event.position,
                        delta,
                    },
                ));
            }
            InputKind::Key {
                phase,
                ref key,
                ref code,
                repeat,
                modifiers,
                ref text,
            } => {
                let name = key_event_name(phase);
                out.push(emit(
                    name,
                    NodeId::DOCUMENT_ELEMENT,
                    EventDetail::Key {
                        key: key.clone(),
                        code: code.clone(),
                        repeat,
                        modifiers,
                        text: text.clone(),
                    },
                ));
            }
        }
    }

    pub fn note_scroll_consumed(&mut self, pointer: PointerId) {
        if self
            .click
            .is_some_and(|candidate| candidate.pointer == pointer)
        {
            self.click = None;
        }
        if let Some(sequence) = self
            .sequence
            .as_mut()
            .filter(|sequence| sequence.pointer == pointer)
        {
            sequence.tap_allowed = false;
            sequence.longpress_deadline = None;
        }
    }

    pub fn on_tick(&mut self, now: f64, host: &impl RouterHost, out: &mut InputDecisions) {
        self.fire_due(now, host, out);
    }

    #[must_use]
    pub fn needs_frame(&self) -> bool {
        self.sequence
            .as_ref()
            .is_some_and(|sequence| sequence.longpress_deadline.is_some())
    }

    #[allow(clippy::too_many_arguments, reason = "one private dispatch site")]
    fn drag_step(
        &mut self,
        event: &InputEvent,
        target: Option<HitTarget>,
        id: PointerId,
        drags_to_scroll: bool,
        phase: PointerPhase,
        host: &impl RouterHost,
        out: &mut InputDecisions,
    ) {
        let drag_slop = self.policy.drag_slop;
        match phase {
            PointerPhase::Down => {
                self.drags.retain(|drag| drag.pointer != id);
                if event.default_prevented || !drags_to_scroll {
                    return;
                }
                let scroller =
                    target.and_then(|hit| host.nearest_user_scrollable(hit, ScrollAxes::BOTH));
                if let Some(scroller) = scroller {
                    self.drags.push(Drag {
                        pointer: id,
                        scroller,
                        origin: event.position,
                        scrolling: false,
                    });
                }
            }
            PointerPhase::Move => {
                let Some(drag) = self.drags.iter_mut().find(|drag| drag.pointer == id) else {
                    return;
                };
                if !host.contains_node(drag.scroller) {
                    self.drags.retain(|drag| drag.pointer != id);
                    return;
                }
                if event.default_prevented {
                    drag.origin = event.position;
                    return;
                }
                let travel = event.position - drag.origin;
                let movement = if drag.scrolling {
                    travel
                } else {
                    let distance = travel.length();
                    if distance <= drag_slop {
                        return;
                    }
                    drag.scrolling = true;
                    travel * ((distance - drag_slop) / distance)
                };
                drag.origin = event.position;
                out.push(InputDecision::Scroll {
                    pointer: Some(id),
                    from: drag.scroller,
                    delta: -movement,
                });
            }
            PointerPhase::Up | PointerPhase::Cancel => {
                let scrolled = self
                    .drags
                    .iter()
                    .any(|drag| drag.pointer == id && drag.scrolling);
                self.drags.retain(|drag| drag.pointer != id);
                if scrolled {
                    out.push(InputDecision::ScrollEnd { pointer: id });
                }
            }
        }
    }

    fn touch_step(
        &mut self,
        event: &InputEvent,
        target: Option<HitTarget>,
        id: PointerId,
        device: PointerKind,
        phase: PointerPhase,
        out: &mut InputDecisions,
    ) {
        if !produces_touch_events(device) {
            return;
        }
        let finite = event.position.x.is_finite() && event.position.y.is_finite();
        let name = touch_event_name(phase);
        let (target, changed) = match phase {
            PointerPhase::Down => {
                if !finite {
                    return;
                }
                self.touches.retain(|touch| touch.pointer != id);
                let Some(hit) = target else {
                    return;
                };
                self.touches.push(ActiveTouch {
                    pointer: id,
                    target: hit.node,
                    position: event.position,
                });
                (hit.node, event.position)
            }
            PointerPhase::Move => {
                if !finite {
                    return;
                }
                let Some(touch) = self.touches.iter_mut().find(|touch| touch.pointer == id) else {
                    return;
                };
                touch.position = event.position;
                (touch.target, event.position)
            }
            PointerPhase::Up | PointerPhase::Cancel => {
                let Some(index) = self.touches.iter().position(|touch| touch.pointer == id) else {
                    return;
                };
                let touch = self.touches.remove(index);
                (
                    touch.target,
                    if finite {
                        event.position
                    } else {
                        touch.position
                    },
                )
            }
        };
        let touches = self.touch_points(target, id, changed);
        let position = touches
            .iter()
            .find(|point| point.flags & TOUCH_ACTIVE != 0)
            .map_or(changed, |point| point.position);
        out.push(emit(
            name,
            target,
            EventDetail::Touch {
                position,
                points: touches,
            },
        ));
    }

    fn touch_points(
        &self,
        target: NodeId,
        changed: PointerId,
        changed_position: Point2D<f32>,
    ) -> TouchPoints {
        let mut points: TouchPoints = self
            .touches
            .iter()
            .map(|touch| {
                let mut flags = TOUCH_ACTIVE;
                if touch.target == target {
                    flags |= TOUCH_TARGET;
                }
                if touch.pointer == changed {
                    flags |= TOUCH_CHANGED;
                }
                TouchPoint {
                    identifier: touch.pointer,
                    position: touch.position,
                    flags,
                }
            })
            .collect();
        if !points.iter().any(|point| point.flags & TOUCH_CHANGED != 0) {
            points.push(TouchPoint {
                identifier: changed,
                position: changed_position,
                flags: TOUCH_CHANGED,
            });
        }
        points
    }

    fn wheel_scroll(
        target: HitTarget,
        delta: Vector2D<f32>,
        host: &impl RouterHost,
        out: &mut InputDecisions,
    ) {
        let axes = ScrollAxes {
            x: delta.x != 0.0,
            y: delta.y != 0.0,
        };
        let Some(scroller) = host.nearest_user_scrollable(target, axes) else {
            return;
        };
        out.push(InputDecision::Scroll {
            pointer: None,
            from: scroller,
            delta,
        });
    }

    fn click_step(
        &mut self,
        target: Option<HitTarget>,
        id: PointerId,
        phase: PointerPhase,
        button: i16,
        detail: &EventDetail,
        out: &mut InputDecisions,
    ) {
        match phase {
            PointerPhase::Down if self.down_pointers.is_empty() && button == 0 => {
                self.click = target.map(|hit| ClickCandidate {
                    pointer: id,
                    target: hit.node,
                });
            }
            PointerPhase::Up => {
                if let Some(candidate) = self.click.take_if(|candidate| candidate.pointer == id)
                    && button == 0
                    && let Some(release) = target
                {
                    out.push(InputDecision::Emit(EmitEvent {
                        name: CLICK_EVENT,
                        target: candidate.target,
                        detail: detail.clone(),
                        release: Some(release.node),
                    }));
                }
            }
            PointerPhase::Cancel if self.click.is_some_and(|candidate| candidate.pointer == id) => {
                self.click = None;
            }
            _ => {}
        }
    }

    #[allow(clippy::too_many_arguments, reason = "one private dispatch site")]
    fn synthesize(
        &mut self,
        event: &InputEvent,
        target: Option<HitTarget>,
        id: PointerId,
        device: PointerKind,
        phase: PointerPhase,
        at: f64,
        out: &mut InputDecisions,
    ) {
        let policy = self.policy;
        match phase {
            PointerPhase::Down => {
                if !self.down_pointers.contains(&id) {
                    self.down_pointers.push(id);
                }
                if self.down_pointers.len() > 1 {
                    self.sequence = None;
                } else {
                    self.sequence = target.map(|target| Sequence {
                        pointer: id,
                        device,
                        target: target.node,
                        down_position: event.position,
                        latest_position: event.position,
                        tap_allowed: true,
                        longpress_deadline: Some(at + policy.long_press_seconds),
                        longpress_fired: false,
                    });
                }
            }
            PointerPhase::Move => {
                if let Some(sequence) = self
                    .sequence
                    .as_mut()
                    .filter(|sequence| sequence.pointer == id)
                {
                    sequence.latest_position = event.position;
                    if sequence.travelled_beyond(event.position, policy.long_press_move_slop) {
                        sequence.longpress_deadline = None;
                    }
                    if sequence.travelled_beyond(event.position, policy.tap_slop) {
                        sequence.tap_allowed = false;
                    }
                }
            }
            PointerPhase::Up => {
                self.down_pointers.retain(|&pointer| pointer != id);
                if let Some(sequence) = self.sequence.take_if(|sequence| sequence.pointer == id)
                    && sequence.tap_allowed
                    && !sequence.longpress_fired
                    && !sequence.travelled_beyond(event.position, policy.tap_slop)
                {
                    out.push(emit(
                        policy.tap_event,
                        sequence.target,
                        pointer_detail(event.position, id, device, 0, 0),
                    ));
                }
            }
            PointerPhase::Cancel => {
                self.down_pointers.retain(|&pointer| pointer != id);
                if self
                    .sequence
                    .as_ref()
                    .is_some_and(|sequence| sequence.pointer == id)
                {
                    self.sequence = None;
                }
            }
        }
    }

    fn fire_due(&mut self, now: f64, host: &impl RouterHost, out: &mut InputDecisions) {
        let long_press_event = self.policy.long_press_event;
        let Some(sequence) = self.sequence.as_mut() else {
            return;
        };
        let Some(deadline) = sequence.longpress_deadline else {
            return;
        };
        if now < deadline {
            return;
        }
        sequence.longpress_deadline = None;
        if host.has_listener(long_press_event) {
            sequence.longpress_fired = true;
            out.push(emit(
                long_press_event,
                sequence.target,
                pointer_detail(
                    sequence.latest_position,
                    sequence.pointer,
                    sequence.device,
                    -1,
                    1,
                ),
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::{InputEvent, PointerKind, PointerPhase};

    const TARGET: u64 = 3;
    const OTHER: u64 = 5;
    const SCROLLER: u64 = 7;

    const POLICY: GesturePolicy = GesturePolicy {
        tap_slop: 50.0,
        long_press_move_slop: 8.0,
        drag_slop: 8.0,
        long_press_seconds: 0.5,
        tap_event: "press",
        long_press_event: "hold",
    };

    fn target() -> NodeId {
        NodeId::from_bits(TARGET).expect("a well-formed packed handle")
    }

    fn other() -> NodeId {
        NodeId::from_bits(OTHER).expect("a well-formed packed handle")
    }

    fn scroller() -> NodeId {
        NodeId::from_bits(SCROLLER).expect("a well-formed packed handle")
    }

    fn touch(id: u32, phase: PointerPhase, x: f32, y: f32) -> InputEvent {
        InputEvent::pointer(Point2D::new(x, y), id, PointerKind::Touch, phase)
    }

    fn mouse(id: u32, phase: PointerPhase, x: f32, y: f32) -> InputEvent {
        InputEvent::pointer(Point2D::new(x, y), id, PointerKind::Mouse, phase)
    }

    struct MockHost {
        scroller: Option<NodeId>,
        live: bool,
        hold_bound: bool,
    }

    impl Default for MockHost {
        fn default() -> Self {
            Self {
                scroller: None,
                live: true,
                hold_bound: true,
            }
        }
    }

    impl RouterHost for MockHost {
        fn nearest_user_scrollable(&self, _from: HitTarget, _axes: ScrollAxes) -> Option<NodeId> {
            self.scroller
        }

        fn contains_node(&self, _node: NodeId) -> bool {
            self.live
        }

        fn has_listener(&self, name: &str) -> bool {
            name == POLICY.long_press_event && self.hold_bound
        }
    }

    struct Harness {
        router: GestureRouter,
        host: MockHost,
        out: InputDecisions,
    }

    impl Harness {
        fn new() -> Self {
            Self {
                router: GestureRouter::new(POLICY),
                host: MockHost::default(),
                out: InputDecisions::new(),
            }
        }

        fn feed(&mut self, event: InputEvent, at: f64) {
            self.feed_routed(event, Some(target()), at);
        }

        #[allow(
            clippy::needless_pass_by_value,
            reason = "the cases hand over event literals"
        )]
        fn feed_routed(&mut self, event: InputEvent, hit: Option<NodeId>, at: f64) {
            let hit = hit.map(|node| HitTarget { node, scroll: None });
            self.router
                .on_input(&event, hit, at, &self.host, &mut self.out);
        }

        fn tick(&mut self, now: f64) {
            self.router.on_tick(now, &self.host, &mut self.out);
        }

        fn trace(&self) -> Vec<String> {
            self.out
                .iter()
                .map(|decision| match decision {
                    InputDecision::Scroll { from, delta, .. } => {
                        format!("scroll@{}:{},{}", from.to_bits(), delta.x, delta.y)
                    }
                    InputDecision::ScrollEnd { pointer } => format!("scrollend#{pointer}"),
                    InputDecision::Emit(event) => {
                        format!("{}@{}", event.name, event.target.to_bits())
                    }
                })
                .collect()
        }

        fn emitted(&self) -> Vec<&'static str> {
            self.out
                .iter()
                .filter_map(|decision| match decision {
                    InputDecision::Emit(event) => Some(event.name),
                    InputDecision::Scroll { .. } | InputDecision::ScrollEnd { .. } => None,
                })
                .collect()
        }

        fn touch_trace(&self) -> Vec<String> {
            self.out
                .iter()
                .filter_map(|decision| match decision {
                    InputDecision::Emit(event) if !event.detail.touch_points().is_empty() => {
                        let points: Vec<String> = event
                            .detail
                            .touch_points()
                            .iter()
                            .map(|point| {
                                let mut lists = String::new();
                                for (flag, letter) in [
                                    (TOUCH_ACTIVE, 'a'),
                                    (TOUCH_TARGET, 't'),
                                    (TOUCH_CHANGED, 'c'),
                                ] {
                                    if point.flags & flag != 0 {
                                        lists.push(letter);
                                    }
                                }
                                format!(
                                    "{}@{},{}:{lists}",
                                    point.identifier, point.position.x, point.position.y
                                )
                            })
                            .collect();
                        let position = event.detail.position().expect("a touch event has one");
                        Some(format!(
                            "{}@{} detail={},{} [{}]",
                            event.name,
                            event.target.to_bits(),
                            position.x,
                            position.y,
                            points.join(" ")
                        ))
                    }
                    _ => None,
                })
                .collect()
        }
    }

    #[test]
    fn a_key_is_keydown_then_keyup_at_the_document_element_with_its_strings() {
        use crate::input::{KeyPhase, Modifiers};

        let mut harness = Harness::new();
        let shift = Modifiers {
            shift: true,
            ..Modifiers::default()
        };
        harness.feed_routed(
            InputEvent::key(KeyPhase::Down, "A", "KeyA", false, shift).with_text(Some("A".into())),
            None,
            0.0,
        );
        harness.feed_routed(
            InputEvent::key(KeyPhase::Up, "A", "KeyA", false, shift),
            Some(target()),
            0.1,
        );
        assert_eq!(
            harness.trace(),
            [
                format!("keydown@{}", NodeId::DOCUMENT_ELEMENT.to_bits()),
                format!("keyup@{}", NodeId::DOCUMENT_ELEMENT.to_bits()),
            ],
            "no focus here: the document element is the target, whatever routing hit"
        );
        let InputDecision::Emit(down) = &harness.out[0] else {
            panic!("an emit");
        };
        assert_eq!(
            down.detail,
            EventDetail::Key {
                key: "A".into(),
                code: "KeyA".into(),
                repeat: false,
                modifiers: shift,
                text: Some("A".into()),
            }
        );
        assert!(!harness.router.needs_frame(), "a key arms nothing");
    }

    fn pen(id: u32, phase: PointerPhase, x: f32, y: f32) -> InputEvent {
        InputEvent::pointer(Point2D::new(x, y), id, PointerKind::Pen, phase)
    }

    fn clicks(harness: &Harness) -> Vec<String> {
        harness
            .out
            .iter()
            .filter_map(|decision| match decision {
                InputDecision::Emit(event) if event.name == CLICK_EVENT => Some(format!(
                    "click@{}>{}",
                    event.target.to_bits(),
                    event.release.map_or(0, NodeId::to_bits)
                )),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_mouse_gets_compatibility_mouse_events_right_after_its_pointer_events() {
        let mut harness = Harness::new();
        harness.feed(mouse(1, PointerPhase::Down, 10.0, 10.0), 0.0);
        harness.feed(mouse(1, PointerPhase::Move, 11.0, 10.0), 0.05);
        harness.feed(mouse(1, PointerPhase::Up, 11.0, 10.0), 0.1);
        assert_eq!(
            harness.trace(),
            [
                "pointerdown",
                "mousedown",
                "pointermove",
                "mousemove",
                "pointerup",
                "mouseup",
                "click",
                "press"
            ]
            .map(|name| format!("{name}@{TARGET}")),
            "each at the pointer event's own target, then the click and the press"
        );
        let InputDecision::Emit(mouse_down) = &harness.out[1] else {
            panic!("an emit");
        };
        let InputDecision::Emit(pointer_down) = &harness.out[0] else {
            panic!("an emit");
        };
        assert_eq!(
            mouse_down.detail, pointer_down.detail,
            "the same position and buttons"
        );
        assert!(matches!(
            mouse_down.detail,
            EventDetail::Pointer {
                button: 0,
                buttons: 1,
                ..
            }
        ));
    }

    #[test]
    fn touch_and_pen_get_no_compatibility_mouse_events_but_do_click() {
        for feed in [touch, pen] {
            let mut harness = Harness::new();
            harness.feed(feed(1, PointerPhase::Down, 10.0, 10.0), 0.0);
            harness.feed(feed(1, PointerPhase::Up, 10.0, 10.0), 0.1);
            let names = harness.emitted();
            assert!(
                !names.iter().any(|name| name.starts_with("mouse")),
                "got {names:?}"
            );
            assert_eq!(clicks(&harness), [format!("click@{TARGET}>{TARGET}")]);
        }
    }

    #[test]
    fn a_click_names_the_down_and_the_release_target_for_the_main_thread() {
        let mut harness = Harness::new();
        harness.feed_routed(
            mouse(1, PointerPhase::Down, 10.0, 10.0),
            Some(target()),
            0.0,
        );
        harness.feed_routed(mouse(1, PointerPhase::Up, 200.0, 10.0), Some(other()), 0.1);
        assert_eq!(
            clicks(&harness),
            [format!("click@{TARGET}>{OTHER}")],
            "no slop: the common ancestor is resolved where the tree is"
        );

        harness.out.clear();
        harness.feed(mouse(1, PointerPhase::Down, 10.0, 10.0), 0.2);
        harness.feed_routed(mouse(1, PointerPhase::Up, 10.0, 10.0), None, 0.3);
        assert_eq!(clicks(&harness), [] as [String; 0]);
    }

    #[test]
    fn a_consumed_scroll_or_a_cancel_suppresses_the_click() {
        let mut harness = Harness::new();
        harness.host.scroller = Some(scroller());
        harness.feed(touch(1, PointerPhase::Down, 10.0, 10.0), 0.0);
        harness.feed(touch(1, PointerPhase::Move, 10.0, 40.0), 0.05);
        harness.router.note_scroll_consumed(1);
        harness.feed(touch(1, PointerPhase::Up, 10.0, 40.0), 0.1);
        assert!(clicks(&harness).is_empty(), "the scroll claimed it");

        harness.feed(touch(2, PointerPhase::Down, 10.0, 10.0), 0.2);
        harness.feed(touch(2, PointerPhase::Cancel, 10.0, 10.0), 0.25);
        harness.feed(touch(2, PointerPhase::Up, 10.0, 10.0), 0.3);
        assert!(clicks(&harness).is_empty(), "the cancel ended it");
    }

    #[test]
    fn only_the_primary_pointer_and_button_click() {
        let mut harness = Harness::new();
        harness.feed(touch(1, PointerPhase::Down, 10.0, 10.0), 0.0);
        harness.feed(touch(2, PointerPhase::Down, 50.0, 10.0), 0.01);
        harness.feed(touch(2, PointerPhase::Up, 50.0, 10.0), 0.02);
        harness.feed(touch(1, PointerPhase::Up, 10.0, 10.0), 0.03);
        assert_eq!(
            clicks(&harness),
            [format!("click@{TARGET}>{TARGET}")],
            "the second finger is not primary; the first still clicks"
        );

        harness.out.clear();
        let secondary = |phase, buttons| mouse(1, phase, 10.0, 10.0).with_buttons(2, buttons);
        harness.feed(secondary(PointerPhase::Down, 2), 0.1);
        harness.feed(secondary(PointerPhase::Up, 0), 0.2);
        assert!(
            clicks(&harness).is_empty(),
            "a secondary button does not click"
        );
    }

    #[test]
    fn a_quick_release_is_pointer_events_then_a_press_at_the_release_point() {
        let mut harness = Harness::new();
        harness.feed(touch(1, PointerPhase::Down, 10.0, 10.0), 0.0);
        harness.feed(touch(1, PointerPhase::Up, 12.0, 11.0), 0.1);
        assert_eq!(
            harness.emitted(),
            [
                "pointerdown",
                "touchstart",
                "pointerup",
                "touchend",
                "click",
                "press"
            ],
            "the touch event rides between the raw event and the synthesis, and the click \
             precedes the press"
        );
        let InputDecision::Emit(press) = &harness.out[5] else {
            panic!("the last decision is the press");
        };
        assert_eq!(press.target, target());
        let position = press.detail.position().expect("a press carries a position");
        assert!((position.x - 12.0).abs() < f32::EPSILON);
    }

    #[test]
    fn travel_beyond_the_tap_slop_cancels_the_press() {
        let mut harness = Harness::new();
        harness.feed(touch(1, PointerPhase::Down, 10.0, 10.0), 0.0);
        harness.feed(touch(1, PointerPhase::Move, 70.0, 10.0), 0.05);
        harness.feed(touch(1, PointerPhase::Up, 12.0, 10.0), 0.1);
        assert_eq!(
            harness.emitted(),
            [
                "pointerdown",
                "touchstart",
                "pointermove",
                "touchmove",
                "pointerup",
                "touchend",
                "click"
            ],
            "60px of travel emits no press, and a click has no slop"
        );
    }

    #[test]
    fn a_release_far_from_the_down_is_not_a_press_even_without_moves() {
        let mut harness = Harness::new();
        harness.feed(touch(1, PointerPhase::Down, 10.0, 10.0), 0.0);
        harness.feed(touch(1, PointerPhase::Up, 100.0, 10.0), 0.1);
        assert_eq!(
            harness.emitted(),
            [
                "pointerdown",
                "touchstart",
                "pointerup",
                "touchend",
                "click"
            ],
            "the click is at the two targets' common ancestor, whatever the distance"
        );
    }

    #[test]
    fn travel_within_the_tap_slop_keeps_the_press_but_kills_the_hold() {
        let mut harness = Harness::new();
        harness.feed(touch(1, PointerPhase::Down, 10.0, 10.0), 0.0);
        harness.feed(touch(1, PointerPhase::Move, 30.0, 10.0), 0.05);
        assert!(!harness.router.needs_frame(), "the deadline is disarmed");
        harness.tick(1.0);
        harness.feed(touch(1, PointerPhase::Up, 30.0, 10.0), 1.1);
        assert_eq!(
            harness.emitted(),
            [
                "pointerdown",
                "touchstart",
                "pointermove",
                "touchmove",
                "pointerup",
                "touchend",
                "click",
                "press"
            ],
            "no hold, press survives"
        );
    }

    #[test]
    fn a_held_pointer_fires_hold_once_and_suppresses_the_press() {
        let mut harness = Harness::new();
        harness.feed(touch(1, PointerPhase::Down, 10.0, 10.0), 0.0);
        assert!(harness.router.needs_frame(), "a deadline is armed");
        harness.tick(0.3);
        assert_eq!(
            harness.emitted(),
            ["pointerdown", "touchstart"],
            "not due yet"
        );
        harness.tick(0.6);
        assert_eq!(
            harness.emitted(),
            ["pointerdown", "touchstart", "hold"],
            "the deadline emits nothing touch-related"
        );
        assert!(!harness.router.needs_frame(), "the deadline lapsed");
        harness.tick(0.7);
        assert_eq!(
            harness.emitted(),
            ["pointerdown", "touchstart", "hold"],
            "it fires exactly once"
        );
        harness.feed(touch(1, PointerPhase::Up, 10.0, 10.0), 0.8);
        assert_eq!(
            harness.emitted(),
            [
                "pointerdown",
                "touchstart",
                "hold",
                "pointerup",
                "touchend",
                "click"
            ],
            "the release emits no press; a held pointer still clicks"
        );
    }

    #[test]
    fn a_release_arriving_after_the_deadline_emits_hold_before_its_pointerup() {
        let mut harness = Harness::new();
        harness.feed(touch(1, PointerPhase::Down, 10.0, 10.0), 0.0);
        harness.feed(touch(1, PointerPhase::Up, 10.0, 10.0), 0.9);
        assert_eq!(
            harness.emitted(),
            [
                "pointerdown",
                "touchstart",
                "hold",
                "pointerup",
                "touchend",
                "click"
            ],
            "the decision order is the delivery order"
        );
    }

    #[test]
    fn without_a_hold_listener_the_deadline_lapses_and_press_survives() {
        let mut harness = Harness::new();
        harness.host.hold_bound = false;
        harness.feed(touch(1, PointerPhase::Down, 10.0, 10.0), 0.0);
        harness.tick(0.6);
        harness.feed(touch(1, PointerPhase::Up, 10.0, 10.0), 0.9);
        assert_eq!(
            harness.emitted(),
            [
                "pointerdown",
                "touchstart",
                "pointerup",
                "touchend",
                "click",
                "press"
            ],
            "an unconsumed hold does not suppress the press"
        );
    }

    #[test]
    fn a_consumed_scroll_cancels_both_syntheses() {
        let mut harness = Harness::new();
        harness.host.scroller = Some(scroller());
        harness.feed(touch(1, PointerPhase::Down, 10.0, 10.0), 0.0);
        harness.feed(touch(1, PointerPhase::Move, 10.0, 40.0), 0.05);
        assert!(
            matches!(harness.out[2], InputDecision::Scroll { .. }),
            "the drag crossed its slop and decided a scroll, before the raw move"
        );
        harness.router.note_scroll_consumed(1);
        assert!(!harness.router.needs_frame());
        harness.tick(0.6);
        harness.feed(touch(1, PointerPhase::Up, 10.0, 40.0), 0.7);
        assert_eq!(
            harness.emitted(),
            [
                "pointerdown",
                "touchstart",
                "pointermove",
                "touchmove",
                "pointerup",
                "touchend"
            ],
            "a claimed sequence synthesizes nothing"
        );
    }

    #[test]
    fn a_scrolling_drag_ends_with_a_scroll_end_ahead_of_its_release_events() {
        let mut harness = Harness::new();
        harness.host.scroller = Some(scroller());
        harness.feed(touch(1, PointerPhase::Down, 10.0, 10.0), 0.0);
        harness.feed(touch(1, PointerPhase::Move, 10.0, 40.0), 0.05);
        harness.out.clear();
        harness.feed(touch(1, PointerPhase::Cancel, 10.0, 40.0), 0.1);
        assert_eq!(
            harness.trace(),
            [
                "scrollend#1".to_owned(),
                format!("pointercancel@{}", target().to_bits()),
                format!("touchcancel@{}", target().to_bits()),
            ],
            "the end is decided before the release's own events, on cancel too"
        );

        harness.feed(touch(2, PointerPhase::Down, 10.0, 10.0), 0.2);
        harness.feed(touch(2, PointerPhase::Move, 12.0, 12.0), 0.25);
        harness.out.clear();
        harness.feed(touch(2, PointerPhase::Up, 12.0, 12.0), 0.3);
        assert!(
            !harness
                .trace()
                .iter()
                .any(|line| line.starts_with("scrollend")),
            "got {:?}",
            harness.trace()
        );
    }

    #[test]
    fn an_unconsumed_boundary_drag_keeps_its_press() {
        let mut harness = Harness::new();
        harness.host.scroller = Some(scroller());
        harness.feed(touch(1, PointerPhase::Down, 10.0, 10.0), 0.0);
        harness.feed(touch(1, PointerPhase::Move, 10.0, 30.0), 0.05);
        harness.feed(touch(1, PointerPhase::Up, 10.0, 30.0), 0.1);
        assert_eq!(
            harness.emitted(),
            [
                "pointerdown",
                "touchstart",
                "pointermove",
                "touchmove",
                "pointerup",
                "touchend",
                "click",
                "press"
            ],
            "an 8-50px nudge that moved nothing still emits the press"
        );
    }

    #[test]
    fn the_first_drag_movement_subtracts_the_slop_and_later_ones_do_not() {
        let mut harness = Harness::new();
        harness.host.scroller = Some(scroller());
        harness.feed(touch(1, PointerPhase::Down, 10.0, 10.0), 0.0);
        harness.feed(touch(1, PointerPhase::Move, 10.0, 26.0), 0.05);
        harness.feed(touch(1, PointerPhase::Move, 10.0, 30.0), 0.1);
        let scrolls: Vec<Vector2D<f32>> = harness
            .out
            .iter()
            .filter_map(|decision| match decision {
                InputDecision::Scroll { delta, .. } => Some(*delta),
                InputDecision::Emit(_) | InputDecision::ScrollEnd { .. } => None,
            })
            .collect();
        assert_eq!(scrolls.len(), 2);
        assert!(
            (scrolls[0].y - -8.0).abs() < 1e-4,
            "16px of travel minus the 8px slop scrolls 8px, got {}",
            scrolls[0].y
        );
        assert!(
            (scrolls[1].y - -4.0).abs() < 1e-4,
            "later movements scroll their full travel, got {}",
            scrolls[1].y
        );
    }

    #[test]
    fn movement_within_the_drag_slop_decides_no_scroll() {
        let mut harness = Harness::new();
        harness.host.scroller = Some(scroller());
        harness.feed(touch(1, PointerPhase::Down, 10.0, 10.0), 0.0);
        harness.feed(touch(1, PointerPhase::Move, 10.0, 15.0), 0.05);
        assert_eq!(
            harness.emitted(),
            ["pointerdown", "touchstart", "pointermove", "touchmove"]
        );
        assert!(
            !harness
                .out
                .iter()
                .any(|decision| matches!(decision, InputDecision::Scroll { .. }))
        );
    }

    #[test]
    fn a_mouse_drag_scrolls_and_its_consumed_scroll_suppresses_click_and_press() {
        let mut harness = Harness::new();
        harness.host.scroller = Some(scroller());
        harness.feed(mouse(1, PointerPhase::Down, 10.0, 10.0), 0.0);
        harness.feed(mouse(1, PointerPhase::Move, 10.0, 70.0), 0.05);
        harness.router.note_scroll_consumed(1);
        harness.feed(mouse(1, PointerPhase::Up, 10.0, 70.0), 0.1);
        assert_eq!(
            harness.trace(),
            [
                format!("pointerdown@{TARGET}"),
                format!("mousedown@{TARGET}"),
                format!("scroll@{SCROLLER}:-0,-52"),
                format!("pointermove@{TARGET}"),
                format!("mousemove@{TARGET}"),
                "scrollend#1".to_owned(),
                format!("pointerup@{TARGET}"),
                format!("mouseup@{TARGET}"),
            ],
            "a scroll decision, the mouse events beside it, and no click or press"
        );
    }

    #[test]
    fn a_mouse_wheel_is_unaffected_by_the_mouse_drag_policy() {
        let mut harness = Harness::new();
        harness.host.scroller = Some(scroller());
        harness.feed(mouse(1, PointerPhase::Down, 10.0, 10.0), 0.0);
        harness.feed(
            InputEvent::wheel(Point2D::new(10.0, 10.0), Vector2D::new(0.0, 30.0)),
            0.05,
        );
        harness.feed(mouse(1, PointerPhase::Up, 10.0, 10.0), 0.1);
        assert_eq!(
            harness.trace(),
            [
                format!("pointerdown@{TARGET}"),
                format!("mousedown@{TARGET}"),
                format!("scroll@{SCROLLER}:0,30"),
                format!("wheel@{TARGET}"),
                format!("pointerup@{TARGET}"),
                format!("mouseup@{TARGET}"),
                format!("click@{TARGET}"),
                format!("press@{TARGET}"),
            ]
        );
    }

    #[test]
    fn a_freed_scroller_ends_the_drag() {
        let mut harness = Harness::new();
        harness.host.scroller = Some(scroller());
        harness.feed(touch(1, PointerPhase::Down, 10.0, 10.0), 0.0);
        harness.host.live = false;
        harness.feed(touch(1, PointerPhase::Move, 10.0, 40.0), 0.05);
        harness.host.live = true;
        harness.feed(touch(1, PointerPhase::Move, 10.0, 80.0), 0.1);
        assert!(
            !harness
                .out
                .iter()
                .any(|decision| matches!(decision, InputDecision::Scroll { .. })),
            "the drag ended when its scroller died and does not come back"
        );
    }

    #[test]
    fn a_prevented_move_rebases_the_drag_origin() {
        let mut harness = Harness::new();
        harness.host.scroller = Some(scroller());
        harness.feed(touch(1, PointerPhase::Down, 10.0, 10.0), 0.0);
        harness.feed_routed(
            touch(1, PointerPhase::Move, 10.0, 60.0).with_default_prevented(true),
            Some(target()),
            0.05,
        );
        harness.feed(touch(1, PointerPhase::Move, 10.0, 65.0), 0.1);
        assert!(
            !harness
                .out
                .iter()
                .any(|decision| matches!(decision, InputDecision::Scroll { .. }))
        );
        assert_eq!(
            harness.touch_trace()[1],
            format!("touchmove@{TARGET} detail=10,60 [1@10,60:atc]")
        );
    }

    #[test]
    fn a_prevented_down_latches_no_drag() {
        let mut harness = Harness::new();
        harness.host.scroller = Some(scroller());
        harness.feed_routed(
            touch(1, PointerPhase::Down, 10.0, 10.0).with_default_prevented(true),
            Some(target()),
            0.0,
        );
        harness.feed(touch(1, PointerPhase::Move, 10.0, 60.0), 0.05);
        assert!(
            !harness
                .out
                .iter()
                .any(|decision| matches!(decision, InputDecision::Scroll { .. }))
        );
    }

    #[test]
    fn two_pointers_drag_independently_but_neither_presses() {
        let mut harness = Harness::new();
        harness.host.scroller = Some(scroller());
        harness.feed(touch(1, PointerPhase::Down, 10.0, 10.0), 0.0);
        harness.feed(touch(2, PointerPhase::Down, 100.0, 10.0), 0.01);
        harness.feed(touch(1, PointerPhase::Move, 10.0, 40.0), 0.05);
        harness.feed(touch(2, PointerPhase::Move, 100.0, 40.0), 0.06);
        let scrolls = harness
            .out
            .iter()
            .filter(|decision| matches!(decision, InputDecision::Scroll { .. }))
            .count();
        assert_eq!(scrolls, 2, "each pointer scrolls independently");
        harness.feed(touch(1, PointerPhase::Up, 10.0, 40.0), 0.1);
        harness.feed(touch(2, PointerPhase::Up, 100.0, 40.0), 0.15);
        assert!(
            !harness.emitted().contains(&"press"),
            "no press from a two-finger overlap"
        );

        harness.feed(touch(1, PointerPhase::Down, 10.0, 10.0), 0.2);
        harness.feed(touch(1, PointerPhase::Up, 10.0, 10.0), 0.3);
        assert!(harness.emitted().contains(&"press"));
    }

    #[test]
    fn a_cancel_ends_the_sequence_without_synthesis() {
        let mut harness = Harness::new();
        harness.feed(touch(1, PointerPhase::Down, 10.0, 10.0), 0.0);
        harness.feed(touch(1, PointerPhase::Cancel, 10.0, 10.0), 0.1);
        harness.tick(0.6);
        assert_eq!(
            harness.emitted(),
            ["pointerdown", "touchstart", "pointercancel", "touchcancel"]
        );
        assert!(!harness.router.needs_frame());
    }

    #[test]
    fn a_down_that_hit_nothing_starts_no_sequence_and_emits_nothing() {
        let mut harness = Harness::new();
        harness.feed_routed(touch(1, PointerPhase::Down, 10.0, 10.0), None, 0.0);
        assert!(!harness.router.needs_frame());
        harness.feed(touch(1, PointerPhase::Move, 11.0, 10.0), 0.05);
        harness.feed(touch(1, PointerPhase::Up, 10.0, 10.0), 0.1);
        assert_eq!(
            harness.emitted(),
            ["pointermove", "pointerup"],
            "an untracked finger emits no touch event for the rest of its life"
        );
    }

    #[test]
    fn one_finger_carries_its_own_lists_from_start_to_end() {
        let mut harness = Harness::new();
        harness.feed(touch(1, PointerPhase::Down, 10.0, 10.0), 0.0);
        harness.feed(touch(1, PointerPhase::Move, 20.0, 12.0), 0.05);
        harness.feed(touch(1, PointerPhase::Up, 22.0, 12.0), 0.1);
        assert_eq!(
            harness.touch_trace(),
            [
                format!("touchstart@{TARGET} detail=10,10 [1@10,10:atc]"),
                format!("touchmove@{TARGET} detail=20,12 [1@20,12:atc]"),
                format!("touchend@{TARGET} detail=22,12 [1@22,12:c]"),
            ]
        );
    }

    #[test]
    fn moves_stay_at_the_down_target_even_when_routing_hits_another_node() {
        let mut harness = Harness::new();
        harness.feed(touch(1, PointerPhase::Down, 10.0, 10.0), 0.0);
        harness.feed_routed(
            touch(1, PointerPhase::Move, 400.0, 400.0),
            Some(other()),
            0.05,
        );
        harness.feed_routed(touch(1, PointerPhase::Up, 400.0, 400.0), Some(other()), 0.1);
        let touch_targets: Vec<String> = harness
            .touch_trace()
            .iter()
            .map(|line| line.split_whitespace().next().unwrap_or("").to_owned())
            .collect();
        assert_eq!(
            touch_targets,
            [
                format!("touchstart@{TARGET}"),
                format!("touchmove@{TARGET}"),
                format!("touchend@{TARGET}"),
            ],
            "the down's node is the finger's target for the finger's whole life"
        );
        assert!(harness.trace().contains(&format!("pointermove@{OTHER}")));
    }

    #[test]
    fn two_fingers_on_different_targets_filter_the_lists_by_target() {
        let mut harness = Harness::new();
        harness.feed(touch(1, PointerPhase::Down, 10.0, 10.0), 0.0);
        harness.feed_routed(
            touch(2, PointerPhase::Down, 50.0, 10.0),
            Some(other()),
            0.01,
        );
        harness.feed(touch(1, PointerPhase::Move, 12.0, 10.0), 0.05);
        harness.feed(touch(1, PointerPhase::Up, 12.0, 10.0), 0.1);
        harness.feed_routed(touch(2, PointerPhase::Up, 50.0, 10.0), Some(other()), 0.15);
        assert_eq!(
            harness.touch_trace(),
            [
                format!("touchstart@{TARGET} detail=10,10 [1@10,10:atc]"),
                format!("touchstart@{OTHER} detail=10,10 [1@10,10:a 2@50,10:atc]"),
                format!("touchmove@{TARGET} detail=12,10 [1@12,10:atc 2@50,10:a]"),
                format!("touchend@{TARGET} detail=50,10 [2@50,10:a 1@12,10:c]"),
                format!("touchend@{OTHER} detail=50,10 [2@50,10:c]"),
            ]
        );
    }

    #[test]
    fn a_cancel_delivers_touchcancel_with_no_active_touches() {
        let mut harness = Harness::new();
        harness.feed(touch(1, PointerPhase::Down, 10.0, 10.0), 0.0);
        harness.feed(touch(1, PointerPhase::Cancel, 14.0, 10.0), 0.1);
        assert_eq!(
            harness.touch_trace()[1],
            format!("touchcancel@{TARGET} detail=14,10 [1@14,10:c]")
        );
    }

    #[test]
    fn a_mouse_produces_no_touch_events() {
        let mut harness = Harness::new();
        harness.feed(mouse(1, PointerPhase::Down, 10.0, 10.0), 0.0);
        harness.feed(mouse(1, PointerPhase::Move, 12.0, 10.0), 0.05);
        harness.feed(mouse(1, PointerPhase::Up, 12.0, 10.0), 0.1);
        assert_eq!(harness.touch_trace(), [] as [String; 0]);
        assert_eq!(
            harness.emitted(),
            [
                "pointerdown",
                "mousedown",
                "pointermove",
                "mousemove",
                "pointerup",
                "mouseup",
                "click",
                "press"
            ]
        );
    }

    #[test]
    fn a_scroll_claim_does_not_interrupt_the_touch_stream() {
        let mut harness = Harness::new();
        harness.host.scroller = Some(scroller());
        harness.feed(touch(1, PointerPhase::Down, 10.0, 10.0), 0.0);
        harness.feed(touch(1, PointerPhase::Move, 10.0, 40.0), 0.05);
        harness.router.note_scroll_consumed(1);
        harness.feed(touch(1, PointerPhase::Move, 10.0, 60.0), 0.1);
        harness.feed(touch(1, PointerPhase::Up, 10.0, 60.0), 0.15);
        assert_eq!(
            harness.touch_trace(),
            [
                format!("touchstart@{TARGET} detail=10,10 [1@10,10:atc]"),
                format!("touchmove@{TARGET} detail=10,40 [1@10,40:atc]"),
                format!("touchmove@{TARGET} detail=10,60 [1@10,60:atc]"),
                format!("touchend@{TARGET} detail=10,60 [1@10,60:c]"),
            ],
            "a claimed drag goes on delivering touchmove and never touchcancel"
        );
    }

    #[test]
    fn a_non_finite_move_neither_moves_a_finger_nor_dispatches() {
        let mut harness = Harness::new();
        harness.feed(touch(1, PointerPhase::Down, 10.0, 10.0), 0.0);
        harness.feed(touch(1, PointerPhase::Move, f32::NAN, 10.0), 0.05);
        harness.feed_routed(
            touch(2, PointerPhase::Down, 50.0, 10.0),
            Some(other()),
            0.06,
        );
        assert_eq!(
            harness.touch_trace(),
            [
                format!("touchstart@{TARGET} detail=10,10 [1@10,10:atc]"),
                format!("touchstart@{OTHER} detail=10,10 [1@10,10:a 2@50,10:atc]"),
            ]
        );
    }

    #[test]
    fn a_non_finite_release_still_ends_the_finger_at_its_last_point() {
        let mut harness = Harness::new();
        harness.feed(touch(1, PointerPhase::Down, 10.0, 10.0), 0.0);
        harness.feed(touch(1, PointerPhase::Move, 20.0, 10.0), 0.05);
        harness.feed(touch(1, PointerPhase::Up, f32::NAN, 10.0), 0.1);
        harness.feed(touch(2, PointerPhase::Down, 50.0, 10.0), 0.2);
        harness.feed(touch(2, PointerPhase::Up, 50.0, 10.0), 0.25);
        assert_eq!(
            harness.touch_trace(),
            [
                format!("touchstart@{TARGET} detail=10,10 [1@10,10:atc]"),
                format!("touchmove@{TARGET} detail=20,10 [1@20,10:atc]"),
                format!("touchend@{TARGET} detail=20,10 [1@20,10:c]"),
                format!("touchstart@{TARGET} detail=50,10 [2@50,10:atc]"),
                format!("touchend@{TARGET} detail=50,10 [2@50,10:c]"),
            ]
        );
    }

    #[test]
    fn a_wheel_over_a_scroller_scrolls_then_dispatches() {
        let mut harness = Harness::new();
        harness.host.scroller = Some(scroller());
        let wheel = InputEvent::wheel(Point2D::new(10.0, 10.0), Vector2D::new(0.0, 5.0));
        harness.feed(wheel, 0.0);
        assert_eq!(
            harness.trace(),
            [
                format!("scroll@{}:0,5", scroller().to_bits()),
                format!("wheel@{}", target().to_bits()),
            ],
            "the default action precedes the dispatch"
        );
    }

    #[test]
    fn a_prevented_wheel_dispatches_but_scrolls_nothing() {
        let mut harness = Harness::new();
        harness.host.scroller = Some(scroller());
        let wheel = InputEvent::wheel(Point2D::new(10.0, 10.0), Vector2D::new(0.0, 5.0))
            .with_default_prevented(true);
        harness.feed(wheel, 0.0);
        assert_eq!(harness.emitted(), ["wheel"]);
        assert!(
            !harness
                .out
                .iter()
                .any(|decision| matches!(decision, InputDecision::Scroll { .. }))
        );
    }

    #[test]
    fn a_wheel_away_from_any_scroller_only_dispatches() {
        let mut harness = Harness::new();
        let wheel = InputEvent::wheel(Point2D::new(10.0, 10.0), Vector2D::new(0.0, 5.0));
        harness.feed(wheel, 0.0);
        assert_eq!(harness.emitted(), ["wheel"]);
        assert!(
            !harness
                .out
                .iter()
                .any(|decision| matches!(decision, InputDecision::Scroll { .. }))
        );
    }

    #[test]
    fn moves_from_another_pointer_do_not_disturb_the_sequence() {
        let mut harness = Harness::new();
        harness.feed(touch(1, PointerPhase::Down, 10.0, 10.0), 0.0);
        harness.feed(touch(9, PointerPhase::Move, 500.0, 500.0), 0.05);
        harness.feed(touch(1, PointerPhase::Up, 10.0, 10.0), 0.1);
        assert!(harness.emitted().contains(&"press"));
    }
}
