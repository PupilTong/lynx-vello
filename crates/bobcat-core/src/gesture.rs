//! The presenting-thread input router: one place that turns routed input into
//! every decision the engine executes — which scroll the user-agent performs,
//! which event is dispatched, and at which target node.
//!
//! Routing (hit testing) stays in `dom`; the propagation chain stays in the
//! event module (`Document::event_steps`) — this layer decides the *type and
//! target*, never the chain. The engine feeds each routed event in and
//! executes the returned [`InputDecision`]s in order: a scroll decision
//! drives `Document::scroll_chain`, an emit decision goes to path
//! construction and the ordered script channel. That order is therefore the
//! delivery order, which is how the layer guarantees a due `longpress`
//! precedes the release that follows it and a `tap` follows its own
//! `pointerup`.
//!
//! # What lives here
//!
//! - **Raw event naming.** A routed pointer phase becomes its W3C name
//!   (`pointerdown`/`pointermove`/`pointerup`/`pointercancel`), a wheel becomes `wheel`, each
//!   targeted at the routed node. Lynx's `tap` and `longpress` are *synthesized* from the sequence
//!   beside them.
//! - **User-agent scrolling.** The drag recognizer (touch/pen, latched at the down on the nearest
//!   user-scrollable, 8px slop with the slop subtracted from the first movement, per-pointer
//!   independent) and wheel scrolling (per-event nearest scrollable filtered by the delta's axes,
//!   line/page delta modes resolved against the scroller). Both were `dom`'s default action; the
//!   engine now routes with `default_prevented` set so `dom` performs none, and this router is the
//!   one decision point. The scroll *primitives* (`scroll_chain`'s remainder chaining, clamping,
//!   the containing-block walk) stay in `dom`.
//! - **Gesture synthesis** per the 2026-08-21 ruling (recorded in `docs/tracking/deviations.md`):
//!   `tap` fires at release, targeted at the down-routed node, unless the sequence travelled past
//!   the 50px radial [`TAP_SLOP`], the drag recognizer's scroll consumed (reported back by the
//!   executor through [`GestureRouter::note_scroll_consumed`], so a boundary drag that moves
//!   nothing keeps its tap — the Lynx Android behavior), a second pointer joined, or a `longpress`
//!   was delivered. `longpress` fires [`LONG_PRESS_SECONDS`] into a hold that stayed within
//!   [`LONG_PRESS_MOVE_SLOP`], gated on a listener existing anywhere ([`RouterHost::has_listener`]
//!   — name-level, a recorded approximation).
//!
//! The router owns no clock and holds no document: time arrives as the
//! engine's `AnimationClock` reading, and every document fact arrives through
//! the borrowed [`RouterHost`] queries. That keeps the core a plain state
//! machine a test can drive without an engine.
//!
//! # What this module deliberately is not
//!
//! No fling or velocity, no `:active` driving, no `consume-slide-event`, no
//! per-element `GestureDetector`/arena relations, no `click` — recorded
//! follow-ups. Scroll *events* (`scroll`/`scrolltolower`…) are component
//! events above this layer.

use dom::input::{DeltaMode, InputEvent, InputKind, PointerId, PointerPhase};
use dom::scroll::ScrollAxes;
use dom::{NodeId, Point2D, Vector2D};

/// How far a sequence may travel, in viewport CSS px, and still deliver `tap`.
///
/// Lynx reads this from the page config (`tapSlop`, default `"50px"`); wiring
/// the config key is a recorded follow-up, so today every view uses the
/// default.
pub(crate) const TAP_SLOP: f32 = 50.0;

/// Movement beyond this, in viewport CSS px, cancels a pending `longpress`.
///
/// Lynx cancels its long-press timer at the platform touch slop (nominally
/// 8dp on Android) — the same threshold the drag recognizer latches at, so on
/// scrollable content the long-press cancel and the scroll claim engage at
/// the same travel.
pub(crate) const LONG_PRESS_MOVE_SLOP: f32 = 8.0;

/// How far a touch/pen drag travels before it starts scrolling, in viewport
/// CSS px. The first movement subtracts the slop instead of jumping.
pub(crate) const DRAG_SLOP: f32 = 8.0;

/// How many CSS px one wheel "line" scrolls ([`DeltaMode::Line`]).
pub(crate) const WHEEL_LINE_PX: f32 = 40.0;

/// How long a pointer must stay down before `longpress` fires.
///
/// Lynx's default on every platform (Android `getLongPressTimeout()`, iOS
/// `minimumPressDuration`, Harmony). The page-config override
/// (`longPressDuration`) is a recorded follow-up.
pub(crate) const LONG_PRESS_SECONDS: f64 = 0.5;

/// The event name a held sequence synthesizes.
pub(crate) const LONG_PRESS_EVENT: &str = "longpress";

/// The event name a released sequence synthesizes.
pub(crate) const TAP_EVENT: &str = "tap";

/// The document facts the router may ask for while deciding. Borrowed for
/// exactly one call; the router retains nothing of the host's.
pub(crate) trait RouterHost {
    /// The nearest ancestor-or-self the user may scroll on any of `axes`.
    fn nearest_user_scrollable(&self, node: NodeId, axes: ScrollAxes) -> Option<NodeId>;

    /// Whether `node` still names a live node. A latched scroller can be
    /// freed mid-gesture; its id is retired rather than reissued, so this is
    /// a plain liveness question and the drag simply ends.
    fn contains_node(&self, node: NodeId) -> bool;

    /// The scroller's visible area, for resolving [`DeltaMode::Page`].
    fn scrollport_size(&self, node: NodeId) -> Option<(f32, f32)>;

    /// Whether any listener for `name` exists anywhere in the document.
    fn has_listener(&self, name: &str) -> bool;
}

/// One event this layer decided to dispatch: the type and the target. The
/// propagation chain is the event module's to compute from the target.
#[derive(Clone, Copy, Debug)]
pub(crate) struct EmitEvent {
    pub(crate) name: &'static str,
    /// The node the event targets. It may have been freed since the decision
    /// was formed — the executor checks liveness before building a path.
    pub(crate) target: NodeId,
    /// The device position the event's `detail` reports.
    pub(crate) position: Point2D<f32>,
    /// The wheel delta, for the one event whose `detail` carries one.
    pub(crate) wheel: Option<Vector2D<f32>>,
}

/// One decision the engine executes, in order.
#[derive(Clone, Copy, Debug)]
pub(crate) enum InputDecision {
    /// Drive the user-agent scroll chain from `from` by `delta` CSS px.
    /// `pointer` names the drag the scroll belongs to, so consumption can be
    /// reported back; a wheel scroll has none.
    Scroll {
        pointer: Option<PointerId>,
        from: NodeId,
        delta: Vector2D<f32>,
    },
    /// Dispatch one event through the ordinary path.
    Emit(EmitEvent),
}

fn emit(name: &'static str, target: NodeId, position: Point2D<f32>) -> InputDecision {
    InputDecision::Emit(EmitEvent {
        name,
        target,
        position,
        wheel: None,
    })
}

/// The W3C name a routed pointer phase dispatches under. Lynx's
/// `tap`/`longpress` are synthesized from the whole sequence beside these.
fn pointer_event_name(phase: PointerPhase) -> Option<&'static str> {
    match phase {
        PointerPhase::Down => Some("pointerdown"),
        PointerPhase::Move => Some("pointermove"),
        PointerPhase::Up => Some("pointerup"),
        PointerPhase::Cancel => Some("pointercancel"),
        // `PointerPhase` is `#[non_exhaustive]` so keyboard and focus can
        // arrive without a break; an unnamed phase dispatches nothing rather
        // than guessing.
        _ => None,
    }
}

/// One pointer's latched scroll drag.
#[derive(Clone, Copy, Debug)]
struct Drag {
    pointer: PointerId,
    scroller: NodeId,
    origin: Point2D<f32>,
    scrolling: bool,
}

/// One pointer sequence being watched from down to release for synthesis.
#[derive(Debug)]
struct Sequence {
    pointer: PointerId,
    target: NodeId,
    down_position: Point2D<f32>,
    latest_position: Point2D<f32>,
    /// Cleared by travel beyond [`TAP_SLOP`] or a consumed scroll.
    tap_allowed: bool,
    /// Armed at down; disarmed by travel beyond [`LONG_PRESS_MOVE_SLOP`], a
    /// consumed scroll, or expiry.
    longpress_deadline: Option<f64>,
    /// A `longpress` was delivered for this sequence, which is what makes the
    /// release not a `tap`.
    longpress_fired: bool,
}

impl Sequence {
    fn travelled_beyond(&self, position: Point2D<f32>, slop: f32) -> bool {
        let dx = position.x - self.down_position.x;
        let dy = position.y - self.down_position.y;
        dx * dx + dy * dy > slop * slop
    }
}

/// The per-view router state. Owned by the engine, fed on the presenting
/// thread only.
#[derive(Debug, Default)]
pub(crate) struct GestureRouter {
    /// Every pointer currently down, in down order. Length is what matters
    /// for synthesis: more than one cancels it for the whole overlap. Drags
    /// stay per-pointer regardless — each pointer scrolls independently.
    down_pointers: Vec<PointerId>,
    /// The one sequence that can still synthesize. `None` while no pointer is
    /// down, while more than one is, or after the sequence disqualified
    /// itself.
    sequence: Option<Sequence>,
    /// The scroll drags currently latched to a pointer.
    drags: Vec<Drag>,
}

impl GestureRouter {
    /// Feeds one routed input event and appends the decisions it produces.
    ///
    /// `target` is what routing hit and `at` is the engine clock's reading
    /// when the event arrived — arrival time rather than drain time, so a
    /// sequence buffered behind a busy document keeps its real duration. Due
    /// deadlines resolve first, so their emits precede the event's own on
    /// the ordered channel. Within one pointer event the order is: due
    /// `longpress`, the scroll decision (the user-agent default action runs
    /// first, as it always has), the raw event, then a synthesized `tap`.
    ///
    /// The event's own `default_prevented` is the embedder's suppression
    /// seam: a prevented event produces no scroll decision (a prevented move
    /// re-bases the drag origin, exactly as `dom`'s recognizer did) but still
    /// dispatches and still feeds synthesis.
    pub(crate) fn on_input(
        &mut self,
        event: &InputEvent,
        target: Option<NodeId>,
        at: f64,
        host: &impl RouterHost,
        out: &mut Vec<InputDecision>,
    ) {
        match event.kind {
            InputKind::Pointer { id, device, phase } => {
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
                if let Some(target) = target
                    && let Some(name) = pointer_event_name(phase)
                {
                    out.push(emit(name, target, event.position));
                }
                self.synthesize(event, target, id, phase, at, out);
            }
            InputKind::Wheel { delta, mode } => {
                let Some(target) = target else {
                    return;
                };
                if !event.default_prevented {
                    Self::wheel_scroll(target, delta, mode, host, out);
                }
                out.push(InputDecision::Emit(EmitEvent {
                    name: "wheel",
                    target,
                    position: event.position,
                    wheel: Some(delta),
                }));
            }
            // `InputKind` is `#[non_exhaustive]`; an unknown kind decides
            // nothing rather than guessing.
            _ => {}
        }
    }

    /// The executor reports that a scroll decision for `pointer`'s drag
    /// actually moved something. That consumption — not the drag alone — is
    /// what claims the sequence away from synthesis, so a drag at a
    /// scroller's boundary that moves nothing keeps its tap, matching Lynx.
    pub(crate) fn note_scroll_consumed(&mut self, pointer: PointerId) {
        if let Some(sequence) = self
            .sequence
            .as_mut()
            .filter(|sequence| sequence.pointer == pointer)
        {
            sequence.tap_allowed = false;
            sequence.longpress_deadline = None;
        }
    }

    /// Fires a due long-press deadline against the frame clock.
    ///
    /// The engine calls this once per produced frame; while a deadline is
    /// armed [`Self::needs_frame`] keeps frames coming, the same continuation
    /// contract running animations use.
    pub(crate) fn on_tick(
        &mut self,
        now: f64,
        host: &impl RouterHost,
        out: &mut Vec<InputDecision>,
    ) {
        self.fire_due(now, host, out);
    }

    /// Whether a deadline is armed and so owes the timeline another frame.
    pub(crate) fn needs_frame(&self) -> bool {
        self.sequence
            .as_ref()
            .is_some_and(|sequence| sequence.longpress_deadline.is_some())
    }

    /// The drag half of the pointer default action: latch at down, slop at
    /// the first movement, one scroll decision per move — `dom`'s recognizer,
    /// relocated behind this layer's decision boundary.
    #[allow(clippy::too_many_arguments, reason = "one private dispatch site")]
    fn drag_step(
        &mut self,
        event: &InputEvent,
        target: Option<NodeId>,
        id: PointerId,
        drags_to_scroll: bool,
        phase: PointerPhase,
        host: &impl RouterHost,
        out: &mut Vec<InputDecision>,
    ) {
        match phase {
            PointerPhase::Down => {
                self.drags.retain(|drag| drag.pointer != id);
                if event.default_prevented || !drags_to_scroll {
                    return;
                }
                let scroller =
                    target.and_then(|node| host.nearest_user_scrollable(node, ScrollAxes::BOTH));
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
                    if distance <= DRAG_SLOP {
                        return;
                    }
                    drag.scrolling = true;
                    // Subtract the slop from the first movement rather than
                    // jumping by the full travel.
                    travel * ((distance - DRAG_SLOP) / distance)
                };
                drag.origin = event.position;
                out.push(InputDecision::Scroll {
                    pointer: Some(id),
                    from: drag.scroller,
                    delta: -movement,
                });
            }
            PointerPhase::Up | PointerPhase::Cancel => {
                self.drags.retain(|drag| drag.pointer != id);
            }
            _ => {}
        }
    }

    /// The wheel half of the default action: per-event nearest scrollable on
    /// the delta's axes, line/page modes resolved against that scroller.
    /// Stateless — a wheel latches nothing.
    fn wheel_scroll(
        target: NodeId,
        delta: Vector2D<f32>,
        mode: DeltaMode,
        host: &impl RouterHost,
        out: &mut Vec<InputDecision>,
    ) {
        let axes = ScrollAxes {
            x: delta.x != 0.0,
            y: delta.y != 0.0,
        };
        let Some(scroller) = host.nearest_user_scrollable(target, axes) else {
            return;
        };
        let pixels = match mode {
            DeltaMode::Line => delta * WHEEL_LINE_PX,
            DeltaMode::Page => host
                .scrollport_size(scroller)
                .map_or(delta, |(width, height)| {
                    Vector2D::new(delta.x * width, delta.y * height)
                }),
            // `Pixel` is already CSS px, and `DeltaMode` is
            // `#[non_exhaustive]` — an unknown mode scrolls by its raw value
            // rather than guessing a unit.
            _ => delta,
        };
        out.push(InputDecision::Scroll {
            pointer: None,
            from: scroller,
            delta: pixels,
        });
    }

    /// The synthesis half: sequence lifecycle and the `tap` decision.
    fn synthesize(
        &mut self,
        event: &InputEvent,
        target: Option<NodeId>,
        id: PointerId,
        phase: PointerPhase,
        at: f64,
        out: &mut Vec<InputDecision>,
    ) {
        match phase {
            PointerPhase::Down => {
                if !self.down_pointers.contains(&id) {
                    self.down_pointers.push(id);
                }
                if self.down_pointers.len() > 1 {
                    // Lynx delivers `tap` for single-finger sequences only;
                    // the overlap disqualifies both fingers, and recognition
                    // resumes at the next solitary down.
                    self.sequence = None;
                } else {
                    // A down that hit nothing can name no target later, so
                    // it starts no sequence.
                    self.sequence = target.map(|target| Sequence {
                        pointer: id,
                        target,
                        down_position: event.position,
                        latest_position: event.position,
                        tap_allowed: true,
                        longpress_deadline: Some(at + LONG_PRESS_SECONDS),
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
                    if sequence.travelled_beyond(event.position, LONG_PRESS_MOVE_SLOP) {
                        sequence.longpress_deadline = None;
                    }
                    if sequence.travelled_beyond(event.position, TAP_SLOP) {
                        sequence.tap_allowed = false;
                    }
                }
            }
            PointerPhase::Up => {
                self.down_pointers.retain(|&pointer| pointer != id);
                if let Some(sequence) = self.sequence.take_if(|sequence| sequence.pointer == id)
                    && sequence.tap_allowed
                    && !sequence.longpress_fired
                    && !sequence.travelled_beyond(event.position, TAP_SLOP)
                {
                    out.push(emit(TAP_EVENT, sequence.target, event.position));
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
            _ => {}
        }
    }

    fn fire_due(&mut self, now: f64, host: &impl RouterHost, out: &mut Vec<InputDecision>) {
        let Some(sequence) = self.sequence.as_mut() else {
            return;
        };
        let Some(deadline) = sequence.longpress_deadline else {
            return;
        };
        if now < deadline {
            return;
        }
        // The deadline lapses exactly once, whatever the listener answer: the
        // walk Lynx runs at this instant either finds a handler or the
        // sequence's long-press half is simply over.
        sequence.longpress_deadline = None;
        if host.has_listener(LONG_PRESS_EVENT) {
            sequence.longpress_fired = true;
            out.push(emit(
                LONG_PRESS_EVENT,
                sequence.target,
                sequence.latest_position,
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use dom::input::{InputEvent, PointerKind, PointerPhase};

    use super::*;

    const TARGET: u64 = 3;
    const SCROLLER: u64 = 7;

    fn target() -> NodeId {
        NodeId::from_bits(TARGET).expect("a well-formed packed handle")
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

    /// A host whose answers the test scripts directly.
    struct MockHost {
        scroller: Option<NodeId>,
        live: bool,
        scrollport: (f32, f32),
        longpress_bound: bool,
    }

    impl Default for MockHost {
        fn default() -> Self {
            Self {
                scroller: None,
                live: true,
                scrollport: (200.0, 200.0),
                longpress_bound: true,
            }
        }
    }

    impl RouterHost for MockHost {
        fn nearest_user_scrollable(&self, _node: NodeId, _axes: ScrollAxes) -> Option<NodeId> {
            self.scroller
        }

        fn contains_node(&self, _node: NodeId) -> bool {
            self.live
        }

        fn scrollport_size(&self, _node: NodeId) -> Option<(f32, f32)> {
            Some(self.scrollport)
        }

        fn has_listener(&self, name: &str) -> bool {
            name == LONG_PRESS_EVENT && self.longpress_bound
        }
    }

    struct Harness {
        router: GestureRouter,
        host: MockHost,
        out: Vec<InputDecision>,
    }

    impl Harness {
        fn new() -> Self {
            Self {
                router: GestureRouter::default(),
                host: MockHost::default(),
                out: Vec::new(),
            }
        }

        fn feed(&mut self, event: InputEvent, at: f64) {
            self.feed_routed(event, Some(target()), at);
        }

        fn feed_routed(&mut self, event: InputEvent, hit: Option<NodeId>, at: f64) {
            self.router
                .on_input(&event, hit, at, &self.host, &mut self.out);
        }

        fn tick(&mut self, now: f64) {
            self.router.on_tick(now, &self.host, &mut self.out);
        }

        /// Every decision so far, flattened to a comparable line.
        fn trace(&self) -> Vec<String> {
            self.out
                .iter()
                .map(|decision| match decision {
                    InputDecision::Scroll { from, delta, .. } => {
                        format!("scroll@{}:{},{}", from.to_bits(), delta.x, delta.y)
                    }
                    InputDecision::Emit(event) => {
                        format!("{}@{}", event.name, event.target.to_bits())
                    }
                })
                .collect()
        }

        /// Only the emitted event names, for synthesis-focused tests.
        fn emitted(&self) -> Vec<&'static str> {
            self.out
                .iter()
                .filter_map(|decision| match decision {
                    InputDecision::Emit(event) => Some(event.name),
                    InputDecision::Scroll { .. } => None,
                })
                .collect()
        }
    }

    #[test]
    fn a_quick_release_is_pointer_events_then_a_tap_at_the_release_point() {
        let mut harness = Harness::new();
        harness.feed(touch(1, PointerPhase::Down, 10.0, 10.0), 0.0);
        harness.feed(touch(1, PointerPhase::Up, 12.0, 11.0), 0.1);
        assert_eq!(harness.emitted(), ["pointerdown", "pointerup", "tap"]);
        let InputDecision::Emit(tap) = harness.out[2] else {
            panic!("the last decision is the tap");
        };
        assert_eq!(tap.target, target());
        assert!((tap.position.x - 12.0).abs() < f32::EPSILON);
    }

    #[test]
    fn travel_beyond_the_tap_slop_cancels_the_tap() {
        let mut harness = Harness::new();
        harness.feed(touch(1, PointerPhase::Down, 10.0, 10.0), 0.0);
        harness.feed(touch(1, PointerPhase::Move, 70.0, 10.0), 0.05);
        harness.feed(touch(1, PointerPhase::Up, 12.0, 10.0), 0.1);
        assert_eq!(
            harness.emitted(),
            ["pointerdown", "pointermove", "pointerup"],
            "60px of travel is not a tap"
        );
    }

    #[test]
    fn a_release_far_from_the_down_is_not_a_tap_even_without_moves() {
        let mut harness = Harness::new();
        harness.feed(touch(1, PointerPhase::Down, 10.0, 10.0), 0.0);
        harness.feed(touch(1, PointerPhase::Up, 100.0, 10.0), 0.1);
        assert_eq!(harness.emitted(), ["pointerdown", "pointerup"]);
    }

    #[test]
    fn travel_within_the_tap_slop_keeps_the_tap_but_kills_the_longpress() {
        let mut harness = Harness::new();
        harness.feed(touch(1, PointerPhase::Down, 10.0, 10.0), 0.0);
        // 20px: beyond the 8px long-press slop, inside the 50px tap slop.
        harness.feed(touch(1, PointerPhase::Move, 30.0, 10.0), 0.05);
        assert!(!harness.router.needs_frame(), "the deadline is disarmed");
        harness.tick(1.0);
        harness.feed(touch(1, PointerPhase::Up, 30.0, 10.0), 1.1);
        assert_eq!(
            harness.emitted(),
            ["pointerdown", "pointermove", "pointerup", "tap"],
            "no longpress, tap survives"
        );
    }

    #[test]
    fn a_held_pointer_fires_longpress_once_and_suppresses_the_tap() {
        let mut harness = Harness::new();
        harness.feed(touch(1, PointerPhase::Down, 10.0, 10.0), 0.0);
        assert!(harness.router.needs_frame(), "a deadline is armed");
        harness.tick(0.3);
        assert_eq!(harness.emitted(), ["pointerdown"], "not due yet");
        harness.tick(0.6);
        assert_eq!(harness.emitted(), ["pointerdown", "longpress"]);
        assert!(!harness.router.needs_frame(), "the deadline lapsed");
        harness.tick(0.7);
        assert_eq!(
            harness.emitted(),
            ["pointerdown", "longpress"],
            "it fires exactly once"
        );
        harness.feed(touch(1, PointerPhase::Up, 10.0, 10.0), 0.8);
        assert_eq!(
            harness.emitted(),
            ["pointerdown", "longpress", "pointerup"],
            "the release is not a tap"
        );
    }

    #[test]
    fn a_release_arriving_after_the_deadline_emits_longpress_before_its_pointerup() {
        let mut harness = Harness::new();
        harness.feed(touch(1, PointerPhase::Down, 10.0, 10.0), 0.0);
        // No tick ran in between: the up itself is past the deadline.
        harness.feed(touch(1, PointerPhase::Up, 10.0, 10.0), 0.9);
        assert_eq!(
            harness.emitted(),
            ["pointerdown", "longpress", "pointerup"],
            "the decision order is the delivery order"
        );
    }

    #[test]
    fn without_a_longpress_listener_the_deadline_lapses_and_tap_survives() {
        let mut harness = Harness::new();
        harness.host.longpress_bound = false;
        harness.feed(touch(1, PointerPhase::Down, 10.0, 10.0), 0.0);
        harness.tick(0.6);
        harness.feed(touch(1, PointerPhase::Up, 10.0, 10.0), 0.9);
        assert_eq!(
            harness.emitted(),
            ["pointerdown", "pointerup", "tap"],
            "an unconsumed longpress does not suppress the tap"
        );
    }

    #[test]
    fn a_consumed_scroll_cancels_both_syntheses() {
        let mut harness = Harness::new();
        harness.host.scroller = Some(scroller());
        harness.feed(touch(1, PointerPhase::Down, 10.0, 10.0), 0.0);
        harness.feed(touch(1, PointerPhase::Move, 10.0, 40.0), 0.05);
        assert!(
            matches!(harness.out[1], InputDecision::Scroll { .. }),
            "the drag crossed its slop and decided a scroll, before the raw move"
        );
        harness.router.note_scroll_consumed(1);
        assert!(!harness.router.needs_frame());
        harness.tick(0.6);
        harness.feed(touch(1, PointerPhase::Up, 10.0, 40.0), 0.7);
        assert_eq!(
            harness.emitted(),
            ["pointerdown", "pointermove", "pointerup"],
            "a claimed sequence synthesizes nothing"
        );
    }

    #[test]
    fn an_unconsumed_boundary_drag_keeps_its_tap() {
        let mut harness = Harness::new();
        harness.host.scroller = Some(scroller());
        harness.feed(touch(1, PointerPhase::Down, 10.0, 10.0), 0.0);
        // 20px drag: a scroll is decided, but the executor reports nothing
        // consumed (the scroller sits at its boundary) — no claim.
        harness.feed(touch(1, PointerPhase::Move, 10.0, 30.0), 0.05);
        harness.feed(touch(1, PointerPhase::Up, 10.0, 30.0), 0.1);
        assert_eq!(
            harness.emitted(),
            ["pointerdown", "pointermove", "pointerup", "tap"],
            "an 8-50px nudge that moved nothing still taps, matching Lynx"
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
                InputDecision::Emit(_) => None,
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
        assert_eq!(harness.emitted(), ["pointerdown", "pointermove"]);
        assert!(
            !harness
                .out
                .iter()
                .any(|decision| matches!(decision, InputDecision::Scroll { .. }))
        );
    }

    #[test]
    fn a_mouse_drag_does_not_scroll_but_still_taps() {
        let mut harness = Harness::new();
        harness.host.scroller = Some(scroller());
        harness.feed(mouse(1, PointerPhase::Down, 10.0, 10.0), 0.0);
        harness.feed(mouse(1, PointerPhase::Move, 10.0, 30.0), 0.05);
        harness.feed(mouse(1, PointerPhase::Up, 10.0, 30.0), 0.1);
        assert_eq!(
            harness.emitted(),
            ["pointerdown", "pointermove", "pointerup", "tap"],
            "matching every browser, a mouse drag scrolls nothing"
        );
        assert!(
            !harness
                .out
                .iter()
                .any(|decision| matches!(decision, InputDecision::Scroll { .. }))
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
        // The next unprevented move measures from the re-based origin: 5px
        // of travel is inside the slop, so still no scroll.
        harness.feed(touch(1, PointerPhase::Move, 10.0, 65.0), 0.1);
        assert!(
            !harness
                .out
                .iter()
                .any(|decision| matches!(decision, InputDecision::Scroll { .. }))
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
    fn two_pointers_drag_independently_but_neither_taps() {
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
            !harness.emitted().contains(&"tap"),
            "no tap from a two-finger overlap"
        );

        // The next solitary sequence recognizes again.
        harness.feed(touch(1, PointerPhase::Down, 10.0, 10.0), 0.2);
        harness.feed(touch(1, PointerPhase::Up, 10.0, 10.0), 0.3);
        assert!(harness.emitted().contains(&"tap"));
    }

    #[test]
    fn a_cancel_ends_the_sequence_without_synthesis() {
        let mut harness = Harness::new();
        harness.feed(touch(1, PointerPhase::Down, 10.0, 10.0), 0.0);
        harness.feed(touch(1, PointerPhase::Cancel, 10.0, 10.0), 0.1);
        harness.tick(0.6);
        assert_eq!(harness.emitted(), ["pointerdown", "pointercancel"]);
        assert!(!harness.router.needs_frame());
    }

    #[test]
    fn a_down_that_hit_nothing_starts_no_sequence_and_emits_nothing() {
        let mut harness = Harness::new();
        harness.feed_routed(touch(1, PointerPhase::Down, 10.0, 10.0), None, 0.0);
        assert!(!harness.router.needs_frame());
        harness.feed(touch(1, PointerPhase::Up, 10.0, 10.0), 0.1);
        assert_eq!(harness.emitted(), ["pointerup"], "only the routed release");
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
    fn wheel_line_and_page_modes_resolve_against_the_scroller() {
        let mut harness = Harness::new();
        harness.host.scroller = Some(scroller());
        harness.host.scrollport = (300.0, 500.0);
        let line = InputEvent::wheel_with_mode(
            Point2D::new(10.0, 10.0),
            Vector2D::new(0.0, 2.0),
            DeltaMode::Line,
        );
        let page = InputEvent::wheel_with_mode(
            Point2D::new(10.0, 10.0),
            Vector2D::new(0.0, 1.0),
            DeltaMode::Page,
        );
        harness.feed(line, 0.0);
        harness.feed(page, 0.1);
        let scrolls: Vec<Vector2D<f32>> = harness
            .out
            .iter()
            .filter_map(|decision| match decision {
                InputDecision::Scroll { delta, .. } => Some(*delta),
                InputDecision::Emit(_) => None,
            })
            .collect();
        assert!((scrolls[0].y - 80.0).abs() < 1e-4, "2 lines × 40px");
        assert!(
            (scrolls[1].y - 500.0).abs() < 1e-4,
            "1 page × scrollport height"
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
        // A hover move from a different device id, no down.
        harness.feed(touch(9, PointerPhase::Move, 500.0, 500.0), 0.05);
        harness.feed(touch(1, PointerPhase::Up, 10.0, 10.0), 0.1);
        assert!(harness.emitted().contains(&"tap"));
    }
}
