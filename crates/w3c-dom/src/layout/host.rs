//! [`LayoutNode`] implemented **directly on `&Node<T>`**, plus the pass
//! pipeline built on it.

#[cfg(feature = "layout-test-utils")]
use neutron_star::compute::compute_leaf_layout_with_measurement_for_testing;
use neutron_star::compute::{
    compute_absolute_layout, compute_boundary_relayout, compute_cached_layout,
    compute_flexbox_layout, compute_grid_layout, compute_leaf_layout, compute_linear_layout,
    compute_relative_layout, compute_root_layout, compute_skipped_contents_layout, hide_subtree,
    round_layout, round_layout_subtree,
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

/// The box-tree child iterator: DOM children with `display: contents` levels
/// dissolved away (css-display-3 §2.5 — a contents element is replaced by
/// its children), recursively, in document order.
///
/// This is what the layout protocol, rounding, hiding, and the visual
/// builder all traverse, so dissolved grandchildren become real layout items
/// of the box parent with locations, static positions, and `Layout.order`
/// ranks in the box parent's space. Stylo's restyle traversal keeps walking
/// the plain DOM [`ChildrenIter`] — contents elements still cascade and
/// relay inheritance.
///
/// Skipped contents nodes have their stored layouts zeroed here (guarded by
/// an is-default read, so steady-state passes never write): this iterator is
/// the only traversal guaranteed to visit them in every pass shape, and the
/// zeroed layout is the invariant that keeps raw-DOM-chain origin sums
/// (`accumulated_unrounded_origin`) equal to box-tree sums. Consequently no
/// `layout_results` borrow of a `display: contents` node may be held across
/// any walk that drives this iterator.
#[derive(Debug)]
pub struct BoxTreeChildren<'dom, T> {
    stack: smallvec::SmallVec<[ChildrenIter<'dom, T>; 2]>,
}

pub(crate) fn box_tree_children<T>(node: &Node<T>) -> BoxTreeChildren<'_, T> {
    let mut stack = smallvec::SmallVec::new();
    stack.push(Node::children(node));
    BoxTreeChildren { stack }
}

/// The nearest box-generating ancestor: the DOM parent unless it is a
/// styled `display: contents` element, in which case the chain is walked
/// upward past every contents level.
pub(crate) fn box_tree_parent<T>(node: &Node<T>) -> Option<&Node<T>> {
    let mut current = node.parent();
    while let Some(candidate) = current {
        if !dissolves(candidate) {
            return Some(candidate);
        }
        current = candidate.parent();
    }
    None
}

/// Whether a node is dissolved by the box-tree traversal: a styled element
/// whose computed display is `contents`. Unstyled elements (descendants of
/// `display: none` roots) are not dissolved — they stay visible to the hide
/// machinery exactly as before.
fn dissolves<T>(node: &Node<T>) -> bool {
    node.is_element()
        && StyleView::try_of(node)
            .is_some_and(|style| style.values().get_box().display.is_contents())
}

impl<'dom, T> Iterator for BoxTreeChildren<'dom, T> {
    type Item = &'dom Node<T>;

    fn next(&mut self) -> Option<&'dom Node<T>> {
        loop {
            let frame = self.stack.last_mut()?;
            let Some(child) = frame.next() else {
                self.stack.pop();
                continue;
            };
            if dissolves(child) {
                zero_dissolved_layout(child);
                self.stack.push(Node::children(child));
                continue;
            }
            return Some(child);
        }
    }
}

/// Enforces the zeroed-layout invariant on a dissolved node. Both stored
/// layouts must be checked: `hide_subtree` zeroes only the unrounded side,
/// so a contents child of a hidden interior can sit at default-unrounded /
/// stale-rounded until this runs.
fn zero_dissolved_layout<T>(node: &Node<T>) {
    let stale = {
        let results = node.layout_results.borrow();
        results.unrounded != Layout::default() || results.rounded != Layout::default()
    };
    if stale {
        let mut results = node.layout_results.borrow_mut();
        results.unrounded = Layout::default();
        results.rounded = Layout::default();
    }
}

impl<'dom, T> LayoutNode for &'dom Node<T> {
    type Style = StyleView<'dom, T>;
    type ChildIter = BoxTreeChildren<'dom, T>;

    fn children(self) -> Self::ChildIter {
        box_tree_children(self)
    }

    fn child_count(self) -> usize {
        // Approximate capacity hint only (over-counts contents children,
        // under-counts their dissolved grandchildren); the engine uses it
        // solely for Vec::with_capacity.
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
            if display == DisplayMode::Contents {
                // Reachable only out-of-band (e.g. a stale parked boundary
                // that flipped to contents after parking — the parked loops
                // also gate on this): the element has no box. Zero self
                // WITHOUT recursing — a protocol hide here would clobber the
                // box parent's live items. This check must precede
                // skips_contents: content-visibility is inert on a boxless
                // element and must not synthesize a box.
                zero_dissolved_layout(self);
                return LayoutOutput::HIDDEN;
            }
            if view.skips_contents() {
                return compute_skipped_contents_layout(self, input);
            }
            display
        };

        compute_cached_layout(self, input, move |node, input| match display {
            DisplayMode::None | DisplayMode::Contents => {
                unreachable!("hidden and boxless nodes never reach the cache wrapper")
            }
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
            && parked_boundary_is_current(node)
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
        position_hoisted_subtree(root, viewport);
        round_layout(root, scale);
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
        if !node.is_element() || !parked_boundary_is_current(node) {
            continue;
        }
        if has_parked_ancestor(document, node) {
            continue;
        }
        position_hoisted_subtree(node, viewport);
        let parent_origin = node
            .parent()
            .map_or(Point::ZERO, accumulated_unrounded_origin);
        round_layout_subtree(node, scale, parent_origin);
    }
}

/// Re-validates a parked relayout root against its *current* style: it must
/// still be a containment boundary, and it must still generate a box — a
/// boundary that flipped to `display: contents` after parking has no box to
/// lay out (`is_relayout_boundary` is containment-only and display-agnostic,
/// so it alone would let the stale root through). Skipping is safe: the
/// flip's own damage cleared the box parent's spine, so the regular pass
/// recomputes everything through the dissolving iterator.
fn parked_boundary_is_current<T>(node: &Node<T>) -> bool {
    let style = StyleView::of(node);
    display_mode(style.display()) != DisplayMode::Contents && is_relayout_boundary(&style)
}

fn has_parked_ancestor<T>(document: &Document<T>, node: &Node<T>) -> bool {
    let mut current = node.parent();
    while let Some(ancestor) = current {
        if document.is_relayout_root_parked(ancestor.id()) {
            return true;
        }
        current = ancestor.parent();
    }
    false
}

fn accumulated_unrounded_origin<T>(node: &Node<T>) -> Point<f32> {
    let mut origin = Point::ZERO;
    let mut current = Some(node);
    while let Some(step) = current {
        let location = step.layout_results.borrow().unrounded.location;
        origin.x += location.x;
        origin.y += location.y;
        current = step.parent();
    }
    origin
}

fn boundary_depth<T>(document: &Document<T>, id: crate::NodeId) -> usize {
    let mut depth = 0;
    let mut current = document.get(id).and_then(Node::parent);
    while let Some(node) = current {
        depth += 1;
        current = node.parent();
    }
    depth
}

fn position_hoisted_subtree<T>(node: &Node<T>, viewport: Size<f32>) {
    let Some(style) = StyleView::try_of(node) else {
        return;
    };
    let display = display_mode(style.display());
    if display == DisplayMode::None {
        return;
    }
    if display == DisplayMode::Contents {
        // Boxless: nothing to hoist (position is inert), and skips_contents
        // does not apply — just walk through to the dissolved descendants.
        for child in Node::children(node) {
            position_hoisted_subtree(child, viewport);
        }
        return;
    }
    if node.parent().is_some_and(Node::is_element)
        && resolve_position(node, style.values()) == PositionProperty::Fixed
    {
        position_hoisted(node, viewport);
    }
    if display == DisplayMode::Leaf || skips_contents(style.values()) {
        return;
    }
    for child in Node::children(node) {
        position_hoisted_subtree(child, viewport);
    }
}

fn position_hoisted<T>(node: &Node<T>, viewport: Size<f32>) {
    let Some(parent) = node.parent() else {
        return;
    };
    let fixed = StyleView::try_of(node)
        .is_some_and(|style| style.values().clone_position() == PositionProperty::Fixed);

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

/// The paint rank of a hoisted (out-of-flow) box among the **box-tree**
/// siblings of its nearest box-generating ancestor — the same merged
/// `(effective order, dissolved index)` space the layout algorithms rank
/// in-flow items in, so `Layout.order` values stay comparable after
/// `display: contents` dissolution merges DOM sibling sets.
fn sibling_paint_order<T>(dom_parent: &Node<T>, target: crate::NodeId) -> u32 {
    let box_parent = if dissolves(dom_parent) {
        match box_tree_parent(dom_parent) {
            Some(ancestor) => ancestor,
            // Detached-tree guard: attached chains always end at the
            // document node, which never dissolves.
            None => dom_parent,
        }
    } else {
        dom_parent
    };
    let mut rank = 0_u32;
    let mut earlier = 0_u32;
    let mut seen_target = false;
    for child in box_tree_children(box_parent) {
        let Some(order) = sibling_effective_paint_order(child) else {
            continue;
        };
        if child.id() == target {
            debug_assert_eq!(
                order, 0,
                "sibling_paint_order is only called for out-of-flow (hoisted) \
                 targets, whose effective paint order is 0"
            );
            seen_target = true;
            rank += earlier;
            continue;
        }
        if seen_target {
            // After the target: only strictly negative orders sort below
            // the target's (0, index) key.
            if order < 0 {
                rank += 1;
            }
        } else if order <= 0 {
            // Before the target: any non-positive order sorts below it.
            earlier += 1;
        }
    }
    if seen_target { rank } else { 0 }
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
