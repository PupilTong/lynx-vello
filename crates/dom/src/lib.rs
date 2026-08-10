#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![deny(unreachable_pub)]

//! `dom` — a generic, stylo-integrated W3C-DOM-subset document tree.

#[cfg(test)]
extern crate self as dom;

#[cfg(test)]
#[path = "../tests/common/mod.rs"]
mod test_common;

pub mod input;
pub mod layout;
mod paint;
pub mod render;
pub mod scroll;
mod style;
mod tree;
mod visual;

pub use euclid::default::{Point2D, Size2D, Vector2D};
/// The CSS vocabulary door: computed-value types (what
/// [`Node::computed_style`] returns) and the test-harness device inputs are
/// stylo's, reached exclusively through this re-export, mirroring [`vello`]
/// as the render-stack door.
pub use stylo;
pub use stylo_dom::ElementState;
/// Embedders configure wgpu/peniko/kurbo exclusively through this re-export,
/// never a second copy of the render stack.
pub use vello;

pub use crate::render::images::ImageStore;
pub use crate::style::device::Device;
#[doc(hidden)]
pub use crate::style::device::standards_device;
pub use crate::style::engine::StylesheetOrigin;
pub use crate::tree::custom::CustomElement;
pub use crate::tree::document::{Document, NodeId};
/// Stylo names this iterator in the public `TElement` implementation for
/// [`Node`]; callers should normally use [`Node::children`] and its opaque
/// return type.
#[doc(hidden)]
pub use crate::tree::node::ChildrenIter;
pub use crate::tree::node::Node;
pub use crate::tree::shadow::ShadowRootMode;
