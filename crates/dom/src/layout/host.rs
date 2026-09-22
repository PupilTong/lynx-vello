//! The statically split [`LayoutTree`] host over the document's immutable
//! tree/style arenas and mutable layout/text state.
//!
//! The pass runs in **slot space**: [`LayoutTree::NodeId`] binds to
//! [`NodeSlot`], which is what the tree's own links already hold, so
//! `children`/`style`/`layout` index an arena directly instead of resolving
//! an id first. Ids are resolved once where the pass enters — at the root and
//! at each parked boundary — because those happen once per pass while the
//! trait calls happen many times per node.

#[cfg(feature = "layout-test-utils")]
use hughie::compute::compute_leaf_layout_with_measurement_for_testing;
use hughie::compute::{
    compute_absolute_layout, compute_boundary_relayout, compute_cached_layout,
    compute_flexbox_layout, compute_grid_lanes_layout, compute_grid_layout, compute_leaf_layout,
    compute_linear_layout, compute_relative_layout, compute_root_layout,
    compute_skipped_contents_size, hide_skipped_contents, hide_subtree,
    round_layout_subtree_with as round_with,
};
use hughie::geometry::{Edges, Point, Size};
use hughie::invalidate::is_relayout_boundary;
use hughie::style::{CoreStyle, PositionProperty};
use hughie::tree::{AvailableSpace, Layout, LayoutInput, LayoutOutput, LayoutSlot, LayoutTree};
use rustc_hash::FxHashSet;

use super::committed_box;
use super::style::{
    DisplayMode, StyleView, box_parent, display_mode, establishes_absolute_containing_block,
    establishes_fixed_containing_block, resolve_position,
};
use super::text_block::compute_text_block_layout;
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
        = core::iter::Copied<core::slice::Iter<'tree, NodeSlot>>
    where
        Self: 'tree;

    fn children(&self, node: NodeSlot) -> Self::ChildIter<'_> {
        self.at(node).flat_children().iter().copied()
    }

    fn style(&self, node: NodeSlot) -> Self::Style<'_> {
        StyleView::of(self.at(node))
    }

    fn layout<'state>(&self, state: &'state Self::State, node: NodeSlot) -> &'state LayoutSlot {
        &state.at(node).slot
    }

    fn layout_mut<'state>(
        &self,
        state: &'state mut Self::State,
        node: NodeSlot,
    ) -> &'state mut LayoutSlot {
        &mut state.at_mut(node).slot
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

    fn compute_layout(
        &self,
        state: &mut Self::State,
        node: NodeSlot,
        input: LayoutInput,
    ) -> LayoutOutput {
        let node_ref = self.at(node);
        let display = if node_ref.is_text_node() {
            DisplayMode::Leaf
        } else {
            let view = self.style(node);
            let display = display_mode(view.display());
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
                && display != DisplayMode::Text
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
                DisplayMode::Flex => compute_flexbox_layout(tree, state, node, input),
                DisplayMode::Text => compute_text_block_layout(tree, state, node, input),
                DisplayMode::Grid => compute_grid_layout(tree, state, node, input),
                DisplayMode::GridLanes => compute_grid_lanes_layout(tree, state, node, input),
                DisplayMode::Linear => compute_linear_layout(tree, state, node, input),
                DisplayMode::Relative => compute_relative_layout(tree, state, node, input),
                DisplayMode::Leaf => {
                    let node_ref = tree.at(node);
                    // A text node is content of the block above it, never a
                    // box of its own. One outside a text block lays out
                    // nothing at all: in Lynx text exists only inside a
                    // `display: -lynx-text` container.
                    let output = if node_ref.is_text_node() {
                        LayoutOutput::HIDDEN
                    } else {
                        let view = tree.style(node);
                        #[cfg(feature = "layout-test-utils")]
                        if let Some(metrics) = node_ref.test_leaf_metrics() {
                            compute_leaf_layout_with_measurement_for_testing(
                                input,
                                &view,
                                None,
                                |_measure_input| metrics,
                            )
                        } else {
                            compute_leaf_layout(input, &view, node_ref.natural_size())
                        }
                        #[cfg(not(feature = "layout-test-utils"))]
                        compute_leaf_layout(input, &view, node_ref.natural_size())
                    };
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
    let mut escalated = false;
    {
        let (tree, state, _) = document.layout_parts();
        let root = tree.live_slot(root_id);
        state.begin_container_interleave(interleave);
        if !full {
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
                            let output =
                                compute_boundary_relayout(tree, state, slot, pending.input);
                            tree.layout_mut(state, slot)
                                .set_unrounded_content_size(output.content_size);
                        }
                    }
                    RelayoutKind::InPlace { previous } => {
                        let output = tree.compute_layout(state, slot, pending.input);
                        // A reproduced output proves nothing above this node
                        // can observe the change; anything else falls back to
                        // the whole-tree pass, which reuses the caches just
                        // filled.
                        if output != previous {
                            escalated = true;
                            let mut current = tree.at(slot).flat_parent_slot();
                            while let Some(ancestor) = current {
                                state.clear_layout_cache(ancestor);
                                current = tree.at(ancestor).flat_parent_slot();
                            }
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
    let full = full || escalated;
    let settled = settle_deferred_containers(document, resized, &mut deferred);
    // A deferred container is a relayout boundary, so its own box did not move
    // and nothing above it has to be laid out again — but its subtree was
    // finished after the boundaries the pass had parked, so the incremental
    // rounding tail has to be told about it.
    let parked_ids = if settled.is_empty() {
        None
    } else {
        let mut ids: FxHashSet<NodeId> = parked.iter().map(|&(_, p)| p.node_id).collect();
        for &pending in &settled {
            if ids.insert(pending.node_id) {
                parked.push((boundary_depth(document, pending.node_id), pending));
            }
        }
        Some(ids)
    };
    {
        let (tree, state, live_parked_ids) = document.layout_parts();
        let root = tree.live_slot(root_id);
        state.begin_container_interleave(false);
        if full {
            let position = |tree: &TreeArenas<T>, state: &mut DocumentLayoutState, node| {
                pre_position(tree, state, node, viewport)
            };
            round_with(tree, state, root, scale, Point::ZERO, rescale, position);
        } else {
            position_and_round_parked_boundaries(
                tree,
                state,
                parked_ids.as_ref().unwrap_or(live_parked_ids),
                &parked,
                viewport,
                scale,
            );
        }
        // Every text node this pass measured but did not commit still holds
        // the probe's line break; painting reads the committed one.
        state.restore_probed_text();
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
/// [`Document::layout`](crate::Document::layout)'s is, and its last iteration
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
        let (tree, state, _) = document.layout_parts();
        state.begin_container_interleave(!last);
        for &DeferredContainer { node, input } in deferred.iter() {
            let Some(slot) = tree.slot(node) else {
                continue;
            };
            if !tree.at(slot).is_element() || !is_relayout_boundary(&StyleView::of(tree.at(slot))) {
                continue;
            }
            // The deferred run cached the size it published; the relay is the
            // run that lays the contents out under it.
            state.clear_layout_cache(slot);
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

fn position_and_round_parked_boundaries<T: Sync>(
    tree: &TreeArenas<T>,
    state: &mut DocumentLayoutState,
    parked_ids: &FxHashSet<NodeId>,
    parked: &[(usize, PendingRelayout)],
    viewport: Size<f32>,
    scale: f32,
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
        if has_parked_ancestor(tree, node, parked_ids) {
            continue;
        }
        let parent_origin = node.flat_parent_slot().map_or(Point::ZERO, |parent| {
            accumulated_unrounded_origin(tree, state, parent)
        });
        let position = |tree: &TreeArenas<T>, state: &mut DocumentLayoutState, node| {
            pre_position(tree, state, node, viewport)
        };
        round_with(tree, state, slot, scale, parent_origin, false, position);
    }
}

fn has_parked_ancestor<T>(
    tree: &TreeArenas<T>,
    node: &Node<T>,
    parked_ids: &FxHashSet<NodeId>,
) -> bool {
    let mut current = node.flat_parent_slot();
    while let Some(slot) = current {
        let ancestor = tree.at(slot);
        if parked_ids.contains(&ancestor.id()) {
            return true;
        }
        current = ancestor.flat_parent_slot();
    }
    false
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

fn pre_position<T: Sync>(
    tree: &TreeArenas<T>,
    state: &mut DocumentLayoutState,
    node_id: NodeSlot,
    viewport: Size<f32>,
) -> bool {
    let node = tree.at(node_id);
    let Some(style) = StyleView::try_of(node) else {
        return false;
    };
    let display = display_mode(style.display());
    if display == DisplayMode::None {
        return false;
    }
    if display == DisplayMode::Contents {
        tree.layout_mut(state, node_id)
            .set_unrounded(Layout::default());
        return !super::text_block::replaces_children(node);
    }
    if node
        .flat_parent_id()
        .and_then(|id| tree.get(id))
        .is_some_and(Node::is_element)
        && resolve_position(node, style.values()) == PositionProperty::Fixed
    {
        let fixed = style.values().clone_position() == PositionProperty::Fixed;
        position_hoisted(tree, state, node_id, viewport, fixed);
    }
    display != DisplayMode::Leaf
        && !style.skips_contents()
        && !(display == DisplayMode::Text && super::text_block::replaces_children(node))
}

fn position_hoisted<T: Sync>(
    tree: &TreeArenas<T>,
    state: &mut DocumentLayoutState,
    node_id: NodeSlot,
    viewport: Size<f32>,
    fixed: bool,
) {
    let node = tree.at(node_id);
    let Some(parent_slot) = node.flat_parent_slot() else {
        return;
    };

    let mut containing = None;
    let mut ancestor = Some(parent_slot);
    while let Some(current_id) = ancestor {
        let current = tree.at(current_id);
        let Some(style) = StyleView::try_of(current) else {
            break;
        };
        let establishes = if fixed {
            establishes_fixed_containing_block(current, style.values())
        } else {
            establishes_absolute_containing_block(current, style.values())
        };
        if establishes {
            containing = Some(current_id);
            break;
        }
        ancestor = current.flat_parent_slot();
    }

    let (containing_origin, containing_size) = match containing {
        Some(block) => {
            let origin = accumulated_unrounded_origin(tree, state, block);
            let layout = &tree.layout(state, block).unrounded;
            (
                Point::new(origin.x + layout.border.left, origin.y + layout.border.top),
                Size::new(
                    (layout.size.width - layout.border.horizontal_sum()).max(0.0),
                    (layout.size.height - layout.border.vertical_sum()).max(0.0),
                ),
            )
        }
        None => (Point::ZERO, viewport),
    };

    let parent_origin = accumulated_unrounded_origin(tree, state, parent_slot);
    let static_position = tree.layout(state, node_id).static_position;
    let static_in_cb = Point::new(
        parent_origin.x + static_position.x - containing_origin.x,
        parent_origin.y + static_position.y - containing_origin.y,
    );

    let mut layout = compute_absolute_layout(tree, state, node_id, containing_size, static_in_cb);

    layout.location = Point::new(
        containing_origin.x + layout.location.x - parent_origin.x,
        containing_origin.y + layout.location.y - parent_origin.y,
    );
    let ordering_parent = box_parent(node).map_or(parent_slot, Node::slot);
    layout.order = sibling_paint_order(tree, ordering_parent, node_id);
    tree.layout_mut(state, node_id).set_unrounded(layout);
}

fn sibling_paint_order<T>(tree: &TreeArenas<T>, parent_id: NodeSlot, target: NodeSlot) -> u32 {
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
            if display_mode(style.display()) == DisplayMode::None {
                None
            } else if matches!(
                style.values().clone_position(),
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
