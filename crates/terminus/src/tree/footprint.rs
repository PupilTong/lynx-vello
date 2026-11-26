//! How much of the document's own arena its elements occupy: the reading a
//! memory monitor reports for the element tree, as Lynx's
//! `ReportElementMemoryInfo` reports `sizeof` of each element.

use std::mem::size_of;

use crate::tree::document::Document;
use crate::tree::node::Node;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MemoryFootprint {
    pub live_elements: usize,
    pub node_stride_bytes: usize,
    pub slot_bound: usize,
}

impl MemoryFootprint {
    #[must_use]
    pub const fn element_bytes(&self) -> usize {
        self.live_elements.saturating_mul(self.node_stride_bytes)
    }
}

impl<T> Document<T> {
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
