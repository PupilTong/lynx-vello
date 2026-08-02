#![cfg_attr(coverage_nightly, feature(coverage_attribute))]

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
pub mod visual;
mod walker;

pub(crate) use pulsar::vello;
pub use pulsar::{self, ImageStore};
pub use stylo::device::Device;
pub use stylo_dom::ElementState;

pub use crate::contain::{Contain, ContentVisibility, effective_containment};
pub use crate::damage::{FlushStatus, FlushSummary, StyleDamage, StyleDamageEntry};
pub use crate::document::{DOCUMENT_NODE_ID, Document, NodeId};
pub use crate::engine::{
    ComputedStyle, CssDeclaration, CssRule, StylesheetOrigin, property_is_supported,
};
pub use crate::flush::Parallelism;
pub use crate::node::{ChildrenIter, Node, NodeType};
