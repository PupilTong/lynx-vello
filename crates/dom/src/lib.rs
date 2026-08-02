#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![warn(unreachable_pub)]

//! `dom` — a generic, stylo-integrated W3C-DOM-subset document tree.

#[cfg(test)]
extern crate self as dom;

#[cfg(test)]
#[path = "../tests/common/mod.rs"]
mod test_common;

mod contain;
mod convert;
mod damage;
mod document;
mod engine;
mod flush;
pub mod input;
mod invalidation;
pub mod layout;
mod node;
mod paint;
mod painter;
pub mod scroll;
mod shape;
mod traits;
mod visual;
mod walker;

pub use euclid::default::{Point2D, Size2D, Vector2D};
pub(crate) use pulsar::{ImageStore, vello};
pub use stylo::device::Device;
pub use stylo_dom::ElementState;

#[cfg(feature = "style-test-utils")]
#[doc(hidden)]
pub use crate::damage::{
    StyleDamageEntryForTesting, StyleDamageForTesting, StyleFlushSummaryForTesting,
};
pub use crate::document::{Document, NodeId};
pub use crate::engine::StylesheetOrigin;
pub use crate::node::Node;
