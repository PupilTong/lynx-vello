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
    compute_flexbox_layout, compute_grid_layout, compute_leaf_layout, compute_linear_layout,
    compute_relative_layout, compute_root_layout, compute_skipped_contents_layout, hide_subtree,
    round_layout_subtree_with as round_with,
};
use hughie::geometry::{Point, Size};
use hughie::invalidate::is_relayout_boundary;
use hughie::style::{CoreStyle, PositionProperty, TextRun};
use hughie::text::TextMeasurer;
use hughie::tree::{
    AvailableSpace, Layout, LayoutGoal, LayoutInput, LayoutOutput, LayoutSlot, LayoutTree,
};
use rustc_hash::FxHashSet;

use super::style::{
    DisplayMode, StyleView, TextStyleView, box_parent, display_mode,
    establishes_absolute_containing_block, establishes_fixed_containing_block, resolve_position,
    skips_contents,
};
use crate::tree::document::{
    Document, DocumentLayoutState, NodeId, NodeSlot, PendingRelayout, RelayoutKind, TreeArenas,
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

    fn child_count(&self, node: NodeSlot) -> usize {
        self.at(node).flat_children().len()
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
                return compute_skipped_contents_layout(self, state, node, input);
            }
            if node_ref.is_replaced() {
                DisplayMode::Leaf
            } else {
                display
            }
        };

        compute_cached_layout(self, state, node, input, move |tree, state, node, input| {
            match display {
                DisplayMode::None | DisplayMode::Contents => {
                    unreachable!("a box-less element has no box to lay out")
                }
                DisplayMode::Flex => compute_flexbox_layout(tree, state, node, input),
                DisplayMode::Grid => compute_grid_layout(tree, state, node, input),
                DisplayMode::Linear => compute_linear_layout(tree, state, node, input),
                DisplayMode::Relative => compute_relative_layout(tree, state, node, input),
                DisplayMode::Leaf => {
                    let node_ref = tree.at(node);
                    let output = if node_ref.is_text_node() {
                        let view = TextStyleView::of(node_ref);
                        let run = TextRun {
                            text: node_ref.text().unwrap_or_default(),
                            style: &view,
                            preserve_newlines: false,
                        };
                        let (context, artifacts) = state.text_parts(node);
                        let mut measurer =
                            TextMeasurer::new(context, artifacts, &view, std::iter::once(run));
                        measurer.compute_layout(input)
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
                    if input.goal == LayoutGoal::Commit {
                        for grandchild in tree.children(node) {
                            hide_subtree(tree, state, grandchild);
                        }
                    }
                    output
                }
            }
        })
    }

    fn clear_layout_cache(&self, state: &mut Self::State, node: NodeSlot) {
        state.clear_layout_cache(node);
    }
}

pub(super) fn run_layout<T: Sync>(
    document: &mut Document<T>,
    viewport: Size<f32>,
    scale: f32,
    full: bool,
    rescale: bool,
) {
    let root = document.document_element().id();
    let parked = collect_parked_boundaries(document);
    let (tree, state, parked_ids) = document.layout_parts();
    let root = tree.live_slot(root);
    let mut escalated = false;
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
                        let output = compute_boundary_relayout(tree, state, slot, pending.input);
                        tree.layout_mut(state, slot)
                            .set_unrounded_content_size(output.content_size);
                    }
                }
                RelayoutKind::InPlace { previous } => {
                    let output = tree.compute_layout(state, slot, pending.input);
                    // A reproduced output proves nothing above this node can
                    // observe the change; anything else falls back to the
                    // whole-tree pass, which reuses the caches just filled.
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
    let full = full || escalated;
    compute_root_layout(
        tree,
        state,
        root,
        Size::new(
            AvailableSpace::Definite(viewport.width),
            AvailableSpace::Definite(viewport.height),
        ),
    );
    if full {
        let position = |tree: &TreeArenas<T>, state: &mut DocumentLayoutState, node| {
            pre_position(tree, state, node, viewport)
        };
        round_with(tree, state, root, scale, Point::ZERO, rescale, position);
    } else {
        position_and_round_parked_boundaries(tree, state, parked_ids, &parked, viewport, scale);
    }
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
        return true;
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
    display != DisplayMode::Leaf && !skips_contents(style.values())
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
