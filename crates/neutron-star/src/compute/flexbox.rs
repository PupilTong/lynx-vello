//! CSS Flexible Box Layout Module Level 1 layout algorithm.
//!
//! The implementation follows the pass ordering in Flexbox §9. Style and
//! topology stay in an immutable [`FlexSource`], while recursive measurement
//! and durable writes go through a separate [`LayoutSession`]. This separation
//! lets borrowed GAT style views remain live across child layout without raw
//! style snapshots or self-referential scratch structures.
//!
//! The current protocol deliberately leaves formatting-tree preprocessing
//! (anonymous item generation) to the host and has no representation for
//! `visibility: collapse`, `flex-basis: content`, replaced-vs-non-replaced
//! automatic minimums, or non-horizontal writing modes. The algorithm is
//! spec-oriented over the representable surface; ordinary items use the
//! non-replaced §4.5 automatic-minimum rule.

// Item and line counts are transient Vec lengths. A flex container cannot
// practically approach f32's exact-integer limit, while alignment division
// necessarily operates in the engine's f32 coordinate space.
#![allow(clippy::cast_precision_loss)]

use core::cmp::Ordering;

use super::compute_absolute_layout;
use super::util::{
    apply_aspect_ratio, apply_box_sizing, auto_edges_to_zero, clamp, resolve_dimension,
    resolve_edges, resolve_insets, resolve_length_percentage, resolve_optional_edges, resolve_size,
    subtract_available_space,
};
use crate::geometry::{Edges, Point, Size};
use crate::style::alignment::{AlignContent, AlignItems};
use crate::style::value::{CalcHandle, Dimension, LengthPercentageAuto};
use crate::style::{
    BoxGenerationMode, BoxSizing, CoreStyle, Direction, FlexContainerStyle, FlexDirection,
    FlexItemStyle, FlexWrap, Overflow, Position,
};
use crate::tree::{
    AvailableSpace, FlexSource, Layout, LayoutGoal, LayoutInput, LayoutOutput, LayoutSession,
    NodeId, RequestedAxis, SizingMode,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Axis {
    Horizontal,
    Vertical,
}

impl Axis {
    #[inline]
    fn size<T: Copy>(self, size: Size<T>) -> T {
        match self {
            Self::Horizontal => size.width,
            Self::Vertical => size.height,
        }
    }

    #[inline]
    fn set_size<T>(self, size: &mut Size<T>, value: T) {
        match self {
            Self::Horizontal => size.width = value,
            Self::Vertical => size.height = value,
        }
    }

    #[inline]
    fn point<T: Copy>(self, point: Point<T>) -> T {
        match self {
            Self::Horizontal => point.x,
            Self::Vertical => point.y,
        }
    }

    #[inline]
    fn set_point<T>(self, point: &mut Point<T>, value: T) {
        match self {
            Self::Horizontal => point.x = value,
            Self::Vertical => point.y = value,
        }
    }

    #[inline]
    const fn requested(self) -> RequestedAxis {
        match self {
            Self::Horizontal => RequestedAxis::Horizontal,
            Self::Vertical => RequestedAxis::Vertical,
        }
    }
}

/// Physical main/cross-axis mapping used by every flex pass.
///
/// Scratch positions stay flow-relative; the reversal flags convert them to
/// physical coordinates only when final child locations are produced.
#[derive(Debug, Clone, Copy)]
#[allow(clippy::struct_excessive_bools)]
struct Axes {
    main: Axis,
    cross: Axis,
    /// Physical direction of flex main-start: right/bottom when true.
    main_reverse: bool,
    /// Physical direction of the un-reversed logical main-start.
    main_base_reverse: bool,
    /// Physical direction of flex cross-start, including wrap-reverse.
    cross_reverse: bool,
    /// Physical direction of the un-reversed logical cross-start.
    cross_base_reverse: bool,
}

impl Axes {
    fn new(direction: FlexDirection, wrap: FlexWrap, inline_direction: Direction) -> Self {
        let main = if direction.is_row() {
            Axis::Horizontal
        } else {
            Axis::Vertical
        };
        let cross = if direction.is_row() {
            Axis::Vertical
        } else {
            Axis::Horizontal
        };
        let rtl = inline_direction == Direction::Rtl;
        let main_base_reverse = direction.is_row() && rtl;
        let main_reverse = main_base_reverse ^ direction.is_reverse();
        let cross_base_reverse = direction.is_column() && rtl;
        let cross_reverse = cross_base_reverse ^ (wrap == FlexWrap::WrapReverse);
        Self {
            main,
            cross,
            main_reverse,
            main_base_reverse,
            cross_reverse,
            cross_base_reverse,
        }
    }
}

/// Compact order/classification record retained before an item is resolved.
/// Raw style stays in the immutable source and is reborrowed by each pass.
#[derive(Debug, Clone, Copy)]
struct PendingItem {
    node: NodeId,
    document_index: usize,
    css_order: i32,
    layout_order: u32,
}

impl PendingItem {
    fn key(self) -> ItemKey {
        ItemKey {
            node: self.node,
            layout_order: self.layout_order,
        }
    }
}

/// Stable identity and paint order retained when resolved item scratch is
/// rebuilt after a cyclic percentage basis becomes definite.
#[derive(Debug, Clone, Copy)]
struct ItemKey {
    node: NodeId,
    layout_order: u32,
}

/// Transient per-item state accumulated across the Flexbox §9 sizing,
/// flexing, cross-size, and alignment passes. It stores only resolved values
/// and compact hot style fields; raw CSS values are reborrowed from `node`.
#[derive(Debug)]
struct FlexItem {
    key: ItemKey,
    direction: Direction,
    align_self: AlignItems,
    size_is_auto: Size<bool>,
    flex_grow: f32,
    flex_shrink: f32,
    preferred_size: Size<Option<f32>>,
    min_size: Size<Option<f32>>,
    max_size: Size<Option<f32>>,
    margin: Edges<f32>,
    margin_auto: Edges<bool>,
    padding: Edges<f32>,
    border: Edges<f32>,
    scrollbar: Size<f32>,
    inset: Edges<Option<f32>>,
    flex_basis: f32,
    inner_flex_basis: f32,
    min_content_contribution: f32,
    max_content_contribution: f32,
    resolved_min_main: f32,
    hypothetical_main: f32,
    target_main: f32,
    hypothetical_cross: f32,
    target_cross: f32,
    baseline: f32,
    measured_baselines: Point<Option<f32>>,
    frozen: bool,
    violation: f32,
    main_position: f32,
    cross_position: f32,
}

/// One consecutive range in the order-modified item array plus its resolved
/// cross-axis size and position.
#[derive(Debug, Clone, Copy)]
struct FlexLine {
    start: usize,
    end: usize,
    cross_size: f32,
    cross_position: f32,
}

impl FlexLine {
    #[inline]
    fn len(self) -> usize {
        self.end - self.start
    }
}

#[inline]
fn size_from_axes<T: Copy>(axes: Axes, main: T, cross: T) -> Size<T> {
    match axes.main {
        Axis::Horizontal => Size::new(main, cross),
        Axis::Vertical => Size::new(cross, main),
    }
}

#[inline]
fn physical_start(edges: Edges<f32>, axis: Axis) -> f32 {
    match axis {
        Axis::Horizontal => edges.left,
        Axis::Vertical => edges.top,
    }
}

#[inline]
fn physical_end(edges: Edges<f32>, axis: Axis) -> f32 {
    match axis {
        Axis::Horizontal => edges.right,
        Axis::Vertical => edges.bottom,
    }
}

#[inline]
fn axis_sum(edges: Edges<f32>, axis: Axis) -> f32 {
    physical_start(edges, axis) + physical_end(edges, axis)
}

#[inline]
fn flow_start(edges: Edges<f32>, axis: Axis, reverse: bool) -> f32 {
    if reverse {
        physical_end(edges, axis)
    } else {
        physical_start(edges, axis)
    }
}

#[inline]
fn flow_end(edges: Edges<f32>, axis: Axis, reverse: bool) -> f32 {
    if reverse {
        physical_start(edges, axis)
    } else {
        physical_end(edges, axis)
    }
}

#[inline]
fn flow_start_bool(edges: Edges<bool>, axis: Axis, reverse: bool) -> bool {
    match (axis, reverse) {
        (Axis::Horizontal, false) => edges.left,
        (Axis::Horizontal, true) => edges.right,
        (Axis::Vertical, false) => edges.top,
        (Axis::Vertical, true) => edges.bottom,
    }
}

#[inline]
fn flow_end_bool(edges: Edges<bool>, axis: Axis, reverse: bool) -> bool {
    match (axis, reverse) {
        (Axis::Horizontal, false) => edges.right,
        (Axis::Horizontal, true) => edges.left,
        (Axis::Vertical, false) => edges.bottom,
        (Axis::Vertical, true) => edges.top,
    }
}

#[inline]
fn set_flow_start(edges: &mut Edges<f32>, axis: Axis, reverse: bool, value: f32) {
    match (axis, reverse) {
        (Axis::Horizontal, false) => edges.left = value,
        (Axis::Horizontal, true) => edges.right = value,
        (Axis::Vertical, false) => edges.top = value,
        (Axis::Vertical, true) => edges.bottom = value,
    }
}

#[inline]
fn set_flow_end(edges: &mut Edges<f32>, axis: Axis, reverse: bool, value: f32) {
    match (axis, reverse) {
        (Axis::Horizontal, false) => edges.right = value,
        (Axis::Horizontal, true) => edges.left = value,
        (Axis::Vertical, false) => edges.bottom = value,
        (Axis::Vertical, true) => edges.top = value,
    }
}

#[inline]
fn padding_border_size(padding: Edges<f32>, border: Edges<f32>, scrollbar: Size<f32>) -> Size<f32> {
    Size::new(
        padding.horizontal_sum() + border.horizontal_sum() + scrollbar.width,
        padding.vertical_sum() + border.vertical_sum() + scrollbar.height,
    )
}

#[inline]
fn resolve_quantitative_sizes(
    size: Size<Dimension>,
    basis: Size<Option<f32>>,
    aspect_ratio: Option<f32>,
    box_sizing: BoxSizing,
    inset_size: Size<f32>,
    resolve_calc: &impl Fn(CalcHandle, f32) -> f32,
) -> Size<Option<f32>> {
    apply_box_sizing(
        apply_aspect_ratio(resolve_size(size, basis, resolve_calc), aspect_ratio),
        box_sizing,
        inset_size,
    )
}

#[inline]
fn clamp_axis(value: f32, min: Option<f32>, max: Option<f32>, floor: f32) -> f32 {
    clamp(value, min, max).max(floor)
}

fn alignment_distribution(
    value: AlignContent,
    free_space: f32,
    count: usize,
    flow_reverse: bool,
    base_reverse: bool,
) -> (f32, f32) {
    if count == 0 {
        return (0.0, 0.0);
    }

    let value = if free_space < 0.0 {
        match value {
            AlignContent::SpaceBetween | AlignContent::Stretch => AlignContent::FlexStart,
            AlignContent::SpaceAround | AlignContent::SpaceEvenly => AlignContent::Center,
            other => other,
        }
    } else {
        value
    };

    match value {
        AlignContent::Start => {
            if flow_reverse == base_reverse {
                (0.0, 0.0)
            } else {
                (free_space, 0.0)
            }
        }
        AlignContent::End => {
            if flow_reverse == base_reverse {
                (free_space, 0.0)
            } else {
                (0.0, 0.0)
            }
        }
        AlignContent::FlexEnd => (free_space, 0.0),
        AlignContent::Center => (free_space / 2.0, 0.0),
        AlignContent::SpaceBetween if count > 1 => (0.0, free_space / (count - 1) as f32),
        AlignContent::FlexStart | AlignContent::Stretch | AlignContent::SpaceBetween => (0.0, 0.0),
        AlignContent::SpaceAround => {
            let between = free_space / count as f32;
            (between / 2.0, between)
        }
        AlignContent::SpaceEvenly => {
            let between = free_space / (count + 1) as f32;
            (between, between)
        }
    }
}

#[inline]
fn relative_offset(inset: Edges<Option<f32>>, direction: Direction) -> Point<f32> {
    let x = match (inset.left, inset.right) {
        (Some(left), Some(right)) => {
            if direction == Direction::Rtl {
                -right
            } else {
                left
            }
        }
        (Some(left), None) => left,
        (None, Some(right)) => -right,
        (None, None) => 0.0,
    };
    let y = inset.top.unwrap_or_else(|| -inset.bottom.unwrap_or(0.0));
    Point::new(x, y)
}

#[inline]
fn copied_scrollbar_size(overflow: Point<Overflow>, width: f32) -> Size<f32> {
    debug_assert!(width.is_finite() && width >= 0.0);
    Size::new(
        if overflow.y == Overflow::Scroll {
            width
        } else {
            0.0
        },
        if overflow.x == Overflow::Scroll {
            width
        } else {
            0.0
        },
    )
}

fn resolve_item<Source: FlexSource>(
    source: &Source,
    key: ItemKey,
    container_inner_size: Size<Option<f32>>,
    default_alignment: AlignItems,
) -> FlexItem {
    let style = source.flex_item_style(key.node);
    let flex_grow = style.flex_grow();
    let flex_shrink = style.flex_shrink();
    debug_assert!(
        flex_grow.is_finite() && flex_grow >= 0.0,
        "flex-grow must be finite and non-negative"
    );
    debug_assert!(
        flex_shrink.is_finite() && flex_shrink >= 0.0,
        "flex-shrink must be finite and non-negative"
    );
    let resolve_calc = |handle, basis| source.resolve_calc(handle, basis);
    let inline_basis = container_inner_size.width;
    let size_value = style.size();
    let padding = resolve_edges(style.padding(), inline_basis, &resolve_calc);
    let border = resolve_edges(style.border(), inline_basis, &resolve_calc);
    let scrollbar = copied_scrollbar_size(style.overflow(), style.scrollbar_width());
    let inset_size = padding_border_size(padding, border, scrollbar);
    let preferred_size = resolve_quantitative_sizes(
        size_value,
        container_inner_size,
        style.aspect_ratio(),
        style.box_sizing(),
        inset_size,
        &resolve_calc,
    );
    let min_size = resolve_quantitative_sizes(
        style.min_size(),
        container_inner_size,
        style.aspect_ratio(),
        style.box_sizing(),
        inset_size,
        &resolve_calc,
    );
    let max_size = resolve_quantitative_sizes(
        style.max_size(),
        container_inner_size,
        style.aspect_ratio(),
        style.box_sizing(),
        inset_size,
        &resolve_calc,
    );
    let margin_value = style.margin();
    let optional_margin = resolve_optional_edges(margin_value, inline_basis, &resolve_calc);
    let margin_auto = margin_value.map(LengthPercentageAuto::is_auto);
    let margin = auto_edges_to_zero(optional_margin);
    let inset = resolve_insets(style.inset(), container_inner_size, &resolve_calc);

    FlexItem {
        key,
        direction: style.direction(),
        align_self: style.align_self().unwrap_or(default_alignment),
        size_is_auto: size_value.map(Dimension::is_auto),
        flex_grow,
        flex_shrink,
        preferred_size,
        min_size,
        max_size,
        margin,
        margin_auto,
        padding,
        border,
        scrollbar,
        inset,
        flex_basis: 0.0,
        inner_flex_basis: 0.0,
        min_content_contribution: 0.0,
        max_content_contribution: 0.0,
        resolved_min_main: 0.0,
        hypothetical_main: 0.0,
        target_main: 0.0,
        hypothetical_cross: 0.0,
        target_cross: 0.0,
        baseline: 0.0,
        measured_baselines: Point::NONE,
        frozen: false,
        violation: 0.0,
        main_position: 0.0,
        cross_position: 0.0,
    }
}

#[allow(clippy::too_many_arguments)]
fn child_measurement<Source, Session>(
    source: &Source,
    session: &mut Session,
    node: NodeId,
    known_dimensions: Size<Option<f32>>,
    parent_size: Size<Option<f32>>,
    available_space: Size<AvailableSpace>,
    sizing_mode: SizingMode,
    requested_axis: RequestedAxis,
) -> LayoutOutput
where
    Source: FlexSource,
    Session: LayoutSession<Source>,
{
    let mut input = LayoutInput::compute_size(
        known_dimensions,
        parent_size,
        available_space,
        requested_axis,
    );
    input.sizing_mode = sizing_mode;
    session.compute_child_layout(source, node, input)
}

#[allow(clippy::too_many_lines)]
fn determine_flex_base_sizes<Source, Session>(
    source: &Source,
    session: &mut Session,
    items: &mut [FlexItem],
    axes: Axes,
    container_inner_size: Size<Option<f32>>,
    available_space: Size<AvailableSpace>,
    flex_basis_percentage_basis: Option<f32>,
) where
    Source: FlexSource,
    Session: LayoutSession<Source>,
{
    let container_main = axes.main.size(container_inner_size);
    let available_main = axes.main.size(available_space);

    for item in items {
        // Source and session are deliberately separate: this borrowed style
        // view remains valid across both recursive measurements below.
        let style = source.flex_item_style(item.key.node);
        let inset_size = padding_border_size(item.padding, item.border, item.scrollbar);
        let main_floor = axes.main.size(inset_size);
        let cross_preferred = axes.cross.size(item.preferred_size);
        let mut known = Size::NONE;
        axes.cross.set_size(&mut known, cross_preferred);

        let min_available = size_from_axes(
            axes,
            AvailableSpace::MinContent,
            axes.cross.size(available_space),
        );
        let max_available = size_from_axes(
            axes,
            AvailableSpace::MaxContent,
            axes.cross.size(available_space),
        );
        let contribution_parent_size =
            size_from_axes(axes, None, axes.cross.size(container_inner_size));
        let min_content = axes.main.size(
            child_measurement(
                source,
                session,
                item.key.node,
                known,
                contribution_parent_size,
                min_available,
                SizingMode::ContentSize,
                axes.main.requested(),
            )
            .size,
        );
        let max_content = axes.main.size(
            child_measurement(
                source,
                session,
                item.key.node,
                known,
                contribution_parent_size,
                max_available,
                SizingMode::ContentSize,
                axes.main.requested(),
            )
            .size,
        );

        let resolve_intrinsic_dimension = |value: Dimension| -> Option<f32> {
            match value {
                Dimension::MinContent => Some(min_content),
                Dimension::MaxContent => Some(max_content),
                Dimension::FitContent(limit) => {
                    let resolve_calc = |handle, basis| source.resolve_calc(handle, basis);
                    let limit = resolve_length_percentage(limit, container_main, &resolve_calc)
                        .unwrap_or(max_content);
                    Some(max_content.min(limit.max(min_content)))
                }
                Dimension::Length(_)
                | Dimension::Percent(_)
                | Dimension::Calc(_)
                | Dimension::Auto => None,
            }
        };
        if axes.main.size(item.preferred_size).is_none()
            && let Some(value) = resolve_intrinsic_dimension(axes.main.size(style.size()))
        {
            axes.main.set_size(&mut item.preferred_size, Some(value));
        }
        if axes.main.size(item.min_size).is_none()
            && let Some(value) = resolve_intrinsic_dimension(axes.main.size(style.min_size()))
        {
            axes.main.set_size(&mut item.min_size, Some(value));
        }
        if axes.main.size(item.max_size).is_none()
            && let Some(value) = resolve_intrinsic_dimension(axes.main.size(style.max_size()))
        {
            axes.main.set_size(&mut item.max_size, Some(value));
        }

        let resolved_basis = {
            let resolve_calc = |handle, basis| source.resolve_calc(handle, basis);
            resolve_dimension(
                style.flex_basis(),
                flex_basis_percentage_basis,
                &resolve_calc,
            )
            .map(|basis| {
                if style.box_sizing() == BoxSizing::ContentBox {
                    basis + main_floor
                } else {
                    basis
                }
            })
        };

        let preferred_main = axes.main.size(item.preferred_size);
        let preferred_flex_basis = if style.flex_basis().is_auto() {
            preferred_main
        } else {
            None
        };
        item.flex_basis = if let Some(basis) = resolved_basis.or(preferred_flex_basis) {
            basis
        } else {
            let content_basis = if style.flex_basis().is_auto() {
                axes.main.size(style.size())
            } else {
                style.flex_basis()
            };
            match content_basis {
                Dimension::MinContent => min_content,
                Dimension::MaxContent | Dimension::Length(_) | Dimension::Calc(_) => max_content,
                Dimension::FitContent(limit) => {
                    let resolve_calc = |handle, basis| source.resolve_calc(handle, basis);
                    let limit = super::util::resolve_length_percentage(
                        limit,
                        flex_basis_percentage_basis,
                        &resolve_calc,
                    )
                    .unwrap_or(max_content);
                    max_content.min(limit.max(min_content))
                }
                Dimension::Auto | Dimension::Percent(_) => {
                    if available_main == AvailableSpace::MinContent {
                        min_content
                    } else {
                        max_content
                    }
                }
            }
        };

        // Flexbox §9.2 deliberately allows a negative inner flex base.
        item.inner_flex_basis = item.flex_basis - main_floor;

        let explicit_min = axes.main.size(item.min_size);
        item.resolved_min_main = if let Some(minimum) = explicit_min {
            minimum.max(main_floor)
        } else if style.overflow().x.is_scroll_container()
            || style.overflow().y.is_scroll_container()
        {
            main_floor
        } else {
            let raw_main_size = axes.main.size(style.size());
            let raw_cross_size = axes.cross.size(style.size());
            let specified_suggestion = (!raw_main_size.is_auto())
                .then_some(preferred_main)
                .flatten();
            let transferred_suggestion = (style.aspect_ratio().is_some()
                && !raw_cross_size.is_auto())
            .then_some(preferred_main)
            .flatten();
            let mut content_suggestion = min_content;
            // The current protocol cannot distinguish replaced elements;
            // ordinary flex items use the non-replaced max(content,
            // transferred) rule from §4.5.
            if let Some(transferred) = transferred_suggestion {
                content_suggestion = content_suggestion.max(transferred);
            }
            if let Some(specified) = specified_suggestion {
                content_suggestion = content_suggestion.min(specified);
            }
            content_suggestion =
                content_suggestion.min(axes.main.size(item.max_size).unwrap_or(f32::INFINITY));
            content_suggestion.max(main_floor)
        };

        item.hypothetical_main = clamp_axis(
            item.flex_basis,
            Some(item.resolved_min_main),
            axes.main.size(item.max_size),
            main_floor,
        );
        let margin_main = axis_sum(item.margin, axes.main);
        let preferred_contribution = preferred_main.unwrap_or(0.0);
        let contribution = |content: f32| {
            let mut value = content.max(preferred_contribution);
            if item.flex_grow == 0.0 {
                value = value.min(item.flex_basis);
            }
            if item.flex_shrink == 0.0 {
                value = value.max(item.flex_basis);
            }
            clamp_axis(
                value,
                Some(item.resolved_min_main),
                axes.main.size(item.max_size),
                main_floor,
            ) + margin_main
        };
        item.min_content_contribution = contribution(min_content);
        item.max_content_contribution = contribution(max_content);
        item.target_main = item.hypothetical_main;
    }
}

#[inline]
fn item_outer_hypothetical_main(item: &FlexItem, axes: Axes) -> f32 {
    item.hypothetical_main + axis_sum(item.margin, axes.main)
}

#[inline]
fn item_outer_target_main(item: &FlexItem, axes: Axes) -> f32 {
    item.target_main + axis_sum(item.margin, axes.main)
}

fn collect_flex_lines(
    items: &[FlexItem],
    wrap: FlexWrap,
    available_main: AvailableSpace,
    gap: f32,
    axes: Axes,
) -> Vec<FlexLine> {
    if wrap == FlexWrap::NoWrap || available_main == AvailableSpace::MaxContent {
        return vec![FlexLine {
            start: 0,
            end: items.len(),
            cross_size: 0.0,
            cross_position: 0.0,
        }];
    }
    if items.is_empty() {
        return Vec::new();
    }
    if available_main == AvailableSpace::MinContent {
        return (0..items.len())
            .map(|index| FlexLine {
                start: index,
                end: index + 1,
                cross_size: 0.0,
                cross_position: 0.0,
            })
            .collect();
    }

    let AvailableSpace::Definite(limit) = available_main else {
        unreachable!("intrinsic available-space variants handled above")
    };
    let mut lines = Vec::new();
    let mut start = 0;
    while start < items.len() {
        let mut end = start;
        let mut occupied = 0.0;
        while end < items.len() {
            let item_size = item_outer_hypothetical_main(&items[end], axes);
            let candidate_gap = if end == start { 0.0 } else { gap };
            let candidate = occupied + candidate_gap + item_size;
            // The first item always establishes a line. A zero-sized item at
            // an exact boundary remains on the preceding line (§9.3 note).
            if end > start && candidate > limit && !(item_size == 0.0 && candidate_gap == 0.0) {
                break;
            }
            occupied = candidate;
            end += 1;
        }
        lines.push(FlexLine {
            start,
            end,
            cross_size: 0.0,
            cross_position: 0.0,
        });
        start = end;
    }
    lines
}

fn line_intrinsic_main(items: &[FlexItem], line: FlexLine, gap: f32, axes: Axes) -> f32 {
    let item_sum = items[line.start..line.end]
        .iter()
        .map(|item| item.flex_basis.max(item.resolved_min_main) + axis_sum(item.margin, axes.main))
        .sum::<f32>();
    item_sum + gap * line.len().saturating_sub(1) as f32
}

fn line_content_contribution(items: &[FlexItem], line: FlexLine, gap: f32, maximum: bool) -> f32 {
    let item_sum = items[line.start..line.end]
        .iter()
        .map(|item| {
            if maximum {
                item.max_content_contribution
            } else {
                item.min_content_contribution
            }
        })
        .sum::<f32>();
    item_sum + gap * line.len().saturating_sub(1) as f32
}

#[allow(clippy::too_many_arguments)]
fn determine_auto_main_size(
    items: &[FlexItem],
    lines: &[FlexLine],
    gap: f32,
    axes: Axes,
    available_main: AvailableSpace,
    inset_main: f32,
    min_outer: Option<f32>,
    max_outer: Option<f32>,
) -> f32 {
    let content = match available_main {
        AvailableSpace::MaxContent => lines
            .iter()
            .copied()
            .map(|line| line_content_contribution(items, line, gap, true))
            .max_by(f32::total_cmp)
            .unwrap_or(0.0),
        AvailableSpace::MinContent => lines
            .iter()
            .copied()
            .map(|line| line_content_contribution(items, line, gap, false))
            .max_by(f32::total_cmp)
            .unwrap_or(0.0),
        AvailableSpace::Definite(_) => lines
            .iter()
            .copied()
            .map(|line| line_intrinsic_main(items, line, gap, axes))
            .max_by(f32::total_cmp)
            .unwrap_or(0.0),
    };
    let content = match available_main {
        AvailableSpace::Definite(available) if lines.len() > 1 => content.max(available),
        _ => content,
    };
    clamp_axis(content + inset_main, min_outer, max_outer, inset_main)
}

#[allow(clippy::too_many_lines)]
fn resolve_flexible_lengths(
    items: &mut [FlexItem],
    line: FlexLine,
    inner_main_size: f32,
    gap: f32,
    axes: Axes,
) {
    let line_items = &mut items[line.start..line.end];
    if line_items.is_empty() {
        return;
    }
    let total_gap = gap * line_items.len().saturating_sub(1) as f32;
    let hypothetical_sum = total_gap
        + line_items
            .iter()
            .map(|item| item_outer_hypothetical_main(item, axes))
            .sum::<f32>();
    let growing = hypothetical_sum < inner_main_size;
    let initial_delta = inner_main_size - hypothetical_sum;

    for item in line_items.iter_mut() {
        item.frozen = false;
        item.violation = 0.0;
        item.target_main = item.flex_basis;
        let factor_is_zero = if growing {
            item.flex_grow == 0.0
        } else {
            item.flex_shrink == 0.0
        };
        let clamp_requires_freeze = if growing {
            item.flex_basis > item.hypothetical_main
        } else {
            item.flex_basis < item.hypothetical_main
        };
        if initial_delta.abs() <= f32::EPSILON || factor_is_zero || clamp_requires_freeze {
            item.target_main = item.hypothetical_main;
            item.frozen = true;
        }
    }

    let initial_used = total_gap
        + line_items
            .iter()
            .map(|item| {
                let main = if item.frozen {
                    item.target_main
                } else {
                    item.flex_basis
                };
                main + axis_sum(item.margin, axes.main)
            })
            .sum::<f32>();
    let initial_free_space = inner_main_size - initial_used;

    for _ in 0..=line_items.len() {
        if line_items.iter().all(|item| item.frozen) {
            return;
        }

        let used = total_gap
            + line_items
                .iter()
                .map(|item| {
                    let main = if item.frozen {
                        item.target_main
                    } else {
                        item.flex_basis
                    };
                    main + axis_sum(item.margin, axes.main)
                })
                .sum::<f32>();
        let mut remaining = inner_main_size - used;
        let factor_sum = line_items
            .iter()
            .filter(|item| !item.frozen)
            .map(|item| {
                if growing {
                    item.flex_grow
                } else {
                    item.flex_shrink
                }
            })
            .sum::<f32>();
        if factor_sum < 1.0 {
            let scaled = initial_free_space * factor_sum;
            if scaled.abs() < remaining.abs() {
                remaining = scaled;
            }
        }

        if growing {
            if factor_sum > 0.0 {
                for item in line_items.iter_mut().filter(|item| !item.frozen) {
                    item.target_main = item.flex_basis + remaining * item.flex_grow / factor_sum;
                }
            }
        } else {
            let scaled_sum = line_items
                .iter()
                .filter(|item| !item.frozen)
                .map(|item| item.flex_shrink * item.inner_flex_basis)
                .sum::<f32>();
            if scaled_sum > 0.0 {
                for item in line_items.iter_mut().filter(|item| !item.frozen) {
                    let scaled = item.flex_shrink * item.inner_flex_basis;
                    item.target_main = item.flex_basis + remaining * scaled / scaled_sum;
                }
            }
        }

        let mut total_violation = 0.0;
        for item in line_items.iter_mut().filter(|item| !item.frozen) {
            let floor = axes.main.size(padding_border_size(
                item.padding,
                item.border,
                item.scrollbar,
            ));
            let unclamped = item.target_main;
            item.target_main = clamp_axis(
                unclamped,
                Some(item.resolved_min_main),
                axes.main.size(item.max_size),
                floor,
            );
            item.violation = item.target_main - unclamped;
            total_violation += item.violation;
        }

        let mut froze_any = false;
        for item in line_items.iter_mut().filter(|item| !item.frozen) {
            let freeze = if total_violation > f32::EPSILON {
                item.violation > 0.0
            } else if total_violation < -f32::EPSILON {
                item.violation < 0.0
            } else {
                true
            };
            if freeze {
                item.frozen = true;
                froze_any = true;
            }
        }
        if !froze_any {
            // Floating-point cancellation must not turn the normative freeze
            // loop into an infinite loop.
            for item in line_items.iter_mut() {
                item.frozen = true;
            }
            return;
        }
    }

    debug_assert!(false, "flex freeze loop exceeded the item-count bound");
}

fn determine_hypothetical_cross_sizes<Source, Session>(
    source: &Source,
    session: &mut Session,
    items: &mut [FlexItem],
    lines: &[FlexLine],
    axes: Axes,
    container_inner_size: Size<Option<f32>>,
    available_space: Size<AvailableSpace>,
) where
    Source: FlexSource,
    Session: LayoutSession<Source>,
{
    for line in lines {
        for item in &mut items[line.start..line.end] {
            let mut known = Size::NONE;
            axes.main.set_size(&mut known, Some(item.target_main));
            axes.cross
                .set_size(&mut known, axes.cross.size(item.preferred_size));
            let child_available = size_from_axes(
                axes,
                AvailableSpace::Definite(item.target_main),
                axes.cross.size(available_space),
            );
            let output = child_measurement(
                source,
                session,
                item.key.node,
                known,
                container_inner_size,
                child_available,
                SizingMode::InherentSize,
                RequestedAxis::Both,
            );
            let inset_size = padding_border_size(item.padding, item.border, item.scrollbar);
            let cross_floor = axes.cross.size(inset_size);
            item.hypothetical_cross = clamp_axis(
                axes.cross.size(output.size),
                axes.cross.size(item.min_size),
                axes.cross.size(item.max_size),
                cross_floor,
            );
            item.target_cross = item.hypothetical_cross;
            item.measured_baselines = output.first_baselines;
            item.baseline = if axes.main == Axis::Horizontal {
                output.first_baselines.y.unwrap_or(output.size.height)
            } else {
                output.first_baselines.x.unwrap_or(output.size.width)
            };
        }
    }
}

fn calculate_line_cross_sizes(
    items: &[FlexItem],
    lines: &mut [FlexLine],
    axes: Axes,
    wrap: FlexWrap,
    known_inner_cross: Option<f32>,
) {
    if wrap == FlexWrap::NoWrap
        && let (Some(line), Some(cross_size)) = (lines.first_mut(), known_inner_cross)
    {
        line.cross_size = cross_size.max(0.0);
        return;
    }

    for line in lines {
        let mut largest_outer = 0.0_f32;
        let mut largest_before_baseline = 0.0_f32;
        let mut largest_after_baseline = 0.0_f32;
        let mut has_baseline_item = false;
        for item in &items[line.start..line.end] {
            let outer_cross = item.hypothetical_cross + axis_sum(item.margin, axes.cross);
            if axes.main == Axis::Horizontal
                && item.align_self == AlignItems::Baseline
                && !flow_start_bool(item.margin_auto, axes.cross, axes.cross_reverse)
                && !flow_end_bool(item.margin_auto, axes.cross, axes.cross_reverse)
            {
                // Baseline geometry is physical top-to-bottom even when
                // wrap-reverse changes flex cross-start.
                let before = item.margin.top + item.baseline;
                let after = outer_cross - before;
                largest_before_baseline = largest_before_baseline.max(before);
                largest_after_baseline = largest_after_baseline.max(after);
                has_baseline_item = true;
            } else {
                largest_outer = largest_outer.max(outer_cross);
            }
        }
        let baseline_outer = if has_baseline_item {
            largest_before_baseline + largest_after_baseline
        } else {
            0.0
        };
        line.cross_size = largest_outer.max(baseline_outer).max(0.0);
    }
}

fn determine_auto_cross_size(
    lines: &[FlexLine],
    cross_gap: f32,
    inset_cross: f32,
    min_outer: Option<f32>,
    max_outer: Option<f32>,
    axes: Axes,
    cross_constraint: AvailableSpace,
) -> f32 {
    let lines_size =
        if axes.main == Axis::Vertical && cross_constraint == AvailableSpace::MinContent {
            lines
                .iter()
                .map(|line| line.cross_size)
                .max_by(f32::total_cmp)
                .unwrap_or(0.0)
        } else {
            lines.iter().map(|line| line.cross_size).sum::<f32>()
                + cross_gap * lines.len().saturating_sub(1) as f32
        };
    clamp_axis(lines_size + inset_cross, min_outer, max_outer, inset_cross)
}

fn stretch_lines(
    lines: &mut [FlexLine],
    wrap: FlexWrap,
    align_content: AlignContent,
    inner_cross: f32,
    cross_gap: f32,
) {
    if wrap == FlexWrap::NoWrap || align_content != AlignContent::Stretch || lines.is_empty() {
        return;
    }
    let used = lines.iter().map(|line| line.cross_size).sum::<f32>()
        + cross_gap * lines.len().saturating_sub(1) as f32;
    if used < inner_cross {
        let addition = (inner_cross - used) / lines.len() as f32;
        for line in lines {
            line.cross_size += addition;
        }
    }
}

fn determine_used_cross_sizes(items: &mut [FlexItem], lines: &[FlexLine], axes: Axes) {
    for line in lines {
        for item in &mut items[line.start..line.end] {
            let inset_size = padding_border_size(item.padding, item.border, item.scrollbar);
            let cross_floor = axes.cross.size(inset_size);
            let should_stretch = item.align_self == AlignItems::Stretch
                && axes.cross.size(item.size_is_auto)
                && !flow_start_bool(item.margin_auto, axes.cross, axes.cross_reverse)
                && !flow_end_bool(item.margin_auto, axes.cross, axes.cross_reverse);
            item.target_cross = if should_stretch {
                clamp_axis(
                    line.cross_size - axis_sum(item.margin, axes.cross),
                    axes.cross.size(item.min_size),
                    axes.cross.size(item.max_size),
                    cross_floor,
                )
            } else {
                item.hypothetical_cross
            };
        }
    }
}

fn distribute_main_axis(
    items: &mut [FlexItem],
    lines: &[FlexLine],
    axes: Axes,
    inner_main: f32,
    main_gap: f32,
    justify_content: AlignContent,
) {
    for line in lines {
        let line_items = &mut items[line.start..line.end];
        let fixed_gap = main_gap * line_items.len().saturating_sub(1) as f32;
        let used = fixed_gap
            + line_items
                .iter()
                .map(|item| item_outer_target_main(item, axes))
                .sum::<f32>();
        let free_space = inner_main - used;
        let auto_count = line_items
            .iter()
            .map(|item| {
                usize::from(flow_start_bool(
                    item.margin_auto,
                    axes.main,
                    axes.main_reverse,
                )) + usize::from(flow_end_bool(
                    item.margin_auto,
                    axes.main,
                    axes.main_reverse,
                ))
            })
            .sum::<usize>();

        let (leading, distributed_gap) = if free_space > 0.0 && auto_count > 0 {
            let share = free_space / auto_count as f32;
            for item in line_items.iter_mut() {
                if flow_start_bool(item.margin_auto, axes.main, axes.main_reverse) {
                    set_flow_start(&mut item.margin, axes.main, axes.main_reverse, share);
                }
                if flow_end_bool(item.margin_auto, axes.main, axes.main_reverse) {
                    set_flow_end(&mut item.margin, axes.main, axes.main_reverse, share);
                }
            }
            (0.0, 0.0)
        } else {
            for item in line_items.iter_mut() {
                if flow_start_bool(item.margin_auto, axes.main, axes.main_reverse) {
                    set_flow_start(&mut item.margin, axes.main, axes.main_reverse, 0.0);
                }
                if flow_end_bool(item.margin_auto, axes.main, axes.main_reverse) {
                    set_flow_end(&mut item.margin, axes.main, axes.main_reverse, 0.0);
                }
            }
            alignment_distribution(
                justify_content,
                free_space,
                line_items.len(),
                axes.main_reverse,
                axes.main_base_reverse,
            )
        };

        let mut cursor = leading;
        let line_item_count = line_items.len();
        for (index, item) in line_items.iter_mut().enumerate() {
            cursor += flow_start(item.margin, axes.main, axes.main_reverse);
            item.main_position = cursor;
            cursor += item.target_main + flow_end(item.margin, axes.main, axes.main_reverse);
            if index + 1 < line_item_count {
                cursor += main_gap + distributed_gap;
            }
        }
    }
}

fn align_lines(
    lines: &mut [FlexLine],
    axes: Axes,
    wrap: FlexWrap,
    align_content: AlignContent,
    inner_cross: f32,
    cross_gap: f32,
) {
    let used = lines.iter().map(|line| line.cross_size).sum::<f32>()
        + cross_gap * lines.len().saturating_sub(1) as f32;
    let free_space = inner_cross - used;
    let effective_alignment = if wrap == FlexWrap::NoWrap {
        AlignContent::FlexStart
    } else {
        align_content
    };
    let (leading, distributed_gap) = alignment_distribution(
        effective_alignment,
        free_space,
        lines.len(),
        axes.cross_reverse,
        axes.cross_base_reverse,
    );
    let mut cursor = leading;
    let line_count = lines.len();
    for (index, line) in lines.iter_mut().enumerate() {
        line.cross_position = cursor;
        cursor += line.cross_size;
        if index + 1 < line_count {
            cursor += cross_gap + distributed_gap;
        }
    }
}

fn align_items_cross_axis(items: &mut [FlexItem], lines: &[FlexLine], axes: Axes) {
    for line in lines {
        let max_physical_baseline = if axes.main == Axis::Horizontal {
            items[line.start..line.end]
                .iter()
                .filter(|item| {
                    item.align_self == AlignItems::Baseline
                        && !flow_start_bool(item.margin_auto, axes.cross, axes.cross_reverse)
                        && !flow_end_bool(item.margin_auto, axes.cross, axes.cross_reverse)
                })
                .map(|item| item.margin.top + item.baseline)
                .fold(0.0_f32, f32::max)
        } else {
            0.0
        };

        for item in &mut items[line.start..line.end] {
            let start_auto = flow_start_bool(item.margin_auto, axes.cross, axes.cross_reverse);
            let end_auto = flow_end_bool(item.margin_auto, axes.cross, axes.cross_reverse);
            let free = line.cross_size - item.target_cross - axis_sum(item.margin, axes.cross);
            if start_auto || end_auto {
                if free >= 0.0 {
                    let count = usize::from(start_auto) + usize::from(end_auto);
                    let share = free / count as f32;
                    if start_auto {
                        set_flow_start(&mut item.margin, axes.cross, axes.cross_reverse, share);
                    }
                    if end_auto {
                        set_flow_end(&mut item.margin, axes.cross, axes.cross_reverse, share);
                    }
                } else {
                    // The overflow rule is keyed to logical block/inline
                    // start, not flex cross-start; wrap-reverse must not
                    // swap which auto margin is zeroed.
                    let logical_start_auto =
                        flow_start_bool(item.margin_auto, axes.cross, axes.cross_base_reverse);
                    let logical_end_auto =
                        flow_end_bool(item.margin_auto, axes.cross, axes.cross_base_reverse);
                    if logical_start_auto {
                        set_flow_start(&mut item.margin, axes.cross, axes.cross_base_reverse, 0.0);
                        if logical_end_auto {
                            set_flow_end(
                                &mut item.margin,
                                axes.cross,
                                axes.cross_base_reverse,
                                free,
                            );
                        }
                    } else if logical_end_auto {
                        set_flow_end(&mut item.margin, axes.cross, axes.cross_base_reverse, free);
                    }
                }
                let physical_position = physical_start(item.margin, axes.cross);
                item.cross_position = if axes.cross_reverse {
                    line.cross_size - physical_position - item.target_cross
                } else {
                    physical_position
                };
                continue;
            }

            if item.align_self == AlignItems::Baseline && axes.main == Axis::Horizontal {
                let physical_top = max_physical_baseline - item.baseline;
                item.cross_position = if axes.cross_reverse {
                    line.cross_size - physical_top - item.target_cross
                } else {
                    physical_top
                };
                continue;
            }

            let alignment_offset = match item.align_self {
                AlignItems::Start => {
                    if axes.cross_reverse == axes.cross_base_reverse {
                        0.0
                    } else {
                        free
                    }
                }
                AlignItems::End => {
                    if axes.cross_reverse == axes.cross_base_reverse {
                        free
                    } else {
                        0.0
                    }
                }
                AlignItems::FlexStart | AlignItems::Stretch | AlignItems::Baseline => 0.0,
                AlignItems::FlexEnd => free,
                AlignItems::Center => free / 2.0,
            };
            item.cross_position =
                alignment_offset + flow_start(item.margin, axes.cross, axes.cross_reverse);
        }
    }
}

#[inline]
fn flow_to_physical(flow: f32, box_size: f32, container_size: f32, reverse: bool) -> f32 {
    if reverse {
        container_size - flow - box_size
    } else {
        flow
    }
}

fn item_border_box_location(
    item: &FlexItem,
    line: FlexLine,
    axes: Axes,
    inner_size: Size<f32>,
    content_origin: Point<f32>,
) -> Point<f32> {
    let main = flow_to_physical(
        item.main_position,
        item.target_main,
        axes.main.size(inner_size),
        axes.main_reverse,
    ) + axes.main.point(content_origin);
    let cross_flow = line.cross_position + item.cross_position;
    let cross = flow_to_physical(
        cross_flow,
        item.target_cross,
        axes.cross.size(inner_size),
        axes.cross_reverse,
    ) + axes.cross.point(content_origin);
    let mut point = Point::ZERO;
    axes.main.set_point(&mut point, main);
    axes.cross.set_point(&mut point, cross);
    point
}

fn first_container_baseline(
    items: &[FlexItem],
    lines: &[FlexLine],
    axes: Axes,
    inner_size: Size<f32>,
    content_origin: Point<f32>,
) -> Option<f32> {
    let line = *lines.first()?;
    let first = items[line.start..line.end]
        .iter()
        .find(|item| axes.main == Axis::Vertical || item.align_self == AlignItems::Baseline)
        .or_else(|| items[line.start..line.end].first())?;
    let location = item_border_box_location(first, line, axes, inner_size, content_origin);
    Some(
        location.y
            + first.measured_baselines.y.unwrap_or_else(|| {
                size_from_axes(axes, first.target_main, first.target_cross).height
            }),
    )
}

#[allow(clippy::too_many_arguments)]
fn perform_in_flow_layout<Source, Session>(
    source: &Source,
    session: &mut Session,
    items: &mut [FlexItem],
    lines: &[FlexLine],
    axes: Axes,
    inner_size: Size<f32>,
    content_origin: Point<f32>,
    container_size: Size<f32>,
) -> (Size<f32>, Option<f32>)
where
    Source: FlexSource,
    Session: LayoutSession<Source>,
{
    let parent_size = inner_size.map(Some);
    let mut content_size = container_size;
    let mut first_baseline = None;

    for line in lines {
        for item in &mut items[line.start..line.end] {
            let target_size = size_from_axes(axes, item.target_main, item.target_cross);
            let mut input = LayoutInput::perform_layout(
                target_size.map(Some),
                parent_size,
                target_size.map(AvailableSpace::Definite),
            );
            // The parent has already applied the flex item's own sizing,
            // min/max and aspect-ratio rules to both target axes.
            input.sizing_mode = SizingMode::ContentSize;
            let output = session.compute_child_layout(source, item.key.node, input);
            let offset = relative_offset(item.inset, item.direction);
            let mut location =
                item_border_box_location(item, *line, axes, inner_size, content_origin);
            location.x += offset.x;
            location.y += offset.y;

            let mut layout = Layout::with_order(item.key.layout_order);
            layout.location = location;
            layout.size = output.size;
            layout.content_size = output.content_size;
            layout.scrollbar_size = item.scrollbar;
            layout.border = item.border;
            layout.padding = item.padding;
            layout.margin = item.margin;
            session.set_unrounded_layout(item.key.node, &layout);

            let overflow_width = output.size.width.max(output.content_size.width);
            let overflow_height = output.size.height.max(output.content_size.height);
            content_size.width = content_size.width.max(location.x + overflow_width);
            content_size.height = content_size.height.max(location.y + overflow_height);

            if first_baseline.is_none()
                && (axes.main == Axis::Vertical || item.align_self == AlignItems::Baseline)
            {
                first_baseline =
                    Some(location.y + output.first_baselines.y.unwrap_or(output.size.height));
            }
        }
    }

    if first_baseline.is_none() {
        first_baseline = first_container_baseline(items, lines, axes, inner_size, content_origin);
    }
    (content_size, first_baseline)
}

fn static_position_for_absolute(
    item: &FlexItem,
    axes: Axes,
    inner_size: Size<f32>,
    content_origin: Point<f32>,
    justify_content: AlignContent,
) -> Point<f32> {
    let free_main =
        axes.main.size(inner_size) - item.target_main - axis_sum(item.margin, axes.main);
    let (leading_main, _) = alignment_distribution(
        justify_content,
        free_main,
        1,
        axes.main_reverse,
        axes.main_base_reverse,
    );
    let main_flow = leading_main + flow_start(item.margin, axes.main, axes.main_reverse);

    let free_cross =
        axes.cross.size(inner_size) - item.target_cross - axis_sum(item.margin, axes.cross);
    let cross_alignment = match item.align_self {
        AlignItems::Start => {
            if axes.cross_reverse == axes.cross_base_reverse {
                0.0
            } else {
                free_cross
            }
        }
        AlignItems::End => {
            if axes.cross_reverse == axes.cross_base_reverse {
                free_cross
            } else {
                0.0
            }
        }
        AlignItems::FlexEnd => free_cross,
        AlignItems::Center => free_cross / 2.0,
        AlignItems::FlexStart | AlignItems::Baseline | AlignItems::Stretch => 0.0,
    };
    let cross_flow = cross_alignment + flow_start(item.margin, axes.cross, axes.cross_reverse);

    let main_border = flow_to_physical(
        main_flow,
        item.target_main,
        axes.main.size(inner_size),
        axes.main_reverse,
    ) + axes.main.point(content_origin);
    let cross_border = flow_to_physical(
        cross_flow,
        item.target_cross,
        axes.cross.size(inner_size),
        axes.cross_reverse,
    ) + axes.cross.point(content_origin);
    let mut border_origin = Point::ZERO;
    axes.main.set_point(&mut border_origin, main_border);
    axes.cross.set_point(&mut border_origin, cross_border);

    // The protocol records the hypothetical margin-box origin in the
    // parent's border-box coordinate space.
    Point::new(
        border_origin.x - item.margin.left,
        border_origin.y - item.margin.top,
    )
}

#[allow(clippy::too_many_arguments)]
fn perform_absolute_children<Source, Session>(
    source: &Source,
    session: &mut Session,
    absolute_items: &[PendingItem],
    axes: Axes,
    inner_size: Size<f32>,
    container_size: Size<f32>,
    padding: Edges<f32>,
    border: Edges<f32>,
    justify_content: AlignContent,
    default_alignment: AlignItems,
) -> Size<f32>
where
    Source: FlexSource,
    Session: LayoutSession<Source>,
{
    let content_origin = Point::new(border.left + padding.left, border.top + padding.top);
    let parent_size = inner_size.map(Some);
    let mut content_size = container_size;
    let padding_box_size = Size::new(
        (container_size.width - border.horizontal_sum()).max(0.0),
        (container_size.height - border.vertical_sum()).max(0.0),
    );

    for pending in absolute_items {
        // The borrowed view is safe across recursive session calls; only
        // layout/cache state mutates while the source epoch stays immutable.
        let style = source.flex_item_style(pending.node);
        let mut item = resolve_item(source, pending.key(), parent_size, default_alignment);
        let mut known = item.preferred_size;
        let available = inner_size.map(AvailableSpace::Definite);
        let output = child_measurement(
            source,
            session,
            pending.node,
            known,
            parent_size,
            available,
            SizingMode::InherentSize,
            RequestedAxis::Both,
        );
        let inset_size = padding_border_size(item.padding, item.border, item.scrollbar);
        known.width = Some(clamp_axis(
            output.size.width,
            item.min_size.width,
            item.max_size.width,
            inset_size.width,
        ));
        known.height = Some(clamp_axis(
            output.size.height,
            item.min_size.height,
            item.max_size.height,
            inset_size.height,
        ));
        item.target_main = axes.main.size(known).unwrap_or(0.0);
        item.target_cross = axes.cross.size(known).unwrap_or(0.0);
        let static_position =
            static_position_for_absolute(&item, axes, inner_size, content_origin, justify_content);

        match style.position() {
            Position::Absolute => {
                let static_in_padding_space = Point::new(
                    static_position.x - border.left,
                    static_position.y - border.top,
                );
                let mut layout = compute_absolute_layout(
                    source,
                    session,
                    pending.node,
                    padding_box_size,
                    static_in_padding_space,
                );
                layout.order = pending.layout_order;
                layout.location.x += border.left;
                layout.location.y += border.top;
                content_size.width = content_size
                    .width
                    .max(layout.location.x + layout.size.width.max(layout.content_size.width));
                content_size.height = content_size
                    .height
                    .max(layout.location.y + layout.size.height.max(layout.content_size.height));
                session.set_unrounded_layout(pending.node, &layout);
            }
            Position::AbsoluteHoisted => {
                session.set_static_position(pending.node, static_position);
            }
            _ => {}
        }
    }
    content_size
}

/// Computes one flex container according to CSS Flexible Box Layout §9.
///
/// Style, calc resolution, and child topology are read only from `source`;
/// recursive layout and durable geometry writes use `session`. The function
/// has no dependency on a DOM or styling engine. Child layouts are stored only
/// for [`LayoutGoal::Commit`].
#[allow(clippy::too_many_lines)]
pub fn compute_flexbox_layout<Source, Session>(
    source: &Source,
    session: &mut Session,
    node: NodeId,
    input: LayoutInput,
) -> LayoutOutput
where
    Source: FlexSource,
    Session: LayoutSession<Source>,
{
    // Unlike the former owned snapshot, this GAT view remains borrowed for
    // the whole algorithm while recursive calls mutate only `session`.
    let style = source.flex_container_style(node);
    let flex_wrap = style.flex_wrap();
    let align_content = style.align_content().unwrap_or(AlignContent::Stretch);
    let align_items = style.align_items().unwrap_or(AlignItems::Stretch);
    let justify_content = style.justify_content().unwrap_or(AlignContent::FlexStart);
    let axes = Axes::new(style.flex_direction(), flex_wrap, style.direction());
    let resolve_calc = |handle, basis| source.resolve_calc(handle, basis);
    let padding = resolve_edges(style.padding(), input.parent_size.width, &resolve_calc);
    let border = resolve_edges(style.border(), input.parent_size.width, &resolve_calc);
    let scrollbar = copied_scrollbar_size(style.overflow(), style.scrollbar_width());
    let container_inset_size = padding_border_size(padding, border, scrollbar);
    let margin = auto_edges_to_zero(resolve_optional_edges(
        style.margin(),
        input.parent_size.width,
        &resolve_calc,
    ));

    let (preferred_size, min_size, max_size) = if input.sizing_mode == SizingMode::ContentSize {
        (Size::NONE, Size::NONE, Size::NONE)
    } else {
        (
            resolve_quantitative_sizes(
                style.size(),
                input.parent_size,
                style.aspect_ratio(),
                style.box_sizing(),
                container_inset_size,
                &resolve_calc,
            ),
            resolve_quantitative_sizes(
                style.min_size(),
                input.parent_size,
                style.aspect_ratio(),
                style.box_sizing(),
                container_inset_size,
                &resolve_calc,
            ),
            resolve_quantitative_sizes(
                style.max_size(),
                input.parent_size,
                style.aspect_ratio(),
                style.box_sizing(),
                container_inset_size,
                &resolve_calc,
            ),
        )
    };

    let clamped_preferred = Size::new(
        preferred_size.width.map(|value| {
            clamp_axis(
                value,
                min_size.width,
                max_size.width,
                container_inset_size.width,
            )
        }),
        preferred_size.height.map(|value| {
            clamp_axis(
                value,
                min_size.height,
                max_size.height,
                container_inset_size.height,
            )
        }),
    );
    let mut outer_size = input.known_dimensions.or(clamped_preferred);
    let mut inner_size = Size::new(
        outer_size
            .width
            .map(|value| (value - container_inset_size.width).max(0.0)),
        outer_size
            .height
            .map(|value| (value - container_inset_size.height).max(0.0)),
    );
    let item_inline_basis_was_indefinite = inner_size.width.is_none();
    let main_percentage_basis_was_indefinite = axes.main.size(inner_size).is_none();
    let inner_available_space = Size::new(
        inner_size.width.map_or_else(
            || {
                subtract_available_space(
                    input.available_space.width,
                    margin.horizontal_sum() + container_inset_size.width,
                )
            },
            AvailableSpace::Definite,
        ),
        inner_size.height.map_or_else(
            || {
                subtract_available_space(
                    input.available_space.height,
                    margin.vertical_sum() + container_inset_size.height,
                )
            },
            AvailableSpace::Definite,
        ),
    );
    let gap_value = style.gap();
    let mut gap = Size::new(
        resolve_length_percentage(gap_value.width, inner_size.width, &resolve_calc)
            .unwrap_or(0.0)
            .max(0.0),
        resolve_length_percentage(gap_value.height, inner_size.height, &resolve_calc)
            .unwrap_or(0.0)
            .max(0.0),
    );
    let mut generated = Vec::new();
    let mut absolute_items = Vec::new();
    let mut hidden = Vec::new();
    let child_count = source.child_count(node);
    for document_index in 0..child_count {
        let child = source.child_id(node, document_index);
        let child_style = source.flex_item_style(child);
        if child_style.box_generation_mode() == BoxGenerationMode::None {
            hidden.push((document_index, child));
            continue;
        }
        let pending = PendingItem {
            node: child,
            document_index,
            css_order: child_style.order(),
            layout_order: u32::try_from(document_index).unwrap_or(u32::MAX),
        };
        if matches!(
            child_style.position(),
            Position::Absolute | Position::AbsoluteHoisted
        ) {
            absolute_items.push(pending);
        } else {
            generated.push(pending);
        }
    }
    generated.sort_by(|left, right| match left.css_order.cmp(&right.css_order) {
        Ordering::Equal => left.document_index.cmp(&right.document_index),
        ordering => ordering,
    });
    let mut paint_order = generated
        .iter()
        .map(|item| (item.css_order, item.document_index, item.node))
        .chain(
            absolute_items
                .iter()
                .map(|item| (0, item.document_index, item.node)),
        )
        .collect::<Vec<_>>();
    paint_order.sort_by_key(|&(order, document_index, _)| (order, document_index));
    for (layout_order, &(_, _, ordered_node)) in paint_order.iter().enumerate() {
        let layout_order = u32::try_from(layout_order).unwrap_or(u32::MAX);
        if let Some(item) = generated.iter_mut().find(|item| item.node == ordered_node) {
            item.layout_order = layout_order;
        } else if let Some(item) = absolute_items
            .iter_mut()
            .find(|item| item.node == ordered_node)
        {
            item.layout_order = layout_order;
        }
    }

    let mut items = generated
        .into_iter()
        .map(|item| resolve_item(source, item.key(), inner_size, align_items))
        .collect::<Vec<_>>();
    determine_flex_base_sizes(
        source,
        session,
        &mut items,
        axes,
        inner_size,
        inner_available_space,
        axes.main.size(inner_size),
    );

    let main_gap = axes.main.size(gap);
    let line_available_main = axes.main.size(inner_size).map_or_else(
        || axes.main.size(inner_available_space),
        AvailableSpace::Definite,
    );
    let mut lines = collect_flex_lines(&items, flex_wrap, line_available_main, main_gap, axes);

    let inset_main = axes.main.size(container_inset_size);
    if axes.main.size(outer_size).is_none() {
        let outer_main = determine_auto_main_size(
            &items,
            &lines,
            main_gap,
            axes,
            line_available_main,
            inset_main,
            axes.main.size(min_size),
            axes.main.size(max_size),
        );
        axes.main.set_size(&mut outer_size, Some(outer_main));
        axes.main
            .set_size(&mut inner_size, Some((outer_main - inset_main).max(0.0)));
        let resolve_calc = |handle, basis| source.resolve_calc(handle, basis);
        let resolved_main_gap = resolve_length_percentage(
            axes.main.size(gap_value),
            axes.main.size(inner_size),
            &resolve_calc,
        )
        .unwrap_or(0.0)
        .max(0.0);
        axes.main.set_size(&mut gap, resolved_main_gap);
    }
    let inner_main = axes.main.size(inner_size).unwrap_or(0.0);
    let mut main_gap = axes.main.size(gap);
    for line in lines.iter().copied() {
        resolve_flexible_lengths(&mut items, line, inner_main, main_gap, axes);
    }

    determine_hypothetical_cross_sizes(
        source,
        session,
        &mut items,
        &lines,
        axes,
        inner_size,
        inner_available_space,
    );
    calculate_line_cross_sizes(
        &items,
        &mut lines,
        axes,
        flex_wrap,
        axes.cross.size(inner_size),
    );

    let cross_was_definite = axes.cross.size(outer_size).is_some();
    let inset_cross = axes.cross.size(container_inset_size);
    if !cross_was_definite {
        let outer_cross = determine_auto_cross_size(
            &lines,
            axes.cross.size(gap),
            inset_cross,
            axes.cross.size(min_size),
            axes.cross.size(max_size),
            axes,
            axes.cross.size(inner_available_space),
        );
        axes.cross.set_size(&mut outer_size, Some(outer_cross));
        axes.cross
            .set_size(&mut inner_size, Some((outer_cross - inset_cross).max(0.0)));
    }
    if cross_was_definite {
        let resolve_calc = |handle, basis| source.resolve_calc(handle, basis);
        let resolved_cross_gap = resolve_length_percentage(
            axes.cross.size(gap_value),
            axes.cross.size(inner_size),
            &resolve_calc,
        )
        .unwrap_or(0.0)
        .max(0.0);
        axes.cross.set_size(&mut gap, resolved_cross_gap);
    }
    let inner_cross = axes.cross.size(inner_size).unwrap_or(0.0);
    if item_inline_basis_was_indefinite {
        // Cyclic percentages contribute zero to intrinsic sizing, but their
        // used values resolve against the resulting content-box width. Run
        // the item/line phases once more with that now-definite basis while
        // keeping the intrinsic container size fixed.
        let resolve_calc = |handle, basis| source.resolve_calc(handle, basis);
        gap = Size::new(
            resolve_length_percentage(gap_value.width, inner_size.width, &resolve_calc)
                .unwrap_or(0.0)
                .max(0.0),
            resolve_length_percentage(gap_value.height, inner_size.height, &resolve_calc)
                .unwrap_or(0.0)
                .max(0.0),
        );
        main_gap = axes.main.size(gap);
        // Re-resolve compact scratch in place. Raw style is fetched by NodeId;
        // no second full-style snapshot or parallel style Vec is needed.
        for item in &mut items {
            let key = item.key;
            *item = resolve_item(source, key, inner_size, align_items);
        }
        let final_available_space = Size::new(
            AvailableSpace::Definite(inner_size.width.unwrap_or(0.0)),
            AvailableSpace::Definite(inner_size.height.unwrap_or(0.0)),
        );
        determine_flex_base_sizes(
            source,
            session,
            &mut items,
            axes,
            inner_size,
            final_available_space,
            if main_percentage_basis_was_indefinite {
                None
            } else {
                axes.main.size(inner_size)
            },
        );
        lines = collect_flex_lines(
            &items,
            flex_wrap,
            AvailableSpace::Definite(inner_main),
            main_gap,
            axes,
        );
        for line in lines.iter().copied() {
            resolve_flexible_lengths(&mut items, line, inner_main, main_gap, axes);
        }
        determine_hypothetical_cross_sizes(
            source,
            session,
            &mut items,
            &lines,
            axes,
            inner_size,
            final_available_space,
        );
        calculate_line_cross_sizes(&items, &mut lines, axes, flex_wrap, Some(inner_cross));
    }
    let cross_gap = axes.cross.size(gap);
    if flex_wrap == FlexWrap::NoWrap
        && let Some(line) = lines.first_mut()
    {
        line.cross_size = inner_cross;
    }
    stretch_lines(&mut lines, flex_wrap, align_content, inner_cross, cross_gap);
    determine_used_cross_sizes(&mut items, &lines, axes);
    distribute_main_axis(
        &mut items,
        &lines,
        axes,
        inner_main,
        main_gap,
        justify_content,
    );
    align_lines(
        &mut lines,
        axes,
        flex_wrap,
        align_content,
        inner_cross,
        cross_gap,
    );
    align_items_cross_axis(&mut items, &lines, axes);

    let outer_size = outer_size.unwrap_or(Size::ZERO);
    let inner_size = inner_size.unwrap_or(Size::ZERO);
    let content_origin = Point::new(border.left + padding.left, border.top + padding.top);
    let provisional_baseline =
        first_container_baseline(&items, &lines, axes, inner_size, content_origin);
    if matches!(input.goal, LayoutGoal::Measure(_)) {
        return LayoutOutput::new(outer_size, outer_size)
            .with_first_baselines(Point::new(None, provisional_baseline));
    }

    let (mut content_size, first_baseline) = perform_in_flow_layout(
        source,
        session,
        &mut items,
        &lines,
        axes,
        inner_size,
        content_origin,
        outer_size,
    );
    for (document_index, child) in hidden {
        super::hide_subtree(source, session, child);
        session.set_unrounded_layout(
            child,
            &Layout::with_order(u32::try_from(document_index).unwrap_or(u32::MAX)),
        );
    }
    let absolute_content_size = perform_absolute_children(
        source,
        session,
        &absolute_items,
        axes,
        inner_size,
        outer_size,
        padding,
        border,
        justify_content,
        align_items,
    );
    content_size = content_size.zip_map(absolute_content_size, f32::max);

    LayoutOutput::new(outer_size, content_size)
        .with_first_baselines(Point::new(None, first_baseline.or(provisional_baseline)))
}
