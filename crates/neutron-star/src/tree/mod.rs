//! The statically split tree/state protocol used by every layout algorithm.

mod io;

pub use io::{
    AvailableSpace, Layout, LayoutGoal, LayoutInput, LayoutOutput, RequestedAxis, SizingMode,
};
use smallvec::SmallVec;

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

    /// The node's **source** children, box-generating or not.
    ///
    /// Cleanup and traversal read this: hiding a subtree and the rounding
    /// walk must reach every descendant. Algorithms collecting the items of
    /// a formatting context read [`box_children`](LayoutTree::box_children)
    /// instead.
    fn children(&self, node: Self::NodeId) -> Self::ChildIter<'_>;

    fn child_count(&self, node: Self::NodeId) -> usize {
        self.children(node).count()
    }

    /// The children that generate a box in this node's formatting context,
    /// each paired with the style the walk already had to read.
    ///
    /// A `display: contents` element generates no box of its own while its
    /// children keep generating theirs, in the *nearest box ancestor's*
    /// formatting context ([CSS Display 3
    /// §3.3](https://drafts.csswg.org/css-display/#valdef-display-contents)),
    /// so this iterator splices such an element's own children into the
    /// sequence in its place, recursively, and never yields it. With no
    /// `display: contents` child it yields exactly [`children`] in order.
    ///
    /// Deciding that takes each child's `display`, which item collection needs
    /// anyway, so the style is handed back rather than looked up twice.
    ///
    /// Provided; a host must not override it.
    ///
    /// [`children`]: LayoutTree::children
    fn box_children(&self, node: Self::NodeId) -> BoxChildren<'_, Self>
    where
        Self: Sized,
    {
        BoxChildren {
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

/// The box-generating children of one node — see
/// [`LayoutTree::box_children`].
///
/// `outer` suspends the child iterators of the levels a `display: contents`
/// element was spliced into, so nesting resumes them in order. It stays
/// empty, and the iterator stays a plain pass-through, for the overwhelmingly
/// common node whose children all generate boxes.
pub struct BoxChildren<'tree, T: LayoutTree> {
    tree: &'tree T,
    level: T::ChildIter<'tree>,
    outer: SmallVec<[T::ChildIter<'tree>; 2]>,
}

impl<T: LayoutTree> core::fmt::Debug for BoxChildren<'_, T> {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("BoxChildren")
            .field("suspended_levels", &self.outer.len())
            .finish_non_exhaustive()
    }
}

impl<'tree, T: LayoutTree> Iterator for BoxChildren<'tree, T> {
    type Item = (T::NodeId, T::Style<'tree>);

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let Some(child) = self.level.next() else {
                self.level = self.outer.pop()?;
                continue;
            };
            let style = self.tree.style(child);
            if !style.display().is_contents() {
                return Some((child, style));
            }
            let inner = self.tree.children(child);
            self.outer.push(core::mem::replace(&mut self.level, inner));
        }
    }

    /// Only a lower bound: each `display: contents` child contributes its own
    /// children instead of itself, so the exact count needs the walk itself.
    /// Callers size their item buffers from it.
    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.level.size_hint().0, None)
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
