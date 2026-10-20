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
//! else: plain data (a key's two strings shared behind an `Arc`), no traits to
//! implement, no callbacks to register,
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
//! # use terminus::input::InputEvent;
//! # use terminus::Point2D;
//! # fn f<T: Sync>(document: &terminus::Document<T>) {
//! let target = document.route_input(&InputEvent::wheel(Point2D::new(50.0, 50.0), (0.0, 120.0)));
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
//! second consumer this crate would keep one for — the router itself routes
//! against the published [`crate::CommittedFrame`], which answers the same
//! hit question without the document. Dispatch is likewise not this
//! crate's — [`crate::event`] computes which nodes an event visits and in
//! what order, and reaching a listener means leaving this thread.
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
//! - Three devices are modeled — pointers, wheels and keys (`keydown`/`keyup` only). Focus and IME
//!   have no consumer yet, so a key event is not routed here: the runtime targets it at the
//!   document element until a focus model exists. [`InputEvent`] and its enums are
//!   `#[non_exhaustive]` so the rest can arrive without a break.

use std::sync::Arc;

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
    /// Whether a drag of this device scrolls content. Every device's does:
    /// touch and pen as in every browser, and a mouse too, which browsers do
    /// not do — an engine policy by user ruling (2026-10-13), so that an
    /// embedder reports a mouse as the mouse it is and its drags still
    /// scroll a scroll container. Public because a runtime layer that
    /// suppresses the default action and drives scrolling itself applies
    /// the same device policy.
    #[must_use]
    pub const fn drags_to_scroll(self) -> bool {
        matches!(self, Self::Touch | Self::Pen | Self::Mouse)
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

/// Whether a key went down or came up: W3C UI Events' `keydown` and
/// `keyup`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum KeyPhase {
    Down,
    Up,
}

/// The modifier keys held while a key event happened, as W3C UI Events'
/// `KeyboardEvent` reports them (`altKey`, `ctrlKey`, `metaKey`,
/// `shiftKey`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "the four independent booleans `KeyboardEvent` itself reports"
)]
pub struct Modifiers {
    pub alt: bool,
    pub ctrl: bool,
    pub meta: bool,
    pub shift: bool,
}

#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum InputKind {
    Pointer {
        id: PointerId,
        device: PointerKind,
        phase: PointerPhase,
        /// W3C `MouseEvent.button`: the button whose state this event
        /// changed (`0` primary, `1` auxiliary, `2` secondary), or `-1` when
        /// no button changed, as for a move. A touch or pen contact is the
        /// primary button.
        button: i16,
        /// W3C `MouseEvent.buttons`: every button held once this event has
        /// been applied, as a bitmask (`1` primary, `2` secondary,
        /// `4` auxiliary).
        buttons: u16,
    },
    Wheel {
        /// Scroll delta in viewport CSS pixels. Embedders must normalize any
        /// platform-specific physical-pixel, line, or page units first.
        delta: Vector2D<f32>,
    },
    /// A key went down or came up. Carries no position: a key event is not
    /// routed by hit testing, and the event's position is unused.
    Key {
        phase: KeyPhase,
        /// W3C UI Events `KeyboardEvent.key`: the key's value under the
        /// current layout and modifiers (`"a"`, `"A"`, `"Enter"`,
        /// `"ArrowLeft"`, `" "`).
        key: Arc<str>,
        /// W3C UI Events `KeyboardEvent.code`: the physical key, whatever
        /// the layout (`"KeyA"`, `"Enter"`, `"ArrowLeft"`, `"Space"`).
        code: Arc<str>,
        /// The key is held and this is an auto-repeat.
        repeat: bool,
        modifiers: Modifiers,
        /// The text the key typed, as the platform reports it with the key
        /// (winit's `KeyEvent.text`), or `None` when the key typed none or
        /// the platform delivers typed text another way — through its input
        /// method, as a browser's `beforeinput` or an `insertText:` that
        /// reaches the host as an input-method commit. A `keydown`'s
        /// editing default action inserts this text, never `key`: a host
        /// whose platform sends the text both with the key and through the
        /// input method sends it here as `None` and as one
        /// [`Command::CommitText`](crate::editing::Command::CommitText), so
        /// it is inserted once. W3C's `KeyboardEvent` has no such member,
        /// and no realm sees it.
        text: Option<Arc<str>>,
    },
}

/// A normalized host input event in document coordinates.
///
/// `Clone` rather than `Copy`: a key event carries its two W3C strings, each
/// shared behind an `Arc`, so a copy of the event is two reference-count
/// increments and never an allocation.
#[derive(Debug, Clone, PartialEq)]
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
    /// A pointer event at `position` (viewport CSS px), with the button
    /// state a primary contact has in `phase`: a down presses the primary
    /// button (`button` 0, `buttons` 1), a move keeps it held (-1, 1), an up
    /// releases it (0, 0) and a cancel holds nothing (-1, 0). That is a touch
    /// or pen contact, and a primary-button mouse drag; a mouse embedder that
    /// knows better (a hover move, another button) says so with
    /// [`Self::with_buttons`].
    #[must_use]
    pub fn pointer(
        position: impl Into<Point2D<f32>>,
        id: PointerId,
        device: PointerKind,
        phase: PointerPhase,
    ) -> Self {
        let (button, buttons) = match phase {
            PointerPhase::Down => (0, 1),
            PointerPhase::Move => (-1, 1),
            PointerPhase::Up => (0, 0),
            PointerPhase::Cancel => (-1, 0),
        };
        Self {
            position: position.into(),
            kind: InputKind::Pointer {
                id,
                device,
                phase,
                button,
                buttons,
            },
            default_prevented: false,
        }
    }

    /// This pointer event with W3C `MouseEvent.button` and `buttons` as
    /// given; any other event is returned unchanged.
    #[must_use]
    pub fn with_buttons(mut self, button: i16, buttons: u16) -> Self {
        if let InputKind::Pointer {
            button: held_button,
            buttons: held_buttons,
            ..
        } = &mut self.kind
        {
            *held_button = button;
            *held_buttons = buttons;
        }
        self
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

    /// A key event: `phase` names `keydown` or `keyup`; `key` and `code` are
    /// the W3C UI Events strings, which the embedder maps from its platform
    /// (a browser passes its own through). The position is the origin and
    /// unused: a key event is not hit-tested. It types no text; a host whose
    /// platform reports the text a key typed with the key says so with
    /// [`Self::with_text`].
    #[must_use]
    pub fn key(
        phase: KeyPhase,
        key: impl Into<Arc<str>>,
        code: impl Into<Arc<str>>,
        repeat: bool,
        modifiers: Modifiers,
    ) -> Self {
        Self {
            position: Point2D::origin(),
            kind: InputKind::Key {
                phase,
                key: key.into(),
                code: code.into(),
                repeat,
                modifiers,
                text: None,
            },
            default_prevented: false,
        }
    }

    /// This key event with the text the key typed (see
    /// [`InputKind::Key`]'s `text`); any other event is returned unchanged.
    #[must_use]
    pub fn with_text(mut self, typed: Option<Arc<str>>) -> Self {
        if let InputKind::Key { text, .. } = &mut self.kind {
            *text = typed;
        }
        self
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
            InputKind::Pointer { .. } | InputKind::Key { .. } => true,
            InputKind::Wheel { delta } => delta.x.is_finite() && delta.y.is_finite(),
        };
        position && payload
    }
}

impl<T: Sync> Document<T> {
    /// Routes one input event against the rendered frame: the topmost
    /// hit-testable element at its position, or `None` before the first
    /// render or outside every element. A key event has no position to
    /// route by and answers `None`: its target is the focus target, which
    /// the runtime decides.
    ///
    /// A pure read — no flush, no rebuild, no state. Everything that follows
    /// routing (event naming, the user-agent default action, gesture
    /// synthesis) is the runtime layer's input router.
    #[must_use]
    pub fn route_input(&self, event: &InputEvent) -> Option<NodeId> {
        if matches!(event.kind, InputKind::Key { .. }) {
            return None;
        }
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
            document.route_input(&InputEvent::wheel(Point2D::new(50.0, 50.0), (0.0, 30.0)));
        assert_eq!(over_child, Some(child));

        let over_page = document.route_input(&InputEvent::pointer(
            Point2D::new(500.0, 500.0),
            1,
            PointerKind::Touch,
            PointerPhase::Down,
        ));
        assert_eq!(over_page, Some(root));

        let outside = document.route_input(&InputEvent::wheel(
            Point2D::new(10_000.0, 10.0),
            (0.0, 50.0),
        ));
        assert_eq!(outside, None);
    }

    #[test]
    fn routing_answers_nothing_before_the_first_render() {
        let (document, _root, _child) = page();
        assert_eq!(
            document.route_input(&InputEvent::pointer(
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
