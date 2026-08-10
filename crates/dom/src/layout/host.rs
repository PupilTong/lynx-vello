//! The statically split [`LayoutTree`] host over the document's immutable
//! tree/style arenas and mutable layout/text state.

#[cfg(feature = "layout-test-utils")]
use hughie::compute::compute_leaf_layout_with_measurement_for_testing;
use hughie::compute::{
    AtomicInlineMetrics, LeafMetrics, accumulate_scrollable_overflow, commit_atomic_inline,
    compute_absolute_layout, compute_boundary_relayout, compute_cached_layout,
    compute_flexbox_layout, compute_grid_layout, compute_leaf_layout,
    compute_leaf_layout_with_measurement, compute_linear_layout, compute_relative_layout,
    compute_root_layout, compute_skipped_contents_layout, content_box_origin, hide_subtree,
    measure_atomic_inline, padding_box_geometry, round_layout_subtree_with as round_with,
};
use hughie::geometry::{Point, Size};
use hughie::invalidate::is_relayout_boundary;
use hughie::style::{CoreStyle, PositionProperty, TextRun};
use hughie::text::{
    AtomicInlineBox, InlineItem, InlineMeasurer, PositionedInlineBox, TextMeasurer,
};
use hughie::tree::{
    AvailableSpace, Layout, LayoutGoal, LayoutInput, LayoutOutput, LayoutSlot, LayoutTree,
};
use rustc_hash::{FxHashMap, FxHashSet};
use stylo::properties::ComputedValues;

use super::style::{
    DisplayMode, StyleView, TextStyleView, box_parent, display_mode,
    establishes_absolute_containing_block, establishes_fixed_containing_block, resolve_position,
    skips_contents,
};
use crate::tree::document::{
    Document, DocumentLayoutState, NodeId, TreeArenas, slab_get_for_live_node,
};
use crate::tree::node::Node;

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

    /// The flat-tree children — a host lays out its shadow tree, and a slot
    /// lays out the nodes assigned to it. Every other `LayoutTree` walk
    /// (`flattened_children` included) reads the tree through this one method,
    /// so the whole engine follows the flat tree from here.
    fn children(&self, node: NodeId) -> Self::ChildIter<'_> {
        slab_get_for_live_node(&self.nodes, node)
            .flat_children()
            .iter()
            .copied()
    }

    fn child_count(&self, node: NodeId) -> usize {
        slab_get_for_live_node(&self.nodes, node)
            .flat_children()
            .len()
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
            if display.is_none() {
                hide_subtree(self, state, node);
                return LayoutOutput::HIDDEN;
            }
            if view.skips_contents() {
                return compute_skipped_contents_layout(self, state, node, input);
            }
            // A replaced element has no inner formatting context: its box is
            // filled by external content, so the *inner* display type is simply
            // not applicable to it (css-display-3 §2.3 — `display` on a replaced
            // element sets its outer role only). Routing it to `Leaf` regardless
            // is therefore the W3C-correct behaviour, and it is load-bearing
            // rather than pedantic here: the Lynx UA cascade sets
            // `display: linear` on *every* element, so without this an `<img>`
            // would land in the linear algorithm, ignore its natural size, and
            // lay out at 0x0 with the decode silently wasted.
            //
            // Keyed on replaced *identity*, not on whether a natural size has
            // arrived: an image is replaced before its header lands, and a node
            // that changed formatting context between frames would relayout its
            // whole subtree for nothing.
            if node_ref.is_replaced() {
                DisplayMode::Leaf
            } else {
                display
            }
        };

        compute_cached_layout(self, state, node, input, move |tree, state, node, input| {
            match display {
                // `None` is hidden and returned above. `Contents` generates no
                // box at all and nothing routes one here: `flattened_children`
                // splices it out of every item collection, the positioned pass
                // never hoists it, `is_relayout_boundary` is false for it, and
                // Stylo blockifies it on the document element.
                DisplayMode::None | DisplayMode::Contents => {
                    unreachable!("a box-less element has no box to lay out")
                }
                DisplayMode::Flex { .. } => compute_flexbox_layout(tree, state, node, input),
                DisplayMode::Grid { .. } => compute_grid_layout(tree, state, node, input),
                DisplayMode::Linear { .. } => compute_linear_layout(tree, state, node, input),
                DisplayMode::Relative { .. } => compute_relative_layout(tree, state, node, input),
                DisplayMode::Flow { .. } => compute_flow_layout(tree, state, node, input),
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

#[derive(Debug, Clone, Copy)]
enum InlineSource<'dom> {
    Text {
        text: &'dom str,
        style: &'dom ComputedValues,
    },
    Atomic {
        node: NodeId,
        order: u32,
    },
    Positioned {
        node: NodeId,
        order: u32,
        position: PositionProperty,
    },
}

#[derive(Debug, Clone, Copy)]
struct MeasuredInlineAtom {
    metrics: AtomicInlineMetrics<NodeId>,
    order: u32,
}

/// Lays out the inline contents of a flow box while leaving each atomic
/// child's inner formatting context to its ordinary Hughie algorithm.
///
/// Atomic widths are resolved against the whole containing block before the
/// paragraph is broken. Parley therefore receives indivisible, already-sized
/// margin boxes and alone decides whether each stays in the remaining space
/// or moves to the next line.
#[allow(
    clippy::too_many_lines,
    reason = "the probe, paragraph break, and atomic commit phases share one borrowed state"
)]
fn compute_flow_layout<T>(
    tree: &TreeArenas<T>,
    state: &mut DocumentLayoutState,
    node: NodeId,
    input: LayoutInput,
) -> LayoutOutput {
    let view = tree.style(node);
    let mut sources = Vec::new();
    let mut transparent = Vec::new();
    let mut hidden = Vec::new();
    let mut atomic_order = 0;
    collect_inline_sources(
        tree,
        node,
        view.values(),
        &mut sources,
        &mut transparent,
        &mut hidden,
        &mut atomic_order,
    );

    let mut measured_atoms: Vec<MeasuredInlineAtom> = Vec::new();
    let mut positioned_atoms: Vec<PositionedInlineBox> = Vec::new();
    let mut paragraph_items: Vec<InlineItem<'_, ComputedValues>> =
        Vec::with_capacity(sources.len());
    let mut output =
        compute_leaf_layout_with_measurement(input, &view, None, true, |measure_input| {
            let containing_block = Size::new(
                measure_input
                    .known_dimensions
                    .width
                    .or_else(|| measure_input.available_space.width.definite_value()),
                measure_input
                    .known_dimensions
                    .height
                    .or_else(|| measure_input.available_space.height.definite_value()),
            );
            measured_atoms.clear();
            for source in &sources {
                if let InlineSource::Atomic { node, order } = *source {
                    measured_atoms.push(MeasuredInlineAtom {
                        metrics: measure_atomic_inline(
                            tree,
                            state,
                            node,
                            containing_block,
                            measure_input.available_space,
                        ),
                        order,
                    });
                }
            }

            let mut atom_index = 0;
            paragraph_items.clear();
            paragraph_items.extend(sources.iter().map(|source| match *source {
                InlineSource::Text { text, style } => InlineItem::Text(TextRun {
                    text,
                    style,
                    preserve_newlines: false,
                }),
                InlineSource::Atomic { .. } => {
                    let metrics = measured_atoms[atom_index].metrics;
                    atom_index += 1;
                    let size = metrics.margin_box_size();
                    let baseline = metrics.first_baselines().y.unwrap_or(size.height);
                    InlineItem::Atomic(
                        AtomicInlineBox::new(
                            u64::try_from(metrics.node())
                                .expect("NodeId must fit Parley's inline-box id"),
                            size.width,
                            size.height,
                        )
                        .with_baseline(baseline),
                    )
                }
                InlineSource::Positioned { node, .. } => InlineItem::Atomic(AtomicInlineBox::new(
                    u64::try_from(node).expect("NodeId must fit Parley's inline-box id"),
                    0.0,
                    0.0,
                )),
            }));
            let (context, artifacts) = state.text_parts(node);
            let mut measurer =
                InlineMeasurer::new(context, artifacts, &view, paragraph_items.iter().copied());
            let measurement = measurer.measure(measure_input);
            if measure_input.goal == LayoutGoal::Commit {
                positioned_atoms.clear();
                positioned_atoms.extend(measurement.layout().positioned_inline_boxes());
            }
            LeafMetrics::new(measurement.size()).with_first_baselines(measurement.first_baselines())
        });

    if input.goal == LayoutGoal::Commit {
        let content_origin = content_box_origin(&view, input.parent_size.width);
        let positioned_by_id: FxHashMap<_, _> = positioned_atoms
            .iter()
            .map(|positioned| (positioned.id, *positioned))
            .collect();
        for measured in measured_atoms.iter().copied() {
            let metrics = measured.metrics;
            let id = u64::try_from(metrics.node()).expect("NodeId must fit an inline-box id");
            let Some(positioned) = positioned_by_id.get(&id) else {
                hide_subtree(tree, state, metrics.node());
                continue;
            };
            let committed = commit_atomic_inline(
                tree,
                state,
                metrics,
                Point::new(
                    content_origin.x + positioned.origin.x,
                    content_origin.y + positioned.origin.y,
                ),
                measured.order,
            );
            let layout = &tree.layout(state, metrics.node()).unrounded;
            accumulate_scrollable_overflow(
                &mut output.content_size,
                layout.location,
                layout.size,
                committed.content_size,
                tree.style(metrics.node()).overflow(),
            );
        }

        let (padding_origin, padding_size) = padding_box_geometry(&view, output.size);
        for source in &sources {
            let InlineSource::Positioned {
                node: positioned_node,
                order,
                position,
            } = *source
            else {
                continue;
            };
            let id = u64::try_from(positioned_node).expect("NodeId must fit an inline-box id");
            let Some(positioned) = positioned_by_id.get(&id) else {
                tree.set_static_position(state, positioned_node, Point::ZERO);
                hide_subtree(tree, state, positioned_node);
                continue;
            };
            let static_in_border_space = Point::new(
                content_origin.x + positioned.origin.x,
                content_origin.y + positioned.line_top,
            );
            match position {
                PositionProperty::Absolute => {
                    let static_in_padding_space = Point::new(
                        static_in_border_space.x - padding_origin.x,
                        static_in_border_space.y - padding_origin.y,
                    );
                    let mut layout = compute_absolute_layout(
                        tree,
                        state,
                        positioned_node,
                        padding_size,
                        static_in_padding_space,
                    );
                    layout.location.x += padding_origin.x;
                    layout.location.y += padding_origin.y;
                    layout.order = order;
                    accumulate_scrollable_overflow(
                        &mut output.content_size,
                        layout.location,
                        layout.size,
                        layout.content_size,
                        tree.style(positioned_node).overflow(),
                    );
                    tree.set_unrounded_layout(state, positioned_node, layout);
                }
                PositionProperty::Fixed => {
                    tree.set_static_position(state, positioned_node, static_in_border_space);
                }
                PositionProperty::Static
                | PositionProperty::Relative
                | PositionProperty::Sticky => {
                    unreachable!("only out-of-flow positions become paragraph markers")
                }
            }
        }
        clear_transparent_inline_nodes(tree, state, &transparent);
        clear_inline_text_nodes(tree, state, node);
        for hidden_node in hidden {
            hide_subtree(tree, state, hidden_node);
        }
    }

    output
}

#[allow(clippy::too_many_arguments)]
fn collect_inline_sources<'dom, T>(
    tree: &'dom TreeArenas<T>,
    node: NodeId,
    inherited_style: &'dom ComputedValues,
    sources: &mut Vec<InlineSource<'dom>>,
    transparent: &mut Vec<NodeId>,
    hidden: &mut Vec<NodeId>,
    atomic_order: &mut u32,
) {
    for child in tree.children(node) {
        let child_ref = slab_get_for_live_node(&tree.nodes, child);
        if child_ref.is_text_node() {
            sources.push(InlineSource::Text {
                text: child_ref.text().unwrap_or_default(),
                style: inherited_style,
            });
            continue;
        }
        if !child_ref.is_element() {
            continue;
        }
        let Some(style) = StyleView::try_of(child_ref) else {
            hidden.push(child);
            continue;
        };
        let mode = display_mode(style.display());
        if mode.is_none() {
            hidden.push(child);
            continue;
        }
        let position = resolve_position(child_ref, style.values());
        if matches!(
            position,
            PositionProperty::Absolute | PositionProperty::Fixed
        ) {
            sources.push(InlineSource::Positioned {
                node: child,
                order: *atomic_order,
                position,
            });
            *atomic_order = atomic_order.saturating_add(1);
            continue;
        }
        if mode.is_contents()
            || (mode.is_flow()
                && mode.is_inline()
                && !child_ref.is_replaced()
                && !style.skips_contents())
        {
            transparent.push(child);
            collect_inline_sources(
                tree,
                child,
                style.values(),
                sources,
                transparent,
                hidden,
                atomic_order,
            );
            continue;
        }
        if mode.is_inline() {
            sources.push(InlineSource::Atomic {
                node: child,
                order: *atomic_order,
            });
            *atomic_order = atomic_order.saturating_add(1);
        } else {
            hidden.push(child);
        }
    }
}

fn clear_transparent_inline_nodes<T>(
    tree: &TreeArenas<T>,
    state: &mut DocumentLayoutState,
    nodes: &[NodeId],
) {
    for &node in nodes {
        tree.clear_layout_cache(state, node);
        tree.set_unrounded_layout(state, node, Layout::default());
    }
}

fn clear_inline_text_nodes<T>(tree: &TreeArenas<T>, state: &mut DocumentLayoutState, root: NodeId) {
    for child in tree.children(root) {
        let child_ref = slab_get_for_live_node(&tree.nodes, child);
        if child_ref.is_text_node() {
            tree.clear_layout_cache(state, child);
            tree.set_unrounded_layout(state, child, Layout::default());
            continue;
        }
        let Some(style) = StyleView::try_of(child_ref) else {
            continue;
        };
        let mode = display_mode(style.display());
        if mode.is_contents() || (mode.is_flow() && mode.is_inline() && !child_ref.is_replaced()) {
            clear_inline_text_nodes(tree, state, child);
        }
    }
}

pub(super) fn run_layout<T: Sync>(
    document: &mut Document<T>,
    viewport: Size<f32>,
    scale: f32,
    full: bool,
) {
    let root = document.document_element().id();
    let parked = collect_parked_boundaries(document);
    let (tree, state, parked_ids) = document.layout_parts();
    for &(_, id, input) in &parked {
        if let Some(node) = tree.nodes.get(id)
            && node.is_element()
            && is_relayout_boundary(&StyleView::of(node))
        {
            let output = compute_boundary_relayout(tree, state, id, input);
            tree.layout_mut(state, id).unrounded.content_size = output.content_size;
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
        let parent_origin = node.flat_parent_id().map_or(Point::ZERO, |parent| {
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
    let mut current = node.flat_parent_id();
    while let Some(id) = current {
        if parked_ids.contains(&id) {
            return true;
        }
        current = slab_get_for_live_node(&tree.nodes, id).flat_parent_id();
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
        let location = tree.layout(state, id).unrounded.location;
        origin = Point::new(origin.x + location.x, origin.y + location.y);
        current = slab_get_for_live_node(&tree.nodes, id).flat_parent_id();
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
    node_id: NodeId,
    viewport: Size<f32>,
) -> bool {
    let node = slab_get_for_live_node(&tree.nodes, node_id);
    let Some(style) = StyleView::try_of(node) else {
        return false;
    };
    let display = display_mode(style.display());
    if display.is_none() {
        return false;
    }
    if display.is_contents() {
        // No box: drop any geometry left from when this element still
        // generated one, so it stays a transparent zero-offset pass-through
        // for the rounding walk and reports an empty box to queries. Its
        // children are real boxes of an ancestor's formatting context, so
        // they still need the hook.
        tree.layout_mut(state, node_id).unrounded = Layout::default();
        return true;
    }
    if node
        .flat_parent_id()
        .and_then(|id| tree.nodes.get(id))
        .is_some_and(Node::is_element)
        && resolve_position(node, style.values()) == PositionProperty::Fixed
    {
        let fixed = style.values().clone_position() == PositionProperty::Fixed;
        position_hoisted(tree, state, node_id, viewport, fixed);
    }
    !display.is_leaf() && !skips_contents(style.values())
}

fn position_hoisted<T: Sync>(
    tree: &TreeArenas<T>,
    state: &mut DocumentLayoutState,
    node_id: NodeId,
    viewport: Size<f32>,
    fixed: bool,
) {
    let node = slab_get_for_live_node(&tree.nodes, node_id);
    let Some(parent_id) = node.flat_parent_id() else {
        return;
    };

    let mut containing = None;
    let mut ancestor = node.flat_parent_id();
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
        ancestor = current.flat_parent_id();
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

    let parent_origin = accumulated_unrounded_origin(tree, state, parent_id);
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
    // Rank against the box siblings the engine ordered this node among, which
    // is the flattened item list of the formatting context it was collected
    // in — not the source child list, when box-less elements intervene.
    let ordering_parent = box_parent(node).map_or(parent_id, Node::id);
    layout.order = sibling_paint_order(tree, ordering_parent, node_id);
    tree.layout_mut(state, node_id).unrounded = layout;
}

fn sibling_paint_order<T>(tree: &TreeArenas<T>, parent_id: NodeId, target: NodeId) -> u32 {
    let Some(target_index) = tree
        .flattened_children(parent_id)
        .position(|(id, ..)| id == target)
    else {
        return 0;
    };
    let target_key = (0_i32, target_index);
    let mut rank = 0u32;
    for (index, (child_id, ..)) in tree.flattened_children(parent_id).enumerate() {
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
            if display_mode(style.display()).is_none() {
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
