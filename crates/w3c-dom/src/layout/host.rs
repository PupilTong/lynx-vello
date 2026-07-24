//! [`LayoutNode`] implemented **directly on `&Node<T>`**, plus the pass
//! pipeline built on it.

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
    AvailableSpace, Layout, LayoutGoal, LayoutInput, LayoutNode, LayoutOutput,
};

use super::style::{
    DisplayMode, StyleView, TextStyleView, display_mode, establishes_absolute_containing_block,
    establishes_fixed_containing_block, resolve_position, skips_contents,
};
use crate::document::Document;
use crate::node::{ChildrenIter, Node};

impl<'dom, T> LayoutNode for &'dom Node<T> {
    type Style = StyleView<'dom, T>;
    type ChildIter = ChildrenIter<'dom, T>;

    fn children(self) -> Self::ChildIter {
        Node::children(self)
    }

    fn child_count(self) -> usize {
        self.child_ids().len()
    }

    fn style(self) -> Self::Style {
        StyleView::of(self)
    }

    fn compute_layout(self, input: LayoutInput) -> LayoutOutput {
        let display = if self.is_text_node() {
            DisplayMode::Leaf
        } else {
            let view = self.style();
            let display = display_mode(view.display());
            if display == DisplayMode::None {
                hide_subtree(self);
                return LayoutOutput::HIDDEN;
            }
            if view.skips_contents() {
                return compute_skipped_contents_layout(self, input);
            }
            display
        };

        compute_cached_layout(self, input, move |node, input| match display {
            DisplayMode::None => unreachable!("hidden nodes never reach the cache wrapper"),
            DisplayMode::Flex => compute_flexbox_layout(node, input),
            DisplayMode::Grid => compute_grid_layout(node, input),
            DisplayMode::Linear => compute_linear_layout(node, input),
            DisplayMode::Relative => compute_relative_layout(node, input),
            DisplayMode::Leaf => {
                let output = if node.is_text_node() {
                    let view = TextStyleView::of(node);
                    let run = TextRun {
                        text: node.text().unwrap_or_default(),
                        style: &view,
                        preserve_newlines: false,
                    };
                    let mut context = node.text_context().borrow_mut();
                    let mut artifacts = node.text_artifacts().borrow_mut();
                    let mut measurer = TextMeasurer::new(
                        &mut context,
                        &mut artifacts,
                        &view,
                        std::iter::once(run),
                    );
                    measurer.compute_layout(input)
                } else {
                    let view = node.style();
                    #[cfg(feature = "layout-test-utils")]
                    if let Some(metrics) = node.test_leaf_metrics() {
                        compute_leaf_layout_with_measurement_for_testing(
                            input,
                            &view,
                            None,
                            |_measure_input| metrics,
                        )
                    } else {
                        compute_leaf_layout(input, &view, node.natural_size())
                    }
                    #[cfg(not(feature = "layout-test-utils"))]
                    compute_leaf_layout(input, &view, node.natural_size())
                };
                if input.goal == LayoutGoal::Commit {
                    for grandchild in Node::children(node) {
                        hide_subtree(grandchild);
                    }
                }
                output
            }
        })
    }

    #[inline]
    fn set_unrounded_layout(self, layout: Layout) {
        self.layout_results.borrow_mut().unrounded = layout;
    }

    #[inline]
    fn with_unrounded_layout<R>(self, read: impl FnOnce(&Layout) -> R) -> R {
        let results = self.layout_results.borrow();
        read(&results.unrounded)
    }

    #[inline]
    fn clone_unrounded_layout(self) -> Layout {
        self.layout_results.borrow().unrounded.clone()
    }

    #[inline]
    fn set_rounded_layout(self, layout: Layout) {
        self.layout_results.borrow_mut().rounded = layout;
    }

    fn set_static_position(self, static_position: Point<f32>) {
        self.layout_data().borrow_mut().static_position = static_position;
    }

    fn cached_layout(self, input: LayoutInput) -> Option<LayoutOutput> {
        self.layout_data().borrow().measure_cache.get(input)
    }

    fn store_cached_layout(self, input: LayoutInput, output: LayoutOutput) {
        self.layout_data()
            .borrow_mut()
            .measure_cache
            .store(input, output);
    }

    fn clear_layout_cache(self) {
        self.layout_data().borrow_mut().clear_measurement_cache();
        self.invalidate_text_artifacts();
    }
}

pub(super) fn run_layout<T>(document: &Document<T>, viewport: Size<f32>, scale: f32, full: bool) {
    let Some(root) = document.root_element() else {
        return;
    };
    let parked = collect_parked_boundaries(document);
    for &(_, id, input) in &parked {
        if let Some(node) = document.get(id)
            && node.is_element()
            && is_relayout_boundary(&StyleView::of(node))
        {
            let output = compute_boundary_relayout(node, input);
            node.layout_results.borrow_mut().unrounded.content_size = output.content_size;
        }
    }
    compute_root_layout(
        root,
        Size::new(
            AvailableSpace::Definite(viewport.width),
            AvailableSpace::Definite(viewport.height),
        ),
    );
    if full {
        let position = |node| pre_position(node, viewport);
        round_with(root, scale, Point::ZERO, position);
    } else {
        position_and_round_parked_boundaries(document, &parked, viewport, scale);
    }
}

fn collect_parked_boundaries<T>(
    document: &Document<T>,
) -> Vec<(usize, crate::NodeId, LayoutInput)> {
    let roots = document.relayout_roots();
    if roots.is_empty() {
        return Vec::new();
    }
    let mut parked: Vec<(usize, crate::NodeId, LayoutInput)> = roots
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

fn position_and_round_parked_boundaries<T>(
    document: &Document<T>,
    parked: &[(usize, crate::NodeId, LayoutInput)],
    viewport: Size<f32>,
    scale: f32,
) {
    for &(_, id, _) in parked {
        let Some(node) = document.get(id) else {
            continue;
        };
        if !node.is_element() || !is_relayout_boundary(&StyleView::of(node)) {
            continue;
        }
        if has_parked_ancestor(document, node) {
            continue;
        }
        let parent_origin = node
            .parent()
            .map_or(Point::ZERO, accumulated_unrounded_origin);
        let position = |node| pre_position(node, viewport);
        round_with(node, scale, parent_origin, position);
    }
}

fn has_parked_ancestor<T>(document: &Document<T>, node: &Node<T>) -> bool {
    std::iter::successors(node.parent(), |node| node.parent())
        .any(|ancestor| document.is_relayout_root_parked(ancestor.id()))
}

fn accumulated_unrounded_origin<T>(node: &Node<T>) -> Point<f32> {
    std::iter::successors(Some(node), |node| node.parent()).fold(Point::ZERO, |origin, node| {
        let location = node.layout_results.borrow().unrounded.location;
        Point::new(origin.x + location.x, origin.y + location.y)
    })
}

fn boundary_depth<T>(document: &Document<T>, id: crate::NodeId) -> usize {
    let parent = document.get(id).and_then(Node::parent);
    std::iter::successors(parent, |node| node.parent()).count()
}

fn pre_position<T>(node: &Node<T>, viewport: Size<f32>) -> bool {
    let Some(style) = StyleView::try_of(node) else {
        return false;
    };
    let display = display_mode(style.display());
    if display == DisplayMode::None {
        return false;
    }
    if node.parent().is_some_and(Node::is_element)
        && resolve_position(node, style.values()) == PositionProperty::Fixed
    {
        let fixed = style.values().clone_position() == PositionProperty::Fixed;
        position_hoisted(node, viewport, fixed);
    }
    display != DisplayMode::Leaf && !skips_contents(style.values())
}

fn position_hoisted<T>(node: &Node<T>, viewport: Size<f32>, fixed: bool) {
    let Some(parent) = node.parent() else {
        return;
    };

    let mut containing = None;
    let mut ancestor = node.parent();
    while let Some(current) = ancestor {
        let Some(style) = StyleView::try_of(current) else {
            break;
        };
        let establishes = if fixed {
            establishes_fixed_containing_block(current, style.values())
        } else {
            establishes_absolute_containing_block(current, style.values())
        };
        if establishes {
            containing = Some(current);
            break;
        }
        ancestor = current.parent();
    }

    let (containing_origin, containing_size) = match containing {
        Some(block) => {
            let origin = accumulated_unrounded_origin(block);
            let results = block.layout_results.borrow();
            let layout = &results.unrounded;
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

    let parent_origin = accumulated_unrounded_origin(parent);
    let static_position = node.layout_data().borrow().static_position;
    let static_in_cb = Point::new(
        parent_origin.x + static_position.x - containing_origin.x,
        parent_origin.y + static_position.y - containing_origin.y,
    );

    let mut layout = compute_absolute_layout(node, containing_size, static_in_cb);

    layout.location = Point::new(
        containing_origin.x + layout.location.x - parent_origin.x,
        containing_origin.y + layout.location.y - parent_origin.y,
    );
    layout.order = sibling_paint_order(parent, node.id());
    LayoutNode::set_unrounded_layout(node, layout);
}

fn sibling_paint_order<T>(parent: &Node<T>, target: crate::NodeId) -> u32 {
    let Some(target_index) = parent.child_ids().iter().position(|&id| id == target) else {
        return 0;
    };
    let target_key = (0_i32, target_index);
    let mut rank = 0u32;
    for (index, child) in Node::children(parent).enumerate() {
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
