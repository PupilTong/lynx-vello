//! The document tree core: the arena set, [`Node`](node::Node), and
//! [`Document`](document::Document). [`xml`] parses XML content and builds
//! it into the tree.

pub(crate) mod anonymous;
pub(crate) mod arena;
pub(crate) mod custom;
pub(crate) mod document;
pub(crate) mod footprint;
pub(crate) mod node;
pub(crate) mod popover;
pub(crate) mod shadow;
pub(crate) mod top_layer;
pub(crate) mod xml;
