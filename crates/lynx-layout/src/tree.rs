//! Immutable style-view extension used by the linear layout algorithm.
//!
//! The topology, core box style, and `calc()` resolver remain in
//! [`LayoutSource`]. This trait adds only
//! Lynx-linear container/item views, preserving neutron-star's open display
//! dispatch and the source/session storage split.

use neutron_star::tree::{LayoutSource, NodeId};

use crate::style::{LinearContainerStyle, LinearItemStyle};

/// Adds linear-container and linear-item style views to a layout source.
///
/// The borrowed GAT views keep this boundary statically dispatched and allow
/// adapters to translate host computed styles lazily without materializing a
/// parallel style tree.
pub trait LinearSource: LayoutSource {
    /// Borrowed computed style of a linear container.
    type LinearContainerStyle<'a>: LinearContainerStyle
    where
        Self: 'a;

    /// Borrowed computed style of an item in a linear container.
    type LinearItemStyle<'a>: LinearItemStyle
    where
        Self: 'a;

    /// Returns the linear-container style view of `container`.
    fn linear_container_style(&self, container: NodeId) -> Self::LinearContainerStyle<'_>;

    /// Returns the linear-item style view of `item`.
    fn linear_item_style(&self, item: NodeId) -> Self::LinearItemStyle<'_>;
}
