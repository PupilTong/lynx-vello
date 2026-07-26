#![cfg_attr(coverage_nightly, feature(coverage_attribute))]

//! `w3c-dom` — a generic, stylo-integrated W3C-DOM-subset document tree.

mod contain;
mod damage;
mod document;
mod engine;
mod flush;
mod invalidation;
pub mod layout;
mod node;
mod traits;
pub mod visual;

pub use dom::ElementState;

pub use crate::contain::{Contain, ContentVisibility, effective_containment};
pub use crate::damage::{FlushStatus, FlushSummary, StyleDamage, StyleDamageEntry};
pub use crate::document::{DOCUMENT_NODE_ID, Document, NodeId};
pub use crate::engine::{
    ComputedStyle, CssDeclaration, CssRule, StylesheetOrigin, property_is_supported,
};
pub use crate::flush::Parallelism;
pub use crate::node::{ChildrenIter, Node, NodeType};
