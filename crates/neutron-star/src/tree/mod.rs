//! The statically split tree/state protocol used by every layout algorithm.

mod io;

pub use io::{
    AvailableSpace, Layout, LayoutGoal, LayoutInput, LayoutOutput, RequestedAxis, SizingMode,
};

use crate::cache::Cache;
use crate::geometry::Point;
use crate::style::CoreStyle;

/// Engine-owned values stored once per live node in host-owned storage.
///
/// The host chooses the arena or other container. Layout receives the tree
/// through a shared borrow and the state through a separate exclusive borrow,
/// so these slots need no interior mutability or runtime borrow tracking.
#[derive(Debug, Default)]
pub struct LayoutSlot {
    cache: Cache,
    static_position: Point<f32>,
    unrounded: Layout,
    rounded: Layout,
}

impl LayoutSlot {
    #[must_use]
    pub const fn unrounded(&self) -> &Layout {
        &self.unrounded
    }

    pub const fn unrounded_mut(&mut self) -> &mut Layout {
        &mut self.unrounded
    }

    pub fn set_unrounded(&mut self, layout: Layout) {
        self.unrounded = layout;
    }

    #[must_use]
    pub const fn rounded(&self) -> &Layout {
        &self.rounded
    }

    pub const fn rounded_mut(&mut self) -> &mut Layout {
        &mut self.rounded
    }

    pub fn set_rounded(&mut self, layout: Layout) {
        self.rounded = layout;
    }

    #[must_use]
    pub const fn static_position(&self) -> Point<f32> {
        self.static_position
    }

    pub const fn set_static_position(&mut self, static_position: Point<f32>) {
        self.static_position = static_position;
    }

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

    #[must_use]
    pub fn layout_cache_is_empty(&self) -> bool {
        self.cache.is_empty()
    }
}

/// Immutable tree/style access paired with a separately borrowed mutable
/// layout state.
///
/// Node handles are plain IDs. A layout call receives `&Self` and
/// `&mut Self::State` independently, allowing guarded style borrows from the
/// tree to remain alive while recursion mutates only layout/text state.
pub trait LayoutTree {
    type NodeId: Copy + core::fmt::Debug;
    type State;

    type Style<'tree>: CoreStyle
    where
        Self: 'tree;

    type ChildIter<'tree>: Iterator<Item = Self::NodeId>
    where
        Self: 'tree;

    fn children(&self, node: Self::NodeId) -> Self::ChildIter<'_>;

    fn child_count(&self, node: Self::NodeId) -> usize {
        self.children(node).count()
    }

    fn style(&self, node: Self::NodeId) -> Self::Style<'_>;

    fn layout<'state>(&self, state: &'state Self::State, node: Self::NodeId) -> &'state LayoutSlot;

    fn layout_mut<'state>(
        &self,
        state: &'state mut Self::State,
        node: Self::NodeId,
    ) -> &'state mut LayoutSlot;

    fn set_unrounded_layout(&self, state: &mut Self::State, node: Self::NodeId, layout: Layout) {
        self.layout_mut(state, node).set_unrounded(layout);
    }

    fn set_static_position(
        &self,
        state: &mut Self::State,
        node: Self::NodeId,
        position: Point<f32>,
    ) {
        self.layout_mut(state, node).set_static_position(position);
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

        assert_eq!(slot.unrounded().size, Size::ZERO);
        assert_eq!(slot.unrounded().content_size, Size::ZERO);
        assert_eq!(slot.cached_layout(input), Some(output));
    }

    #[cfg(target_pointer_width = "64")]
    #[test]
    fn layout_slot_fits_the_split_state_memory_budget() {
        let size = core::mem::size_of::<LayoutSlot>();
        assert!(size <= 648, "LayoutSlot grew to {size} bytes");
    }
}
