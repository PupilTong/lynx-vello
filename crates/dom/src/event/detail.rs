//! What an event carries beside its type and its target: one shape per kind
//! of payload, shared by every layer that names an event.
//!
//! [`EventDetail`] is the one description of an event's payload in the
//! engine. The runtime's input router fills one in when it decides an event,
//! the host carries it across the thread boundary unchanged, an engine
//! component reads it on the Rust walk ([`ElementEvent::detail`]), and the
//! script boundary encodes it into the numbers and strings a realm builds its
//! event object out of. No layer has a second model of it.
//!
//! One variant per *shape*, never per event name: `pointerdown`, `click` and
//! a Lynx `tap` all carry a [`EventDetail::Pointer`], and a consumer that
//! needs to know which of them it is reads the event's type. A shape a new
//! event needs is a new variant, and the enum is `#[non_exhaustive]` so a
//! consumer outside this crate keeps compiling when one arrives.
//!
//! [`ElementEvent::detail`]: super::ElementEvent::detail

use std::sync::Arc;

use euclid::default::{Point2D, Vector2D};
use smallvec::SmallVec;

use crate::input::{Modifiers, PointerId, PointerKind};
use crate::tree::document::NodeId;

/// In a touch event's `touches`: the finger is still down once this event
/// has been applied, so a lifted or cancelled finger never carries it.
pub const TOUCH_ACTIVE: u8 = 1;

/// In `targetTouches`: [`TOUCH_ACTIVE`], and the finger's captured target is
/// the target of the event carrying it.
pub const TOUCH_TARGET: u8 = 2;

/// In `changedTouches`: the one finger this event is about.
pub const TOUCH_CHANGED: u8 = 4;

/// One entry of a touch event's three lists: which finger, where it is in
/// viewport CSS px, and which lists it belongs to ([`TOUCH_ACTIVE`],
/// [`TOUCH_TARGET`], [`TOUCH_CHANGED`], or-ed together).
///
/// One point can sit in all three at once (a move of the only finger down on
/// the event's own target does), so the lists are one sequence with flags
/// rather than three.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TouchPoint {
    pub identifier: PointerId,
    pub position: Point2D<f32>,
    pub flags: u8,
}

/// The points one touch event carries, in the order their fingers went down,
/// with a lifted or cancelled finger last.
pub type TouchPoints = SmallVec<[TouchPoint; 2]>;

/// What one event carries, by shape.
///
/// Owned, so the runtime can carry one across a thread boundary as it is; a
/// dispatch lends it out by reference, so nothing on the delivery path copies
/// it.
#[derive(Clone, Debug, Default, PartialEq)]
#[non_exhaustive]
pub enum EventDetail {
    /// Nothing at all: an `error`, a `load`, a dialog's `close`, a
    /// `scroll`, a `slotchange`. What a Lynx handler reads beside a W3C
    /// event's own payload (a `load`'s size, a `scroll`'s metrics) is the
    /// host's to carry, not this type's.
    #[default]
    None,
    /// A pointer at a position: the pointer events, the compatibility mouse
    /// events, `click` and `dblclick`, and the Lynx events the runtime
    /// synthesizes from a pointer sequence (`tap`, `longpress`).
    Pointer {
        /// Viewport CSS px.
        position: Point2D<f32>,
        id: PointerId,
        device: PointerKind,
        /// W3C `MouseEvent.button` ([`InputKind::Pointer`](crate::input::InputKind::Pointer)).
        button: i16,
        /// W3C `MouseEvent.buttons`.
        buttons: u16,
    },
    /// A wheel turned at a position, by a delta in viewport CSS px.
    Wheel {
        position: Point2D<f32>,
        delta: Vector2D<f32>,
    },
    /// A touch event: the position its `detail` reports and the three lists.
    Touch {
        position: Point2D<f32>,
        points: TouchPoints,
    },
    /// A key event (`keydown`, `keyup`): W3C UI Events' `key` and `code`
    /// strings, whether it is an auto-repeat, the modifiers held, and the
    /// text the key typed ([`InputKind::Key`](crate::input::InputKind::Key)'s
    /// `text`). Shared behind `Arc`s, so carrying one is never an
    /// allocation.
    Key {
        key: Arc<str>,
        code: Arc<str>,
        repeat: bool,
        modifiers: Modifiers,
        /// The text the key typed, which the `keydown`'s editing default
        /// action inserts. Read by the engine alone: W3C's `KeyboardEvent`
        /// has no such member, so it never crosses into a realm.
        text: Option<Arc<str>>,
    },
    /// `beforeinput` or `input` (Input Events Level 2): `InputEvent.inputType`,
    /// `data` (`None` for the deletions and `insertLineBreak`) and
    /// `isComposing`. What an [`EditEvent`](crate::editing::EditEvent) of
    /// either kind carries.
    Input {
        input_type: &'static str,
        data: Option<Arc<str>>,
        is_composing: bool,
    },
    /// `compositionstart`, `compositionupdate` or `compositionend` (UI
    /// Events): `CompositionEvent.data`.
    Composition { data: Arc<str> },
    /// `focus`, `blur`, `focusin` or `focusout` (UI Events):
    /// `FocusEvent.relatedTarget` — for `focus`/`focusin` the element that
    /// lost focus, for `blur`/`focusout` the one gaining it — as
    /// [`FocusEvent`](crate::editing::FocusEvent) records it.
    Focus { related_target: Option<NodeId> },
    /// An element's exposure changed. The payload is read off the target at
    /// delivery, so the variant carries nothing.
    Exposure,
    /// [css-contain-2 §4.4](https://drafts.csswg.org/css-contain-2/#content-visibility-auto-state-change-event):
    /// the state a `content-visibility: auto` element changed **to**,
    /// `true` when it now skips its contents.
    ContentVisibility { skipped: bool },
}

impl EventDetail {
    /// The position the event happened at, in viewport CSS px, for every
    /// shape that has one.
    #[must_use]
    pub fn position(&self) -> Option<Point2D<f32>> {
        match self {
            Self::Pointer { position, .. }
            | Self::Wheel { position, .. }
            | Self::Touch { position, .. } => Some(*position),
            Self::None
            | Self::Key { .. }
            | Self::Input { .. }
            | Self::Composition { .. }
            | Self::Focus { .. }
            | Self::Exposure
            | Self::ContentVisibility { .. } => None,
        }
    }

    /// A touch event's points; empty for every other shape.
    #[must_use]
    pub fn touch_points(&self) -> &[TouchPoint] {
        match self {
            Self::Touch { points, .. } => points,
            _ => &[],
        }
    }
}
