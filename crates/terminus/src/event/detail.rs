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

pub const TOUCH_ACTIVE: u8 = 1;

pub const TOUCH_TARGET: u8 = 2;

pub const TOUCH_CHANGED: u8 = 4;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TouchPoint {
    pub identifier: PointerId,
    pub position: Point2D<f32>,
    pub flags: u8,
}

pub type TouchPoints = SmallVec<[TouchPoint; 2]>;

#[derive(Clone, Debug, Default, PartialEq)]
#[non_exhaustive]
pub enum EventDetail {
    #[default]
    None,
    Pointer {
        position: Point2D<f32>,
        id: PointerId,
        device: PointerKind,
        button: i16,
        buttons: u16,
    },
    Wheel {
        position: Point2D<f32>,
        delta: Vector2D<f32>,
    },
    Touch {
        position: Point2D<f32>,
        points: TouchPoints,
    },
    Key {
        key: Arc<str>,
        code: Arc<str>,
        repeat: bool,
        modifiers: Modifiers,
        text: Option<Arc<str>>,
    },
    Input {
        input_type: &'static str,
        data: Option<Arc<str>>,
        is_composing: bool,
    },
    Composition {
        data: Arc<str>,
    },
    Focus {
        related_target: Option<NodeId>,
    },
    Exposure,
    ContentVisibility {
        skipped: bool,
    },
}

impl EventDetail {
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

    #[must_use]
    pub fn touch_points(&self) -> &[TouchPoint] {
        match self {
            Self::Touch { points, .. } => points,
            _ => &[],
        }
    }
}
