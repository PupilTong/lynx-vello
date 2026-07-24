//! Protocol machinery entry points — free generic functions over
//! [`LayoutNode`] handles.
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
    AvailableSpace, Layout, LayoutGoal, LayoutInput, LayoutNode, LayoutOutput, RequestedAxis,
};

pub fn compute_root_layout<N: LayoutNode>(root: N, available_space: Size<AvailableSpace>) {
    let parent_size = available_space.definite_values();
    let output = root.compute_layout(LayoutInput::commit(
        Size::NONE,
        parent_size,
        available_space,
    ));

    let style = root.style();
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
        root.set_unrounded_layout(Layout::default());
        return;
    }

    let mut layout = Layout::with_order(0);
    layout.location = Point::new(margin.left, margin.top);
    layout.size = output.size;
    layout.content_size = output.content_size;
    layout.border = border;
    layout.padding = padding;
    layout.margin = margin;
    root.set_unrounded_layout(layout);
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

pub fn compute_boundary_relayout<N: LayoutNode>(node: N, input: LayoutInput) -> LayoutOutput {
    debug_assert!(
        is_relayout_boundary(&node.style()),
        "compute_boundary_relayout requires a relayout boundary \
         (contain: strict, or a skipped content-visibility box)"
    );
    let mut input = input;
    input.goal = LayoutGoal::Commit;
    node.compute_layout(input)
}

pub fn compute_cached_layout<N, ComputeFn>(
    node: N,
    input: LayoutInput,
    compute_uncached: ComputeFn,
) -> LayoutOutput
where
    N: LayoutNode,
    ComputeFn: FnOnce(N, LayoutInput) -> LayoutOutput,
{
    if let Some(output) = node.cached_layout(input) {
        return output;
    }

    let output = compute_uncached(node, input);
    node.store_cached_layout(input, output);
    output
}

pub fn hide_subtree<N: LayoutNode>(node: N) {
    node.clear_layout_cache();
    node.set_unrounded_layout(Layout::with_order(0));

    for child in node.children() {
        hide_subtree(child);
    }
}

pub fn compute_skipped_contents_layout<N: LayoutNode>(node: N, input: LayoutInput) -> LayoutOutput {
    let style = node.style();
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
        for child in node.children() {
            hide_subtree(child);
        }
    }

    LayoutOutput::new(outer_size, outer_size)
}

#[must_use = "the returned layout is in containing-block space; the host must convert and store it"]
pub fn compute_absolute_layout<N: LayoutNode>(
    node: N,
    containing_block: Size<f32>,
    static_position: Point<f32>,
) -> Layout {
    absolute_layout(
        node,
        containing_block,
        move |_, _| static_position,
        LayoutGoal::Commit,
    )
}

pub(super) fn compute_absolute_layout_with_static_position<N, StaticPosition>(
    node: N,
    containing_block: Size<f32>,
    static_position: StaticPosition,
) -> Layout
where
    N: LayoutNode,
    StaticPosition: FnOnce(Size<f32>, Edges<f32>) -> Point<f32>,
{
    absolute_layout(node, containing_block, static_position, LayoutGoal::Commit)
}

#[must_use]
pub(super) fn measure_absolute_layout<N: LayoutNode>(
    node: N,
    containing_block: Size<f32>,
    requested_axis: RequestedAxis,
) -> Layout {
    absolute_layout(
        node,
        containing_block,
        |_, _| Point::ZERO,
        LayoutGoal::Measure(requested_axis),
    )
}

fn absolute_layout<N, StaticPosition>(
    node: N,
    containing_block: Size<f32>,
    static_position: StaticPosition,
    goal: LayoutGoal,
) -> Layout
where
    N: LayoutNode,
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
    let resolved_style = resolve_absolute_style(node, parent_size);
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
    let child_input = match goal {
        LayoutGoal::Commit => LayoutInput::commit(known_dimensions, parent_size, available_space),
        LayoutGoal::Measure(requested_axis) => LayoutInput::measure(
            known_dimensions,
            parent_size,
            available_space,
            requested_axis,
        ),
    };
    let output = node.compute_layout(child_input);

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
    let ratio_dependent_height = style.aspect_ratio.is_some()
        && horizontal_stretch
        && style.auto_size.width
        && style.auto_size.height;
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

fn resolve_absolute_style<N: LayoutNode>(
    node: N,
    parent_size: Size<Option<f32>>,
) -> ResolvedAbsoluteStyle {
    let style = node.style();
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

pub fn round_layout<N: LayoutNode>(root: N, scale: f32) {
    round_layout_subtree(root, scale, Point::ZERO);
}

pub fn round_layout_subtree<N: LayoutNode>(node: N, scale: f32, parent_position: Point<f32>) {
    round_layout_subtree_with(node, scale, parent_position, |_| false);
}

/// Rounds a subtree after a statically dispatched preorder hook.
/// Returning `false` prunes only later hook calls; rounding still visits descendants.
/// The hook runs before the current unrounded layout is cloned.
#[doc(hidden)]
pub fn round_layout_subtree_with<N: LayoutNode>(
    node: N,
    scale: f32,
    parent_position: Point<f32>,
    mut pre_node: impl FnMut(N) -> bool,
) {
    debug_assert!(
        scale.is_finite() && scale > 0.0,
        "scale must be positive and finite"
    );
    debug_assert!(
        parent_position.x.is_finite() && parent_position.y.is_finite(),
        "accumulated parent position must be finite"
    );
    round_layout_inner(node, scale, parent_position, &mut pre_node, true);
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

fn round_layout_inner<N: LayoutNode>(
    node: N,
    scale: f32,
    parent_position: Point<f32>,
    pre_node: &mut impl FnMut(N) -> bool,
    visit_pre_node: bool,
) {
    let visit_pre_node = visit_pre_node && pre_node(node);
    let unrounded = node.clone_unrounded_layout();
    let position = Point::new(
        parent_position.x + unrounded.location.x,
        parent_position.y + unrounded.location.y,
    );
    let source_size = unrounded.size;
    let source_content_size = unrounded.content_size;
    let source_border = unrounded.border;
    let source_padding = unrounded.padding;
    let source_margin = unrounded.margin;
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
        position.x + source_size.width,
        position.y + source_size.height
    );
    let snapped_content_end = snap_point!(
        position.x + source_content_size.width,
        position.y + source_content_size.height
    );
    let snapped_border_start = snap_point!(
        position.x + source_border.left,
        position.y + source_border.top
    );
    let snapped_border_end = snap_point!(
        position.x + source_size.width - source_border.right,
        position.y + source_size.height - source_border.bottom
    );
    let snapped_padding_start = snap_point!(
        position.x + source_border.left + source_padding.left,
        position.y + source_border.top + source_padding.top
    );
    let snapped_padding_end = snap_point!(
        position.x + source_size.width - source_border.right - source_padding.right,
        position.y + source_size.height - source_border.bottom - source_padding.bottom
    );
    let snapped_margin_start = snap_point!(
        position.x - source_margin.left,
        position.y - source_margin.top
    );
    let snapped_margin_end = snap_point!(
        position.x + source_size.width + source_margin.right,
        position.y + source_size.height + source_margin.bottom
    );
    let mut rounded = unrounded;
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

    node.set_rounded_layout(rounded);

    for child in node.children() {
        round_layout_inner(child, scale, position, pre_node, visit_pre_node);
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

    struct RoundingHost {
        unrounded: Layout,
        rounded: Cell<Option<Layout>>,
    }

    #[derive(Clone, Copy)]
    struct RoundingRef<'a>(&'a RoundingHost);

    impl core::fmt::Debug for RoundingRef<'_> {
        fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
            formatter.write_str("RoundingRef")
        }
    }

    macro_rules! unused_rounding_methods {
        ($($name:ident($($argument:ident: $type:ty),*) $(-> $result:ty)?;)+) => {
            $(
                fn $name(self, $($argument: $type),*) $(-> $result)? {
                    unreachable!("rounding only reads and stores durable layouts")
                }
            )+
        };
    }

    impl LayoutNode for RoundingRef<'_> {
        type Style = &'static RoundingStyle;
        type ChildIter = core::iter::Empty<Self>;

        fn children(self) -> Self::ChildIter {
            core::iter::empty()
        }

        fn style(self) -> Self::Style {
            &RoundingStyle
        }

        fn with_unrounded_layout<R>(self, read: impl FnOnce(&Layout) -> R) -> R {
            read(&self.0.unrounded)
        }

        fn set_rounded_layout(self, layout: Layout) {
            self.0.rounded.set(Some(layout));
        }

        unused_rounding_methods! {
            compute_layout(_input: LayoutInput) -> LayoutOutput;
            set_unrounded_layout(_layout: Layout);
            set_static_position(_position: Point<f32>);
            cached_layout(_input: LayoutInput) -> Option<LayoutOutput>;
            store_cached_layout(_input: LayoutInput, _output: LayoutOutput);
            clear_layout_cache();
        }
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
        let host = RoundingHost {
            unrounded,
            rounded: Cell::new(None),
        };

        ROUND_SNAP_CALLS.set(0);
        round_layout_subtree(RoundingRef(&host), scale, parent_position);
        let actual = host.rounded.take().expect("one durable rounded result");

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
