//! The document tree core: the arena set, [`Node`](node::Node), and
//! [`Document`](document::Document).

pub(crate) mod arena;
pub(crate) mod custom;
pub(crate) mod document;
pub(crate) mod inline_svg;
pub(crate) mod node;
pub(crate) mod shadow;
pub(crate) mod svg_markup;
pub(crate) mod top_layer;
