//! The statically split tree/state protocol used by every layout algorithm.

mod io;

pub use io::{
    AvailableSpace, Layout, LayoutGoal, LayoutInput, LayoutOutput, RequestedAxis, SizingMode,
};
use smallvec::SmallVec;

use crate::cache::{Cache, InputReads};
use crate::geometry::{Edges, Point, Rect, Size};
use crate::style::{CoreStyle, DashedIdent, Display, PhysicalAxis, TreeScoped};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SlotMarks(u8);

impl Default for SlotMarks {
    fn default() -> Self {
        Self(Self::SUBTREE_DIRTY)
    }
}

impl SlotMarks {
    const BOX_CHANGED: u8 = 1 << 0;
    const SUBTREE_DIRTY: u8 = 1 << 1;
    const HIDDEN: u8 = 1 << 2;

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

    #[must_use]
    pub(crate) fn recorded_reads(&self) -> Option<InputReads> {
        self.cache.recorded_reads()
    }

    pub(crate) fn store_cached_layout_recording_reads(
        &mut self,
        input: LayoutInput,
        output: LayoutOutput,
        reads: InputReads,
    ) {
        self.cache.store_recording_reads(input, output, reads);
    }

    pub fn clear_layout_cache(&mut self) {
        self.cache.clear();
        self.marks.insert(SlotMarks::SUBTREE_DIRTY);
    }

    pub fn mark_subtree_dirty(&mut self) {
        self.marks.insert(SlotMarks::SUBTREE_DIRTY);
    }

    pub fn set_unrounded(&mut self, layout: Layout) {
        if self.unrounded != layout {
            self.unrounded = layout;
            self.marks.insert(SlotMarks::BOX_CHANGED);
        }
        self.marks.remove(SlotMarks::HIDDEN);
    }

    #[must_use]
    pub fn needs_rounding(&self) -> bool {
        self.marks
            .intersects(SlotMarks::BOX_CHANGED | SlotMarks::SUBTREE_DIRTY)
    }

    #[must_use]
    pub fn box_changed(&self) -> bool {
        self.marks.intersects(SlotMarks::BOX_CHANGED)
    }

    pub fn clear_rounding_marks(&mut self) {
        self.marks
            .remove(SlotMarks::BOX_CHANGED | SlotMarks::SUBTREE_DIRTY);
    }

    #[must_use]
    pub fn is_hidden(&self) -> bool {
        self.marks.intersects(SlotMarks::HIDDEN)
    }

    pub fn mark_hidden(&mut self) {
        self.marks.insert(SlotMarks::HIDDEN);
    }

    pub fn set_unrounded_content_size(&mut self, content_size: Size<f32>) {
        if self.unrounded.content_size != content_size {
            self.unrounded.content_size = content_size;
            self.marks.insert(SlotMarks::BOX_CHANGED);
        }
    }

    pub fn set_hidden_order(&mut self, order: u32) {
        if self.unrounded.order != order {
            self.unrounded.order = order;
            self.marks.insert(SlotMarks::BOX_CHANGED);
        }
    }

    #[must_use]
    pub fn committed_input(&self) -> Option<LayoutInput> {
        self.cache.committed_input()
    }

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
            slot.mark_subtree_dirty();
        }
    }

    fn hoisted_children(
        &self,
        state: &Self::State,
        node: Self::NodeId,
    ) -> SmallVec<[HoistedChild<Self::NodeId>; 2]> {
        let _ = (state, node);
        SmallVec::new()
    }

    fn reads_definiteness(&self, node: Self::NodeId) -> bool {
        let _ = node;
        true
    }

    fn has_hoisted_children(&self, state: &Self::State, node: Self::NodeId) -> bool {
        !self.hoisted_children(state, node).is_empty()
    }

    fn hoisted_parent_offset(
        &self,
        state: &Self::State,
        containing_block: Self::NodeId,
        node: Self::NodeId,
    ) -> Point<f32> {
        let _ = (state, containing_block, node);
        Point::ZERO
    }

    fn set_hoisted_layout(
        &self,
        state: &mut Self::State,
        containing_block: Self::NodeId,
        node: Self::NodeId,
        layout: Layout,
    ) {
        let _ = containing_block;
        self.set_unrounded_layout(state, node, layout);
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

    fn set_sticky_containing_block(
        &self,
        state: &mut Self::State,
        node: Self::NodeId,
        bounds: Option<Edges<f32>>,
    ) {
        let _ = (state, node, bounds);
    }

    fn sticky_containing_block(
        &self,
        state: &Self::State,
        node: Self::NodeId,
    ) -> Option<Edges<f32>> {
        let _ = (state, node);
        None
    }

    fn set_rounded_sticky_containing_block(
        &self,
        state: &mut Self::State,
        node: Self::NodeId,
        bounds: Edges<f32>,
    ) {
        let _ = (state, node, bounds);
    }

    fn anchor_rect(
        &self,
        state: &Self::State,
        node: Self::NodeId,
        option: usize,
        spec: AnchorSpec<'_>,
    ) -> Option<Rect<f32>> {
        let _ = (state, node, option, spec);
        None
    }

    fn anchor_scrolls_with_default(
        &self,
        state: &Self::State,
        node: Self::NodeId,
        option: usize,
        name: &TreeScoped<DashedIdent>,
        axis: PhysicalAxis,
    ) -> bool {
        let _ = (state, node, option, name, axis);
        false
    }

    fn position_option_count(&self, node: Self::NodeId) -> usize {
        let _ = node;
        0
    }

    fn position_option_style(&self, node: Self::NodeId, index: usize) -> Self::Style<'_> {
        debug_assert_eq!(index, 0, "a host without fallbacks has only option 0");
        self.style(node)
    }

    fn last_successful_option(&self, state: &Self::State, node: Self::NodeId) -> Option<usize> {
        let _ = (state, node);
        None
    }

    fn set_anchor_outcome(
        &self,
        state: &mut Self::State,
        node: Self::NodeId,
        outcome: AnchorOutcome,
    ) {
        let _ = (state, node, outcome);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HoistedChild<N> {
    pub node: N,
    pub via: usize,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AnchorSpec<'a> {
    Default,
    Named(&'a TreeScoped<DashedIdent>),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AnchorOutcome {
    pub chosen: usize,
    pub overflows: bool,
    pub default_anchor_missing: bool,
    pub compensates: Size<bool>,
    pub carried_edges: Edges<bool>,
    pub imcb: Rect<f32>,
    pub margin_box: Rect<f32>,
}

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
    #[must_use]
    #[inline]
    pub fn capacity_hint(&self) -> usize {
        let (lower, upper) = self.level.size_hint();
        upper.unwrap_or(lower)
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
        assert!(
            slot.needs_rounding(),
            "a new subtree has never been rounded"
        );
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
