//! The statically split [`LayoutTree`] host over the document's immutable
//! tree/style arenas and mutable layout/text state.
//!
//! The pass runs in **slot space**: [`LayoutTree::NodeId`] binds to
//! [`NodeSlot`], which is what the tree's own links already hold, so
//! `children`/`style`/`layout` index an arena directly instead of resolving
//! an id first. Ids are resolved once where the pass enters — at the root and
//! at each parked boundary — because those happen once per pass while the
//! trait calls happen many times per node.

use std::collections::hash_map::Entry;

use rustc_hash::FxHashSet;
#[cfg(feature = "layout-test-utils")]
use seldon::compute::compute_leaf_layout_with_measurement_for_testing;
use seldon::compute::{
    NaturalSize, compute_absolute_layout, compute_boundary_relayout, compute_cached_layout,
    compute_flexbox_layout, compute_grid_lanes_layout, compute_grid_layout, compute_hoisted_layout,
    compute_leaf_layout, compute_linear_layout, compute_relative_layout, compute_root_layout,
    compute_skipped_contents_size, hide_skipped_contents, hide_subtree,
    round_layout_subtree_with as round_with,
};
use seldon::geometry::{Edges, Point, Rect, Size};
use seldon::invalidate::is_relayout_boundary;
use seldon::style::{CoreStyle, DashedIdent, PhysicalAxis, PositionProperty, TreeScoped};
use seldon::tree::{
    AnchorOutcome, AnchorSpec, AvailableSpace, HoistedChild, Layout, LayoutInput, LayoutOutput,
    LayoutSlot, LayoutTree,
};
use smallvec::SmallVec;
use stylo::values::generics::position::PreferredRatio;

use super::inline_flow::compute_inline_flow_layout;
use super::paragraph::{establishes_paragraph, replaces_children};
use super::style::{DisplayMode, StyleView, box_parent, resolve_position};
use super::text_block::compute_text_block_layout;
use super::{anchors, committed_box, hoisted};
use crate::tree::document::{
    DeferredContainer, Document, DocumentLayoutState, NodeId, NodeSlot, PendingRelayout,
    RelayoutKind, StickyContainingBlock, TreeArenas,
};
use crate::tree::node::Node;

impl<T> LayoutTree for TreeArenas<T> {
    type NodeId = NodeSlot;
    type State = DocumentLayoutState;
    type Style<'tree>
        = StyleView<'tree, T>
    where
        Self: 'tree;
    type ChildIter<'tree>
        = BoxChildren<'tree, T>
    where
        Self: 'tree;

    /// The flat children that can generate a box, except where loose text
    /// sits among them: there the anonymous boxes that wrap it replace it
    /// (`tree::anonymous`). A text node is content of the paragraph above it
    /// and never a box, so it is left out; the paragraph reads its own text
    /// from the flat children.
    fn children(&self, node: NodeSlot) -> Self::ChildIter<'_> {
        let node_ref = self.at(node);
        let children = if node_ref.may_override_children()
            && let Some(children) = self.anonymous().layout_children(node)
        {
            children
        } else {
            node_ref.flat_children()
        };
        BoxChildren {
            tree: self,
            children: children.iter(),
        }
    }

    fn style(&self, node: NodeSlot) -> Self::Style<'_> {
        StyleView::of(self.at(node))
    }

    fn layout<'state>(&self, state: &'state Self::State, node: NodeSlot) -> &'state LayoutSlot {
        state.slot_at(node)
    }

    fn layout_mut<'state>(
        &self,
        state: &'state mut Self::State,
        node: NodeSlot,
    ) -> &'state mut LayoutSlot {
        state.slot_mut(node)
    }

    fn set_sticky_containing_block(
        &self,
        state: &mut Self::State,
        node: NodeSlot,
        bounds: Option<Edges<f32>>,
    ) {
        let table = &mut state.sticky_containing_blocks;
        let at = table.iter().position(|entry| entry.node == node);
        match (at, bounds) {
            (Some(at), Some(unrounded)) => {
                let entry = &mut table[at];
                if entry.unrounded != unrounded {
                    entry.unrounded = unrounded;
                    entry.rounded = None;
                }
            }
            (Some(at), None) => {
                table.swap_remove(at);
            }
            (None, Some(unrounded)) => {
                // Entries of nodes released since drop out here, on the rare
                // push, rather than on every release; a stale entry answers
                // nothing wrong meanwhile, since a `NodeId` is never reissued.
                table.retain(|entry| self.contains(entry.node));
                table.push(StickyContainingBlock {
                    node,
                    unrounded,
                    rounded: None,
                });
            }
            (None, None) => {}
        }
    }

    fn sticky_containing_block(&self, state: &Self::State, node: NodeSlot) -> Option<Edges<f32>> {
        state
            .sticky_containing_blocks
            .iter()
            .find(|entry| entry.node == node)
            .map(|entry| entry.unrounded)
    }

    fn set_rounded_sticky_containing_block(
        &self,
        state: &mut Self::State,
        node: NodeSlot,
        bounds: Edges<f32>,
    ) {
        if let Some(entry) = state
            .sticky_containing_blocks
            .iter_mut()
            .find(|entry| entry.node == node)
        {
            entry.rounded = Some(bounds);
        }
    }

    /// Records the position and lists the box under the containing block
    /// that lays it out ([`hoisted`]).
    fn set_static_position(&self, state: &mut Self::State, node: NodeSlot, position: Point<f32>) {
        let slot = self.layout_mut(state, node);
        if slot.static_position != position {
            slot.static_position = position;
            slot.mark_subtree_dirty();
        }
        state.register_hoisted(node, hoisted::hoisting_block(self.at(node)));
    }

    #[inline]
    fn hoisted_children(
        &self,
        state: &Self::State,
        node: NodeSlot,
    ) -> SmallVec<[HoistedChild<NodeSlot>; 2]> {
        hoisted::children_of(self, state, node)
    }

    /// Only a container that places boxes reads its definiteness: its
    /// algorithm hands it to its children. An empty flex, linear or relative
    /// box lays out as a leaf ([`lays_out_as_empty_leaf`]), and a paragraph
    /// or block flow builds its atoms' inputs from none of it.
    fn reads_definiteness(&self, node: NodeSlot) -> bool {
        if self.at(node).is_replaced() {
            return false;
        }
        match self.style(node).display_mode() {
            DisplayMode::Grid | DisplayMode::GridLanes => true,
            DisplayMode::Flex | DisplayMode::Linear | DisplayMode::Relative => {
                !lays_out_as_empty_leaf(self, node)
            }
            DisplayMode::Text
            | DisplayMode::BlockFlow
            | DisplayMode::Leaf
            | DisplayMode::None
            | DisplayMode::Contents => false,
        }
    }

    /// A listed block is not proof of a box: `hoisted::children_of`
    /// re-derives each listed box's containing block and skips stale ones.
    #[inline]
    fn has_hoisted_children(&self, state: &Self::State, node: NodeSlot) -> bool {
        state.hoisted_to.contains_key(&node)
    }

    fn hoisted_parent_offset(
        &self,
        state: &Self::State,
        containing_block: NodeSlot,
        node: NodeSlot,
    ) -> Point<f32> {
        hoisted::parent_offset(self, state, containing_block, node)
    }

    fn set_hoisted_layout(
        &self,
        state: &mut Self::State,
        containing_block: NodeSlot,
        node: NodeSlot,
        layout: Layout,
    ) {
        hoisted::commit(self, state, containing_block, node, layout);
    }

    fn compute_layout(
        &self,
        state: &mut Self::State,
        node: NodeSlot,
        input: LayoutInput,
    ) -> LayoutOutput {
        let node_ref = self.at(node);
        // Every caller reaches an element: a text node is never a box
        // (`children` leaves it out), and the root, the parked roots, the
        // atoms and the hoisted boxes are elements.
        debug_assert!(node_ref.is_element(), "only an element is laid out");
        let display = {
            let view = self.style(node);
            let display = view.display_mode();
            if display == DisplayMode::None {
                hide_subtree(self, state, node);
                return LayoutOutput::HIDDEN;
            }
            if view.skips_contents() {
                // A skipped box has two halves and only one of them is a
                // function of the layout input. Hiding the contents answers
                // to the box tree — a child inserted under this box changes
                // what must be hidden without changing anything the box's own
                // size reads — so it runs on every committing call, outside
                // the cache. The size reads no child at all, so it is served
                // from the cache like every other algorithm's output: a list
                // of skipped rows re-resolves no box model when a sibling
                // relayouts.
                hide_skipped_contents(self, state, node, input);
                return compute_cached_layout(
                    self,
                    state,
                    node,
                    input,
                    |tree, _state, node, input| {
                        #[cfg(test)]
                        note_skipped_size_resolution();
                        let view = tree.style(node);
                        let output = compute_skipped_contents_size(&view, input);
                        if input.goal.commits() {
                            // Skipping turns `Contain::SIZE` on, so this call
                            // can only ever *remove* a last remembered size —
                            // the half of css-sizing-4 §5.2.1 that fires when
                            // the `auto` keyword goes away while the box goes
                            // on skipping. A skipped box still has a box,
                            // though, so a query container that is also
                            // skipping supplies the substituted size it was
                            // laid out at.
                            committed_box::record(tree, node, &view, input, output.size);
                        }
                        output
                    },
                );
            }
            if node_ref.is_replaced() {
                DisplayMode::Leaf
            } else {
                display
            }
        };

        compute_cached_layout(self, state, node, input, move |tree, state, node, input| {
            // The css-contain-3 §2.1 **interleave**: this run is about to read
            // every child style under a box whose own size those styles may be
            // resolved against, and `committed_box::container_estimate` can
            // say what that size will be from this box's style and this input
            // alone. When it disagrees with what is published — what the
            // subtree was last cascaded against — laying the contents out here
            // would lay them out at a size that is about to be restyled away.
            // So the run records the new size and stops: `run_layout` publishes
            // it, restyles the `cqw`/`cqh` readers under this box, and relays
            // the subtree once, at the size it will keep.
            //
            // The box's own output is not deferred with it. Both of its axes
            // are size-contained, so this *is* the size the algorithm would
            // have produced, and its ancestors are laid out against the final
            // number rather than against a guess.
            if input.goal.commits()
                && state.interleaves_containers()
                && !establishes_paragraph(display)
            {
                let view = tree.style(node);
                let id = tree.at(node).id();
                if let Some(estimate) = committed_box::container_estimate(&view, input)
                    && estimate.container != tree.committed_box(id).container
                {
                    committed_box::note_container_estimate(tree, id, estimate.container);
                    state.defer_container(node, input);
                    return estimate.output;
                }
            }
            let output = match display {
                DisplayMode::None | DisplayMode::Contents => {
                    unreachable!("a box-less element has no box to lay out")
                }
                DisplayMode::Flex | DisplayMode::Linear | DisplayMode::Relative
                    if lays_out_as_empty_leaf(tree, node) =>
                {
                    compute_leaf_layout(input, &tree.style(node), NaturalSize::NONE)
                }
                DisplayMode::Flex => compute_flexbox_layout(tree, state, node, input),
                DisplayMode::Text => compute_text_block_layout(tree, state, node, input),
                DisplayMode::BlockFlow => compute_inline_flow_layout(tree, state, node, input),
                DisplayMode::Grid => compute_grid_layout(tree, state, node, input),
                DisplayMode::GridLanes => compute_grid_lanes_layout(tree, state, node, input),
                DisplayMode::Linear => compute_linear_layout(tree, state, node, input),
                DisplayMode::Relative => compute_relative_layout(tree, state, node, input),
                DisplayMode::Leaf => {
                    let node_ref = tree.at(node);
                    let view = tree.style(node);
                    #[cfg(feature = "layout-test-utils")]
                    let output = if let Some(metrics) = node_ref.test_leaf_metrics() {
                        compute_leaf_layout_with_measurement_for_testing(
                            input,
                            &view,
                            None,
                            |_measure_input| metrics,
                        )
                    } else {
                        compute_leaf_layout(input, &view, node_ref.natural_size())
                    };
                    #[cfg(not(feature = "layout-test-utils"))]
                    let output = compute_leaf_layout(input, &view, node_ref.natural_size());
                    if input.goal.commits() {
                        for grandchild in tree.children(node) {
                            hide_subtree(tree, state, grandchild);
                        }
                    }
                    output
                }
            };
            // The recording moment (css-sizing-4 §5.2.1 and css-contain-3
            // §2.1, which is the same one): this run laid the box out with
            // its real contents, so its content box is both what the element
            // last rendered at and what a descendant's `cqw` and `cqh`
            // resolve against until the next commit moves it. Only a
            // *committing* run establishes one, and only a cache miss reaches
            // here — a box served from the cache produced the same size it
            // already recorded under the same input.
            if input.goal.commits() && tree.at(node).is_element() {
                let view = tree.style(node);
                committed_box::record(tree, node, &view, input, output.size);
            }
            output
        })
    }

    fn clear_layout_cache(&self, state: &mut Self::State, node: NodeSlot) {
        state.clear_layout_cache(node);
    }

    /// css-anchor-position-1 §2.3's target anchor element for `node`, and
    /// its border box in the padding-box coordinates of the element
    /// generating `node`'s containing block, translated by `node`'s
    /// remembered scroll offsets (§3.3). The lookup and the geometry are
    /// [`anchors`]'s; this records the read for the settle loop.
    ///
    /// A box whose containing block is not its box parent (a `fixed` box, or
    /// an `absolute` one under a static parent) is laid out by that
    /// containing block's absolute pass in tree order with its own
    /// out-of-flow children (`layout::hoisted`), so every acceptable anchor
    /// is placed before the box reads it. An anchor that moves under a
    /// relayout that never reaches the reader is caught by
    /// [`Document::settle_anchors`] after the run.
    fn anchor_rect(
        &self,
        state: &Self::State,
        node: NodeSlot,
        option: usize,
        spec: AnchorSpec<'_>,
    ) -> Option<Rect<f32>> {
        let query = match spec {
            AnchorSpec::Default => anchors::AnchorQuery::Default,
            AnchorSpec::Named(name) => anchors::AnchorQuery::Named(name.clone()),
        };
        self.anchor_query(state, node, option, query).1
    }

    fn anchor_scrolls_with_default(
        &self,
        state: &Self::State,
        node: NodeSlot,
        option: usize,
        name: &TreeScoped<DashedIdent>,
        axis: PhysicalAxis,
    ) -> bool {
        let Some(anchored) = anchors::Query::of(self.at(node)) else {
            return false;
        };
        let liveness = anchors::Liveness::Committed(state);
        let Some(style) = anchors::option_style(self, self.at(node), option) else {
            return false;
        };
        let (Some(named), Some(default)) = (
            anchors::target_anchor(self, &anchored, name, liveness),
            anchors::default_anchor(self, &anchored, style, liveness),
        ) else {
            return false;
        };
        anchors::scrolls_with_default(named, default, axis)
    }

    fn position_option_count(&self, node: NodeSlot) -> usize {
        self.anchors()
            .options(node)
            .map_or(0, |options| options.styles.len() + 1)
    }

    fn position_option_style(&self, node: NodeSlot, index: usize) -> Self::Style<'_> {
        let node_ref = self.at(node);
        match anchors::option_style(self, node_ref, index) {
            Some(style) if index > 0 => StyleView::with_values(node_ref, style),
            _ => StyleView::of(node_ref),
        }
    }

    fn last_successful_option(&self, state: &Self::State, node: NodeSlot) -> Option<usize> {
        state.anchored.get(&node)?.last_successful
    }

    /// Keeps `outcome` and seals the reads the box's layout made since its
    /// last report as the ones the settle loop verifies.
    fn set_anchor_outcome(&self, state: &mut Self::State, node: NodeSlot, outcome: AnchorOutcome) {
        let mut reads = SmallVec::new();
        for (_, read) in state
            .anchor_pending
            .get_mut()
            .extract_if(.., |(reader, _)| *reader == node)
        {
            if !reads.contains(&read) {
                reads.push(read);
            }
        }
        match state.anchored.entry(node) {
            Entry::Occupied(mut entry) => {
                let entry = entry.get_mut();
                entry.outcome = outcome;
                entry.reads = reads;
                entry.reported = true;
            }
            Entry::Vacant(entry) => {
                entry.insert(anchors::AnchoredBox {
                    outcome,
                    reads,
                    remembered: None,
                    last_successful: None,
                    fallback_sensitive: false,
                    redetermine: false,
                    fits_scrolled: false,
                    reported: true,
                });
            }
        }
    }
}

/// [`LayoutTree::children`] for the document: the children that can generate
/// a box, text nodes left out.
pub(crate) struct BoxChildren<'tree, T> {
    tree: &'tree TreeArenas<T>,
    children: core::slice::Iter<'tree, NodeSlot>,
}

impl<T> Iterator for BoxChildren<'_, T> {
    type Item = NodeSlot;

    #[inline]
    fn next(&mut self) -> Option<NodeSlot> {
        self.children
            .by_ref()
            .copied()
            .find(|&child| !self.tree.at(child).is_text_node())
    }

    /// At most the children left; how many of them are text is unknown.
    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        (0, Some(self.children.len()))
    }
}

impl<T> TreeArenas<T> {
    /// Answers one anchor query — the target and its rectangle — and records
    /// it as a read of `node`'s layout.
    fn anchor_query(
        &self,
        state: &DocumentLayoutState,
        node: NodeSlot,
        option: usize,
        query: anchors::AnchorQuery,
    ) -> (Option<NodeSlot>, Option<Rect<f32>>) {
        let node_ref = self.at(node);
        // Only `AnchorMemo` asks, from the absolute pass seldon dispatches
        // `absolute` and `fixed` boxes to (`resolve_position` maps those
        // onto each other).
        debug_assert!(
            node_ref
                .layout_computed_style()
                .is_some_and(|style| matches!(
                    *style.get_box().get_position(),
                    PositionProperty::Absolute | PositionProperty::Fixed
                )),
            "only an absolutely positioned box asks anchor queries"
        );
        let Some(anchored) = anchors::Query::of(node_ref) else {
            return (None, None);
        };
        let (target, rect) = anchors::answer(self, state, &anchored, option, &query);
        #[cfg(debug_assertions)]
        if let Some(target) = target {
            // Acceptability is what guarantees the anchor was laid out
            // before the box reading it; a descendant never is.
            let mut current = self.at(target).flat_parent();
            while let Some(ancestor) = current {
                assert_ne!(
                    ancestor.id(),
                    node,
                    "an acceptable anchor is never inside the box that reads it"
                );
                current = ancestor.flat_parent();
            }
        }
        let generation = anchors::lookup_generation(self, node_ref, option, &query);
        state.anchor_pending.borrow_mut().push((
            node,
            anchors::AnchorRead {
                option,
                query,
                target,
                rect,
                generation,
            },
        ));
        (target, rect)
    }
}

/// Whether a flex, linear or relative `node` lays out as a leaf with no
/// contents, which is what its algorithm computes for it: it has no child
/// that may own a box, `display: none` ones included, and no preferred
/// aspect ratio, which those algorithms do not apply to an automatic size
/// and the leaf would.
#[inline]
fn lays_out_as_empty_leaf<T>(tree: &TreeArenas<T>, node: NodeSlot) -> bool {
    tree.children(node).next().is_none()
        && matches!(tree.style(node).aspect_ratio().ratio, PreferredRatio::None)
}

#[cfg(test)]
std::thread_local! {
    /// How many times a box that skips its contents has resolved its own box
    /// model on this thread — that is, how often the skipped path *missed*
    /// its cache.
    ///
    /// Test-only, because the number is the point of the cache rather than a
    /// runtime fact anything reads: it must stay flat as skipped boxes are
    /// added to a page, where before the cache it was one resolution per
    /// skipped box per pass. Layout runs on the thread that calls it, so a
    /// thread-local count belongs to the test that produced it.
    static SKIPPED_SIZE_RESOLUTIONS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
fn note_skipped_size_resolution() {
    SKIPPED_SIZE_RESOLUTIONS.with(|count| count.set(count.get() + 1));
}

/// Runs `pass` and answers how many skipped boxes resolved their own size in
/// it.
#[cfg(test)]
pub(super) fn skipped_size_resolutions_during(pass: impl FnOnce()) -> usize {
    SKIPPED_SIZE_RESOLUTIONS.with(|count| count.set(0));
    pass();
    SKIPPED_SIZE_RESOLUTIONS.with(std::cell::Cell::get)
}

#[cfg(test)]
std::thread_local! {
    /// How many box layouts have run on this thread — one per [`run_layout`],
    /// whatever it settled inside itself.
    ///
    /// Test-only, and the number the query-container interleave exists to hold
    /// at one: a page whose containers resize used to cost a whole second run
    /// of the document, and now costs the deferred subtrees alone.
    static LAYOUT_RUNS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Runs `pass` and answers how many times the document was laid out in it.
#[cfg(test)]
pub(super) fn layout_runs_during(pass: impl FnOnce()) -> usize {
    LAYOUT_RUNS.with(|count| count.set(0));
    pass();
    LAYOUT_RUNS.with(std::cell::Cell::get)
}

/// One layout run: the boxes, the query-container interleave the boxes owe,
/// and the rounding tail.
///
/// Three phases, and the middle one is why this takes the whole document
/// rather than the split parts. Laying out is a shared read of the tree, but
/// a size query container whose size moved has to be *restyled under* before
/// its contents are worth laying out, and a restyle publishes computed styles
/// — `&mut Document` work. So the box phase stops at such a container
/// ([`DocumentLayoutState::defer_container`]), the restyle phase runs between
/// the two shared borrows, and the relay phase finishes the deferred subtrees
/// before anything is rounded.
pub(super) fn run_layout<T: Sync>(
    document: &mut Document<T>,
    viewport: Size<f32>,
    scale: f32,
    full: bool,
    rescale: bool,
    resized: &mut Vec<NodeId>,
) {
    #[cfg(test)]
    LAYOUT_RUNS.with(|count| count.set(count.get() + 1));
    let root_id = document.document_element().id();
    let mut parked = collect_parked_boundaries(document);
    // The interleave's gate is the recascade loop's: a page whose styles never
    // resolved a `cqw`/`cqh` has nothing to restyle when a container's size
    // moves, so deferring its contents would buy a page with query containers
    // and no container units nothing at all.
    let interleave = document.arenas().uses_container_units();
    let mut deferred = Vec::new();
    // The in-place roots that fell back to the whole-tree pass.
    let mut escalated: SmallVec<[NodeId; 2]> = SmallVec::new();
    {
        let (tree, state) = document.layout_parts();
        let root = tree.live_slot(root_id);
        state.begin_container_interleave(interleave);
        // Every parked root is relaid in place before the root pass, in a
        // whole-tree run too: the root pass answers a box whose cache stayed
        // warm from that cache and never descends, so a parked root under
        // such a box is reached by nobody else, and a dirty box above it
        // takes the relay's result from the cache the relay just filled.
        for &(_, pending) in &parked {
            let Some(slot) = tree.slot(pending.node_id) else {
                continue;
            };
            if !tree.at(slot).is_element() {
                continue;
            }
            match pending.kind {
                RelayoutKind::Boundary => {
                    if is_relayout_boundary(&StyleView::of(tree.at(slot))) {
                        let output = compute_boundary_relayout(tree, state, slot, pending.input);
                        tree.layout_mut(state, slot)
                            .set_unrounded_content_size(output.content_size);
                    }
                }
                RelayoutKind::InPlace { previous } => {
                    let output = tree.compute_layout(state, slot, pending.input);
                    // A reproduced output proves nothing above this node
                    // can observe the change, as long as every box hoisted
                    // out of it can be placed late by the rounding tail;
                    // anything else falls back to the whole-tree pass,
                    // which reuses the caches just filled. The containing
                    // blocks above the node then run their absolute passes,
                    // so the tail places nothing late under it.
                    if output != previous || hoisted::escapes_to_scrollable_block(tree, state, slot)
                    {
                        escalated.push(pending.node_id);
                        let mut current = tree.at(slot).flat_parent_slot();
                        while let Some(ancestor) = current {
                            // Geometry only: a paragraph an ancestor retains
                            // answers for itself whether what it was shaped
                            // from moved, as under `Document::invalidate_layout`.
                            state.clear_box_cache(ancestor);
                            current = tree.at(ancestor).flat_parent_slot();
                        }
                    }
                }
            }
        }
        compute_root_layout(
            tree,
            state,
            root,
            Size::new(
                AvailableSpace::Definite(viewport.width),
                AvailableSpace::Definite(viewport.height),
            ),
        );
        state.take_container_deferrals(&mut deferred);
    }
    let full = full || !escalated.is_empty();
    let settled = settle_deferred_containers(document, resized, &mut deferred);
    // A deferred container is a relayout boundary, so its own box did not move
    // and nothing above it has to be laid out again — but its subtree was
    // finished after the boundaries the pass had parked, so the incremental
    // rounding tail has to be told about it.
    if !settled.is_empty() {
        let mut ids: FxHashSet<NodeId> = parked.iter().map(|&(_, p)| p.node_id).collect();
        for &pending in &settled {
            if ids.insert(pending.node_id) {
                parked.push((boundary_depth(document, pending.node_id), pending));
            }
        }
    }
    // The parked subtrees this run relaid in place, before the root pass, and
    // did not escalate: a box under one of them that escapes to a containing
    // block above it was laid out by nobody, and the tail places it
    // (`hoisted::places_late`). Under an escalated root every containing
    // block above it ran its own absolute pass.
    let relayed: FxHashSet<NodeId> = parked
        .iter()
        .map(|&(_, pending)| pending.node_id)
        .filter(|node| !escalated.contains(node))
        .collect();
    {
        let (tree, state) = document.layout_parts();
        let root = tree.live_slot(root_id);
        state.begin_container_interleave(false);
        state.in_rounding_tail = true;
        if full {
            let position = |tree: &TreeArenas<T>, state: &mut DocumentLayoutState, node| {
                pre_position(tree, state, node, viewport, &relayed)
            };
            round_with(tree, state, root, scale, Point::ZERO, rescale, position);
        }
        // After a whole-tree walk too: that walk descends through marked
        // nodes alone, and the boxes between a parked root and the nearest
        // box above it the pass wrote carry no mark, so it may stop short of
        // the root. A root it did reach answers at once.
        position_and_round_parked_boundaries(tree, state, &parked, viewport, scale, &relayed);
        state.in_rounding_tail = false;
        // Every text node this pass measured but did not commit still holds
        // the probe's line break; painting reads the committed one.
        state.restore_probed_text();
        if !tree.top_layer().is_empty() {
            place_backdrops(tree, state, viewport, scale, rescale);
        }
    }
}

/// The top-layer tail: lays every rendered entry's `::backdrop` box out
/// against the viewport and rounds it (`tree::top_layer`).
///
/// A backdrop node has no parent, so neither the root pass nor the rounding
/// tail reaches it; it is placed here, after both, with the same absolute
/// layout the tail gives any box whose containing block is the initial one
/// and a static position of zero. Its element needs nothing here: the
/// rounding tail already placed it against the viewport, as a box whose
/// containing block is the initial one. Every run lays the backdrops out,
/// which the layout cache answers unless the viewport or the backdrop's
/// style moved.
fn place_backdrops<T: Sync>(
    tree: &TreeArenas<T>,
    state: &mut DocumentLayoutState,
    viewport: Size<f32>,
    scale: f32,
    rescale: bool,
) {
    for entry in tree.top_layer().entries() {
        if !crate::tree::top_layer::is_rendered(tree, state, entry.element())
            || !crate::tree::top_layer::backdrop_generates_box(tree, entry.backdrop)
        {
            continue;
        }
        let layout = compute_absolute_layout(tree, state, entry.backdrop, viewport, Point::ZERO);
        tree.layout_mut(state, entry.backdrop).set_unrounded(layout);
        round_with(
            tree,
            state,
            entry.backdrop,
            scale,
            Point::ZERO,
            rescale,
            |_, _, _| true,
        );
    }
}

/// Publishes what the deferred containers measured, restyles the container-unit
/// readers under them, and lays their subtrees out — until nothing is left
/// deferred.
///
/// The loop is the nesting depth of size query containers that all moved at
/// once, not the number of containers: one iteration settles every container
/// at one level, and the only thing a relay can discover is a container *under*
/// one just settled. It is bounded the way
/// [`Document::layout`](crate::tree::document::Document::layout)'s is, and its last iteration
/// closes the interleave rather than capping it — a relay that may not defer
/// lays every subtree out for real, so this never returns with a subtree
/// nobody laid out.
///
/// Returns every container it settled, for the rounding tail.
fn settle_deferred_containers<T: Sync>(
    document: &mut Document<T>,
    resized: &mut Vec<NodeId>,
    deferred: &mut Vec<DeferredContainer>,
) -> Vec<PendingRelayout> {
    let mut settled = Vec::new();
    for pass in 0..committed_box::CONTAINER_PASSES {
        if deferred.is_empty() {
            break;
        }
        // What the deferred runs recorded becomes readable here — by the style
        // traversal below, which is what the whole deferral was for.
        document.arenas_mut().publish_committed_boxes(resized);
        let mut marked = false;
        for &DeferredContainer { node, input } in deferred.iter() {
            settled.push(PendingRelayout {
                node_id: node,
                input,
                kind: RelayoutKind::Boundary,
            });
            if document.get(node).is_some_and(Node::is_element) {
                marked |= document.mark_container_units_users(node);
            }
        }
        // Every container this call is settling has had its readers marked, so
        // its report is spent; one it is *not* settling — an
        // `inline-size` container, whose block axis answers to its contents and
        // so has no size to publish before them — stays in the list for
        // `Document::layout`'s loop to recascade after the pass.
        let deferred_ids: FxHashSet<NodeId> = deferred.iter().map(|entry| entry.node).collect();
        resized.retain(|id| !deferred_ids.contains(id));
        if marked {
            document.flush_styles_with_damage_sink(&mut |_, _| {});
        }
        let last = pass + 1 == committed_box::CONTAINER_PASSES;
        let (tree, state) = document.layout_parts();
        state.begin_container_interleave(!last);
        for &DeferredContainer { node, input } in deferred.iter() {
            let Some(slot) = tree.slot(node) else {
                continue;
            };
            if !tree.at(slot).is_element() || !is_relayout_boundary(&StyleView::of(tree.at(slot))) {
                continue;
            }
            // The deferred run cached the size it published; the relay is the
            // run that lays the contents out under it. A deferred container
            // establishes no paragraph, so there is no shaped text to drop.
            state.clear_box_cache(slot);
            let output = compute_boundary_relayout(tree, state, slot, input);
            tree.layout_mut(state, slot)
                .set_unrounded_content_size(output.content_size);
        }
        state.take_container_deferrals(deferred);
    }
    debug_assert!(
        deferred.is_empty(),
        "the last relay may not defer, so every deferred subtree has been laid out"
    );
    settled
}

fn collect_parked_boundaries<T>(document: &Document<T>) -> Vec<(usize, PendingRelayout)> {
    let roots = document.relayout_roots();
    if roots.is_empty() {
        return Vec::new();
    }
    let mut parked: Vec<(usize, PendingRelayout)> = roots
        .iter()
        .map(|&pending| (boundary_depth(document, pending.node_id), pending))
        .collect();
    if parked.len() > 1 {
        parked.sort_by_key(|&(depth, ..)| std::cmp::Reverse(depth));
    }
    parked
}

/// The parked roots' rounding tail: every parked root's subtree is
/// positioned and rounded from its parent's accumulated unrounded origin,
/// which is what the whole-tree walk would reach it with, since every
/// unrounded box is final before the tail runs. It runs after every pass,
/// the whole-tree walk included.
///
/// A parked root is walked on its own because no other walk is sure to
/// reach it. A walk descends only through nodes a layout write marked, and
/// the boxes between a parked root and the nearest box above it that the
/// pass wrote — a `display: contents` wrapper, or any box no relayout wrote
/// — are marked by nobody: the root's invalidation stopped at the root, and
/// the walk above it started from another invalidation. Left to that walk,
/// the subtree would keep its old rounded boxes, and a box the tail alone
/// places (`pre_position`: one whose containing block is the initial one, a
/// top-layer element among them) would never be placed. A root another walk
/// did reach carries no mark any more and answers at once; where a box
/// above it did change, that walk forced its whole subtree and rounded the
/// root's a second time, to the same values: rounding is a function of the
/// unrounded boxes alone.
fn position_and_round_parked_boundaries<T: Sync>(
    tree: &TreeArenas<T>,
    state: &mut DocumentLayoutState,
    parked: &[(usize, PendingRelayout)],
    viewport: Size<f32>,
    scale: f32,
    relayed: &FxHashSet<NodeId>,
) {
    for &(_, pending) in parked {
        let Some(slot) = tree.slot(pending.node_id) else {
            continue;
        };
        let node = tree.at(slot);
        if !node.is_element() {
            continue;
        }
        if matches!(pending.kind, RelayoutKind::Boundary)
            && !is_relayout_boundary(&StyleView::of(node))
        {
            continue;
        }
        let parent_origin = node.flat_parent_slot().map_or(Point::ZERO, |parent| {
            accumulated_unrounded_origin(tree, state, parent)
        });
        let position = |tree: &TreeArenas<T>, state: &mut DocumentLayoutState, node| {
            pre_position(tree, state, node, viewport, relayed)
        };
        round_with(tree, state, slot, scale, parent_origin, false, position);
    }
}

fn accumulated_unrounded_origin<T>(
    tree: &TreeArenas<T>,
    state: &DocumentLayoutState,
    node: NodeSlot,
) -> Point<f32> {
    let mut origin = Point::ZERO;
    let mut current = Some(node);
    while let Some(slot) = current {
        let location = tree.layout(state, slot).unrounded.location;
        origin = Point::new(origin.x + location.x, origin.y + location.y);
        current = tree.at(slot).flat_parent_slot();
    }
    origin
}

fn boundary_depth<T>(document: &Document<T>, id: NodeId) -> usize {
    let mut depth = 0;
    let mut current = document.get(id).and_then(Node::flat_parent_id);
    while let Some(id) = current {
        depth += 1;
        current = document.get(id).and_then(Node::flat_parent_id);
    }
    depth
}

/// The rounding tail's pre-node hook: zeroes a `display: contents` box, and
/// places the out-of-flow boxes no containing block's absolute pass laid out
/// this run — the ones whose containing block is the initial containing
/// block, which lays its out-of-flow boxes out after everything else, and
/// the ones a subtree relaid in place hides from their containing block
/// (`hoisted::places_late`).
fn pre_position<T: Sync>(
    tree: &TreeArenas<T>,
    state: &mut DocumentLayoutState,
    node_id: NodeSlot,
    viewport: Size<f32>,
    relayed: &FxHashSet<NodeId>,
) -> bool {
    let node = tree.at(node_id);
    let Some(style) = StyleView::try_of(node) else {
        return false;
    };
    let display = style.display_mode();
    if display == DisplayMode::None {
        return false;
    }
    if display == DisplayMode::Contents {
        tree.layout_mut(state, node_id)
            .set_unrounded(Layout::default());
        return !replaces_children(node);
    }
    if node
        .flat_parent_id()
        .and_then(|id| tree.get(id))
        .is_some_and(Node::is_element)
        && resolve_position(node, style.values()) == PositionProperty::Fixed
    {
        match anchors::containing_block_generator(node) {
            None => position_against_viewport(tree, state, node_id, viewport),
            Some(block) => {
                if hoisted::places_late(tree, node_id, block.id(), relayed) {
                    compute_hoisted_layout(tree, state, block.id(), node_id);
                }
            }
        }
    }
    display != DisplayMode::Leaf
        && !style.skips_contents()
        && !(establishes_paragraph(display) && replaces_children(node))
}

/// Places an out-of-flow box whose containing block is the initial
/// containing block — the viewport here — from the static position its
/// parent recorded.
fn position_against_viewport<T: Sync>(
    tree: &TreeArenas<T>,
    state: &mut DocumentLayoutState,
    node_id: NodeSlot,
    viewport: Size<f32>,
) {
    let node = tree.at(node_id);
    let Some(parent_slot) = node.flat_parent_slot() else {
        return;
    };
    let parent_origin = accumulated_unrounded_origin(tree, state, parent_slot);
    // css-position-4 §3.1: a top-layer element's static position is zero
    // for `left`, `right` and `top`, not the place its parent recorded.
    let static_in_cb = if tree.top_layer().places_against_viewport(node_id) {
        Point::ZERO
    } else {
        let static_position = tree.layout(state, node_id).static_position;
        Point::new(
            parent_origin.x + static_position.x,
            parent_origin.y + static_position.y,
        )
    };

    let mut layout = compute_absolute_layout(tree, state, node_id, viewport, static_in_cb);

    layout.location = Point::new(
        layout.location.x - parent_origin.x,
        layout.location.y - parent_origin.y,
    );
    let ordering_parent = box_parent(node).map_or(parent_slot, Node::slot);
    layout.order = sibling_paint_order(tree, ordering_parent, node_id);
    tree.layout_mut(state, node_id).set_unrounded(layout);
}

pub(super) fn sibling_paint_order<T>(
    tree: &TreeArenas<T>,
    parent_id: NodeSlot,
    target: NodeSlot,
) -> u32 {
    let Some(target_index) = tree
        .flattened_children(parent_id)
        .position(|(id, ..)| id == target)
    else {
        return 0;
    };
    let target_key = (0_i32, target_index);
    let mut rank = 0u32;
    for (index, (child_id, ..)) in tree.flattened_children(parent_id).enumerate() {
        let child = tree.at(child_id);
        let Some(order) = sibling_effective_paint_order(child) else {
            continue;
        };
        if index == target_index {
            debug_assert_eq!(
                order, 0,
                "sibling_paint_order is only called for out-of-flow (hoisted) \
                 targets, whose effective paint order is 0"
            );
            continue;
        }
        if (order, index) < target_key {
            rank += 1;
        }
    }
    rank
}

fn sibling_effective_paint_order<T>(child: &Node<T>) -> Option<i32> {
    match StyleView::try_of(child) {
        Some(style) => {
            if style.display_mode() == DisplayMode::None {
                None
            } else if matches!(
                *style.values().get_box().get_position(),
                PositionProperty::Absolute | PositionProperty::Fixed
            ) {
                Some(0)
            } else {
                Some(style.values().get_position().order)
            }
        }
        None => Some(0),
    }
}
