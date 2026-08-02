#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![deny(unreachable_pub)]

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

pub use crate::document::{Document, NodeId};
pub use crate::engine::StylesheetOrigin;
/// Stylo names this iterator in the public `TElement` implementation for
/// [`Node`]; callers should normally use [`Node::children`] and its opaque
/// return type.
#[doc(hidden)]
pub use crate::node::ChildrenIter;
pub use crate::node::Node;
