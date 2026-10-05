//! The statically split tree/state protocol used by every layout algorithm.

mod io;

pub use io::{
    AvailableSpace, Layout, LayoutGoal, LayoutInput, LayoutOutput, RequestedAxis, SizingMode,
};
use smallvec::SmallVec;

use crate::cache::Cache;
use crate::geometry::{Edges, Point, Rect, Size};
use crate::style::{CoreStyle, DashedIdent, Display, PhysicalAxis, TreeScoped};

/// Per-node marks that outlive one layout pass: what the rounding tail still
/// owes this node, and whether its subtree is already hidden.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SlotMarks(u8);

impl Default for SlotMarks {
    fn default() -> Self {
        // A new slot has never been rounded. In particular, a newly inserted
        // display: contents wrapper has no algorithm or box write to mark
        // it later, but rounding must still reach the children it flattens.
        Self(Self::SUBTREE_DIRTY)
    }
}

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
        // its parent does not lay out — an escaping absolute box, whose box
        // its containing block writes — is reached by no other mark.
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

    /// Records the static position of an out-of-flow child whose
    /// [`CoreStyle::position`] lowered to `fixed` — one whose containing
    /// block its box parent does not generate — in the parent's border-box
    /// coordinates. The parent's algorithm calls this for every such child
    /// on every committing run, so it is also where a host learns which
    /// containing block the child escapes to ([`Self::hoisted_children`]).
    fn set_static_position(
        &self,
        state: &mut Self::State,
        node: Self::NodeId,
        position: Point<f32>,
    ) {
        let slot = self.layout_mut(state, node);
        if slot.static_position != position {
            slot.static_position = position;
            // The box itself is written later — by its containing block's
            // absolute pass, or by the host after the run — from this
            // position; moving it is the only warning the rounding tail gets.
            slot.mark_subtree_dirty();
        }
    }

    /// The out-of-flow boxes whose containing block `node` generates but
    /// whose box parent is another box — an `absolute` box under a
    /// non-positioned parent, a `fixed` one under a transformed ancestor
    /// (css-position-3 §2.1) — in flat tree order, each with the index in
    /// [`Self::flattened_children`]`(node)` of the child whose subtree holds
    /// it.
    ///
    /// `node`'s algorithm lays these out in its absolute pass, interleaved
    /// with its own out-of-flow children in tree order: css-position-3 lays
    /// an absolutely positioned box out with its containing block, after
    /// that block's in-flow content, and css-anchor-position-1 §2.3's
    /// acceptable anchors are exactly the boxes that order has already
    /// placed. The algorithm asks again after each out-of-flow box it lays
    /// out, so a host may report a box first registered by that layout (the
    /// box's parent ran [`Self::set_static_position`] inside it). A box whose
    /// containing block is the initial containing block is not reported:
    /// the host places it after the run.
    ///
    /// The default reports none, which is a host that does not lower
    /// positions.
    fn hoisted_children(
        &self,
        state: &Self::State,
        node: Self::NodeId,
    ) -> SmallVec<[HoistedChild<Self::NodeId>; 2]> {
        let _ = (state, node);
        SmallVec::new()
    }

    /// Whether [`Self::hoisted_children`] may report a box for `node`: what
    /// every container's absolute pass asks before it walks them, once per
    /// container on every committing run, so a host keeps it cheap. It may
    /// answer `true` for a node `hoisted_children` then reports nothing for,
    /// never `false` for one it reports a box for. The default asks
    /// [`Self::hoisted_children`].
    fn has_hoisted_children(&self, state: &Self::State, node: Self::NodeId) -> bool {
        !self.hoisted_children(state, node).is_empty()
    }

    /// The border-box origin of `node`'s box parent in the border-box
    /// coordinates of `containing_block`, from this run's committed boxes:
    /// what turns the static position the parent recorded into the
    /// containing block's space, and the layout computed there back into the
    /// parent's. Asked for a box [`Self::hoisted_children`] reported, just
    /// before it is laid out.
    fn hoisted_parent_offset(
        &self,
        state: &Self::State,
        containing_block: Self::NodeId,
        node: Self::NodeId,
    ) -> Point<f32> {
        let _ = (state, containing_block, node);
        Point::ZERO
    }

    /// Commits the layout of a box [`Self::hoisted_children`] reported, with
    /// `location` in its box parent's border-box coordinates. The host owns
    /// its paint `order` (its place among its parent's children, which the
    /// containing block's algorithm does not know) and whatever marks the
    /// rounding tail needs to reach it below boxes that did not run. The
    /// default only stores it.
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

    /// Records css-position-4's scrollable containing block of the scroll
    /// container `node`, as a committing run of it computed it: the
    /// scrollable overflow of its *in-flow* content measured from its
    /// padding-box origin, never smaller than its padding box. The spec's
    /// area ignores absolutely positioned descendants, so a box laid out
    /// against it cannot grow it. Called only for scroll containers, after
    /// their in-flow layout and before their absolute pass, which is what
    /// lets [`Self::scrollable_containing_block`] answer from the current
    /// run. The default keeps nothing.
    fn set_scrollable_containing_block(
        &self,
        state: &mut Self::State,
        node: Self::NodeId,
        size: Size<f32>,
    ) {
        let _ = (state, node, size);
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

    /// Records the containing block a `position: sticky` item resolves its
    /// insets against when that is not its box parent's content box: a grid
    /// item's grid area (css-position-3 §3.4 over css-grid-1 §9.1), as edges
    /// in the parent's border-box coordinates, unrounded. `None` retracts an
    /// earlier record. Committing layouts call this for every grid item;
    /// the bounds are so rare that the default keeps nothing, and a host
    /// that pins nothing pays nothing per node — which is why they are not
    /// a field of [`Layout`].
    fn set_sticky_containing_block(
        &self,
        state: &mut Self::State,
        node: Self::NodeId,
        bounds: Option<Edges<f32>>,
    ) {
        let _ = (state, node, bounds);
    }

    /// The unrounded bounds a host recorded through
    /// [`Self::set_sticky_containing_block`], read back by the rounding pass.
    fn sticky_containing_block(
        &self,
        state: &Self::State,
        node: Self::NodeId,
    ) -> Option<Edges<f32>> {
        let _ = (state, node);
        None
    }

    /// The rounding pass's device-snapped copy of the recorded bounds, in
    /// the same coordinates the rounded [`Layout`] uses.
    fn set_rounded_sticky_containing_block(
        &self,
        state: &mut Self::State,
        node: Self::NodeId,
        bounds: Edges<f32>,
    ) {
        let _ = (state, node, bounds);
    }

    /// css-anchor-position-1: the unrounded border box of the target anchor
    /// element `spec` selects for the absolutely positioned `node` (§2.3)
    /// while it is laid out with position option `option` (§6.1; `0` is the
    /// box's own style), or `None` when there is none — then the `anchor()`
    /// or `anchor-size()` naming it takes its fallback, and a
    /// `position-area` or `anchor-center` finds no default anchor.
    ///
    /// The rectangle is in the coordinates of the padding box of the element
    /// that generates `node`'s containing block — the frame the containing
    /// block's own absolute pass lays `node` out in. The engine rebases it
    /// itself onto what it actually lays the box out against: a grid area,
    /// the scrollable containing block, a `position-area` region. The host
    /// applies §3.3's remembered scroll offsets (and sticky shifts) of the
    /// anchor's scroll containers up to, not including, that containing
    /// block; layout coordinates are otherwise unscrolled.
    ///
    /// The engine asks while laying `node` out through its containing block's
    /// absolute pass, so the answer must come from the current pass: every
    /// in-flow child of that containing block is committed by then, and so is
    /// every absolutely positioned box it is the containing block of that
    /// comes before `node` in tree order — its own out-of-flow children and
    /// the ones [`Self::hoisted_children`] reports alike, since each
    /// algorithm lays both out after its in-flow commit, interleaved in tree
    /// order. Which elements are acceptable targets — `anchor-name`,
    /// `anchor-scope`, tree scopes, `position-anchor` for
    /// [`AnchorSpec::Default`] under the option's style — is the host's
    /// decision. `anchor-size()` reads the rectangle's size.
    ///
    /// The default answers `None`: a host with no anchors resolves every
    /// function to its fallback and gives no box a default anchor.
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

    /// css-anchor-position-1 §3.3's last compensation condition: whether the
    /// target anchor element `name` selects for `node` (under `option`) has
    /// the same nearest scroll container with `axis` scrollable as `node`'s
    /// default anchor element. Asked only for an `anchor()` that resolved in
    /// a used inset on `axis` while `node` has a default anchor; an
    /// `anchor()` with no name targets the default anchor itself and is not
    /// asked about. The default answers `false`.
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

    /// css-position-4's scrollable containing block: when `node`'s
    /// containing block is generated by a scroll container, the size of that
    /// container's scrollable overflow area measured from its padding-box
    /// origin (never smaller than the padding box), else `None`. The engine
    /// asks only for a box that has a default anchor and whose containing
    /// block is its generator's whole padding box, and lays it out against
    /// that size instead; the origin is the padding box's. The default
    /// answers `None`.
    fn scrollable_containing_block(
        &self,
        state: &Self::State,
        node: Self::NodeId,
    ) -> Option<Size<f32>> {
        let _ = (state, node);
        None
    }

    /// The length of `node`'s position options list (css-anchor-position-1
    /// §6.1): its own style plus one entry per `position-try-fallbacks`
    /// option that produced one. `0` or `1` both mean "no fallbacks" — the
    /// box is laid out once, with its own style. The default answers `0`.
    fn position_option_count(&self, node: Self::NodeId) -> usize {
        let _ = node;
        0
    }

    /// Position option `index` of `node`: `0` is the box's own style, the
    /// rest are its fallbacks in `position-try-fallbacks` order, each already
    /// cascaded by the host (the Position Fallback Origin applied over the
    /// base style, then its try tactic). The engine reads only the accepted
    /// `@position-try` properties from it — insets, margins, sizes and
    /// min/max sizes, `justify-self`/`align-self`, `position-area` (and,
    /// through the host, `position-anchor`) — and everything else from
    /// [`Self::style`]. `index` is below [`Self::position_option_count`].
    fn position_option_style(&self, node: Self::NodeId, index: usize) -> Self::Style<'_> {
        debug_assert_eq!(index, 0, "a host without fallbacks has only option 0");
        self.style(node)
    }

    /// The last successful position option of `node` (§6.5.1.1), which the
    /// fallback determination starts from and keeps while it fits. The host
    /// records it — at its `ResizeObserver`-equivalent moment, from the
    /// [`AnchorOutcome::chosen`] of the last pass — and clears it on a
    /// fallback-sensitive change; the engine never writes it, because §6.5.1.1
    /// records it after layout, not during it. The default answers `None`.
    fn last_successful_option(&self, state: &Self::State, node: Self::NodeId) -> Option<usize> {
        let _ = (state, node);
        None
    }

    /// Hands the host what a committing absolute pass decided for an
    /// anchor-positioned `node`: every box that uses an anchor function,
    /// `position-area`, `anchor-center`, a `position-anchor` naming an
    /// element, or position options — exactly the boxes
    /// [`crate::compute::uses_anchor_positioning`] (or a position option
    /// count above one) selects. Boxes that use none of them are never
    /// reported (the host knows it cleared them). The default keeps
    /// nothing.
    fn set_anchor_outcome(
        &self,
        state: &mut Self::State,
        node: Self::NodeId,
        outcome: AnchorOutcome,
    ) {
        let _ = (state, node, outcome);
    }
}

/// One box [`LayoutTree::hoisted_children`] reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HoistedChild<N> {
    /// The hoisted box.
    pub node: N,
    /// The index, among the containing block's
    /// [`LayoutTree::flattened_children`], of the child whose subtree holds
    /// `node`: the box comes after that child, and after every own
    /// out-of-flow child before it, in tree order.
    pub via: usize,
}

/// Which target anchor element an anchor query names (css-anchor-position-1
/// §2.3).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AnchorSpec<'a> {
    /// The box's default anchor element (§2.4): what `position-area`,
    /// `anchor-center` and an `anchor()`/`anchor-size()` without a name use.
    Default,
    /// A tree-scoped `<anchor-name>`; the host resolves which tree it names.
    Named(&'a TreeScoped<DashedIdent>),
}

/// What one committing absolute pass decided for an anchor-positioned box,
/// for the host to keep for paint, compose and the next pass. Rectangles are
/// in the same coordinates as [`LayoutTree::anchor_rect`]'s answers: the
/// containing block generator's padding box.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AnchorOutcome {
    /// The position option the box was laid out with (`0` = its own style).
    pub chosen: usize,
    /// css-anchor-position-1 §6.5: no option kept the margin box inside the
    /// inset-modified containing block (`position-visibility: no-overflow`).
    pub overflows: bool,
    /// §6.6 `anchor-valid`: the chosen option references the default anchor
    /// — `position-area`, `anchor-center`, or an `anchor()`/`anchor-size()`
    /// without a name — and the box has no default anchor element under it.
    pub default_anchor_missing: bool,
    /// §3.3: per physical axis, whether the box compensates for the scroll
    /// of its default anchor.
    pub compensates: Size<bool>,
    /// Which edges of [`Self::imcb`] keep their relation to the margin box
    /// under the box's default scroll shift, for §6.5's fit test "after
    /// applying any default scroll shift": an edge a non-`auto` inset places
    /// (`anchor()` carries it with the anchor; a length leaves the fit on
    /// that side unconstrained, as Blink's
    /// `CalculateNonOverflowingRangeInOneAxis` does), and an `auto` inset's
    /// `position-area` line that is the default anchor box's own edge. The
    /// other edges — an `auto` inset's containing-block edge — stay put.
    pub carried_edges: Edges<bool>,
    /// The inset-modified containing block the box was laid out in.
    pub imcb: Rect<f32>,
    /// The box's margin box.
    pub margin_box: Rect<f32>,
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
