//! Protocol machinery entry points over a statically split tree and state.
mod flexbox;
mod grid;
mod leaf;
mod linear;
mod relative;
mod single_axis;
mod util;

pub use flexbox::compute_flexbox_layout;
pub use grid::compute_grid_layout;
pub(crate) use leaf::compute_leaf_layout_with_measurement;
#[cfg(feature = "layout-test-utils")]
#[doc(hidden)]
pub use leaf::compute_leaf_layout_with_measurement_for_testing;
pub use leaf::{LeafMeasureInput, LeafMetrics, NaturalSize, compute_leaf_layout};
pub use linear::compute_linear_layout;
pub use relative::compute_relative_layout;
use stylo::computed_values::direction;
use stylo::values::computed::{Margin, Size as StyleSize};

use self::util::{
    apply_box_sizing, auto_edges_to_zero, clamp, clamp_axis, resolve_border, resolve_container_box,
    resolve_insets, resolve_length_percentage, resolve_margins, resolve_max_sizes, resolve_padding,
    resolve_size, style_size_behaves_auto, used_aspect_ratio,
};
use crate::geometry::{Edges, Point, Size};
use crate::invalidate::is_relayout_boundary;
use crate::style::CoreStyle;
use crate::style::containment::contain_intrinsic_length;
use crate::tree::{
    AvailableSpace, Layout, LayoutGoal, LayoutInput, LayoutOutput, LayoutTree, RequestedAxis,
};

pub fn compute_root_layout<T: LayoutTree>(
    tree: &T,
    state: &mut T::State,
    root: T::NodeId,
    available_space: Size<AvailableSpace>,
) {
    let parent_size = available_space.definite_values();
    let mut root_input = LayoutInput::commit(Size::NONE, parent_size, available_space);
    // The viewport imposes the root's constraints; no document content can
    // move them, which is what lets style-imposed sizing chain down from here.
    root_input.content_independent = Size::new(true, true);
    let output = tree.compute_layout(state, root, root_input);

    let style = tree.style(root);
    let margin_value = style.margin();
    let optional_margin = resolve_margins(margin_value, parent_size.width);
    let hidden = style.display().is_none();
    let margin = resolve_root_margins(
        optional_margin,
        margin_value.map(Margin::is_auto),
        available_space.width,
        output.size.width,
    );
    let padding = resolve_padding(style.padding(), parent_size.width);
    let border = resolve_border(&style.border());

    if hidden {
        tree.set_unrounded_layout(state, root, Layout::default());
        return;
    }

    let mut layout = Layout::with_order(0);
    layout.location = Point::new(margin.left, margin.top);
    layout.size = output.size;
    layout.content_size = output.content_size;
    layout.border = border;
    layout.padding = padding;
    layout.margin = margin;
    tree.set_unrounded_layout(state, root, layout);
}

fn resolve_root_margins(
    optional: Edges<Option<f32>>,
    auto: Edges<bool>,
    available_width: AvailableSpace,
    box_width: f32,
) -> Edges<f32> {
    let mut margin = auto_edges_to_zero(optional);
    let AvailableSpace::Definite(available_width) = available_width else {
        return margin;
    };
    let auto_count = usize::from(auto.left) + usize::from(auto.right);
    if auto_count == 0 {
        return margin;
    }
    let remaining = (available_width
        - box_width
        - optional.left.unwrap_or(0.0)
        - optional.right.unwrap_or(0.0))
    .max(0.0);
    let share = if auto_count == 2 {
        remaining / 2.0
    } else {
        remaining
    };
    if auto.left {
        margin.left = share;
    }
    if auto.right {
        margin.right = share;
    }
    margin
}

pub fn compute_boundary_relayout<T: LayoutTree>(
    tree: &T,
    state: &mut T::State,
    node: T::NodeId,
    input: LayoutInput,
) -> LayoutOutput {
    debug_assert!(
        is_relayout_boundary(&tree.style(node)),
        "compute_boundary_relayout requires a relayout boundary \
         (contain: strict, or a skipped content-visibility box)"
    );
    let mut input = input;
    input.goal = LayoutGoal::Commit;
    tree.compute_layout(state, node, input)
}

pub fn compute_cached_layout<T, ComputeFn>(
    tree: &T,
    state: &mut T::State,
    node: T::NodeId,
    input: LayoutInput,
    compute_uncached: ComputeFn,
) -> LayoutOutput
where
    T: LayoutTree,
    ComputeFn: FnOnce(&T, &mut T::State, T::NodeId, LayoutInput) -> LayoutOutput,
{
    if let Some(output) = tree.layout(state, node).cached_layout(input) {
        return output;
    }

    let output = compute_uncached(tree, state, node, input);
    let slot = tree.layout_mut(state, node);
    slot.store_cached_layout(input, output);
    if input.goal == LayoutGoal::Commit {
        // A commit that ran is the one thing that rewrites descendant boxes;
        // a measurement pass reads them and writes none. Marking here is what
        // gives the rounding tail its spine: a commit only reaches a node
        // through an unbroken chain of committing ancestors.
        slot.mark_subtree_dirty();
    }
    output
}

pub fn hide_subtree<T: LayoutTree>(tree: &T, state: &mut T::State, node: T::NodeId) {
    hide_subtree_at_order(tree, state, node, 0);
}

/// Hides a `display: none` child's subtree while keeping its paint-order slot.
pub(super) fn hide_child_at_order<T: LayoutTree>(
    tree: &T,
    state: &mut T::State,
    node: T::NodeId,
    order: u32,
) {
    hide_subtree_at_order(tree, state, node, order);
}

fn hide_subtree_at_order<T: LayoutTree>(
    tree: &T,
    state: &mut T::State,
    node: T::NodeId,
    order: u32,
) {
    if tree.layout(state, node).is_hidden() {
        // The subtree below is already zeroed and stays that way; only the
        // paint-order slot this box keeps among its siblings can still move.
        tree.layout_mut(state, node).set_hidden_order(order);
        return;
    }
    tree.clear_layout_cache(state, node);
    tree.set_unrounded_layout(state, node, Layout::with_order(order));
    tree.layout_mut(state, node).mark_hidden();

    for child in tree.children(node) {
        hide_subtree_at_order(tree, state, child, 0);
    }
}

pub fn compute_skipped_contents_layout<T: LayoutTree>(
    tree: &T,
    state: &mut T::State,
    node: T::NodeId,
    input: LayoutInput,
) -> LayoutOutput {
    let style = tree.style(node);
    let metrics = resolve_container_box(&style, input);
    let intrinsic = Size::new(
        contain_intrinsic_length(&style.contain_intrinsic_width()),
        contain_intrinsic_length(&style.contain_intrinsic_height()),
    );
    let outer_size = Size::new(
        metrics.outer.width.unwrap_or_else(|| {
            clamp_axis(
                intrinsic.width.unwrap_or(0.0) + metrics.box_inset.width,
                metrics.min.width,
                metrics.max.width,
                metrics.box_inset.width,
            )
        }),
        metrics.outer.height.unwrap_or_else(|| {
            clamp_axis(
                intrinsic.height.unwrap_or(0.0) + metrics.box_inset.height,
                metrics.min.height,
                metrics.max.height,
                metrics.box_inset.height,
            )
        }),
    );

    if input.goal == LayoutGoal::Commit {
        for child in tree.children(node) {
            hide_subtree(tree, state, child);
        }
    }

    LayoutOutput::new(outer_size, outer_size)
}

#[must_use = "the returned layout is in containing-block space; the host must convert and store it"]
pub fn compute_absolute_layout<T: LayoutTree>(
    tree: &T,
    state: &mut T::State,
    node: T::NodeId,
    containing_block: Size<f32>,
    static_position: Point<f32>,
) -> Layout {
    compute_absolute_layout_with_static_position(
        tree,
        state,
        node,
        containing_block,
        move |_, _| static_position,
    )
}

pub(super) fn compute_absolute_layout_with_static_position<T, StaticPosition>(
    tree: &T,
    state: &mut T::State,
    node: T::NodeId,
    containing_block: Size<f32>,
    static_position: StaticPosition,
) -> Layout
where
    T: LayoutTree,
    StaticPosition: FnOnce(Size<f32>, Edges<f32>) -> Point<f32>,
{
    absolute_layout(
        tree,
        state,
        node,
        containing_block,
        static_position,
        LayoutGoal::Commit,
    )
}

#[must_use]
pub(super) fn measure_absolute_layout<T: LayoutTree>(
    tree: &T,
    state: &mut T::State,
    node: T::NodeId,
    containing_block: Size<f32>,
    requested_axis: RequestedAxis,
) -> Layout {
    absolute_layout(
        tree,
        state,
        node,
        containing_block,
        |_, _| Point::ZERO,
        LayoutGoal::Measure(requested_axis),
    )
}

fn absolute_layout<T, StaticPosition>(
    tree: &T,
    state: &mut T::State,
    node: T::NodeId,
    containing_block: Size<f32>,
    static_position: StaticPosition,
    goal: LayoutGoal,
) -> Layout
where
    T: LayoutTree,
    StaticPosition: FnOnce(Size<f32>, Edges<f32>) -> Point<f32>,
{
    debug_assert!(
        containing_block.width.is_finite()
            && containing_block.height.is_finite()
            && containing_block.width >= 0.0
            && containing_block.height >= 0.0,
        "containing-block sizes must be finite and non-negative"
    );
    let parent_size = Size::new(Some(containing_block.width), Some(containing_block.height));
    let resolved_style = resolve_absolute_style(tree, node, parent_size);
    let ResolvedAbsoluteStyle {
        insets,
        optional_margin,
        padding,
        border,
        preferred_available,
        direction,
        ..
    } = resolved_style;

    let fixed_margin = auto_edges_to_zero(optional_margin);
    let inset_modified_size = Size::new(
        (containing_block.width - insets.left.unwrap_or(0.0) - insets.right.unwrap_or(0.0))
            .max(0.0),
        (containing_block.height - insets.top.unwrap_or(0.0) - insets.bottom.unwrap_or(0.0))
            .max(0.0),
    );

    let known_dimensions =
        absolute_known_dimensions(&resolved_style, inset_modified_size, fixed_margin);
    let available_space = Size::new(
        preferred_available
            .width
            .unwrap_or(AvailableSpace::Definite(inset_modified_size.width)),
        preferred_available
            .height
            .unwrap_or(AvailableSpace::Definite(inset_modified_size.height)),
    );
    let mut child_input = match goal {
        LayoutGoal::Commit => LayoutInput::commit(known_dimensions, parent_size, available_space),
        LayoutGoal::Measure(requested_axis) => LayoutInput::measure(
            known_dimensions,
            parent_size,
            available_space,
            requested_axis,
        ),
    };
    // Every field above is a function of the containing block and the box's
    // own style — an out-of-flow box is never measured to build its own input.
    // And its content cannot move the containing block back: it contributes to
    // no ancestor's used size, only to their scrollable overflow, which is an
    // output the in-place path compares before it trusts anything.
    child_input.content_independent = Size::new(true, true);
    let output = tree.compute_layout(state, node, child_input);

    let margin = resolve_absolute_margins(
        optional_margin,
        insets,
        inset_modified_size,
        output.size,
        direction,
    );
    let static_position = static_position(output.size, margin);
    debug_assert!(
        static_position.x.is_finite() && static_position.y.is_finite(),
        "static positions must be finite"
    );
    let location = Point::new(
        absolute_axis_location(AbsoluteAxis {
            containing_size: containing_block.width,
            box_size: output.size.width,
            start_inset: insets.left,
            end_inset: insets.right,
            start_margin: margin.left,
            end_margin: margin.right,
            static_position: static_position.x,
            prefer_end: direction == direction::T::Rtl,
        }),
        absolute_axis_location(AbsoluteAxis {
            containing_size: containing_block.height,
            box_size: output.size.height,
            start_inset: insets.top,
            end_inset: insets.bottom,
            start_margin: margin.top,
            end_margin: margin.bottom,
            static_position: static_position.y,
            prefer_end: false,
        }),
    );

    let mut layout = Layout::with_order(0);
    layout.location = location;
    layout.size = output.size;
    layout.content_size = output.content_size;
    layout.border = border;
    layout.padding = padding;
    layout.margin = margin;
    layout
}

/// Resolved box-model and positioning inputs retained across the recursive
/// child-layout call for one absolutely positioned node.
#[derive(Clone, Copy)]
struct ResolvedAbsoluteStyle {
    insets: Edges<Option<f32>>,
    optional_margin: Edges<Option<f32>>,
    padding: Edges<f32>,
    border: Edges<f32>,
    preferred_available: Size<Option<AvailableSpace>>,
    auto_size: Size<bool>,
    min_size: Size<Option<f32>>,
    max_size: Size<Option<f32>>,
    aspect_ratio: Option<f32>,
    direction: direction::T,
    padding_border_size: Size<f32>,
}

fn absolute_known_dimensions(
    style: &ResolvedAbsoluteStyle,
    inset_modified_size: Size<f32>,
    fixed_margin: Edges<f32>,
) -> Size<Option<f32>> {
    let horizontal_stretch =
        style.auto_size.width && style.insets.left.is_some() && style.insets.right.is_some();
    let ratio_dependent_height =
        style.aspect_ratio.is_some() && horizontal_stretch && style.auto_size.height;
    Size::new(
        horizontal_stretch.then_some(
            clamp(
                inset_modified_size.width - fixed_margin.horizontal_sum(),
                style.min_size.width,
                style.max_size.width,
            )
            .max(style.padding_border_size.width),
        ),
        (style.auto_size.height
            && !ratio_dependent_height
            && style.insets.top.is_some()
            && style.insets.bottom.is_some())
        .then_some(
            clamp(
                inset_modified_size.height - fixed_margin.vertical_sum(),
                style.min_size.height,
                style.max_size.height,
            )
            .max(style.padding_border_size.height),
        ),
    )
}

fn resolve_absolute_style<T: LayoutTree>(
    tree: &T,
    node: T::NodeId,
    parent_size: Size<Option<f32>>,
) -> ResolvedAbsoluteStyle {
    let style = tree.style(node);
    let padding = resolve_padding(style.padding(), parent_size.width);
    let border = resolve_border(&style.border());
    let padding_border_size = Size::new(
        padding.horizontal_sum() + border.horizontal_sum(),
        padding.vertical_sum() + border.vertical_sum(),
    );
    let style_size = style.size();
    let preferred_available = Size::new(
        absolute_preferred_available(style_size.width, parent_size.width),
        absolute_preferred_available(style_size.height, parent_size.height),
    );
    let resolved_style_size = apply_box_sizing(
        resolve_size(style_size, parent_size),
        style.box_sizing(),
        padding_border_size,
    );
    let min_size = apply_box_sizing(
        resolve_size(style.min_size(), parent_size),
        style.box_sizing(),
        padding_border_size,
    );
    let max_size = apply_box_sizing(
        resolve_max_sizes(style.max_size(), parent_size),
        style.box_sizing(),
        padding_border_size,
    );

    ResolvedAbsoluteStyle {
        insets: resolve_insets(style.inset(), parent_size),
        optional_margin: resolve_margins(style.margin(), parent_size.width),
        padding,
        border,
        preferred_available,
        auto_size: Size::new(
            style_size_behaves_auto(style_size.width) && resolved_style_size.width.is_none(),
            style_size_behaves_auto(style_size.height) && resolved_style_size.height.is_none(),
        ),
        min_size,
        max_size,
        aspect_ratio: used_aspect_ratio(style.aspect_ratio()),
        direction: style.direction(),
        padding_border_size,
    }
}

#[inline]
fn absolute_preferred_available(value: &StyleSize, basis: Option<f32>) -> Option<AvailableSpace> {
    match value {
        StyleSize::MinContent => Some(AvailableSpace::MinContent),
        StyleSize::MaxContent => Some(AvailableSpace::MaxContent),
        StyleSize::FitContentFunction(limit) => resolve_length_percentage(&limit.0, basis)
            .map(|limit| AvailableSpace::Definite(limit.max(0.0))),
        StyleSize::LengthPercentage(_)
        | StyleSize::Auto
        | StyleSize::FitContent
        | StyleSize::Stretch
        | StyleSize::WebkitFillAvailable => None,
        StyleSize::AnchorSizeFunction(_) | StyleSize::AnchorContainingCalcFunction(_) => {
            unreachable!("anchor sizing is pref-dead under the lynx feature")
        }
    }
}

pub fn round_layout<T: LayoutTree>(tree: &T, state: &mut T::State, root: T::NodeId, scale: f32) {
    round_layout_subtree(tree, state, root, scale, Point::ZERO);
}

pub fn round_layout_subtree<T: LayoutTree>(
    tree: &T,
    state: &mut T::State,
    node: T::NodeId,
    scale: f32,
    parent_position: Point<f32>,
) {
    round_layout_subtree_with(
        tree,
        state,
        node,
        scale,
        parent_position,
        true,
        |_, _, _| false,
    );
}

/// Rounds a subtree after a statically dispatched preorder hook.
/// Returning `false` prunes only later hook calls; rounding still visits descendants.
/// The hook runs before the current unrounded layout is cloned.
///
/// `rescale` says the rounding function itself changed under the caller — a new
/// device scale, or a viewport that moves boxes no layout write touched — and
/// forces the whole subtree. Otherwise the walk descends only where a box was
/// written since the last rounding: a changed node carries its whole subtree
/// with it, because everything under it shifts with its origin and anything
/// positioned against it resolves against its new size.
#[doc(hidden)]
pub fn round_layout_subtree_with<T: LayoutTree>(
    tree: &T,
    state: &mut T::State,
    node: T::NodeId,
    scale: f32,
    parent_position: Point<f32>,
    rescale: bool,
    mut pre_node: impl FnMut(&T, &mut T::State, T::NodeId) -> bool,
) {
    debug_assert!(
        scale.is_finite() && scale > 0.0,
        "scale must be positive and finite"
    );
    debug_assert!(
        parent_position.x.is_finite() && parent_position.y.is_finite(),
        "accumulated parent position must be finite"
    );
    round_layout_inner(
        tree,
        state,
        node,
        scale,
        parent_position,
        &mut pre_node,
        true,
        rescale,
    );
}

#[inline]
fn css_round_to_integer(value: f32) -> f32 {
    debug_assert!(value.is_finite(), "CSS pixel coordinates must be finite");
    let lower = value.floor();
    if value - lower < 0.5 {
        lower
    } else {
        lower + 1.0
    }
}

fn resolve_absolute_margins(
    optional: Edges<Option<f32>>,
    insets: Edges<Option<f32>>,
    available: Size<f32>,
    size: Size<f32>,
    direction: direction::T,
) -> Edges<f32> {
    let mut margin = auto_edges_to_zero(optional);

    if insets.left.is_some() && insets.right.is_some() {
        let remaining = available.width
            - size.width
            - optional.left.unwrap_or(0.0)
            - optional.right.unwrap_or(0.0);
        match (optional.left.is_none(), optional.right.is_none()) {
            (true, true) if remaining < 0.0 && direction == direction::T::Rtl => {
                margin.left = remaining;
            }
            (true, true) if remaining < 0.0 => margin.right = remaining,
            (true, true) => {
                margin.left = remaining / 2.0;
                margin.right = remaining / 2.0;
            }
            (true, false) => margin.left = remaining,
            (false, true) => margin.right = remaining,
            (false, false) => {}
        }
    }

    if insets.top.is_some() && insets.bottom.is_some() {
        let remaining = available.height
            - size.height
            - optional.top.unwrap_or(0.0)
            - optional.bottom.unwrap_or(0.0);
        match (optional.top.is_none(), optional.bottom.is_none()) {
            (true, true) => {
                margin.top = remaining / 2.0;
                margin.bottom = remaining / 2.0;
            }
            (true, false) => margin.top = remaining,
            (false, true) => margin.bottom = remaining,
            (false, false) => {}
        }
    }

    margin
}

#[inline]
fn absolute_axis_location(axis: AbsoluteAxis) -> f32 {
    let AbsoluteAxis {
        containing_size,
        box_size,
        start_inset,
        end_inset,
        start_margin,
        end_margin,
        static_position,
        prefer_end,
    } = axis;
    match (start_inset, end_inset) {
        (None, None) => static_position + start_margin,
        (Some(_), Some(end)) if prefer_end => containing_size - end - box_size - end_margin,
        (Some(start), _) => start + start_margin,
        (None, Some(end)) => containing_size - end - box_size - end_margin,
    }
}

/// One physical-axis instance of the absolute-position equation used to turn
/// insets, used margins, and the static fallback into a border-box offset.
#[derive(Clone, Copy)]
struct AbsoluteAxis {
    containing_size: f32,
    box_size: f32,
    start_inset: Option<f32>,
    end_inset: Option<f32>,
    start_margin: f32,
    end_margin: f32,
    static_position: f32,
    prefer_end: bool,
}

fn rounded_layout(
    source: &Layout,
    scale: f32,
    parent_position: Point<f32>,
) -> (Layout, Point<f32>) {
    let position = Point::new(
        parent_position.x + source.location.x,
        parent_position.y + source.location.y,
    );
    let mut snap = |value: f32| {
        #[cfg(test)]
        tests::ROUND_SNAP_CALLS.with(|calls| calls.set(calls.get() + 1));
        css_round_to_integer(value * scale) / scale
    };
    macro_rules! snap_point {
        ($x:expr, $y:expr) => {
            Point::new($x, $y).map(&mut snap)
        };
    }
    let snapped_parent_position = parent_position.map(&mut snap);
    let snapped_position = position.map(&mut snap);
    let snapped_box_end = snap_point!(
        position.x + source.size.width,
        position.y + source.size.height
    );
    let snapped_content_end = snap_point!(
        position.x + source.content_size.width,
        position.y + source.content_size.height
    );
    let snapped_border_start = snap_point!(
        position.x + source.border.left,
        position.y + source.border.top
    );
    let snapped_border_end = snap_point!(
        position.x + source.size.width - source.border.right,
        position.y + source.size.height - source.border.bottom
    );
    let snapped_padding_start = snap_point!(
        position.x + source.border.left + source.padding.left,
        position.y + source.border.top + source.padding.top
    );
    let snapped_padding_end = snap_point!(
        position.x + source.size.width - source.border.right - source.padding.right,
        position.y + source.size.height - source.border.bottom - source.padding.bottom
    );
    let snapped_margin_start = snap_point!(
        position.x - source.margin.left,
        position.y - source.margin.top
    );
    let snapped_margin_end = snap_point!(
        position.x + source.size.width + source.margin.right,
        position.y + source.size.height + source.margin.bottom
    );
    let mut rounded = Layout::with_order(source.order);
    rounded.location = Point::new(
        snapped_position.x - snapped_parent_position.x,
        snapped_position.y - snapped_parent_position.y,
    );
    rounded.size = Size::new(
        snapped_box_end.x - snapped_position.x,
        snapped_box_end.y - snapped_position.y,
    );
    rounded.content_size = Size::new(
        snapped_content_end.x - snapped_position.x,
        snapped_content_end.y - snapped_position.y,
    );
    rounded.border.left = snapped_border_start.x - snapped_position.x;
    rounded.border.right = snapped_box_end.x - snapped_border_end.x;
    rounded.border.top = snapped_border_start.y - snapped_position.y;
    rounded.border.bottom = snapped_box_end.y - snapped_border_end.y;
    rounded.padding.left = snapped_padding_start.x - snapped_border_start.x;
    rounded.padding.right = snapped_border_end.x - snapped_padding_end.x;
    rounded.padding.top = snapped_padding_start.y - snapped_border_start.y;
    rounded.padding.bottom = snapped_border_end.y - snapped_padding_end.y;
    rounded.margin.left = snapped_position.x - snapped_margin_start.x;
    rounded.margin.right = snapped_margin_end.x - snapped_box_end.x;
    rounded.margin.top = snapped_position.y - snapped_margin_start.y;
    rounded.margin.bottom = snapped_margin_end.y - snapped_box_end.y;

    (rounded, position)
}

#[allow(
    clippy::fn_params_excessive_bools,
    clippy::too_many_arguments,
    reason = "the extra parameters are propagation state of one recursion, not an API surface"
)]
fn round_layout_inner<T: LayoutTree>(
    tree: &T,
    state: &mut T::State,
    node: T::NodeId,
    scale: f32,
    parent_position: Point<f32>,
    pre_node: &mut impl FnMut(&T, &mut T::State, T::NodeId) -> bool,
    visit_pre_node: bool,
    ancestor_moved: bool,
) {
    if !ancestor_moved && !tree.layout(state, node).needs_rounding() {
        // Nothing under here was written since the last rounding, and it
        // rounds from the same accumulated position, so every `rounded` box in
        // this subtree already holds the value this walk would recompute.
        return;
    }
    let visit_pre_node = visit_pre_node && pre_node(tree, state, node);
    let (rounded, position) =
        rounded_layout(&tree.layout(state, node).unrounded, scale, parent_position);
    let slot = tree.layout_mut(state, node);
    // The hook writes the boxes of out-of-flow children here rather than
    // during layout, so read the mark after it has run.
    let moved = ancestor_moved || slot.box_changed();
    slot.rounded = rounded;
    slot.clear_rounding_marks();

    for child in tree.children(node) {
        round_layout_inner(
            tree,
            state,
            child,
            scale,
            position,
            pre_node,
            visit_pre_node,
            moved,
        );
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::float_cmp)]
mod tests {
    use core::cell::Cell;

    use stylo::values::computed::Display;

    use super::*;

    std::thread_local! {
        pub(super) static ROUND_SNAP_CALLS: Cell<usize> = const { Cell::new(0) };
    }

    fn edges<T>(left: T, right: T, top: T, bottom: T) -> Edges<T> {
        Edges {
            left,
            right,
            top,
            bottom,
        }
    }

    macro_rules! assert_cases {
        ($function:ident; $(
            $name:literal: ($($argument:expr),+ $(,)?) => $expected:expr;
        )+) => {
            $(assert_eq!($function($($argument),+), $expected, "{}", $name);)+
        };
    }

    #[derive(Debug)]
    struct RoundingStyle;

    impl CoreStyle for RoundingStyle {
        fn display(&self) -> Display {
            Display::Flex
        }
    }

    struct RoundingTree;

    impl LayoutTree for RoundingTree {
        type NodeId = ();
        type State = crate::tree::LayoutSlot;
        type Style<'tree> = &'static RoundingStyle;
        type ChildIter<'tree> = core::iter::Empty<()>;

        fn children(&self, (): ()) -> Self::ChildIter<'_> {
            core::iter::empty()
        }

        fn style(&self, (): ()) -> Self::Style<'_> {
            &RoundingStyle
        }

        fn layout<'state>(
            &self,
            state: &'state Self::State,
            (): (),
        ) -> &'state crate::tree::LayoutSlot {
            state
        }

        fn layout_mut<'state>(
            &self,
            state: &'state mut Self::State,
            (): (),
        ) -> &'state mut crate::tree::LayoutSlot {
            state
        }

        fn compute_layout(
            &self,
            _state: &mut Self::State,
            (): (),
            _input: LayoutInput,
        ) -> LayoutOutput {
            unreachable!("rounding does not compute box layouts")
        }
    }

    /// Two branches under one root, each with a leaf. The "algorithm" is a
    /// commit that writes every child's box, so a pass over it leaves exactly
    /// the marks a real one would.
    struct BranchTree;

    impl BranchTree {
        const CHILDREN: [&'static [usize]; 5] = [&[1, 3], &[2], &[], &[4], &[]];

        fn box_of(node: usize) -> Layout {
            let mut layout = Layout::with_order(0);
            #[allow(clippy::cast_precision_loss)]
            let index = node as f32;
            layout.location = Point::new(0.0, index * 10.0);
            layout.size = Size::new(50.0, 20.0);
            layout
        }

        fn commit_input() -> LayoutInput {
            LayoutInput::commit(
                Size::new(Some(50.0), Some(20.0)),
                Size::NONE,
                Size::new(
                    AvailableSpace::Definite(50.0),
                    AvailableSpace::Definite(20.0),
                ),
            )
        }
    }

    impl LayoutTree for BranchTree {
        type NodeId = usize;
        type State = Vec<crate::tree::LayoutSlot>;
        type Style<'tree> = &'static RoundingStyle;
        type ChildIter<'tree> = core::iter::Copied<core::slice::Iter<'tree, usize>>;

        fn children(&self, node: usize) -> Self::ChildIter<'_> {
            Self::CHILDREN[node].iter().copied()
        }

        fn style(&self, _node: usize) -> Self::Style<'_> {
            &RoundingStyle
        }

        fn layout<'state>(
            &self,
            state: &'state Self::State,
            node: usize,
        ) -> &'state crate::tree::LayoutSlot {
            &state[node]
        }

        fn layout_mut<'state>(
            &self,
            state: &'state mut Self::State,
            node: usize,
        ) -> &'state mut crate::tree::LayoutSlot {
            &mut state[node]
        }

        fn compute_layout(
            &self,
            state: &mut Self::State,
            node: usize,
            input: LayoutInput,
        ) -> LayoutOutput {
            compute_cached_layout(self, state, node, input, |tree, state, node, _input| {
                for child in tree.children(node) {
                    tree.compute_layout(state, child, Self::commit_input());
                    tree.set_unrounded_layout(state, child, Self::box_of(child));
                }
                LayoutOutput::new(Size::new(50.0, 20.0), Size::new(50.0, 20.0))
            })
        }
    }

    fn run_branch_pass(tree: &BranchTree, state: &mut Vec<crate::tree::LayoutSlot>) {
        tree.compute_layout(state, 0, BranchTree::commit_input());
        tree.set_unrounded_layout(state, 0, BranchTree::box_of(0));
    }

    fn round_branches(tree: &BranchTree, state: &mut Vec<crate::tree::LayoutSlot>) -> Vec<usize> {
        let mut visited = Vec::new();
        round_layout_subtree_with(tree, state, 0, 1.0, Point::ZERO, false, |_, _, node| {
            visited.push(node);
            true
        });
        visited
    }

    #[test]
    fn the_rounding_tail_visits_only_the_dirty_spine_and_what_moved() {
        let tree = BranchTree;
        let mut state = (0..5).map(|_| crate::tree::LayoutSlot::default()).collect();

        run_branch_pass(&tree, &mut state);
        assert_eq!(
            round_branches(&tree, &mut state),
            vec![0, 1, 2, 3, 4],
            "the first pass writes every box, so the tail owes every node",
        );

        run_branch_pass(&tree, &mut state);
        assert!(
            round_branches(&tree, &mut state).is_empty(),
            "a pass that rewrote nothing leaves the tail nothing to do",
        );

        // What a host's invalidation does: clear the mutated node and the
        // spine above it, leaving the other branch's caches alone.
        for node in [2, 1, 0] {
            tree.clear_layout_cache(&mut state, node);
        }
        run_branch_pass(&tree, &mut state);
        assert_eq!(
            round_branches(&tree, &mut state),
            vec![0, 1, 2],
            "only the recomputed spine is walked; the untouched branch is skipped",
        );

        // A box that actually moved carries its whole subtree with it: every
        // descendant rounds from a new accumulated position. Both marks come
        // from the same place a pass would leave them — the parent ran, and
        // wrote one child a different box.
        let mut moved = BranchTree::box_of(3);
        moved.location.y += 4.0;
        tree.layout_mut(&mut state, 0).mark_subtree_dirty();
        tree.set_unrounded_layout(&mut state, 3, moved);
        assert_eq!(
            round_branches(&tree, &mut state),
            vec![0, 3, 4],
            "the moved child pulls its subtree in; its unmoved sibling stays out",
        );
    }

    #[test]
    fn hiding_an_already_hidden_subtree_costs_nothing() {
        let tree = BranchTree;
        let mut state: Vec<crate::tree::LayoutSlot> =
            (0..5).map(|_| crate::tree::LayoutSlot::default()).collect();
        run_branch_pass(&tree, &mut state);
        let _ = round_branches(&tree, &mut state);

        // Hiding runs from the parent's own commit, which is what puts the
        // spine mark on the parent.
        tree.layout_mut(&mut state, 0).mark_subtree_dirty();
        hide_subtree(&tree, &mut state, 1);
        assert_eq!(
            round_branches(&tree, &mut state),
            vec![0, 1, 2],
            "hiding zeroes the subtree's boxes, which the tail has to pick up",
        );

        tree.layout_mut(&mut state, 0).mark_subtree_dirty();
        hide_subtree(&tree, &mut state, 1);
        assert_eq!(
            round_branches(&tree, &mut state),
            vec![0],
            "hiding an already-hidden subtree writes nothing the tail must chase",
        );

        // Laying the node out again takes the subtree back out of hiding.
        tree.set_unrounded_layout(&mut state, 1, BranchTree::box_of(1));
        assert!(!tree.layout(&state, 1).is_hidden());
    }

    #[test]
    fn rounding_reuses_twenty_unique_snaps_without_changing_bits() {
        let unrounded = Layout {
            order: 17,
            location: Point::new(0.37, -0.42),
            size: Size::new(20.18, 13.73),
            content_size: Size::new(24.91, 15.09),
            border: edges(1.13, 2.27, 0.77, 1.91),
            padding: edges(3.08, 0.66, 2.42, 1.36),
            margin: edges(4.17, -0.83, 1.27, 3.44),
        };
        let scale = 1.25;
        let parent_position = Point::new(-7.31, 5.19);
        let expected = Layout {
            order: 17,
            location: Point::ZERO,
            size: Size::new(20.8, 13.599_999),
            content_size: Size::new(24.8, 15.2),
            border: edges(1.599_999_9, 2.400_000_6, 0.799_999_7, 1.600_000_4),
            padding: edges(3.199_999_8, 0.800_000_2, 2.4, 1.599_999_4),
            margin: edges(4.0, -0.800_000_2, 1.600_000_1, 3.200_000_8),
        };
        let tree = RoundingTree;
        let mut state = crate::tree::LayoutSlot::default();
        state.unrounded = unrounded;

        ROUND_SNAP_CALLS.set(0);
        round_layout_subtree(&tree, &mut state, (), scale, parent_position);
        let actual = &state.rounded;

        assert_eq!(ROUND_SNAP_CALLS.get(), 20);
        assert_eq!(actual.order, expected.order);
        macro_rules! assert_field_bits {
            ($($field:ident),+ $(,)?) => {
                $(assert_eq!(
                    actual.$field.map(f32::to_bits),
                    expected.$field.map(f32::to_bits),
                    stringify!($field),
                );)+
            };
        }
        assert_field_bits!(location, size, content_size, border, padding, margin);
    }

    #[test]
    fn root_auto_margins_cover_indefinite_fixed_single_and_double_auto_cases() {
        let fixed = edges(Some(3.0), Some(7.0), Some(2.0), Some(4.0));
        let expected_fixed = edges(3.0, 7.0, 2.0, 4.0);
        let definite = AvailableSpace::Definite(100.0);
        assert_cases! { resolve_root_margins;
            "fixed indefinite":
                (fixed, Edges::uniform(false), AvailableSpace::MaxContent, 40.0) => expected_fixed;
            "fixed definite":
                (fixed, Edges::uniform(false), definite, 40.0) => expected_fixed;
            "both horizontal auto":
                (Edges::uniform(None), edges(true, true, false, false), definite, 40.0)
                => edges(30.0, 30.0, 0.0, 0.0);
            "right auto":
                (edges(Some(5.0), None, None, None), edges(false, true, false, false),
                 definite, 40.0) => edges(5.0, 55.0, 0.0, 0.0);
        }
    }

    fn absolute_style() -> ResolvedAbsoluteStyle {
        ResolvedAbsoluteStyle {
            insets: Edges::uniform(Some(0.0)),
            optional_margin: Edges::uniform(None),
            padding: Edges::ZERO,
            border: Edges::ZERO,
            preferred_available: Size::new(None, None),
            auto_size: Size::new(true, true),
            min_size: Size::new(Some(20.0), Some(10.0)),
            max_size: Size::new(Some(90.0), Some(60.0)),
            aspect_ratio: None,
            direction: direction::T::Ltr,
            padding_border_size: Size::new(8.0, 6.0),
        }
    }

    #[test]
    fn absolute_known_dimensions_clamp_stretch_and_defer_ratio_height() {
        let style = absolute_style();
        let mut ratio = style;
        ratio.aspect_ratio = Some(2.0);
        let mut vertical_only = style;
        vertical_only.auto_size.width = false;
        assert_cases! { absolute_known_dimensions;
            "stretch clamp":
                (&style, Size::new(100.0, 80.0), Edges::uniform(5.0))
                => Size::new(Some(90.0), Some(60.0));
            "ratio defers height":
                (&ratio, Size::new(100.0, 80.0), Edges::uniform(5.0))
                => Size::new(Some(90.0), None);
            "vertical only clamps minimum":
                (&vertical_only, Size::new(100.0, 30.0), Edges::uniform(20.0))
                => Size::new(None, Some(10.0));
        }
    }

    #[test]
    fn absolute_auto_margins_cover_positive_negative_and_one_sided_equations() {
        use direction::T::{Ltr, Rtl};

        let insets = Edges::uniform(Some(0.0));
        let all_auto = Edges::uniform(None);
        let normal = Size::new(100.0, 80.0);
        let overflow = Size::new(40.0, 80.0);
        let box_size = Size::new(60.0, 40.0);
        assert_cases! { resolve_absolute_margins;
            "centered":
                (all_auto, insets, normal, box_size, Ltr) => Edges::uniform(20.0);
            "ltr overflow":
                (all_auto, insets, overflow, box_size, Ltr)
                => edges(0.0, -20.0, 20.0, 20.0);
            "rtl overflow":
                (all_auto, insets, overflow, box_size, Rtl)
                => edges(-20.0, 0.0, 20.0, 20.0);
            "start edges auto":
                (edges(None, Some(3.0), None, Some(4.0)), insets, normal, box_size, Ltr)
                => edges(37.0, 3.0, 36.0, 4.0);
            "end edges auto":
                (edges(Some(2.0), None, Some(5.0), None), insets, normal, box_size, Ltr)
                => edges(2.0, 38.0, 5.0, 35.0);
        }
    }

    #[test]
    fn absolute_axis_location_covers_static_start_end_and_rtl_preference() {
        let base = AbsoluteAxis {
            containing_size: 100.0,
            box_size: 20.0,
            start_inset: None,
            end_inset: None,
            start_margin: 3.0,
            end_margin: 4.0,
            static_position: 11.0,
            prefer_end: false,
        };
        let mut start = base;
        start.start_inset = Some(7.0);
        let mut end = base;
        end.end_inset = Some(9.0);
        let mut prefer_end = start;
        prefer_end.end_inset = Some(9.0);
        prefer_end.prefer_end = true;
        assert_cases! { absolute_axis_location;
            "static position": (base) => 14.0;
            "start inset": (start) => 10.0;
            "end inset": (end) => 67.0;
            "prefer end with both insets": (prefer_end) => 67.0;
        }
    }
}
