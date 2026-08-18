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
//! let hit = document.elements_from_point(position).first().copied();
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
//! by reading the frame the last [`Document::render`] retained — the same
//! pure read as [`Document::elements_from_point`], never a flush or a
//! rebuild. Input therefore targets what is on screen: before the first
//! render nothing is. A node freed since that render drops out of the answer
//! on its own, because its id is retired rather than reissued and so names
//! nothing — the same check that ends a latched drag whose scroller was
//! removed mid-gesture, and the reason a removal elsewhere in the tree no
//! longer blanks routing. Geometric staleness is allowed and wanted: an
//! event that lands between a scroll and its repaint hits the content the
//! user was actually shown. The frame itself stays a DOM implementation detail; callers never
//! coordinate or retain a `PaintOrder` beside the document.
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
use crate::scroll::ScrollAxes;
use crate::tree::document::Document;

const TOUCH_SLOP: f32 = 8.0;

const WHEEL_LINE_PX: f32 = 40.0;

pub type PointerId = u32;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum PointerKind {
    Mouse,
    Touch,
    Pen,
}

impl PointerKind {
    #[must_use]
    const fn drags_to_scroll(self) -> bool {
        matches!(self, Self::Touch | Self::Pen)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum PointerPhase {
    Down,
    Move,
    Up,
    Cancel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[non_exhaustive]
pub enum DeltaMode {
    #[default]
    Pixel,
    Line,
    Page,
}

#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub enum InputKind {
    Pointer {
        id: PointerId,
        device: PointerKind,
        phase: PointerPhase,
    },
    Wheel {
        delta: Vector2D<f32>,
        mode: DeltaMode,
    },
}

/// A normalized host input event in document coordinates.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub struct InputEvent {
    /// Viewport CSS px — the same space [`Document::elements_from_point`] and
    /// the document's own `Device` viewport speak. A host that works in
    /// physical pixels divides by its device pixel ratio first.
    pub position: Point2D<f32>,
    pub kind: InputKind,
    /// Whether an upper layer already prevented the default action.
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

    #[must_use]
    fn is_finite(&self) -> bool {
        let position = self.position.x.is_finite() && self.position.y.is_finite();
        let payload = match self.kind {
            InputKind::Pointer { .. } => true,
            InputKind::Wheel { delta, .. } => delta.x.is_finite() && delta.y.is_finite(),
        };
        position && payload
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub enum DefaultAction {
    None,
    Scroll { node: NodeId, delta: Vector2D<f32> },
}

/// What one event resolved to.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub struct InputResponse {
    /// Topmost hit-testable element at the event position.
    pub target: Option<NodeId>,
    pub default_action: DefaultAction,
}

/// One pointer's latched scroll gesture.
#[derive(Debug, Clone, Copy)]
struct Drag {
    pointer: PointerId,
    scroller: NodeId,
    origin: Point2D<f32>,
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
    fn find(&mut self, pointer: PointerId) -> Option<&mut Drag> {
        self.drags.iter_mut().find(|drag| drag.pointer == pointer)
    }

    fn scroller(&self, pointer: PointerId) -> Option<NodeId> {
        self.drags
            .iter()
            .find(|drag| drag.pointer == pointer)
            .map(|drag| drag.scroller)
    }

    fn end(&mut self, pointer: PointerId) {
        self.drags.retain(|drag| drag.pointer != pointer);
    }
}

impl<T: Sync> Document<T> {
    /// Routes input against the rendered frame and applies default scrolling.
    pub fn handle_input(&mut self, event: InputEvent) -> InputResponse {
        if !event.is_finite() {
            debug_assert!(false, "host input events must be finite, got {event:?}");
            return InputResponse {
                target: None,
                default_action: DefaultAction::None,
            };
        }
        let mut response = InputResponse {
            target: self.rendered_element_at(event.position),
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
        match phase {
            PointerPhase::Down => {
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
                        origin: event.position,
                        scrolling: false,
                    });
                }
                DefaultAction::None
            }
            PointerPhase::Move => self.drag_step(event, id),
            PointerPhase::Up | PointerPhase::Cancel => {
                self.input_state_mut().end(id);
                DefaultAction::None
            }
        }
    }

    fn drag_step(&mut self, event: &InputEvent, id: PointerId) -> DefaultAction {
        let Some(scroller) = self.input_state().scroller(id) else {
            return DefaultAction::None;
        };
        // The latched scroller can be freed mid-gesture. Its id is retired
        // rather than reissued, so this is a plain liveness question and the
        // gesture simply ends.
        if !self.contains_node(scroller) {
            self.input_state_mut().end(id);
            return DefaultAction::None;
        }
        let drag = self
            .input_state_mut()
            .find(id)
            .expect("the drag was found one statement ago");
        if event.default_prevented {
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
    use crate::tree::document::tests::device;
    use crate::{NodeId, StylesheetOrigin};

    fn scrolling_page() -> (Document<()>, NodeId, NodeId, NodeId) {
        let mut document: Document<()> = Document::new(device(), "page", ());
        document.add_stylesheet(
            "page { display: flex; width: 800px; height: 600px; }
             .outer { display: flex; overflow: scroll; width: 200px; height: 200px; }
             .list { display: flex; overflow: scroll; width: 100px; height: 100px; }
             .content { flex-shrink: 0; width: 100px; height: 400px; }
             .filler { flex-shrink: 0; width: 100px; height: 1000px; }",
            StylesheetOrigin::Author,
        );
        let root = document.document_element().id();
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

    fn gesture(document: &mut Document<()>, events: &[InputEvent]) -> InputResponse {
        let mut last = None;
        for event in events {
            document.render();
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
                touch((50.0, 30.0), PointerPhase::Move),
            ],
        );

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
        document.scroll_to(list, Vector2D::new(0.0, 300.0));

        gesture(
            &mut document,
            &[
                touch((50.0, 90.0), PointerPhase::Down),
                touch((50.0, 32.0), PointerPhase::Move),
            ],
        );

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
        let mut document: Document<()> = Document::new(device(), "page", ());
        document.add_stylesheet(
            "page { display: flex; width: 800px; height: 600px; }",
            StylesheetOrigin::Author,
        );
        let root = document.document_element().id();
        document.render();

        let response =
            document.handle_input(InputEvent::wheel(Point2D::new(10.0, 10.0), (0.0, 50.0)));
        assert_eq!(response.target, Some(root));
        assert_eq!(response.default_action, DefaultAction::None);

        let outside =
            document.handle_input(InputEvent::wheel(Point2D::new(10_000.0, 10.0), (0.0, 50.0)));
        assert_eq!(outside.target, None);
    }
}
