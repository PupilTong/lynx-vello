//! The host input seam: the event vocabulary a window, a canvas, or a test
//! harness hands to the runtime, and the one thing the document itself does
//! with an event — route it.
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
//! # What the document does, and what it leaves to the runtime layer
//!
//! [`Document::route_input`] does the one thing that is unambiguously the
//! browser core's job and impossible to do from outside it: it **routes** the
//! event — hit testing through the frame's true paint order, honoring
//! transforms, clips, `visibility` and `pointer-events` — and reports the
//! node it hit.
//!
//! ```
//! # use dom::input::InputEvent;
//! # use dom::Point2D;
//! # fn f<T: Sync>(document: &dom::Document<T>) {
//! let target = document.route_input(InputEvent::wheel(Point2D::new(50.0, 50.0), (0.0, 120.0)));
//! # let _ = target;
//! # }
//! ```
//!
//! Everything after routing belongs to the runtime layer above: naming the
//! event, choosing which routed pointer phase becomes which event type,
//! deciding and driving the user-agent default action — scrolling, through
//! [`Document::scroll_by`] and [`Document::scroll_chain`], whose unconsumed
//! remainders exist for exactly that caller — and everything Lynx-shaped:
//! `bindEvent`/`catchEvent` phase encoding, gesture-arena arbitration,
//! `hit-slop`, `user-interaction-enabled`, tap/long-press synthesis, fling
//! momentum. This crate has no default-action machinery and no recognizer:
//! that single decision point is the runtime's input router, and there is no
//! second consumer this crate would keep one for. Dispatch is likewise not
//! this crate's — [`crate::event`] computes which nodes an event visits and
//! in what order, and reaching a listener means leaving this thread.
//!
//! [`InputEvent::default_prevented`] is `preventDefault()` by another name,
//! and this crate never reads it. Lynx has no cancelable event: a handler
//! that wants to suppress a built-in behavior goes through gesture
//! arbitration (`consumeGesture`/`interceptGesture`), not through the event
//! object. The flag is the embedder's word to the runtime's input router that
//! something already claimed the event, and the router answers it by deciding
//! no scroll.
//!
//! # Visual-frame ownership
//!
//! Routing uses the exact stacking, transform, and clip model painting uses,
//! by reading the frame the last [`Document::render`] retained — the same
//! pure read as [`Document::elements_from_point`], never a flush or a
//! rebuild. Input therefore targets what is on screen: before the first
//! render nothing is. A node freed since that render drops out of the answer
//! on its own, because its id is retired rather than reissued and so names
//! nothing. Geometric staleness is allowed and wanted: an event that lands
//! between a scroll and its repaint hits the content the user was actually
//! shown. The frame itself stays a DOM implementation detail; callers never
//! coordinate or retain a `PaintOrder` beside the document.
//!
//! # Recorded limits
//!
//! - Two devices are modeled — pointers and wheels. Keyboard, focus, and IME have no consumer yet;
//!   [`InputEvent`] and its enums are `#[non_exhaustive]` so they can arrive without a break.

use euclid::default::{Point2D, Vector2D};

use crate::NodeId;
use crate::tree::document::Document;

pub type PointerId = u32;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum PointerKind {
    Mouse,
    Touch,
    Pen,
}

impl PointerKind {
    /// Whether a drag of this device scrolls content: touch and pen do, a
    /// mouse drag does not, matching every browser. Public because a runtime
    /// layer that suppresses the default action and drives scrolling itself
    /// applies the same device policy.
    #[must_use]
    pub const fn drags_to_scroll(self) -> bool {
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

#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub enum InputKind {
    Pointer {
        id: PointerId,
        device: PointerKind,
        phase: PointerPhase,
    },
    Wheel {
        /// Scroll delta in viewport CSS pixels. Embedders must normalize any
        /// platform-specific physical-pixel, line, or page units first.
        delta: Vector2D<f32>,
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

    /// A wheel event at `position` with `delta` in viewport CSS pixels.
    ///
    /// The embedder owns conversion from platform-specific physical-pixel,
    /// line, or page units; the runtime input protocol has one coordinate
    /// vocabulary only.
    #[must_use]
    pub fn wheel(position: impl Into<Point2D<f32>>, delta: impl Into<Vector2D<f32>>) -> Self {
        Self {
            position: position.into(),
            kind: InputKind::Wheel {
                delta: delta.into(),
            },
            default_prevented: false,
        }
    }

    /// This event with [`Self::default_prevented`] set as given — the
    /// embedder's word to the runtime's input router that something already
    /// claimed the event. This crate never reads the flag.
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
            InputKind::Wheel { delta } => delta.x.is_finite() && delta.y.is_finite(),
        };
        position && payload
    }
}

impl<T: Sync> Document<T> {
    /// Routes one input event against the rendered frame: the topmost
    /// hit-testable element at its position, or `None` before the first
    /// render or outside every element.
    ///
    /// A pure read — no flush, no rebuild, no state. Everything that follows
    /// routing (event naming, the user-agent default action, gesture
    /// synthesis) is the runtime layer's input router.
    #[must_use]
    pub fn route_input(&self, event: InputEvent) -> Option<NodeId> {
        if !event.is_finite() {
            debug_assert!(false, "host input events must be finite, got {event:?}");
            return None;
        }
        self.rendered_element_at(event.position)
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use crate::StylesheetOrigin;
    use crate::tree::document::tests::device;

    fn page() -> (Document<()>, crate::NodeId, crate::NodeId) {
        let mut document: Document<()> = Document::new(device(), "page", ());
        document.add_stylesheet(
            "page { display: flex; width: 800px; height: 600px; }
             .box { display: flex; width: 100px; height: 100px; }",
            StylesheetOrigin::Author,
        );
        let root = document.document_element().id();
        let child = document.create_element("view", ());
        document.add_class(child, "box");
        document.append_child(root, child);
        (document, root, child)
    }

    #[test]
    fn routing_reports_the_topmost_element_and_nothing_outside() {
        let (mut document, root, child) = page();
        document.render();

        let over_child =
            document.route_input(InputEvent::wheel(Point2D::new(50.0, 50.0), (0.0, 30.0)));
        assert_eq!(over_child, Some(child));

        let over_page = document.route_input(InputEvent::pointer(
            Point2D::new(500.0, 500.0),
            1,
            PointerKind::Touch,
            PointerPhase::Down,
        ));
        assert_eq!(over_page, Some(root));

        let outside =
            document.route_input(InputEvent::wheel(Point2D::new(10_000.0, 10.0), (0.0, 50.0)));
        assert_eq!(outside, None);
    }

    #[test]
    fn routing_answers_nothing_before_the_first_render() {
        let (document, _root, _child) = page();
        assert_eq!(
            document.route_input(InputEvent::pointer(
                Point2D::new(50.0, 50.0),
                1,
                PointerKind::Touch,
                PointerPhase::Down,
            )),
            None,
            "input targets what is on screen, and nothing is yet"
        );
    }
}
