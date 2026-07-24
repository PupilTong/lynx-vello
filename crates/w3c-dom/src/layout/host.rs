//! The statically split [`LayoutTree`] host over the document's immutable
//! tree/style arenas and mutable layout/text state.

#[cfg(feature = "layout-test-utils")]
use neutron_star::compute::compute_leaf_layout_with_measurement_for_testing;
use neutron_star::compute::{
    compute_absolute_layout, compute_boundary_relayout, compute_cached_layout,
    compute_flexbox_layout, compute_grid_layout, compute_leaf_layout, compute_linear_layout,
    compute_relative_layout, compute_root_layout, compute_skipped_contents_layout, hide_subtree,
    round_layout_subtree_with as round_with,
};
use neutron_star::geometry::{Point, Size};
use neutron_star::invalidate::is_relayout_boundary;
use neutron_star::style::{CoreStyle, PositionProperty, TextRun};
use neutron_star::text::TextMeasurer;
use neutron_star::tree::{
    AvailableSpace, LayoutGoal, LayoutInput, LayoutOutput, LayoutSlot, LayoutTree,
};
use rustc_hash::FxHashSet;

use super::style::{
    DisplayMode, StyleView, TextStyleView, display_mode, establishes_absolute_containing_block,
    establishes_fixed_containing_block, resolve_position, skips_contents,
};
use crate::document::{Document, DocumentLayoutState, NodeId, TreeArenas, slab_get_for_live_node};
use crate::node::Node;

impl<T> LayoutTree for TreeArenas<T> {
    type NodeId = NodeId;
    type State = DocumentLayoutState;
    type Style<'tree>
        = StyleView<'tree, T>
    where
        Self: 'tree;
    type ChildIter<'tree>
        = core::iter::Copied<core::slice::Iter<'tree, NodeId>>
    where
        Self: 'tree;

    fn children(&self, node: NodeId) -> Self::ChildIter<'_> {
        slab_get_for_live_node(&self.nodes, node)
            .child_ids()
            .iter()
            .copied()
    }

    fn child_count(&self, node: NodeId) -> usize {
        slab_get_for_live_node(&self.nodes, node).child_ids().len()
    }

    fn style(&self, node: NodeId) -> Self::Style<'_> {
        StyleView::of(slab_get_for_live_node(&self.nodes, node))
    }

    fn layout<'state>(&self, state: &'state Self::State, node: NodeId) -> &'state LayoutSlot {
        &slab_get_for_live_node(&state.nodes, node).slot
    }

    fn layout_mut<'state>(
        &self,
        state: &'state mut Self::State,
        node: NodeId,
    ) -> &'state mut LayoutSlot {
        &mut state
            .nodes
            .get_mut(node)
            .expect("live node must have layout-arena state")
            .slot
    }

    fn compute_layout(
        &self,
        state: &mut Self::State,
        node: NodeId,
        input: LayoutInput,
    ) -> LayoutOutput {
        let node_ref = slab_get_for_live_node(&self.nodes, node);
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
            display
        };

        compute_cached_layout(self, state, node, input, move |tree, state, node, input| {
            match display {
                DisplayMode::None => unreachable!("hidden nodes never reach the cache wrapper"),
                DisplayMode::Flex => compute_flexbox_layout(tree, state, node, input),
                DisplayMode::Grid => compute_grid_layout(tree, state, node, input),
                DisplayMode::Linear => compute_linear_layout(tree, state, node, input),
                DisplayMode::Relative => compute_relative_layout(tree, state, node, input),
                DisplayMode::Leaf => {
                    let node_ref = slab_get_for_live_node(&tree.nodes, node);
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

    fn clear_layout_cache(&self, state: &mut Self::State, node: NodeId) {
        state.clear_layout_cache(node);
    }
}

pub(super) fn run_layout<T: Sync>(
    document: &mut Document<T>,
    viewport: Size<f32>,
    scale: f32,
    full: bool,
) {
    let Some(root) = document.root_element().map(Node::id) else {
        return;
    };
    let parked = collect_parked_boundaries(document);
    let (tree, state, parked_ids) = document.layout_parts();
    for &(_, id, input) in &parked {
        if let Some(node) = tree.nodes.get(id)
            && node.is_element()
            && is_relayout_boundary(&StyleView::of(node))
        {
            let output = compute_boundary_relayout(tree, state, id, input);
            tree.layout_mut(state, id).unrounded_mut().content_size = output.content_size;
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
    if full {
        let position = |tree: &TreeArenas<T>, state: &mut DocumentLayoutState, node| {
            pre_position(tree, state, node, viewport)
        };
        round_with(tree, state, root, scale, Point::ZERO, position);
    } else {
        position_and_round_parked_boundaries(tree, state, parked_ids, &parked, viewport, scale);
    }
}

fn collect_parked_boundaries<T>(document: &Document<T>) -> Vec<(usize, NodeId, LayoutInput)> {
    let roots = document.relayout_roots();
    if roots.is_empty() {
        return Vec::new();
    }
    let mut parked: Vec<(usize, NodeId, LayoutInput)> = roots
        .iter()
        .map(|pending| {
            (
                boundary_depth(document, pending.node_id),
                pending.node_id,
                pending.input,
            )
        })
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
    parked: &[(usize, NodeId, LayoutInput)],
    viewport: Size<f32>,
    scale: f32,
) {
    for &(_, id, _) in parked {
        let Some(node) = tree.nodes.get(id) else {
            continue;
        };
        if !node.is_element() || !is_relayout_boundary(&StyleView::of(node)) {
            continue;
        }
        if has_parked_ancestor(tree, node, parked_ids) {
            continue;
        }
        let parent_origin = node.parent_id().map_or(Point::ZERO, |parent| {
            accumulated_unrounded_origin(tree, state, parent)
        });
        let position = |tree: &TreeArenas<T>, state: &mut DocumentLayoutState, node| {
            pre_position(tree, state, node, viewport)
        };
        round_with(tree, state, id, scale, parent_origin, position);
    }
}

fn has_parked_ancestor<T>(
    tree: &TreeArenas<T>,
    node: &Node<T>,
    parked_ids: &FxHashSet<NodeId>,
) -> bool {
    let mut current = node.parent_id();
    while let Some(id) = current {
        if parked_ids.contains(&id) {
            return true;
        }
        current = slab_get_for_live_node(&tree.nodes, id).parent_id();
    }
    false
}

fn accumulated_unrounded_origin<T>(
    tree: &TreeArenas<T>,
    state: &DocumentLayoutState,
    node: NodeId,
) -> Point<f32> {
    let mut origin = Point::ZERO;
    let mut current = Some(node);
    while let Some(id) = current {
        let location = tree.layout(state, id).unrounded().location;
        origin = Point::new(origin.x + location.x, origin.y + location.y);
        current = slab_get_for_live_node(&tree.nodes, id).parent_id();
    }
    origin
}

fn boundary_depth<T>(document: &Document<T>, id: NodeId) -> usize {
    let mut depth = 0;
    let mut current = document.get(id).and_then(Node::parent_id);
    while let Some(id) = current {
        depth += 1;
        current = document.get(id).and_then(Node::parent_id);
    }
    depth
}

fn pre_position<T: Sync>(
    tree: &TreeArenas<T>,
    state: &mut DocumentLayoutState,
    node_id: NodeId,
    viewport: Size<f32>,
) -> bool {
    let node = slab_get_for_live_node(&tree.nodes, node_id);
    let Some(style) = StyleView::try_of(node) else {
        return false;
    };
    let display = display_mode(style.display());
    if display == DisplayMode::None {
        return false;
    }
    if node
        .parent_id()
        .and_then(|id| tree.nodes.get(id))
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
    node_id: NodeId,
    viewport: Size<f32>,
    fixed: bool,
) {
    let node = slab_get_for_live_node(&tree.nodes, node_id);
    let Some(parent_id) = node.parent_id() else {
        return;
    };

    let mut containing = None;
    let mut ancestor = node.parent_id();
    while let Some(current_id) = ancestor {
        let current = slab_get_for_live_node(&tree.nodes, current_id);
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
        ancestor = current.parent_id();
    }

    let (containing_origin, containing_size) = match containing {
        Some(block) => {
            let origin = accumulated_unrounded_origin(tree, state, block);
            let layout = tree.layout(state, block).unrounded();
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

    let parent_origin = accumulated_unrounded_origin(tree, state, parent_id);
    let static_position = tree.layout(state, node_id).static_position();
    let static_in_cb = Point::new(
        parent_origin.x + static_position.x - containing_origin.x,
        parent_origin.y + static_position.y - containing_origin.y,
    );

    let mut layout = compute_absolute_layout(tree, state, node_id, containing_size, static_in_cb);

    layout.location = Point::new(
        containing_origin.x + layout.location.x - parent_origin.x,
        containing_origin.y + layout.location.y - parent_origin.y,
    );
    layout.order = sibling_paint_order(tree, parent_id, node_id);
    tree.layout_mut(state, node_id).set_unrounded(layout);
}

fn sibling_paint_order<T>(tree: &TreeArenas<T>, parent_id: NodeId, target: NodeId) -> u32 {
    let parent = slab_get_for_live_node(&tree.nodes, parent_id);
    let Some(target_index) = parent.child_ids().iter().position(|&id| id == target) else {
        return 0;
    };
    let target_key = (0_i32, target_index);
    let mut rank = 0u32;
    for (index, &child_id) in parent.child_ids().iter().enumerate() {
        let child = slab_get_for_live_node(&tree.nodes, child_id);
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
