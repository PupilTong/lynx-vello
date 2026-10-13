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

use euclid::default::{Point2D, Vector2D};
use smallvec::SmallVec;

use crate::input::{PointerId, PointerKind};

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
    /// Nothing at all: an `error`, a dialog's `close`, a `slotchange`.
    #[default]
    None,
    /// A pointer at a position: the pointer events, and every event the
    /// runtime synthesizes from a pointer sequence.
    Pointer {
        /// Viewport CSS px.
        position: Point2D<f32>,
        id: PointerId,
        device: PointerKind,
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
    /// A size in px: an `<image>`'s or `<svg>`'s `load`.
    Size { width: f64, height: f64 },
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
            Self::None | Self::Size { .. } | Self::Exposure | Self::ContentVisibility { .. } => {
                None
            }
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
