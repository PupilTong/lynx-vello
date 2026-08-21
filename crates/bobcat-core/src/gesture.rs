//! Gesture synthesis on the presenting thread: the recognizer that turns a
//! routed pointer sequence into Lynx's `tap` and `longpress` events.
//!
//! This is the first slice of the gesture layer `crate::engine::event_name`'s
//! doc comment promised: raw routed pointers keep their W3C names, and the
//! Lynx names are *synthesized* here from the sequence. The router runs on the
//! presenting thread, beside input routing, because that is where the clock
//! and the hit-test result live and because recognition must not wait on the
//! script thread — nothing here is cancelable, so no round trip exists.
//!
//! # Recognition rules (user ruling 2026-08-21, recorded in
//! `docs/tracking/deviations.md`)
//!
//! - **`tap` fires on release.** The target is the node the *down* routed to — Lynx does not
//!   re-hit-test at release for `tap` (only `click`, which this engine does not synthesize yet,
//!   re-targets at up). A sequence stops being a tap when it travels more than [`TAP_SLOP`] from
//!   its down point (radial, the behavior Lynx's Android source names as intended and iOS
//!   implements — Android's shipping per-axis comparison is the deviation), when the user-agent
//!   scroll claims it, when a second pointer goes down, or when a `longpress` was delivered for it.
//! - **`longpress` fires while still pressed**, [`LONG_PRESS_SECONDS`] after the down, if the
//!   pointer has stayed within [`LONG_PRESS_MOVE_SLOP`] — the same 8px threshold the drag
//!   recognizer uses, which is Lynx's platform touch slop, not the much larger tap slop. A sequence
//!   whose `longpress` was delivered does not also deliver `tap` — Lynx's `long_press_consumed`
//!   rule, expressed here as the tap recognizer waiting on the longpress recognizer's outcome.
//! - **Listener presence gates `longpress`.** Lynx drops a sequence's `tap` only when the
//!   `longpress` walk actually found a handler. The router asks the injected query at the deadline;
//!   when no listener exists the deadline lapses silently and `tap` stays eligible. The query is
//!   name-level over the whole document rather than per-chain — a recorded approximation: a
//!   `longpress` listener anywhere suppresses this sequence's `tap` even when the chain it fires on
//!   has none.
//! - **One pointer at a time.** A second concurrent pointer cancels synthesis for the whole
//!   overlap, matching Lynx's single-finger `tap` gate. Every pointer kind synthesizes: Lynx
//!   targets touch, but this engine's embedders feed mice through the same seam and a mouse press
//!   is a tap on the web target.
//!
//! # What this module deliberately is not
//!
//! No fling, no velocity, no `:active` driving, no `consume-slide-event`, no
//! per-element `GestureDetector`/arena relations — those arrive with the
//! arena's growth, and the recorded design keeps this core pure so they slot
//! in as recognizers beside these two. The router owns no clock and touches no
//! document: time arrives as an argument (the engine's [`AnimationClock`]
//! reading, in seconds), targets arrive from routing, and the scroll claim
//! arrives as the routed event's already-performed default action.
//!
//! [`AnimationClock`]: crate::clock::AnimationClock

use dom::input::{InputEvent, InputKind, PointerId, PointerPhase};
use dom::{NodeId, Point2D};

/// How far a sequence may travel, in viewport CSS px, and still deliver `tap`.
///
/// Lynx reads this from the page config (`tapSlop`, default `"50px"`); wiring
/// the config key is a recorded follow-up, so today every view uses the
/// default.
pub(crate) const TAP_SLOP: f32 = 50.0;

/// Movement beyond this, in viewport CSS px, cancels a pending `longpress`.
///
/// Lynx cancels its long-press timer at the platform touch slop (nominally
/// 8dp on Android), which is also `dom`'s drag-recognizer slop — so on
/// scrollable content the long-press cancel and the scroll claim engage at
/// the same travel.
pub(crate) const LONG_PRESS_MOVE_SLOP: f32 = 8.0;

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

/// One synthesized Lynx event, ready for the ordinary dispatch path.
#[derive(Clone, Copy, Debug)]
pub(crate) struct SynthesizedEvent {
    pub(crate) name: &'static str,
    /// The node the sequence's down routed to. It may have been freed since —
    /// the dispatcher checks liveness before building a path.
    pub(crate) target: NodeId,
    /// Where the synthesizing sample was: the release point for `tap`, the
    /// latest known position for `longpress`.
    pub(crate) position: Point2D<f32>,
}

/// One pointer sequence being watched from down to release.
#[derive(Debug)]
struct Sequence {
    pointer: PointerId,
    target: NodeId,
    down_position: Point2D<f32>,
    latest_position: Point2D<f32>,
    /// Cleared by travel beyond [`TAP_SLOP`] or a scroll claim.
    tap_allowed: bool,
    /// Armed at down; disarmed by travel beyond [`LONG_PRESS_MOVE_SLOP`], a
    /// scroll claim, or expiry.
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

/// The per-view recognizer state. Owned by the engine, fed on the presenting
/// thread only.
#[derive(Debug, Default)]
pub(crate) struct GestureRouter {
    /// Every pointer currently down, in down order. Length is what matters:
    /// more than one cancels synthesis for the whole overlap.
    down_pointers: Vec<PointerId>,
    /// The one sequence that can still synthesize. `None` while no pointer is
    /// down, while more than one is, or after the sequence disqualified
    /// itself.
    sequence: Option<Sequence>,
}

impl GestureRouter {
    /// Feeds one routed pointer event.
    ///
    /// `target` is what routing hit, `scrolled` is whether the event's
    /// user-agent default action consumed scroll, and `at` is the engine
    /// clock's reading when the event arrived — arrival time rather than
    /// drain time, so a sequence buffered behind a busy document keeps its
    /// real duration. Due deadlines fire before the event applies, which is
    /// what lets a release that arrives after the long-press deadline still
    /// suppress its tap. Delivery ordering needs more than this internal
    /// rule, though: the engine also drains due deadlines through
    /// [`Self::on_tick`] and dispatches them *before* the raw event, so the
    /// `longpress` crosses the ordered channel ahead of the `pointerup`
    /// that followed it.
    pub(crate) fn on_pointer(
        &mut self,
        event: &InputEvent,
        target: Option<NodeId>,
        scrolled: bool,
        at: f64,
        longpress_bound: impl FnOnce() -> bool,
        out: &mut Vec<SynthesizedEvent>,
    ) {
        let InputKind::Pointer { id, phase, .. } = event.kind else {
            return;
        };
        self.fire_due(at, longpress_bound, out);
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
                    if scrolled {
                        // The user-agent scroll claimed the sequence: Lynx's
                        // gesture-recognized gate, which ends both syntheses.
                        sequence.tap_allowed = false;
                        sequence.longpress_deadline = None;
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
                    out.push(SynthesizedEvent {
                        name: TAP_EVENT,
                        target: sequence.target,
                        position: event.position,
                    });
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
            // `PointerPhase` is `#[non_exhaustive]`; an unknown phase changes
            // no sequence rather than guessing.
            _ => {}
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
        longpress_bound: impl FnOnce() -> bool,
        out: &mut Vec<SynthesizedEvent>,
    ) {
        self.fire_due(now, longpress_bound, out);
    }

    /// Whether a deadline is armed and so owes the timeline another frame.
    pub(crate) fn needs_frame(&self) -> bool {
        self.sequence
            .as_ref()
            .is_some_and(|sequence| sequence.longpress_deadline.is_some())
    }

    fn fire_due(
        &mut self,
        now: f64,
        longpress_bound: impl FnOnce() -> bool,
        out: &mut Vec<SynthesizedEvent>,
    ) {
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
        if longpress_bound() {
            sequence.longpress_fired = true;
            out.push(SynthesizedEvent {
                name: LONG_PRESS_EVENT,
                target: sequence.target,
                position: sequence.latest_position,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use dom::input::{InputEvent, PointerKind, PointerPhase};

    use super::*;

    const TARGET: u64 = 3;

    fn target() -> NodeId {
        NodeId::from_bits(TARGET).expect("a well-formed packed handle")
    }

    fn pointer(id: u32, phase: PointerPhase, x: f32, y: f32) -> InputEvent {
        InputEvent::pointer(Point2D::new(x, y), id, PointerKind::Touch, phase)
    }

    struct Harness {
        router: GestureRouter,
        out: Vec<SynthesizedEvent>,
        longpress_bound: bool,
    }

    impl Harness {
        fn new() -> Self {
            Self {
                router: GestureRouter::default(),
                out: Vec::new(),
                longpress_bound: true,
            }
        }

        fn feed(&mut self, event: InputEvent, at: f64) {
            self.feed_routed(event, Some(target()), false, at);
        }

        fn feed_routed(&mut self, event: InputEvent, hit: Option<NodeId>, scrolled: bool, at: f64) {
            let bound = self.longpress_bound;
            self.router
                .on_pointer(&event, hit, scrolled, at, || bound, &mut self.out);
        }

        fn tick(&mut self, now: f64) {
            let bound = self.longpress_bound;
            self.router.on_tick(now, || bound, &mut self.out);
        }

        fn names(&self) -> Vec<&'static str> {
            self.out.iter().map(|event| event.name).collect()
        }
    }

    #[test]
    fn a_quick_release_is_a_tap_at_the_release_point() {
        let mut harness = Harness::new();
        harness.feed(pointer(1, PointerPhase::Down, 10.0, 10.0), 0.0);
        harness.feed(pointer(1, PointerPhase::Up, 12.0, 11.0), 0.1);
        assert_eq!(harness.names(), ["tap"]);
        let tap = &harness.out[0];
        assert_eq!(tap.target, target());
        assert!((tap.position.x - 12.0).abs() < f32::EPSILON);
    }

    #[test]
    fn travel_beyond_the_tap_slop_cancels_the_tap() {
        let mut harness = Harness::new();
        harness.feed(pointer(1, PointerPhase::Down, 10.0, 10.0), 0.0);
        harness.feed(pointer(1, PointerPhase::Move, 70.0, 10.0), 0.05);
        harness.feed(pointer(1, PointerPhase::Up, 12.0, 10.0), 0.1);
        assert!(harness.out.is_empty(), "60px of travel is not a tap");
    }

    #[test]
    fn a_release_far_from_the_down_is_not_a_tap_even_without_moves() {
        let mut harness = Harness::new();
        harness.feed(pointer(1, PointerPhase::Down, 10.0, 10.0), 0.0);
        harness.feed(pointer(1, PointerPhase::Up, 100.0, 10.0), 0.1);
        assert!(
            harness.out.is_empty(),
            "the release point itself is checked"
        );
    }

    #[test]
    fn travel_within_the_tap_slop_keeps_the_tap_but_kills_the_longpress() {
        let mut harness = Harness::new();
        harness.feed(pointer(1, PointerPhase::Down, 10.0, 10.0), 0.0);
        // 20px: beyond the 8px long-press slop, inside the 50px tap slop.
        harness.feed(pointer(1, PointerPhase::Move, 30.0, 10.0), 0.05);
        assert!(!harness.router.needs_frame(), "the deadline is disarmed");
        harness.tick(1.0);
        harness.feed(pointer(1, PointerPhase::Up, 30.0, 10.0), 1.1);
        assert_eq!(harness.names(), ["tap"], "no longpress, tap survives");
    }

    #[test]
    fn a_held_pointer_fires_longpress_once_and_suppresses_the_tap() {
        let mut harness = Harness::new();
        harness.feed(pointer(1, PointerPhase::Down, 10.0, 10.0), 0.0);
        assert!(harness.router.needs_frame(), "a deadline is armed");
        harness.tick(0.3);
        assert!(harness.out.is_empty(), "not due yet");
        harness.tick(0.6);
        assert_eq!(harness.names(), ["longpress"]);
        assert!(!harness.router.needs_frame(), "the deadline lapsed");
        harness.tick(0.7);
        assert_eq!(harness.names(), ["longpress"], "it fires exactly once");
        harness.feed(pointer(1, PointerPhase::Up, 10.0, 10.0), 0.8);
        assert_eq!(harness.names(), ["longpress"], "the release is not a tap");
    }

    #[test]
    fn a_release_arriving_after_the_deadline_sees_the_longpress_first() {
        let mut harness = Harness::new();
        harness.feed(pointer(1, PointerPhase::Down, 10.0, 10.0), 0.0);
        // No tick ran in between: the up itself is past the deadline.
        harness.feed(pointer(1, PointerPhase::Up, 10.0, 10.0), 0.9);
        assert_eq!(harness.names(), ["longpress"]);
    }

    #[test]
    fn without_a_longpress_listener_the_deadline_lapses_and_tap_survives() {
        let mut harness = Harness::new();
        harness.longpress_bound = false;
        harness.feed(pointer(1, PointerPhase::Down, 10.0, 10.0), 0.0);
        harness.tick(0.6);
        assert!(harness.out.is_empty(), "no listener, no longpress");
        harness.feed(pointer(1, PointerPhase::Up, 10.0, 10.0), 0.9);
        assert_eq!(
            harness.names(),
            ["tap"],
            "an unconsumed longpress does not suppress the tap"
        );
    }

    #[test]
    fn a_scroll_claim_cancels_both_syntheses() {
        let mut harness = Harness::new();
        harness.feed(pointer(1, PointerPhase::Down, 10.0, 10.0), 0.0);
        // Movement inside every slop, but the drag recognizer scrolled.
        harness.feed_routed(
            pointer(1, PointerPhase::Move, 12.0, 10.0),
            Some(target()),
            true,
            0.05,
        );
        assert!(!harness.router.needs_frame());
        harness.tick(0.6);
        harness.feed(pointer(1, PointerPhase::Up, 12.0, 10.0), 0.7);
        assert!(
            harness.out.is_empty(),
            "a claimed sequence synthesizes nothing"
        );
    }

    #[test]
    fn a_second_pointer_cancels_the_sequence_for_the_whole_overlap() {
        let mut harness = Harness::new();
        harness.feed(pointer(1, PointerPhase::Down, 10.0, 10.0), 0.0);
        harness.feed(pointer(2, PointerPhase::Down, 50.0, 50.0), 0.05);
        harness.feed(pointer(2, PointerPhase::Up, 50.0, 50.0), 0.1);
        harness.feed(pointer(1, PointerPhase::Up, 10.0, 10.0), 0.15);
        assert!(harness.out.is_empty(), "no tap from a two-finger overlap");

        // The next solitary sequence recognizes again.
        harness.feed(pointer(1, PointerPhase::Down, 10.0, 10.0), 0.2);
        harness.feed(pointer(1, PointerPhase::Up, 10.0, 10.0), 0.3);
        assert_eq!(harness.names(), ["tap"]);
    }

    #[test]
    fn a_cancel_ends_the_sequence_without_synthesis() {
        let mut harness = Harness::new();
        harness.feed(pointer(1, PointerPhase::Down, 10.0, 10.0), 0.0);
        harness.feed(pointer(1, PointerPhase::Cancel, 10.0, 10.0), 0.1);
        harness.tick(0.6);
        assert!(harness.out.is_empty());
        assert!(!harness.router.needs_frame());
    }

    #[test]
    fn a_down_that_hit_nothing_starts_no_sequence() {
        let mut harness = Harness::new();
        harness.feed_routed(pointer(1, PointerPhase::Down, 10.0, 10.0), None, false, 0.0);
        assert!(!harness.router.needs_frame());
        harness.feed(pointer(1, PointerPhase::Up, 10.0, 10.0), 0.1);
        assert!(harness.out.is_empty());
    }

    #[test]
    fn a_wheel_event_is_not_a_sequence() {
        let mut harness = Harness::new();
        let wheel = InputEvent::wheel(Point2D::new(10.0, 10.0), dom::Vector2D::new(0.0, 5.0));
        harness.feed(wheel, 0.0);
        assert!(harness.out.is_empty());
        assert!(!harness.router.needs_frame());
    }

    #[test]
    fn moves_from_another_pointer_do_not_disturb_the_sequence() {
        let mut harness = Harness::new();
        harness.feed(pointer(1, PointerPhase::Down, 10.0, 10.0), 0.0);
        // A hover move from a different device id, no down.
        harness.feed(pointer(9, PointerPhase::Move, 500.0, 500.0), 0.05);
        harness.feed(pointer(1, PointerPhase::Up, 10.0, 10.0), 0.1);
        assert_eq!(harness.names(), ["tap"]);
    }
}
