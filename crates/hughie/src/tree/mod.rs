//! The statically split tree/state protocol used by every layout algorithm.

mod io;

pub use io::{
    AvailableSpace, Layout, LayoutGoal, LayoutInput, LayoutOutput, RequestedAxis, SizingMode,
};
use smallvec::SmallVec;

use crate::cache::Cache;
use crate::geometry::{Point, Size};
use crate::style::{CoreStyle, Display};

/// Per-node marks that outlive one layout pass: what the rounding tail still
/// owes this node, and whether its subtree is already hidden.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct SlotMarks(u8);

impl SlotMarks {
    /// The node's own unrounded box moved or resized since it was last
    /// rounded. Its descendants inherit the visit: their accumulated position
    /// shifted with it, and anything positioned against it as a containing
    /// block resolves differently.
    const BOX_CHANGED: u8 = 1 << 0;
    /// The node re-ran its own algorithm — or had its cache cleared — since it
    /// was last rounded, so a descendant's box may have been rewritten. Only
    /// the walk down to those descendants is owed, not this node's own box.
    const SUBTREE_DIRTY: u8 = 1 << 1;
    /// The node and its whole subtree already carry the hidden layout, so
    /// hiding it again is a no-op below the root of the hidden subtree.
    const HIDDEN: u8 = 1 << 2;

    /// Whether any mark in the set is present.
    #[inline]
    const fn intersects(self, marks: u8) -> bool {
        self.0 & marks != 0
    }

    #[inline]
    const fn insert(&mut self, mark: u8) {
        self.0 |= mark;
    }

    #[inline]
    const fn remove(&mut self, mark: u8) {
        self.0 &= !mark;
    }
}

/// Engine-owned values stored for each live node in host-owned storage.
#[derive(Debug, Default)]
pub struct LayoutSlot {
    cache: Cache,
    marks: SlotMarks,
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
        // The node will be laid out again by whoever cleared this, and a node
        // its parent lays out out-of-flow — an escaping absolute box, whose
        // box the rounding tail itself writes — is reached by no other mark.
        self.marks.insert(SlotMarks::SUBTREE_DIRTY);
    }

    /// Records that the node re-ran its own algorithm, so the rounding tail
    /// must walk down to whatever that rewrote.
    pub fn mark_subtree_dirty(&mut self) {
        self.marks.insert(SlotMarks::SUBTREE_DIRTY);
    }

    /// Replaces the node's committed box, marking it for the rounding tail
    /// when the box actually moved. A node that is written its own box is by
    /// definition no longer hidden.
    pub fn set_unrounded(&mut self, layout: Layout) {
        if self.unrounded != layout {
            self.unrounded = layout;
            self.marks.insert(SlotMarks::BOX_CHANGED);
        }
        self.marks.remove(SlotMarks::HIDDEN);
    }

    /// Whether the rounding tail owes this node or its subtree a visit.
    #[must_use]
    pub fn needs_rounding(&self) -> bool {
        self.marks
            .intersects(SlotMarks::BOX_CHANGED | SlotMarks::SUBTREE_DIRTY)
    }

    /// Whether this node's own box moved, which every descendant inherits.
    #[must_use]
    pub fn box_changed(&self) -> bool {
        self.marks.intersects(SlotMarks::BOX_CHANGED)
    }

    /// Clears what the tail has just paid off for this node.
    pub fn clear_rounding_marks(&mut self) {
        self.marks
            .remove(SlotMarks::BOX_CHANGED | SlotMarks::SUBTREE_DIRTY);
    }

    /// Whether this node and its whole subtree already carry the hidden
    /// layout.
    #[must_use]
    pub fn is_hidden(&self) -> bool {
        self.marks.intersects(SlotMarks::HIDDEN)
    }

    /// Marks the node as the root of an already-hidden subtree.
    pub fn mark_hidden(&mut self) {
        self.marks.insert(SlotMarks::HIDDEN);
    }

    /// Replaces only the committed box's scrollable content size, which a
    /// containment boundary recomputes without moving the box itself.
    pub fn set_unrounded_content_size(&mut self, content_size: Size<f32>) {
        if self.unrounded.content_size != content_size {
            self.unrounded.content_size = content_size;
            self.marks.insert(SlotMarks::BOX_CHANGED);
        }
    }

    /// Moves an already-hidden subtree to a new paint-order slot, the one
    /// thing about it that a sibling reorder can still change.
    pub fn set_hidden_order(&mut self, order: u32) {
        if self.unrounded.order != order {
            self.unrounded.order = order;
            self.marks.insert(SlotMarks::BOX_CHANGED);
        }
    }

    /// The complete input the last commit ran under, independence payload
    /// included: the packed committed entry carries it beside the key bits.
    #[must_use]
    pub fn committed_input(&self) -> Option<LayoutInput> {
        self.cache.committed_input()
    }

    /// Returns the committed input/output pair when the committing parent
    /// marked the input content-independent on both axes — the license for a
    /// host to relayout this subtree in place under the stored input.
    #[must_use]
    pub fn committed_independent(&self) -> Option<(LayoutInput, LayoutOutput)> {
        let input = self.committed_input()?;
        if input.goal.independence() != Some(Size::new(true, true)) {
            return None;
        }
        self.cache.committed_output().map(|output| (input, output))
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
        self.layout_mut(state, node).set_unrounded(layout);
    }

    fn set_static_position(
        &self,
        state: &mut Self::State,
        node: Self::NodeId,
        position: Point<f32>,
    ) {
        let slot = self.layout_mut(state, node);
        if slot.static_position != position {
            slot.static_position = position;
            // The box itself is written by the rounding tail, from this
            // position; moving it is the only warning the tail gets.
            slot.mark_subtree_dirty();
        }
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
            Size::new(false, false),
        );
        let output = LayoutOutput::new(Size::new(40.0, 20.0), Size::new(50.0, 30.0));

        slot.store_cached_layout(input, output);

        assert_eq!(slot.unrounded.size, Size::ZERO);
        assert_eq!(slot.unrounded.content_size, Size::ZERO);
        assert_eq!(slot.cached_layout(input), Some(output));
    }

    #[test]
    fn the_packed_committed_slot_carries_the_independence_payload() {
        let mut slot = LayoutSlot::default();
        let known = Size::new(Some(40.0), Some(20.0));
        let mut input =
            LayoutInput::commit(known, Size::NONE, Size::MAX_CONTENT, Size::new(true, false));
        let output = LayoutOutput::new(Size::new(40.0, 20.0), Size::new(50.0, 30.0));

        slot.store_cached_layout(input, output);
        assert_eq!(slot.committed_input(), Some(input));
        assert_eq!(
            slot.committed_independent(),
            None,
            "one independent axis is not a license: the other one still moves",
        );

        input = LayoutInput::commit(known, Size::NONE, Size::MAX_CONTENT, Size::new(false, true));
        slot.store_cached_layout(input, output);
        assert_eq!(slot.committed_input(), Some(input));
        assert_eq!(slot.committed_independent(), None);

        input = LayoutInput::commit(known, Size::NONE, Size::MAX_CONTENT, Size::new(true, true));
        slot.store_cached_layout(input, output);
        assert_eq!(slot.committed_independent(), Some((input, output)));

        let measured =
            LayoutInput::measure(known, Size::NONE, Size::MAX_CONTENT, RequestedAxis::Both);
        slot.store_cached_layout(measured, output);
        assert_eq!(
            slot.committed_independent(),
            Some((input, output)),
            "a measurement neither commits nor disturbs the committed payload",
        );

        slot.clear_layout_cache();
        assert_eq!(slot.committed_input(), None);
        assert_eq!(slot.committed_independent(), None);
        assert!(
            slot.needs_rounding(),
            "clearing the cache is itself a promise the node will be laid out again",
        );
    }

    #[test]
    fn slot_marks_track_what_the_rounding_tail_and_the_hider_still_owe() {
        let mut slot = LayoutSlot::default();
        let mut layout = Layout::with_order(3);
        layout.size = Size::new(40.0, 20.0);

        slot.set_unrounded(layout);
        assert!(slot.needs_rounding() && slot.box_changed());

        slot.clear_rounding_marks();
        assert!(!slot.needs_rounding() && !slot.box_changed());

        let mut same = Layout::with_order(3);
        same.size = Size::new(40.0, 20.0);
        slot.set_unrounded(same);
        assert!(
            !slot.needs_rounding(),
            "rewriting the same box is not a change the tail has to chase",
        );

        slot.set_unrounded_content_size(Size::new(60.0, 30.0));
        assert!(slot.box_changed());
        slot.clear_rounding_marks();
        slot.set_unrounded_content_size(Size::new(60.0, 30.0));
        assert!(!slot.needs_rounding());

        slot.mark_subtree_dirty();
        assert!(slot.needs_rounding() && !slot.box_changed());
        slot.clear_rounding_marks();

        assert!(!slot.is_hidden());
        slot.mark_hidden();
        assert!(slot.is_hidden());
        slot.set_hidden_order(3);
        assert!(
            !slot.needs_rounding(),
            "the box already sits at that paint-order slot",
        );
        slot.set_hidden_order(9);
        assert!(slot.box_changed() && slot.is_hidden());
        let mut relaid = Layout::with_order(3);
        relaid.size = Size::new(40.0, 20.0);
        slot.set_unrounded(relaid);
        assert!(
            !slot.is_hidden(),
            "a node written its own box is no longer hidden",
        );
    }

    #[cfg(target_pointer_width = "64")]
    #[test]
    fn layout_slot_fits_the_split_state_memory_budget() {
        let size = core::mem::size_of::<LayoutSlot>();
        assert!(size <= 336, "LayoutSlot grew to {size} bytes");
    }
}
