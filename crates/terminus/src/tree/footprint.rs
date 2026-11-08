//! How much of the document's own arena its elements occupy: the reading a
//! memory monitor reports for the element tree, as Lynx's
//! `ReportElementMemoryInfo` reports `sizeof` of each element.

use std::mem::size_of;

use crate::tree::document::Document;
use crate::tree::node::Node;

/// What the document's element arena holds, read in constant time
/// ([`Document::memory_footprint`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MemoryFootprint {
    /// How many element nodes are live: text nodes, comments and the
    /// arena's reserved slot are not counted.
    pub live_elements: usize,
    /// The size of one node in the arena, `size_of::<Node<T>>()`: every
    /// node's inline part, whatever its kind. What a node owns behind a
    /// pointer — its computed style, its attribute storage, its text — is
    /// not counted.
    pub node_stride_bytes: usize,
    /// One past the highest arena key ever occupied: the number of slots
    /// the arena's side tables are sized to, live or vacant.
    pub slot_bound: usize,
}

impl MemoryFootprint {
    /// `live_elements × node_stride_bytes`: the inline bytes of the live
    /// elements, saturating.
    #[must_use]
    pub const fn element_bytes(&self) -> usize {
        self.live_elements.saturating_mul(self.node_stride_bytes)
    }
}

impl<T> Document<T> {
    /// How much of the arena the document's elements occupy now. Three
    /// field reads and a constant; nothing is walked.
    #[must_use]
    pub fn memory_footprint(&self) -> MemoryFootprint {
        let arenas = self.arenas();
        MemoryFootprint {
            live_elements: arenas.live_elements(),
            node_stride_bytes: size_of::<Node<T>>(),
            slot_bound: arenas.slot_bound(),
        }
    }
}
