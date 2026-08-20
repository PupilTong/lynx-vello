//! The statically split tree/state protocol used by every layout algorithm.

mod io;

pub use io::{
    AvailableSpace, Layout, LayoutGoal, LayoutInput, LayoutOutput, RequestedAxis, SizingMode,
};
use smallvec::SmallVec;

use crate::cache::Cache;
use crate::geometry::Point;
use crate::style::{CoreStyle, Display};

/// Engine-owned values stored for each live node in host-owned storage.
#[derive(Debug, Default)]
pub struct LayoutSlot {
    cache: Cache,
    pub static_position: Point<f32>,
    pub unrounded: Layout,
    pub rounded: Layout,
}

impl LayoutSlot {
    #[must_use]
    pub fn cached_layout(&self, input: LayoutInput) -> Option<LayoutOutput> {
        self.cache.get(input)
    }

    pub fn store_cached_layout(&mut self, input: LayoutInput, output: LayoutOutput) {
        self.cache.store(input, output);
    }

    pub fn clear_layout_cache(&mut self) {
        self.cache.clear();
    }

    #[must_use]
    pub fn committed_input(&self) -> Option<LayoutInput> {
        self.cache.committed_input()
    }

    /// See [`crate::cache::Cache::set_committed_content_independent`].
    pub fn set_committed_content_independent(&mut self, independent: bool) {
        self.cache.set_committed_content_independent(independent);
    }

    /// See [`crate::cache::Cache::committed_independent`].
    #[must_use]
    pub fn committed_independent(&self) -> Option<(LayoutInput, LayoutOutput)> {
        self.cache.committed_independent()
    }

    #[must_use]
    pub fn layout_cache_is_empty(&self) -> bool {
        self.cache.is_empty()
    }
}

/// Immutable topology and style access with separately borrowed layout state.
pub trait LayoutTree {
    type NodeId: Copy + core::fmt::Debug;
    type State;

    type Style<'tree>: CoreStyle
    where
        Self: 'tree;

    type ChildIter<'tree>: Iterator<Item = Self::NodeId>
    where
        Self: 'tree;

    /// Returns all source children, including nodes that generate no box.
    fn children(&self, node: Self::NodeId) -> Self::ChildIter<'_>;

    fn child_count(&self, node: Self::NodeId) -> usize {
        self.children(node).count()
    }

    /// Flattens `display: contents` subtrees while preserving source order.
    /// Each item includes the style and display value already read by the walk.
    fn flattened_children(&self, node: Self::NodeId) -> FlattenedChildren<'_, Self>
    where
        Self: Sized,
    {
        FlattenedChildren {
            tree: self,
            level: self.children(node),
            outer: SmallVec::new(),
        }
    }

    fn style(&self, node: Self::NodeId) -> Self::Style<'_>;

    fn layout<'state>(&self, state: &'state Self::State, node: Self::NodeId) -> &'state LayoutSlot;

    fn layout_mut<'state>(
        &self,
        state: &'state mut Self::State,
        node: Self::NodeId,
    ) -> &'state mut LayoutSlot;

    fn set_unrounded_layout(&self, state: &mut Self::State, node: Self::NodeId, layout: Layout) {
        self.layout_mut(state, node).unrounded = layout;
    }

    fn set_static_position(
        &self,
        state: &mut Self::State,
        node: Self::NodeId,
        position: Point<f32>,
    ) {
        self.layout_mut(state, node).static_position = position;
    }

    fn compute_layout(
        &self,
        state: &mut Self::State,
        node: Self::NodeId,
        input: LayoutInput,
    ) -> LayoutOutput;

    fn clear_layout_cache(&self, state: &mut Self::State, node: Self::NodeId) {
        self.layout_mut(state, node).clear_layout_cache();
    }
}

/// An iterator over source children with `display: contents` flattened.
pub struct FlattenedChildren<'tree, T: LayoutTree> {
    tree: &'tree T,
    level: T::ChildIter<'tree>,
    outer: SmallVec<[T::ChildIter<'tree>; 2]>,
}

impl<T: LayoutTree> core::fmt::Debug for FlattenedChildren<'_, T> {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("FlattenedChildren")
            .field("suspended_levels", &self.outer.len())
            .finish_non_exhaustive()
    }
}

impl<T: LayoutTree> FlattenedChildren<'_, T> {
    /// Returns a buffer-capacity estimate for the flattened walk.
    #[must_use]
    #[inline]
    pub fn capacity_hint(&self) -> usize {
        self.level.size_hint().0
    }
}

impl<'tree, T: LayoutTree> Iterator for FlattenedChildren<'tree, T> {
    type Item = (T::NodeId, T::Style<'tree>, Display);

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let Some(child) = self.level.next() else {
                self.level = self.outer.pop()?;
                continue;
            };
            let style = self.tree.style(child);
            let display = style.display();
            if !display.is_contents() {
                return Some((child, style, display));
            }
            let inner = self.tree.children(child);
            self.outer.push(core::mem::replace(&mut self.level, inner));
        }
    }

    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        (0, None)
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use crate::geometry::Size;

    #[test]
    fn committed_cache_is_independent_from_durable_layout_geometry() {
        let mut slot = LayoutSlot::default();
        let input = LayoutInput::commit(
            Size::new(Some(40.0), Some(20.0)),
            Size::NONE,
            Size::MAX_CONTENT,
        );
        let output = LayoutOutput::new(Size::new(40.0, 20.0), Size::new(50.0, 30.0));

        slot.store_cached_layout(input, output);

        assert_eq!(slot.unrounded.size, Size::ZERO);
        assert_eq!(slot.unrounded.content_size, Size::ZERO);
        assert_eq!(slot.cached_layout(input), Some(output));
    }

    #[cfg(target_pointer_width = "64")]
    #[test]
    fn layout_slot_fits_the_split_state_memory_budget() {
        let size = core::mem::size_of::<LayoutSlot>();
        assert!(size <= 440, "LayoutSlot grew to {size} bytes");
    }
}
