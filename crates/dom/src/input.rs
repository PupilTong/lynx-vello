//! The host input seam: how a window, a canvas, or a test harness hands user
//! interaction to the document.
//!
//! # The shape of the seam
//!
//! Integration code differs wildly — an HTML canvas gets `PointerEvent`s, a
//! native window gets winit/UIKit/Android motion events, a headless test
//! harness gets whatever its author types — but all of them can produce the
//! same three facts: *where* (viewport CSS px), *what kind of device*, and
//! *what it just did*. [`InputEvent`] is exactly those three facts and nothing
//! else: plain `Copy` data, no traits to implement, no callbacks to register,
//! no host handle for the document to hold. A host adapter is a `match` and a
//! constructor call, and a test is a literal.
//!
//! ```
//! # use dom::input::InputEvent;
//! # use dom::Point2D;
//! # fn f<T: Sync>(document: &mut dom::Document<T>) {
//! let response = document.handle_input(InputEvent::wheel(Point2D::new(50.0, 50.0), (0.0, 120.0)));
//! # let _ = response;
//! # }
//! ```
//!
//! # What the document does, and what it leaves to you
//!
//! [`Document::handle_input`] does the two things that are unambiguously the
//! browser core's job and impossible to do from outside it: it **routes** the
//! event (hit testing through the frame's true paint order, honoring
//! transforms, clips, `visibility` and `pointer-events`), and it performs the
//! **UA default action** the event resolves to — today, scrolling a scroll
//! container. It reports both in an [`InputResponse`].
//!
//! It does *not* dispatch to listeners, because this crate has none: there is
//! no `EventTarget`, no capture/bubble walk, no script. That belongs to the
//! runtime layer above, along with everything Lynx-shaped — `bindEvent`/
//! `catchEvent` phase encoding, gesture-arena arbitration, `hit-slop`,
//! `user-interaction-enabled`, tap/long-press synthesis, fling momentum.
//!
//! The seam between the two halves is [`InputEvent::default_prevented`], which
//! is `preventDefault()` by another name. A runtime layer with script in the
//! loop routes first, dispatches, and then hands the event back saying whether
//! anything claimed it:
//!
//! ```
//! # use dom::input::InputEvent;
//! # use dom::Point2D;
//! # fn f<T: Sync>(document: &mut dom::Document<T>, event: InputEvent, position: Point2D<f32>) {
//! # fn dispatch_to_script(_: Option<dom::NodeId>) -> bool { false }
//! let hit = document.hit_test(position);
//! let claimed = dispatch_to_script(hit); // capture/bubble, gesture arena, ...
//! document.handle_input(event.with_default_prevented(claimed));
//! # }
//! ```
//!
//! A runtime that wants a *different* default action — Lynx's `parent-first`
//! nested scrolling, rubber-band overscroll, fling — prevents the default and
//! drives [`Document::scroll_by`] and [`Document::scroll_chain`] itself. Those
//! primitives are the whole point of reporting an unconsumed remainder.
//!
//! # Visual-frame ownership
//!
//! Routing uses the exact stacking, transform, and clip model painting uses,
//! but that frame is a DOM implementation detail. Each call builds the current
//! private frame before routing; callers never coordinate or retain a
//! `PaintOrder` beside the document.
//!
//! # Recorded limits
//!
//! - Two devices are modeled — pointers and wheels. Keyboard, focus, and IME have no consumer in
//!   this crate yet; [`InputEvent`] and its enums are `#[non_exhaustive]` so they can arrive
//!   without a break.
//! - Scroll gestures are drag and wheel only. There is no momentum, no rubber-band overscroll, no
//!   scrollbar to grab, and no smooth (`scroll-behavior`) animation: this crate owns no clock, and
//!   a frame pump is the layer above's.
//! - Only touch and pen drags scroll. A mouse drag does not, matching every browser: a mouse
//!   scrolls with its wheel.
//! - Multi-touch is tracked per pointer id, but each pointer scrolls independently. Pinch/zoom is
//!   not a thing this crate recognizes.

use euclid::default::{Point2D, Vector2D};
use smallvec::SmallVec;

use crate::NodeId;
use crate::document::Document;
use crate::scroll::ScrollAxes;

/// How far a touch or pen must travel before the document reads it as a scroll
/// rather than a stationary press, in CSS px.
///
/// The value native toolkits converge on (Android's `ViewConfiguration` touch
/// slop, Lynx's own `tapSlop`) is 8 device-independent px, and this is the same
/// affordance: a threshold below which a shaky finger is still holding still.
/// A runtime layer arbitrating a scroll against its own tap and long-press
/// recognizers wants its own threshold — it gets one by preventing the default
/// action and driving [`Document::scroll_chain`] on its own terms.
pub const TOUCH_SLOP: f32 = 8.0;

/// CSS px one [`DeltaMode::Line`] step scrolls: the conventional UA value.
pub const WHEEL_LINE_PX: f32 = 40.0;

/// A pointing device's identity for the life of one interaction. Hosts that
/// have no notion of pointer ids (a single mouse) can use any constant.
pub type PointerId = u32;

/// What kind of pointing device produced an event. Only touch and pen drag to
/// scroll; a mouse scrolls with its wheel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum PointerKind {
    Mouse,
    Touch,
    Pen,
}

impl PointerKind {
    /// Whether dragging this device is a scroll gesture.
    #[must_use]
    const fn drags_to_scroll(self) -> bool {
        matches!(self, Self::Touch | Self::Pen)
    }
}

/// Where a pointer is in its interaction, mirroring the Pointer Events
/// lifecycle (`pointerdown`/`pointermove`/`pointerup`/`pointercancel`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum PointerPhase {
    Down,
    Move,
    Up,
    /// The system took the interaction away — a call arrived, the window lost
    /// focus, the gesture was claimed elsewhere. Any latched gesture ends
    /// without completing.
    Cancel,
}

/// How to read a wheel delta, mirroring `WheelEvent.deltaMode`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[non_exhaustive]
pub enum DeltaMode {
    /// CSS px. Trackpads and high-resolution wheels report this.
    #[default]
    Pixel,
    /// Text lines, resolved at [`WHEEL_LINE_PX`] each.
    Line,
    /// Scrollport-sized jumps, resolved against the box that ends up scrolling.
    Page,
}

/// Which device an event came from, and what it did.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub enum InputKind {
    Pointer {
        id: PointerId,
        device: PointerKind,
        phase: PointerPhase,
    },
    Wheel {
        /// Positive `y` scrolls the content down (the reading position moves
        /// toward the end), matching `WheelEvent.deltaY`.
        delta: Vector2D<f32>,
        mode: DeltaMode,
    },
}

/// One host input event, normalized to the document's own coordinate space.
///
/// Construct with [`InputEvent::pointer`] or [`InputEvent::wheel`]; the struct
/// is `#[non_exhaustive]` so devices and fields can be added without breaking
/// every host adapter.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub struct InputEvent {
    /// Viewport CSS px — the same space [`Document::hit_test`] and the
    /// document's own `Device` viewport speak. A host that works in physical
    /// pixels divides by its device pixel ratio first.
    pub position: Point2D<f32>,
    pub kind: InputKind,
    /// Set when the layer above already claimed this event — a script listener
    /// called `preventDefault()`, a gesture recognizer took the touch. The
    /// document still routes and reports the event, but performs no default
    /// action for it.
    ///
    /// Preventing a `Down` suppresses the whole gesture it would have started;
    /// preventing a `Move` suppresses only that step, and the drag resumes,
    /// without back-applying the movement it skipped, on the next one.
    pub default_prevented: bool,
}

impl InputEvent {
    /// A pointer event at `position` (viewport CSS px).
    #[must_use]
    pub fn pointer(
        position: impl Into<Point2D<f32>>,
        id: PointerId,
        device: PointerKind,
        phase: PointerPhase,
    ) -> Self {
        Self {
            position: position.into(),
            kind: InputKind::Pointer { id, device, phase },
            default_prevented: false,
        }
    }

    /// A wheel event at `position` with a pixel `delta` — the common case.
    /// Use [`Self::wheel_with_mode`] for line- or page-quantized wheels.
    #[must_use]
    pub fn wheel(position: impl Into<Point2D<f32>>, delta: impl Into<Vector2D<f32>>) -> Self {
        Self::wheel_with_mode(position, delta, DeltaMode::Pixel)
    }

    #[must_use]
    pub fn wheel_with_mode(
        position: impl Into<Point2D<f32>>,
        delta: impl Into<Vector2D<f32>>,
        mode: DeltaMode,
    ) -> Self {
        Self {
            position: position.into(),
            kind: InputKind::Wheel {
                delta: delta.into(),
                mode,
            },
            default_prevented: false,
        }
    }

    /// This event with [`Self::default_prevented`] set as given — the handoff
    /// point for a layer that dispatches to script before letting the document
    /// act.
    #[must_use]
    pub const fn with_default_prevented(mut self, prevented: bool) -> Self {
        self.default_prevented = prevented;
        self
    }

    /// Whether every coordinate this event carries is a real number.
    /// [`Document::handle_input`] drops events that fail this.
    #[must_use]
    pub fn is_finite(&self) -> bool {
        let position = self.position.x.is_finite() && self.position.y.is_finite();
        let payload = match self.kind {
            InputKind::Pointer { .. } => true,
            InputKind::Wheel { delta, .. } => delta.x.is_finite() && delta.y.is_finite(),
        };
        position && payload
    }
}

/// The UA behavior the document performed for an event on its own.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub enum DefaultAction {
    /// Nothing: the event was prevented, hit nothing scrollable, or every
    /// scroll container in the chain was already at its boundary.
    None,
    /// A scroll container moved. `node` is the innermost box that consumed
    /// anything — the one an event would name — and `delta` is the total the
    /// chain consumed, which may include an ancestor's share.
    Scroll { node: NodeId, delta: Vector2D<f32> },
}

/// What one event resolved to.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub struct InputResponse {
    /// The topmost hit-testable element under [`InputEvent::position`], or
    /// `None` outside every box. This is `elementFromPoint`: the layer above
    /// starts its own capture/bubble walk here.
    pub target: Option<NodeId>,
    /// [`InputEvent::position`] in the hit item's own border-box coordinates.
    /// For a text-run hit, `target` is the styled parent element while this
    /// point belongs to the run's own box.
    pub local_position: Option<Point2D<f32>>,
    pub default_action: DefaultAction,
}

/// One pointer's latched scroll gesture.
#[derive(Debug, Clone, Copy)]
struct Drag {
    pointer: PointerId,
    /// The box this gesture scrolls. Resolved once at pointer-down and kept:
    /// re-resolving mid-drag would hand the gesture to whatever slid under the
    /// finger.
    scroller: NodeId,
    /// [`Document::node_removal_epoch`] when `scroller` was resolved. A freed
    /// id can be recycled, so the drag dies with the epoch rather than
    /// scrolling a stranger.
    epoch: u64,
    /// The position movement is measured from — the press point until the slop
    /// threshold falls, then the last consumed position.
    origin: Point2D<f32>,
    /// Whether the slop threshold has been crossed.
    scrolling: bool,
}

/// Per-document interaction state: the gestures currently latched to a
/// pointer. Two inline slots cover a mouse and a couple of fingers without
/// allocating.
#[derive(Debug, Default)]
pub(crate) struct InputState {
    drags: SmallVec<[Drag; 2]>,
}

impl InputState {
    fn find(&mut self, pointer: PointerId, epoch: u64) -> Option<&mut Drag> {
        self.drags
            .iter_mut()
            .find(|drag| drag.pointer == pointer && drag.epoch == epoch)
    }

    fn end(&mut self, pointer: PointerId) {
        self.drags.retain(|drag| drag.pointer != pointer);
    }
}

impl<T: Sync> Document<T> {
    /// Builds the document's private visual frame, routes one host input event,
    /// and performs whatever UA default action it resolves to.
    ///
    /// A non-finite position or delta is dropped entirely — no routing, no
    /// state change. This is the untrusted boundary, and NaN here is
    /// load-bearing garbage rather than a harmless miss: it would poison a
    /// latched drag's origin, so every later move of that gesture computes a
    /// NaN delta too. The `debug_assert` makes a host adapter that produces
    /// one loud in development rather than mysteriously inert.
    ///
    /// # Panics
    ///
    /// Panics if computed styles are unavailable because a style traversal was
    /// left incomplete. In debug builds, also on a non-finite position or
    /// wheel delta.
    pub fn handle_input(&mut self, event: InputEvent) -> InputResponse {
        if !event.is_finite() {
            debug_assert!(false, "host input events must be finite, got {event:?}");
            return InputResponse {
                target: None,
                local_position: None,
                default_action: DefaultAction::None,
            };
        }
        let frame = self.build_paint_order();
        let hit = frame.hit_test_local(self, event.position);
        let mut response = InputResponse {
            target: hit.map(|(node, _)| node),
            local_position: hit.map(|(_, local)| local.position),
            default_action: DefaultAction::None,
        };
        response.default_action = match event.kind {
            InputKind::Pointer { id, device, phase } => {
                self.handle_pointer(&event, response.target, id, device, phase)
            }
            InputKind::Wheel { delta, mode } => {
                if event.default_prevented {
                    DefaultAction::None
                } else {
                    self.handle_wheel(response.target, delta, mode)
                }
            }
        };
        response
    }

    fn handle_pointer(
        &mut self,
        event: &InputEvent,
        target: Option<NodeId>,
        id: PointerId,
        device: PointerKind,
        phase: PointerPhase,
    ) -> DefaultAction {
        let epoch = self.node_removal_epoch();
        match phase {
            PointerPhase::Down => {
                // A second press with a live id supersedes the first; a
                // pointer that was never lifted has nothing to finish.
                self.input_state_mut().end(id);
                if event.default_prevented || !device.drags_to_scroll() {
                    return DefaultAction::None;
                }
                let scroller =
                    target.and_then(|node| self.nearest_user_scrollable(node, ScrollAxes::BOTH));
                if let Some(scroller) = scroller {
                    self.input_state_mut().drags.push(Drag {
                        pointer: id,
                        scroller,
                        epoch,
                        origin: event.position,
                        scrolling: false,
                    });
                }
                DefaultAction::None
            }
            PointerPhase::Move => self.drag_step(event, id, epoch),
            PointerPhase::Up | PointerPhase::Cancel => {
                self.input_state_mut().end(id);
                DefaultAction::None
            }
        }
    }

    /// Advances a latched drag by the movement since its origin, consuming the
    /// touch-slop distance the first time the threshold falls so the gesture
    /// neither jumps by the slop nor loses the overshoot past it.
    fn drag_step(&mut self, event: &InputEvent, id: PointerId, epoch: u64) -> DefaultAction {
        let Some(drag) = self.input_state_mut().find(id, epoch) else {
            return DefaultAction::None;
        };
        if event.default_prevented {
            // Skip this step without back-applying it later.
            drag.origin = event.position;
            return DefaultAction::None;
        }
        let travel = event.position - drag.origin;
        let movement = if drag.scrolling {
            travel
        } else {
            let distance = travel.length();
            if distance <= TOUCH_SLOP {
                return DefaultAction::None;
            }
            drag.scrolling = true;
            travel * ((distance - TOUCH_SLOP) / distance)
        };
        drag.origin = event.position;
        let scroller = drag.scroller;

        // Content follows the finger, so the scroll position moves against it.
        match self.scroll_chain(scroller, -movement) {
            Some((node, delta)) => DefaultAction::Scroll { node, delta },
            None => DefaultAction::None,
        }
    }

    fn handle_wheel(
        &mut self,
        target: Option<NodeId>,
        delta: Vector2D<f32>,
        mode: DeltaMode,
    ) -> DefaultAction {
        let axes = ScrollAxes {
            x: delta.x != 0.0,
            y: delta.y != 0.0,
        };
        let Some(scroller) = target.and_then(|node| self.nearest_user_scrollable(node, axes))
        else {
            return DefaultAction::None;
        };
        let pixels = match mode {
            DeltaMode::Pixel => delta,
            DeltaMode::Line => delta * WHEEL_LINE_PX,
            // Resolved against the box that will actually take the first
            // bite; anything that chains onward keeps those px.
            DeltaMode::Page => self.scroll_box(scroller).map_or(delta, |scroll_box| {
                Vector2D::new(
                    delta.x * scroll_box.scrollport.width,
                    delta.y * scroll_box.scrollport.height,
                )
            }),
        };
        match self.scroll_chain(scroller, pixels) {
            Some((node, consumed)) => DefaultAction::Scroll {
                node,
                delta: consumed,
            },
            None => DefaultAction::None,
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use crate::document::tests::device;
    use crate::{NodeId, StylesheetOrigin};

    /// A 100×100 `overflow: scroll` list inside a 200×200 scrolling page.
    fn scrolling_page() -> (Document<()>, NodeId, NodeId, NodeId) {
        let mut document: Document<()> = Document::new(device());
        document.add_stylesheet(
            "page { display: flex; width: 800px; height: 600px; }
             .outer { display: flex; overflow: scroll; width: 200px; height: 200px; }
             .list { display: flex; overflow: scroll; width: 100px; height: 100px; }
             .content { flex-shrink: 0; width: 100px; height: 400px; }
             .filler { flex-shrink: 0; width: 100px; height: 1000px; }",
            StylesheetOrigin::Author,
        );
        let root = document.create_element("page", ());
        document.append_document_element(root);
        let outer = document.create_element("view", ());
        document.add_class(outer, "outer");
        document.append_child(root, outer);
        let list = document.create_element("view", ());
        document.add_class(list, "list");
        document.append_child(outer, list);
        let content = document.create_element("view", ());
        document.add_class(content, "content");
        document.append_child(list, content);
        let filler = document.create_element("view", ());
        document.add_class(filler, "filler");
        document.append_child(outer, filler);
        document.layout();
        (document, outer, list, content)
    }

    fn touch(position: (f32, f32), phase: PointerPhase) -> InputEvent {
        InputEvent::pointer(
            Point2D::new(position.0, position.1),
            1,
            PointerKind::Touch,
            phase,
        )
    }

    /// Feeds a gesture and returns the last response.
    fn gesture(document: &mut Document<()>, events: &[InputEvent]) -> InputResponse {
        let mut last = None;
        for event in events {
            last = Some(document.handle_input(*event));
        }
        last.expect("a gesture has at least one event")
    }

    #[test]
    fn a_touch_drag_scrolls_the_box_under_the_finger() {
        let (mut document, _outer, list, content) = scrolling_page();

        let response = gesture(
            &mut document,
            &[
                touch((50.0, 90.0), PointerPhase::Down),
                // 8px of slop is eaten, so 60 of travel becomes 52 of scroll.
                touch((50.0, 30.0), PointerPhase::Move),
            ],
        );

        // The event targets the topmost box under the finger; the *scroll*
        // resolved outward from there to the nearest user-scrollable ancestor.
        assert_eq!(response.target, Some(content));
        assert_eq!(
            response.default_action,
            DefaultAction::Scroll {
                node: list,
                delta: Vector2D::new(0.0, 52.0),
            },
        );
        assert_eq!(document.scroll_offset(list), Vector2D::new(0.0, 52.0));

        gesture(&mut document, &[touch((50.0, 30.0), PointerPhase::Up)]);
        assert_eq!(document.scroll_offset(list), Vector2D::new(0.0, 52.0));
    }

    #[test]
    fn a_drag_below_the_slop_threshold_scrolls_nothing() {
        let (mut document, _outer, list, _content) = scrolling_page();

        let response = gesture(
            &mut document,
            &[
                touch((50.0, 50.0), PointerPhase::Down),
                touch((50.0, 44.0), PointerPhase::Move),
                touch((50.0, 45.0), PointerPhase::Up),
            ],
        );

        assert_eq!(response.default_action, DefaultAction::None);
        assert_eq!(document.scroll_offset(list), Vector2D::zero());
    }

    #[test]
    fn a_mouse_drag_does_not_scroll_but_its_wheel_does() {
        let (mut document, _outer, list, _content) = scrolling_page();
        let at = |y: f32, phase| {
            InputEvent::pointer(Point2D::new(50.0, y), 1, PointerKind::Mouse, phase)
        };

        gesture(
            &mut document,
            &[at(50.0, PointerPhase::Down), at(-50.0, PointerPhase::Move)],
        );
        assert_eq!(document.scroll_offset(list), Vector2D::zero());

        let response =
            document.handle_input(InputEvent::wheel(Point2D::new(50.0, 50.0), (0.0, 30.0)));
        assert_eq!(
            response.default_action,
            DefaultAction::Scroll {
                node: list,
                delta: Vector2D::new(0.0, 30.0),
            },
        );
        assert_eq!(document.scroll_offset(list), Vector2D::new(0.0, 30.0));
    }

    #[test]
    fn wheel_delta_modes_resolve_to_pixels() {
        let (mut document, _outer, list, _content) = scrolling_page();

        gesture(
            &mut document,
            &[InputEvent::wheel_with_mode(
                Point2D::new(50.0, 50.0),
                (0.0, 1.0),
                DeltaMode::Line,
            )],
        );
        assert_eq!(document.scroll_offset(list), Vector2D::new(0.0, 40.0));

        document.scroll_to(list, Vector2D::zero());
        // One page is one scrollport: 100px tall here.
        gesture(
            &mut document,
            &[InputEvent::wheel_with_mode(
                Point2D::new(50.0, 50.0),
                (0.0, 1.0),
                DeltaMode::Page,
            )],
        );
        assert_eq!(document.scroll_offset(list), Vector2D::new(0.0, 100.0));
    }

    #[test]
    fn a_gesture_stays_latched_to_the_box_it_started_on() {
        let (mut document, outer, list, _content) = scrolling_page();
        // Park the inner list at its bottom so the drag has to chain.
        document.scroll_to(list, Vector2D::new(0.0, 300.0));

        gesture(
            &mut document,
            &[
                touch((50.0, 90.0), PointerPhase::Down),
                touch((50.0, 32.0), PointerPhase::Move),
            ],
        );

        // The list is pinned, so its 50px of travel chained to the outer box —
        // and it chained there even though 50px of scrolling slid a
        // non-scrolling box under the finger.
        assert_eq!(document.scroll_offset(list), Vector2D::new(0.0, 300.0));
        assert_eq!(document.scroll_offset(outer), Vector2D::new(0.0, 50.0));
    }

    #[test]
    fn preventing_the_press_suppresses_the_whole_gesture() {
        let (mut document, _outer, list, _content) = scrolling_page();

        gesture(
            &mut document,
            &[
                touch((50.0, 50.0), PointerPhase::Down).with_default_prevented(true),
                touch((50.0, -50.0), PointerPhase::Move),
            ],
        );

        assert_eq!(document.scroll_offset(list), Vector2D::zero());
    }

    #[test]
    fn preventing_one_move_skips_only_that_step() {
        let (mut document, _outer, list, _content) = scrolling_page();

        gesture(
            &mut document,
            &[
                touch((50.0, 50.0), PointerPhase::Down),
                touch((50.0, 20.0), PointerPhase::Move).with_default_prevented(true),
                touch((50.0, 0.0), PointerPhase::Move),
            ],
        );

        // The prevented 30px never scrolled, and the 20px after it paid the
        // slop toll on its own rather than inheriting a crossed threshold.
        assert_eq!(document.scroll_offset(list), Vector2D::new(0.0, 12.0));
    }

    #[test]
    fn a_cancelled_pointer_stops_scrolling() {
        let (mut document, _outer, list, _content) = scrolling_page();

        gesture(
            &mut document,
            &[
                touch((50.0, 50.0), PointerPhase::Down),
                touch((50.0, 20.0), PointerPhase::Move),
                touch((50.0, 10.0), PointerPhase::Cancel),
                touch((50.0, -50.0), PointerPhase::Move),
            ],
        );

        assert_eq!(document.scroll_offset(list), Vector2D::new(0.0, 22.0));
    }

    #[test]
    fn two_fingers_drag_their_own_scrollers() {
        let (mut document, outer, list, _content) = scrolling_page();
        let finger = |id, position: (f32, f32), phase| {
            InputEvent::pointer(
                Point2D::new(position.0, position.1),
                id,
                PointerKind::Touch,
                phase,
            )
        };

        gesture(
            &mut document,
            &[
                finger(1, (50.0, 50.0), PointerPhase::Down),
                // Outside the 100×100 list, inside the 200×200 outer box.
                finger(2, (150.0, 150.0), PointerPhase::Down),
                finger(1, (50.0, 22.0), PointerPhase::Move),
                finger(2, (150.0, 102.0), PointerPhase::Move),
            ],
        );

        assert_eq!(document.scroll_offset(list), Vector2D::new(0.0, 20.0));
        assert_eq!(document.scroll_offset(outer), Vector2D::new(0.0, 40.0));
    }

    #[test]
    fn an_event_over_nothing_scrollable_reports_a_target_and_no_action() {
        let mut document: Document<()> = Document::new(device());
        document.add_stylesheet(
            "page { display: flex; width: 800px; height: 600px; }",
            StylesheetOrigin::Author,
        );
        let root = document.create_element("page", ());
        document.append_document_element(root);
        document.layout();

        let response =
            document.handle_input(InputEvent::wheel(Point2D::new(10.0, 10.0), (0.0, 50.0)));
        assert_eq!(response.target, Some(root));
        assert_eq!(response.default_action, DefaultAction::None);

        let outside =
            document.handle_input(InputEvent::wheel(Point2D::new(10_000.0, 10.0), (0.0, 50.0)));
        assert_eq!(outside.target, None);
        assert_eq!(outside.local_position, None);
    }
}
