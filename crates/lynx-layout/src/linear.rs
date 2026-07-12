//! Starlight linear layout.
//!
//! Linear is a Lynx-only, single-axis formatting context. It deliberately
//! lives in the host adapter instead of neutron-star: the latter supplies the
//! generic box model, cache, positioned-layout, and tree/session protocols,
//! while this module owns `linear-*` vocabulary and behavior.

// Item counts are transient `Vec` lengths and layout coordinates are `f32`.
// Converting a practical child count to `f32` for space distribution is safe.
#![allow(clippy::cast_precision_loss)]

use core::cmp::Ordering;

use neutron_star::compute::support::{
    apply_aspect_ratio, clamp_axis, padding_border_size, preferred_size_definiteness,
    relative_offset, resolve_edges, resolve_insets, resolve_length_percentage,
    resolve_optional_edges, resolve_quantitative_sizes, scrollbar_size, subtract_available_space,
};
use neutron_star::compute::{compute_absolute_layout, hide_subtree, measure_absolute_layout};
use neutron_star::geometry::{Edges, Point, Size};
use neutron_star::style::{
    AlignItems, BoxGenerationMode, BoxSizing, CoreStyle, Dimension, Direction, JustifyContent,
    LengthPercentage, LengthPercentageAuto, Position,
};
use neutron_star::tree::{
    AvailableSpace, Layout, LayoutGoal, LayoutInput, LayoutOutput, LayoutSession, NodeId,
    RequestedAxis, SizingMode,
};

use crate::style::{
    LinearContainerStyle, LinearCrossGravity, LinearGravity, LinearItemStyle, LinearLayoutGravity,
    LinearOrientation,
};
use crate::tree::LinearSource;

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
}

/// Main/cross mapping. Scratch coordinates remain flow-relative and are
/// converted to physical coordinates only when geometry is exported.
#[derive(Debug, Clone, Copy)]
struct LinearAxes {
    main: Axis,
    cross: Axis,
    main_reverse: bool,
    cross_reverse: bool,
}

impl LinearAxes {
    #[inline]
    fn new(orientation: LinearOrientation, direction: Direction) -> Self {
        let horizontal = orientation.is_horizontal();
        let rtl = direction == Direction::Rtl;
        Self {
            main: if horizontal {
                Axis::Horizontal
            } else {
                Axis::Vertical
            },
            cross: if horizontal {
                Axis::Vertical
            } else {
                Axis::Horizontal
            },
            main_reverse: orientation.is_reverse() ^ (horizontal && rtl),
            cross_reverse: !horizontal && rtl,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CrossGravity {
    None,
    Start,
    End,
    Center,
    Stretch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MainGravity {
    Start,
    End,
    Center,
    SpaceBetween,
}

#[derive(Debug, Clone, Copy)]
struct ItemKey {
    node: NodeId,
    document_index: usize,
    css_order: i32,
    layout_order: u32,
}

/// One allocation-friendly scratch record per in-flow item. Raw style remains
/// in the immutable source and is reborrowed only for intrinsic probes or a
/// cyclic percentage re-resolution.
#[derive(Debug)]
#[allow(clippy::struct_excessive_bools)]
struct LinearItem {
    key: ItemKey,
    direction: Direction,
    gravity: CrossGravity,
    weight: f32,
    size_is_auto: Size<bool>,
    size_is_intrinsic: Size<bool>,
    preferred_size: Size<Option<f32>>,
    preferred_size_is_definite: Size<bool>,
    min_size: Size<Option<f32>>,
    max_size: Size<Option<f32>>,
    margin: Edges<f32>,
    margin_auto: Edges<bool>,
    padding: Edges<f32>,
    border: Edges<f32>,
    scrollbar: Size<f32>,
    inset: Edges<Option<f32>>,
    box_sizing: BoxSizing,
    aspect_ratio: Option<f32>,
    main_size: f32,
    cross_size: f32,
    main_position: f32,
    cross_position: f32,
    first_baselines: Point<Option<f32>>,
    main_size_is_definite: bool,
    cross_size_is_definite: bool,
    frozen: bool,
    violation: f32,
    depends_on_inline_basis: bool,
}

#[inline]
fn size_from_axes<T: Copy>(axes: LinearAxes, main: T, cross: T) -> Size<T> {
    match axes.main {
        Axis::Horizontal => Size::new(main, cross),
        Axis::Vertical => Size::new(cross, main),
    }
}

#[inline]
fn physical_start<T: Copy>(edges: Edges<T>, axis: Axis) -> T {
    match axis {
        Axis::Horizontal => edges.left,
        Axis::Vertical => edges.top,
    }
}

#[inline]
fn physical_end<T: Copy>(edges: Edges<T>, axis: Axis) -> T {
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
fn flow_start<T: Copy>(edges: Edges<T>, axis: Axis, reverse: bool) -> T {
    if reverse {
        physical_end(edges, axis)
    } else {
        physical_start(edges, axis)
    }
}

#[inline]
fn flow_end<T: Copy>(edges: Edges<T>, axis: Axis, reverse: bool) -> T {
    if reverse {
        physical_start(edges, axis)
    } else {
        physical_end(edges, axis)
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
fn flow_to_physical(flow: f32, box_size: f32, container_size: f32, reverse: bool) -> f32 {
    if reverse {
        container_size - flow - box_size
    } else {
        flow
    }
}

#[inline]
fn dimension_is_intrinsic(value: Dimension) -> bool {
    matches!(
        value,
        Dimension::MinContent | Dimension::MaxContent | Dimension::FitContent(_)
    )
}

#[inline]
fn length_depends_on_basis(value: LengthPercentage) -> bool {
    matches!(
        value,
        LengthPercentage::Percent(_) | LengthPercentage::Calc(_)
    )
}

#[inline]
fn auto_length_depends_on_basis(value: LengthPercentageAuto) -> bool {
    matches!(
        value,
        LengthPercentageAuto::Percent(_) | LengthPercentageAuto::Calc(_)
    )
}

#[inline]
fn width_dimension_depends_on_basis(value: Dimension) -> bool {
    matches!(value, Dimension::Percent(_) | Dimension::Calc(_))
        || matches!(value, Dimension::FitContent(limit) if length_depends_on_basis(limit))
}

#[allow(clippy::too_many_arguments)]
fn style_depends_on_inline_basis(
    size: Size<Dimension>,
    min_size: Size<Dimension>,
    max_size: Size<Dimension>,
    margin: Edges<LengthPercentageAuto>,
    padding: Edges<LengthPercentage>,
    border: Edges<LengthPercentage>,
    inset: Edges<LengthPercentageAuto>,
) -> bool {
    width_dimension_depends_on_basis(size.width)
        || width_dimension_depends_on_basis(min_size.width)
        || width_dimension_depends_on_basis(max_size.width)
        || auto_length_depends_on_basis(margin.left)
        || auto_length_depends_on_basis(margin.right)
        || auto_length_depends_on_basis(margin.top)
        || auto_length_depends_on_basis(margin.bottom)
        || length_depends_on_basis(padding.left)
        || length_depends_on_basis(padding.right)
        || length_depends_on_basis(padding.top)
        || length_depends_on_basis(padding.bottom)
        || length_depends_on_basis(border.left)
        || length_depends_on_basis(border.right)
        || length_depends_on_basis(border.top)
        || length_depends_on_basis(border.bottom)
        || auto_length_depends_on_basis(inset.left)
        || auto_length_depends_on_basis(inset.right)
}

#[inline]
fn map_align(value: AlignItems) -> CrossGravity {
    match value {
        AlignItems::Stretch => CrossGravity::Stretch,
        AlignItems::Start | AlignItems::FlexStart => CrossGravity::Start,
        AlignItems::End | AlignItems::FlexEnd => CrossGravity::End,
        AlignItems::Center => CrossGravity::Center,
        AlignItems::Baseline => CrossGravity::None,
    }
}

fn computed_cross_gravity(
    mut item: LinearLayoutGravity,
    align_self: Option<AlignItems>,
    container_cross: LinearCrossGravity,
    align_items: Option<AlignItems>,
    axes: LinearAxes,
) -> CrossGravity {
    if item == LinearLayoutGravity::None
        && let Some(value) = align_self
    {
        let mapped = map_align(value);
        if mapped != CrossGravity::None {
            return mapped;
        }
    }

    if item == LinearLayoutGravity::None {
        let mapped = match container_cross {
            LinearCrossGravity::None => CrossGravity::None,
            LinearCrossGravity::Start => CrossGravity::Start,
            LinearCrossGravity::End => CrossGravity::End,
            LinearCrossGravity::Center => CrossGravity::Center,
            LinearCrossGravity::Stretch => CrossGravity::Stretch,
        };
        if mapped != CrossGravity::None {
            return mapped;
        }
    }

    if item == LinearLayoutGravity::None
        && let Some(value) = align_items
        && value != AlignItems::Stretch
    {
        let mapped = map_align(value);
        if mapped != CrossGravity::None {
            return mapped;
        }
    }

    // In a vertical RTL container logical cross-start is the physical right.
    // Swapping the physical aliases before classifying them preserves their
    // physical meaning through the later flow-to-physical conversion.
    if axes.main == Axis::Vertical && axes.cross_reverse {
        item = match item {
            LinearLayoutGravity::Left => LinearLayoutGravity::Right,
            LinearLayoutGravity::Right => LinearLayoutGravity::Left,
            other => other,
        };
    }

    match item {
        LinearLayoutGravity::None => CrossGravity::None,
        LinearLayoutGravity::Start | LinearLayoutGravity::Left | LinearLayoutGravity::Top => {
            CrossGravity::Start
        }
        LinearLayoutGravity::End | LinearLayoutGravity::Right | LinearLayoutGravity::Bottom => {
            CrossGravity::End
        }
        LinearLayoutGravity::Center
        | LinearLayoutGravity::CenterHorizontal
        | LinearLayoutGravity::CenterVertical => CrossGravity::Center,
        LinearLayoutGravity::Stretch
        | LinearLayoutGravity::FillHorizontal
        | LinearLayoutGravity::FillVertical => CrossGravity::Stretch,
    }
}

fn computed_main_gravity(
    gravity: LinearGravity,
    justify_content: Option<JustifyContent>,
    axes: LinearAxes,
) -> MainGravity {
    let gravity = if gravity == LinearGravity::None {
        return match justify_content {
            Some(JustifyContent::End | JustifyContent::FlexEnd) => MainGravity::End,
            Some(JustifyContent::Center) => MainGravity::Center,
            Some(JustifyContent::SpaceBetween) => MainGravity::SpaceBetween,
            None
            | Some(
                JustifyContent::Start
                | JustifyContent::FlexStart
                | JustifyContent::Stretch
                | JustifyContent::SpaceAround
                | JustifyContent::SpaceEvenly,
            ) => MainGravity::Start,
        };
    } else {
        gravity
    };

    match gravity {
        LinearGravity::None | LinearGravity::Start => MainGravity::Start,
        LinearGravity::End => MainGravity::End,
        LinearGravity::Center | LinearGravity::CenterHorizontal | LinearGravity::CenterVertical => {
            MainGravity::Center
        }
        LinearGravity::SpaceBetween => MainGravity::SpaceBetween,
        LinearGravity::Left if axes.main == Axis::Horizontal && axes.main_reverse => {
            MainGravity::End
        }
        LinearGravity::Right if axes.main == Axis::Horizontal && !axes.main_reverse => {
            MainGravity::End
        }
        LinearGravity::Top if axes.main == Axis::Vertical && axes.main_reverse => MainGravity::End,
        LinearGravity::Bottom if axes.main == Axis::Vertical && !axes.main_reverse => {
            MainGravity::End
        }
        LinearGravity::Left | LinearGravity::Right | LinearGravity::Top | LinearGravity::Bottom => {
            MainGravity::Start
        }
    }
}

fn resolve_item<Source: LinearSource>(
    source: &Source,
    style: impl LinearItemStyle,
    key: ItemKey,
    percentage_basis: Size<Option<f32>>,
    container_cross: LinearCrossGravity,
    align_items: Option<AlignItems>,
    axes: LinearAxes,
) -> LinearItem {
    let resolve_calc = |handle, basis| source.resolve_calc(handle, basis);
    let size_value = style.size();
    let min_size_value = style.min_size();
    let max_size_value = style.max_size();
    let padding_value = style.padding();
    let border_value = style.border();
    let inset_value = style.inset();
    let padding = resolve_edges(padding_value, percentage_basis.width, &resolve_calc);
    let border = resolve_edges(border_value, percentage_basis.width, &resolve_calc);
    let scrollbar = scrollbar_size(&style);
    let inset_size = padding_border_size(padding, border, scrollbar);
    let aspect_ratio = style.aspect_ratio();
    let box_sizing = style.box_sizing();
    let preferred_size = resolve_quantitative_sizes(
        size_value,
        percentage_basis,
        aspect_ratio,
        box_sizing,
        inset_size,
        &resolve_calc,
    );
    let min_size = resolve_quantitative_sizes(
        min_size_value,
        percentage_basis,
        aspect_ratio,
        box_sizing,
        inset_size,
        &resolve_calc,
    );
    let max_size = resolve_quantitative_sizes(
        max_size_value,
        percentage_basis,
        aspect_ratio,
        box_sizing,
        inset_size,
        &resolve_calc,
    );
    let margin_value = style.margin();
    let optional_margin =
        resolve_optional_edges(margin_value, percentage_basis.width, &resolve_calc);
    let weight = style.linear_weight();
    debug_assert!(
        weight.is_finite() && weight >= 0.0,
        "linear-weight must be finite and non-negative"
    );

    LinearItem {
        key,
        direction: style.direction(),
        gravity: computed_cross_gravity(
            style.linear_layout_gravity(),
            style.align_self(),
            container_cross,
            align_items,
            axes,
        ),
        weight,
        size_is_auto: size_value.map(Dimension::is_auto),
        size_is_intrinsic: size_value.map(dimension_is_intrinsic),
        preferred_size,
        preferred_size_is_definite: preferred_size_definiteness(
            size_value,
            percentage_basis,
            aspect_ratio,
        ),
        min_size,
        max_size,
        margin: optional_margin.map(|value| value.unwrap_or(0.0)),
        margin_auto: margin_value.map(LengthPercentageAuto::is_auto),
        padding,
        border,
        scrollbar,
        inset: resolve_insets(inset_value, percentage_basis, &resolve_calc),
        box_sizing,
        aspect_ratio,
        main_size: 0.0,
        cross_size: 0.0,
        main_position: 0.0,
        cross_position: 0.0,
        first_baselines: Point::NONE,
        main_size_is_definite: false,
        cross_size_is_definite: false,
        frozen: false,
        violation: 0.0,
        depends_on_inline_basis: style_depends_on_inline_basis(
            size_value,
            min_size_value,
            max_size_value,
            margin_value,
            padding_value,
            border_value,
            inset_value,
        ),
    }
}

#[allow(clippy::too_many_arguments)]
fn child_measurement<Source, Session>(
    source: &Source,
    session: &mut Session,
    node: NodeId,
    known_dimensions: Size<Option<f32>>,
    definite_dimensions: Size<bool>,
    parent_size: Size<Option<f32>>,
    available_space: Size<AvailableSpace>,
    sizing_mode: SizingMode,
    requested_axis: RequestedAxis,
) -> LayoutOutput
where
    Source: LinearSource,
    Session: LayoutSession<Source>,
{
    let mut input = LayoutInput::compute_size(
        known_dimensions,
        parent_size,
        available_space,
        requested_axis,
    );
    input.definite_dimensions = definite_dimensions;
    input.sizing_mode = sizing_mode;
    session.compute_child_layout(source, node, input)
}

#[inline]
fn has_intrinsic_dimension(values: [Dimension; 3]) -> bool {
    values.into_iter().any(dimension_is_intrinsic)
}

#[inline]
fn needs_max_content(values: [Dimension; 3]) -> bool {
    values
        .into_iter()
        .any(|value| matches!(value, Dimension::MaxContent | Dimension::FitContent(_)))
}

fn intrinsic_measurement<Source, Session>(
    source: &Source,
    session: &mut Session,
    item: &LinearItem,
    percentage_basis: Size<Option<f32>>,
    requested: Size<bool>,
    target_available: AvailableSpace,
) -> LayoutOutput
where
    Source: LinearSource,
    Session: LayoutSession<Source>,
{
    let inset = padding_border_size(item.padding, item.border, item.scrollbar);
    let resolved_known = Size::new(
        item.preferred_size
            .width
            .map(|value| clamp_axis(value, item.min_size.width, item.max_size.width, inset.width)),
        item.preferred_size.height.map(|value| {
            clamp_axis(
                value,
                item.min_size.height,
                item.max_size.height,
                inset.height,
            )
        }),
    );
    let known = Size::new(
        (!requested.width).then_some(resolved_known.width).flatten(),
        (!requested.height)
            .then_some(resolved_known.height)
            .flatten(),
    );
    let definite = Size::new(
        !requested.width && item.preferred_size_is_definite.width && known.width.is_some(),
        !requested.height && item.preferred_size_is_definite.height && known.height.is_some(),
    );
    let available = Size::new(
        if requested.width {
            target_available
        } else {
            known
                .width
                .map_or(AvailableSpace::MaxContent, AvailableSpace::Definite)
        },
        if requested.height {
            target_available
        } else {
            known
                .height
                .map_or(AvailableSpace::MaxContent, AvailableSpace::Definite)
        },
    );
    let requested_axis = match (requested.width, requested.height) {
        (true, false) => RequestedAxis::Horizontal,
        (false, true) => RequestedAxis::Vertical,
        (true, true) => RequestedAxis::Both,
        (false, false) => unreachable!("an intrinsic probe must request at least one axis"),
    };
    child_measurement(
        source,
        session,
        item.key.node,
        known,
        definite,
        percentage_basis,
        available,
        SizingMode::ContentSize,
        requested_axis,
    )
}

#[inline]
#[allow(clippy::too_many_arguments)]
fn intrinsic_axis_value<Source: LinearSource>(
    source: &Source,
    value: Dimension,
    quantitative: Option<f32>,
    minimum: f32,
    maximum: f32,
    basis: Option<f32>,
    inset: f32,
    box_sizing: BoxSizing,
) -> Option<f32> {
    match value {
        Dimension::MinContent => Some(minimum),
        Dimension::MaxContent => Some(maximum),
        Dimension::FitContent(limit) => {
            let resolve_calc = |handle, basis| source.resolve_calc(handle, basis);
            let mut limit =
                resolve_length_percentage(limit, basis, &resolve_calc).unwrap_or(maximum);
            if box_sizing == BoxSizing::ContentBox {
                limit += inset;
            }
            Some(maximum.min(limit.max(minimum)))
        }
        Dimension::Length(_) | Dimension::Percent(_) | Dimension::Calc(_) | Dimension::Auto => {
            quantitative
        }
    }
}

#[allow(clippy::too_many_lines)]
fn resolve_intrinsic_sizes<Source, Session>(
    source: &Source,
    session: &mut Session,
    item: &mut LinearItem,
    percentage_basis: Size<Option<f32>>,
) where
    Source: LinearSource,
    Session: LayoutSession<Source>,
{
    let (size, min_size, max_size) = {
        let style = source.linear_item_style(item.key.node);
        (style.size(), style.min_size(), style.max_size())
    };
    let need_min = Size::new(
        has_intrinsic_dimension([size.width, min_size.width, max_size.width]),
        has_intrinsic_dimension([size.height, min_size.height, max_size.height]),
    );
    if !need_min.width && !need_min.height {
        return;
    }

    let min_output = intrinsic_measurement(
        source,
        session,
        item,
        percentage_basis,
        need_min,
        AvailableSpace::MinContent,
    );
    let need_max = Size::new(
        needs_max_content([size.width, min_size.width, max_size.width]),
        needs_max_content([size.height, min_size.height, max_size.height]),
    );
    let max_output = if need_max.width || need_max.height {
        intrinsic_measurement(
            source,
            session,
            item,
            percentage_basis,
            need_max,
            AvailableSpace::MaxContent,
        )
    } else {
        min_output
    };
    let max_content = Size::new(
        if need_max.width {
            max_output.size.width
        } else {
            min_output.size.width
        },
        if need_max.height {
            max_output.size.height
        } else {
            min_output.size.height
        },
    );
    let inset = padding_border_size(item.padding, item.border, item.scrollbar);

    item.preferred_size = Size::new(
        intrinsic_axis_value(
            source,
            size.width,
            item.preferred_size.width,
            min_output.size.width,
            max_content.width,
            percentage_basis.width,
            inset.width,
            item.box_sizing,
        ),
        intrinsic_axis_value(
            source,
            size.height,
            item.preferred_size.height,
            min_output.size.height,
            max_content.height,
            percentage_basis.height,
            inset.height,
            item.box_sizing,
        ),
    );
    item.min_size = Size::new(
        intrinsic_axis_value(
            source,
            min_size.width,
            item.min_size.width,
            min_output.size.width,
            max_content.width,
            percentage_basis.width,
            inset.width,
            item.box_sizing,
        ),
        intrinsic_axis_value(
            source,
            min_size.height,
            item.min_size.height,
            min_output.size.height,
            max_content.height,
            percentage_basis.height,
            inset.height,
            item.box_sizing,
        ),
    );
    item.max_size = Size::new(
        intrinsic_axis_value(
            source,
            max_size.width,
            item.max_size.width,
            min_output.size.width,
            max_content.width,
            percentage_basis.width,
            inset.width,
            item.box_sizing,
        ),
        intrinsic_axis_value(
            source,
            max_size.height,
            item.max_size.height,
            min_output.size.height,
            max_content.height,
            percentage_basis.height,
            inset.height,
            item.box_sizing,
        ),
    );
    item.preferred_size = apply_aspect_ratio(item.preferred_size, item.aspect_ratio);
}

#[inline]
fn ratio_cross_size(item: &LinearItem, axes: LinearAxes, forced_main: f32) -> Option<f32> {
    let ratio = item.aspect_ratio?;
    if !ratio.is_finite() || ratio <= 0.0 || !axes.cross.size(item.size_is_auto) {
        return None;
    }
    let inset = padding_border_size(item.padding, item.border, item.scrollbar);
    let sizing_main = if item.box_sizing == BoxSizing::ContentBox {
        (forced_main - axes.main.size(inset)).max(0.0)
    } else {
        forced_main
    };
    let sizing_cross = if axes.main == Axis::Horizontal {
        sizing_main / ratio
    } else {
        sizing_main * ratio
    };
    Some(if item.box_sizing == BoxSizing::ContentBox {
        sizing_cross + axes.cross.size(inset)
    } else {
        sizing_cross
    })
}

#[inline]
fn apply_border_box_ratio(
    mut size: Size<Option<f32>>,
    aspect_ratio: Option<f32>,
    box_sizing: BoxSizing,
    inset: Size<f32>,
) -> Size<Option<f32>> {
    let Some(ratio) = aspect_ratio else {
        return size;
    };
    if !ratio.is_finite() || ratio <= 0.0 {
        return size;
    }
    match (size.width, size.height) {
        (Some(width), None) => {
            let sizing_width = if box_sizing == BoxSizing::ContentBox {
                (width - inset.width).max(0.0)
            } else {
                width
            };
            let sizing_height = sizing_width / ratio;
            size.height = Some(if box_sizing == BoxSizing::ContentBox {
                sizing_height + inset.height
            } else {
                sizing_height
            });
        }
        (None, Some(height)) => {
            let sizing_height = if box_sizing == BoxSizing::ContentBox {
                (height - inset.height).max(0.0)
            } else {
                height
            };
            let sizing_width = sizing_height * ratio;
            size.width = Some(if box_sizing == BoxSizing::ContentBox {
                sizing_width + inset.width
            } else {
                sizing_width
            });
        }
        _ => {}
    }
    size
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
fn measure_item<Source, Session>(
    source: &Source,
    session: &mut Session,
    item: &mut LinearItem,
    axes: LinearAxes,
    percentage_basis: Size<Option<f32>>,
    definite_inner_size: Size<Option<f32>>,
    available_space: Size<AvailableSpace>,
    forced_main: Option<f32>,
) where
    Source: LinearSource,
    Session: LayoutSession<Source>,
{
    let inset = padding_border_size(item.padding, item.border, item.scrollbar);
    let main_floor = axes.main.size(inset);
    let cross_floor = axes.cross.size(inset);
    let mut known = item.preferred_size;
    let mut known_definite = item.preferred_size_is_definite;

    if let Some(main) = forced_main {
        axes.main.set_size(&mut known, Some(main));
        axes.main.set_size(&mut known_definite, true);
        if let Some(ratio_cross) = ratio_cross_size(item, axes, main) {
            axes.cross.set_size(
                &mut known,
                Some(clamp_axis(
                    ratio_cross,
                    axes.cross.size(item.min_size),
                    axes.cross.size(item.max_size),
                    cross_floor,
                )),
            );
            axes.cross.set_size(&mut known_definite, true);
        }
    }

    if let Some(value) = axes.main.size(known) {
        axes.main.set_size(
            &mut known,
            Some(clamp_axis(
                value,
                axes.main.size(item.min_size),
                axes.main.size(item.max_size),
                main_floor,
            )),
        );
    }
    if let Some(value) = axes.cross.size(known) {
        axes.cross.set_size(
            &mut known,
            Some(clamp_axis(
                value,
                axes.cross.size(item.min_size),
                axes.cross.size(item.max_size),
                cross_floor,
            )),
        );
    }

    let cross_constraint = axes.cross.size(definite_inner_size);
    let ratio_fixed_cross = forced_main
        .and_then(|main| ratio_cross_size(item, axes, main))
        .is_some();
    let should_stretch = cross_constraint.is_some()
        && (item.gravity == CrossGravity::Stretch
            || (item.gravity == CrossGravity::None
                && axes.cross.size(item.size_is_auto)
                && !axes.cross.size(item.size_is_intrinsic)
                && !ratio_fixed_cross));
    if should_stretch {
        let stretched =
            (cross_constraint.unwrap_or(0.0) - axis_sum(item.margin, axes.cross)).max(0.0);
        axes.cross.set_size(
            &mut known,
            Some(clamp_axis(
                stretched,
                axes.cross.size(item.min_size),
                axes.cross.size(item.max_size),
                cross_floor,
            )),
        );
        axes.cross.set_size(&mut known_definite, true);
    }

    let intrinsic_main_available = match axes.main.size(available_space) {
        AvailableSpace::MinContent => AvailableSpace::MinContent,
        AvailableSpace::Definite(_) | AvailableSpace::MaxContent => AvailableSpace::MaxContent,
    };
    let child_available = size_from_axes(
        axes,
        axes.main
            .size(known)
            .map_or(intrinsic_main_available, AvailableSpace::Definite),
        axes.cross.size(known).map_or_else(
            || {
                cross_constraint.map_or_else(
                    || axes.cross.size(available_space),
                    AvailableSpace::Definite,
                )
            },
            AvailableSpace::Definite,
        ),
    );
    let output = child_measurement(
        source,
        session,
        item.key.node,
        known,
        known_definite,
        percentage_basis,
        child_available,
        SizingMode::InherentSize,
        RequestedAxis::Both,
    );
    item.main_size = forced_main.unwrap_or_else(|| {
        clamp_axis(
            axes.main.size(output.size),
            axes.main.size(item.min_size),
            axes.main.size(item.max_size),
            main_floor,
        )
    });
    item.cross_size = clamp_axis(
        axes.cross.size(output.size),
        axes.cross.size(item.min_size),
        axes.cross.size(item.max_size),
        cross_floor,
    );
    item.first_baselines = output.first_baselines;
    item.main_size_is_definite = forced_main.is_some() || axes.main.size(known_definite);
    item.cross_size_is_definite = axes.cross.size(known_definite);
}

#[inline]
fn outer_main(item: &LinearItem, axes: LinearAxes) -> f32 {
    item.main_size + axis_sum(item.margin, axes.main)
}

#[inline]
fn outer_cross(item: &LinearItem, axes: LinearAxes) -> f32 {
    item.cross_size + axis_sum(item.margin, axes.cross)
}

/// Starlight uses the same signed-violation freeze rule as Flexbox, but with
/// zero bases and positive weights only. The loop performs no child layout and
/// needs no side vectors; every pass either freezes at least one item or exits.
fn distribute_weighted_items(
    items: &mut [LinearItem],
    axes: LinearAxes,
    inner_main: f32,
    weight_sum_override: f32,
) {
    let total_weight = items
        .iter()
        .filter(|item| item.weight > 0.0)
        .map(|item| item.weight)
        .sum::<f32>();
    if total_weight <= 0.0 {
        return;
    }

    let fixed_outer = items
        .iter()
        .filter(|item| item.weight <= 0.0)
        .map(|item| outer_main(item, axes))
        .sum::<f32>();
    let weighted_margins = items
        .iter()
        .filter(|item| item.weight > 0.0)
        .map(|item| axis_sum(item.margin, axes.main))
        .sum::<f32>();
    let initial_free_space = inner_main - fixed_outer - weighted_margins;
    let mut active_weight = total_weight;

    for item in items.iter_mut().filter(|item| item.weight > 0.0) {
        item.main_size = 0.0;
        item.frozen = false;
        item.violation = 0.0;
    }

    let weighted_count = items.iter().filter(|item| item.weight > 0.0).count();
    for _ in 0..=weighted_count {
        if active_weight <= 0.0 {
            return;
        }
        let frozen_size = items
            .iter()
            .filter(|item| item.weight > 0.0 && item.frozen)
            .map(|item| item.main_size)
            .sum::<f32>();
        let remaining_space = initial_free_space - frozen_size;
        let adjusted_space = if weight_sum_override > 0.0 {
            initial_free_space * total_weight / weight_sum_override
        } else {
            // This preserves Starlight's fractional-weight behavior: a total
            // below one reserves the undistributed fraction of free space.
            initial_free_space * active_weight
        };
        let free_space = if adjusted_space.abs() < remaining_space.abs() {
            adjusted_space
        } else {
            remaining_space
        };

        let mut total_violation = 0.0;
        for item in items
            .iter_mut()
            .filter(|item| item.weight > 0.0 && !item.frozen)
        {
            let tentative = if free_space > 0.0 {
                free_space * item.weight / active_weight
            } else {
                0.0
            };
            let floor = axes.main.size(padding_border_size(
                item.padding,
                item.border,
                item.scrollbar,
            ));
            let clamped = clamp_axis(
                tentative,
                axes.main.size(item.min_size),
                axes.main.size(item.max_size),
                floor,
            );
            item.main_size = clamped;
            item.violation = clamped - tentative;
            total_violation += item.violation;
        }

        let tolerance = 1.0e-5_f32.max(inner_main.abs() * f32::EPSILON * 8.0);
        if total_violation.abs() <= tolerance {
            return;
        }

        let freeze_min = total_violation > 0.0;
        let mut froze_any = false;
        for item in items
            .iter_mut()
            .filter(|item| item.weight > 0.0 && !item.frozen)
        {
            let violating = if freeze_min {
                item.violation > 0.0
            } else {
                item.violation < 0.0
            };
            if violating {
                item.frozen = true;
                active_weight -= item.weight;
                froze_any = true;
            }
        }
        if !froze_any {
            return;
        }
    }

    debug_assert!(
        false,
        "linear weight freeze loop exceeded the item-count bound"
    );
}

#[allow(clippy::too_many_arguments)]
fn size_items<Source, Session>(
    source: &Source,
    session: &mut Session,
    items: &mut [LinearItem],
    axes: LinearAxes,
    percentage_basis: Size<Option<f32>>,
    definite_inner_size: Size<Option<f32>>,
    available_space: Size<AvailableSpace>,
    main_was_definite: bool,
    weight_sum: f32,
) where
    Source: LinearSource,
    Session: LayoutSession<Source>,
{
    for item in items.iter_mut() {
        resolve_intrinsic_sizes(source, session, item, percentage_basis);
    }

    for item in items.iter_mut() {
        if !(main_was_definite && item.weight > 0.0) {
            measure_item(
                source,
                session,
                item,
                axes,
                percentage_basis,
                definite_inner_size,
                available_space,
                None,
            );
        }
    }

    if main_was_definite {
        distribute_weighted_items(
            items,
            axes,
            axes.main.size(definite_inner_size).unwrap_or(0.0),
            weight_sum,
        );
        for item in items.iter_mut().filter(|item| item.weight > 0.0) {
            let resolved_main = item.main_size;
            measure_item(
                source,
                session,
                item,
                axes,
                percentage_basis,
                definite_inner_size,
                available_space,
                Some(resolved_main),
            );
        }
    }
}

#[inline]
fn natural_content_size(items: &[LinearItem], axes: LinearAxes) -> Size<f32> {
    let main = items.iter().map(|item| outer_main(item, axes)).sum::<f32>();
    let cross = items
        .iter()
        .map(|item| outer_cross(item, axes))
        .fold(0.0_f32, f32::max);
    size_from_axes(axes, main, cross)
}

#[inline]
fn main_axis_distribution(
    main_gravity: MainGravity,
    free_main: f32,
    item_count: usize,
) -> (f32, f32) {
    match main_gravity {
        MainGravity::End => (free_main, 0.0),
        MainGravity::Center => (free_main / 2.0, 0.0),
        MainGravity::SpaceBetween if item_count > 1 => {
            (0.0, free_main.max(0.0) / (item_count - 1) as f32)
        }
        MainGravity::Start | MainGravity::SpaceBetween => (0.0, 0.0),
    }
}

#[inline]
fn cross_alignment_offset(gravity: CrossGravity, free_cross: f32) -> f32 {
    match gravity {
        CrossGravity::End => free_cross,
        CrossGravity::Center => free_cross / 2.0,
        CrossGravity::None | CrossGravity::Start | CrossGravity::Stretch => 0.0,
    }
}

fn position_items(
    items: &mut [LinearItem],
    axes: LinearAxes,
    inner_size: Size<f32>,
    main_gravity: MainGravity,
) {
    let used_main = items.iter().map(|item| outer_main(item, axes)).sum::<f32>();
    let free_main = axes.main.size(inner_size) - used_main;
    let (leading, between) = main_axis_distribution(main_gravity, free_main, items.len());

    let mut cursor = leading;
    let item_count = items.len();
    for (index, item) in items.iter_mut().enumerate() {
        cursor += flow_start(item.margin, axes.main, axes.main_reverse);
        item.main_position = cursor;
        cursor += item.main_size + flow_end(item.margin, axes.main, axes.main_reverse);
        if index + 1 < item_count {
            cursor += between;
        }

        let start_auto = flow_start(item.margin_auto, axes.cross, axes.cross_reverse);
        let end_auto = flow_end(item.margin_auto, axes.cross, axes.cross_reverse);
        let free_cross =
            axes.cross.size(inner_size) - item.cross_size - axis_sum(item.margin, axes.cross);
        if start_auto || end_auto {
            if free_cross > 0.0 {
                match (start_auto, end_auto) {
                    (true, true) => {
                        set_flow_start(
                            &mut item.margin,
                            axes.cross,
                            axes.cross_reverse,
                            free_cross / 2.0,
                        );
                        set_flow_end(
                            &mut item.margin,
                            axes.cross,
                            axes.cross_reverse,
                            free_cross / 2.0,
                        );
                    }
                    (true, false) => {
                        set_flow_start(
                            &mut item.margin,
                            axes.cross,
                            axes.cross_reverse,
                            free_cross,
                        );
                    }
                    (false, true) => {
                        set_flow_end(&mut item.margin, axes.cross, axes.cross_reverse, free_cross);
                    }
                    (false, false) => unreachable!(),
                }
            } else {
                if start_auto {
                    set_flow_start(&mut item.margin, axes.cross, axes.cross_reverse, 0.0);
                }
                if end_auto {
                    set_flow_end(&mut item.margin, axes.cross, axes.cross_reverse, 0.0);
                }
            }
            item.cross_position = flow_start(item.margin, axes.cross, axes.cross_reverse);
            continue;
        }

        let alignment = cross_alignment_offset(item.gravity, free_cross);
        item.cross_position = alignment + flow_start(item.margin, axes.cross, axes.cross_reverse);
    }
}

fn absolute_static_position(
    axes: LinearAxes,
    containing_size: Size<f32>,
    containing_origin: Point<f32>,
    size: Size<f32>,
    margin: Edges<f32>,
    gravity: CrossGravity,
    main_gravity: MainGravity,
) -> Point<f32> {
    let main_size = axes.main.size(size);
    let free_main = axes.main.size(containing_size) - main_size - axis_sum(margin, axes.main);
    let (leading, _) = main_axis_distribution(main_gravity, free_main, 1);
    let main_position = leading + flow_start(margin, axes.main, axes.main_reverse);

    let cross_size = axes.cross.size(size);
    let free_cross = axes.cross.size(containing_size) - cross_size - axis_sum(margin, axes.cross);
    let cross_position = cross_alignment_offset(gravity, free_cross)
        + flow_start(margin, axes.cross, axes.cross_reverse);

    let main = flow_to_physical(
        main_position,
        main_size,
        axes.main.size(containing_size),
        axes.main_reverse,
    ) + axes.main.point(containing_origin);
    let cross = flow_to_physical(
        cross_position,
        cross_size,
        axes.cross.size(containing_size),
        axes.cross_reverse,
    ) + axes.cross.point(containing_origin);
    let mut border_origin = Point::ZERO;
    axes.main.set_point(&mut border_origin, main);
    axes.cross.set_point(&mut border_origin, cross);
    Point::new(border_origin.x - margin.left, border_origin.y - margin.top)
}

fn item_location(
    item: &LinearItem,
    axes: LinearAxes,
    inner_size: Size<f32>,
    content_origin: Point<f32>,
) -> Point<f32> {
    let main = flow_to_physical(
        item.main_position,
        item.main_size,
        axes.main.size(inner_size),
        axes.main_reverse,
    ) + axes.main.point(content_origin);
    let cross = flow_to_physical(
        item.cross_position,
        item.cross_size,
        axes.cross.size(inner_size),
        axes.cross_reverse,
    ) + axes.cross.point(content_origin);
    let mut point = Point::ZERO;
    axes.main.set_point(&mut point, main);
    axes.cross.set_point(&mut point, cross);
    point
}

fn container_baseline(
    items: &[LinearItem],
    axes: LinearAxes,
    inner_size: Size<f32>,
    content_origin: Point<f32>,
) -> Option<f32> {
    if axes.main == Axis::Horizontal {
        items
            .iter()
            .map(|item| {
                let location = item_location(item, axes, inner_size, content_origin);
                location.y + item.first_baselines.y.unwrap_or(item.cross_size)
            })
            .reduce(f32::max)
    } else {
        let first = items.first()?;
        let baseline = first.first_baselines.y?;
        Some(item_location(first, axes, inner_size, content_origin).y + baseline)
    }
}

#[allow(clippy::too_many_arguments)]
fn commit_in_flow<Source, Session>(
    source: &Source,
    session: &mut Session,
    items: &mut [LinearItem],
    axes: LinearAxes,
    inner_size: Size<f32>,
    outer_size: Size<f32>,
    content_origin: Point<f32>,
) -> Size<f32>
where
    Source: LinearSource,
    Session: LayoutSession<Source>,
{
    let parent_size = inner_size.map(Some);
    let mut content_size = outer_size;
    for item in items {
        let target_size = size_from_axes(axes, item.main_size, item.cross_size);
        let mut input = LayoutInput::perform_layout(
            target_size.map(Some),
            parent_size,
            target_size.map(AvailableSpace::Definite),
        );
        input.sizing_mode = SizingMode::ContentSize;
        input.definite_dimensions = size_from_axes(
            axes,
            item.main_size_is_definite,
            item.cross_size_is_definite,
        );
        let output = session.compute_child_layout(source, item.key.node, input);
        item.first_baselines = output.first_baselines;

        let offset = relative_offset(item.inset, item.direction);
        let mut location = item_location(item, axes, inner_size, content_origin);
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

        content_size.width = content_size
            .width
            .max(location.x + output.size.width.max(output.content_size.width));
        content_size.height = content_size
            .height
            .max(location.y + output.size.height.max(output.content_size.height));
    }
    content_size
}

#[allow(clippy::too_many_arguments)]
fn commit_non_in_flow_children<Source, Session>(
    source: &Source,
    session: &mut Session,
    node: NodeId,
    in_flow_items: &[LinearItem],
    axes: LinearAxes,
    outer_size: Size<f32>,
    border: Edges<f32>,
    container_cross: LinearCrossGravity,
    align_items: Option<AlignItems>,
    main_gravity: MainGravity,
    mut content_size: Size<f32>,
) -> Size<f32>
where
    Source: LinearSource,
    Session: LayoutSession<Source>,
{
    let padding_box_size = Size::new(
        (outer_size.width - border.horizontal_sum()).max(0.0),
        (outer_size.height - border.vertical_sum()).max(0.0),
    );
    let padding_box_origin = Point::new(border.left, border.top);
    let child_count = source.child_count(node);
    let mut absolute_before = 0usize;
    let mut in_flow_before = 0usize;

    for document_index in 0..child_count {
        let child = source.child_id(node, document_index);
        let style = source.linear_item_style(child);
        if style.box_generation_mode() == BoxGenerationMode::None {
            hide_subtree(source, session, child);
            session.set_unrounded_layout(
                child,
                &Layout::with_order(u32::try_from(document_index).unwrap_or(u32::MAX)),
            );
            continue;
        }
        let position = style.position();
        if !matches!(position, Position::Absolute | Position::AbsoluteHoisted) {
            continue;
        }
        // Out-of-flow children participate in sibling paint order with an
        // effective order of zero, even though they do not participate in
        // linear sizing. `in_flow_items` is already sorted by (order,
        // document index). Absolute children arrive in document order, so a
        // monotonic merge cursor plus the number of earlier absolute siblings
        // gives the rank without another allocation or repeated searches.
        while in_flow_before < in_flow_items.len()
            && (
                in_flow_items[in_flow_before].key.css_order,
                in_flow_items[in_flow_before].key.document_index,
            ) < (0, document_index)
        {
            in_flow_before = in_flow_before.saturating_add(1);
        }
        let layout_order =
            u32::try_from(in_flow_before.saturating_add(absolute_before)).unwrap_or(u32::MAX);
        absolute_before = absolute_before.saturating_add(1);

        let gravity = computed_cross_gravity(
            style.linear_layout_gravity(),
            style.align_self(),
            container_cross,
            align_items,
            axes,
        );
        drop(style);
        let measured = measure_absolute_layout(source, session, child, padding_box_size);
        // Static-position fallback is gravity-driven. Out-of-flow automatic
        // margins are resolved by the common positioned algorithm, not by the
        // in-flow Linear cross-axis auto-margin rule.
        let static_position = absolute_static_position(
            axes,
            padding_box_size,
            padding_box_origin,
            measured.size,
            measured.margin,
            gravity,
            main_gravity,
        );

        match position {
            Position::Absolute => {
                let static_in_padding_space = Point::new(
                    static_position.x - border.left,
                    static_position.y - border.top,
                );
                let mut layout = compute_absolute_layout(
                    source,
                    session,
                    child,
                    padding_box_size,
                    static_in_padding_space,
                );
                layout.order = layout_order;
                layout.location.x += border.left;
                layout.location.y += border.top;
                content_size.width = content_size
                    .width
                    .max(layout.location.x + layout.size.width.max(layout.content_size.width));
                content_size.height = content_size
                    .height
                    .max(layout.location.y + layout.size.height.max(layout.content_size.height));
                session.set_unrounded_layout(child, &layout);
            }
            Position::AbsoluteHoisted => {
                session.set_static_position(child, static_position);
            }
            _ => unreachable!(),
        }
    }
    content_size
}

/// Computes a Starlight `display: linear` formatting context.
///
/// The algorithm is single-line and single-axis: in-flow items are measured
/// in order, positive weights share definite remaining main space, main and
/// cross gravity place final margin boxes, and out-of-flow children are laid
/// out after the container size is known. Child geometry is stored only for a
/// [`LayoutGoal::Commit`] call.
#[allow(clippy::too_many_lines)]
pub fn compute_linear_layout<Source, Session>(
    source: &Source,
    session: &mut Session,
    node: NodeId,
    input: LayoutInput,
) -> LayoutOutput
where
    Source: LinearSource,
    Session: LayoutSession<Source>,
{
    let style = source.linear_container_style(node);
    let orientation = style.linear_orientation();
    let axes = LinearAxes::new(orientation, style.direction());
    let container_cross = style.linear_cross_gravity();
    let align_items = style.align_items();
    let main_gravity = computed_main_gravity(style.linear_gravity(), style.justify_content(), axes);
    let weight_sum = style.linear_weight_sum();
    debug_assert!(
        weight_sum.is_finite() && weight_sum >= 0.0,
        "linear-weight-sum must be finite and non-negative"
    );
    let resolve_calc = |handle, basis| source.resolve_calc(handle, basis);
    let padding = resolve_edges(style.padding(), input.parent_size.width, &resolve_calc);
    let border = resolve_edges(style.border(), input.parent_size.width, &resolve_calc);
    let scrollbar = scrollbar_size(&style);
    let container_inset = padding_border_size(padding, border, scrollbar);
    let margin = resolve_optional_edges(style.margin(), input.parent_size.width, &resolve_calc)
        .map(|value| value.unwrap_or(0.0));
    let container_aspect_ratio = style.aspect_ratio();
    let container_box_sizing = style.box_sizing();

    let (preferred_size, min_size, max_size, style_definite) =
        if input.sizing_mode == SizingMode::ContentSize {
            (Size::NONE, Size::NONE, Size::NONE, Size::new(false, false))
        } else {
            let size = style.size();
            let preferred = resolve_quantitative_sizes(
                size,
                input.parent_size,
                container_aspect_ratio,
                container_box_sizing,
                container_inset,
                &resolve_calc,
            );
            (
                preferred,
                resolve_quantitative_sizes(
                    style.min_size(),
                    input.parent_size,
                    container_aspect_ratio,
                    container_box_sizing,
                    container_inset,
                    &resolve_calc,
                ),
                resolve_quantitative_sizes(
                    style.max_size(),
                    input.parent_size,
                    container_aspect_ratio,
                    container_box_sizing,
                    container_inset,
                    &resolve_calc,
                ),
                preferred_size_definiteness(size, input.parent_size, container_aspect_ratio),
            )
        };
    let preferred_size = Size::new(
        preferred_size
            .width
            .map(|value| clamp_axis(value, min_size.width, max_size.width, container_inset.width)),
        preferred_size.height.map(|value| {
            clamp_axis(
                value,
                min_size.height,
                max_size.height,
                container_inset.height,
            )
        }),
    );
    let mut outer_definite = Size::new(
        input.definite_dimensions.width || style_definite.width,
        input.definite_dimensions.height || style_definite.height,
    );
    if container_aspect_ratio.is_some() {
        if outer_definite.width {
            outer_definite.height = true;
        } else if outer_definite.height {
            outer_definite.width = true;
        }
    }
    let mut outer_size = input.known_dimensions.or(preferred_size);
    if input.sizing_mode != SizingMode::ContentSize {
        let before_ratio = outer_size;
        outer_size = apply_border_box_ratio(
            outer_size,
            container_aspect_ratio,
            container_box_sizing,
            container_inset,
        );
        if before_ratio.width.is_none()
            && let Some(width) = outer_size.width
        {
            outer_size.width = Some(clamp_axis(
                width,
                min_size.width,
                max_size.width,
                container_inset.width,
            ));
        }
        if before_ratio.height.is_none()
            && let Some(height) = outer_size.height
        {
            outer_size.height = Some(clamp_axis(
                height,
                min_size.height,
                max_size.height,
                container_inset.height,
            ));
        }
    }
    let inner_size = Size::new(
        outer_size
            .width
            .map(|value| (value - container_inset.width).max(0.0)),
        outer_size
            .height
            .map(|value| (value - container_inset.height).max(0.0)),
    );
    let mut definite_inner_size = inner_size;
    if !outer_definite.width {
        definite_inner_size.width = None;
    }
    if !outer_definite.height {
        definite_inner_size.height = None;
    }
    let main_was_definite = axes.main.size(definite_inner_size).is_some();
    let inner_available_space = Size::new(
        inner_size.width.map_or_else(
            || {
                subtract_available_space(
                    input.available_space.width,
                    margin.horizontal_sum() + container_inset.width,
                )
            },
            AvailableSpace::Definite,
        ),
        inner_size.height.map_or_else(
            || {
                subtract_available_space(
                    input.available_space.height,
                    margin.vertical_sum() + container_inset.height,
                )
            },
            AvailableSpace::Definite,
        ),
    );
    let mut percentage_basis = definite_inner_size;

    let child_count = source.child_count(node);
    let mut items = Vec::with_capacity(child_count);
    let mut has_nonzero_order = false;
    let mut has_non_in_flow_children = false;
    let mut absolute_count = 0usize;
    for document_index in 0..child_count {
        let child = source.child_id(node, document_index);
        let child_style = source.linear_item_style(child);
        let box_generation_mode = child_style.box_generation_mode();
        let position = child_style.position();
        let is_absolute = matches!(position, Position::Absolute | Position::AbsoluteHoisted);
        if box_generation_mode == BoxGenerationMode::None {
            has_non_in_flow_children = true;
            continue;
        }
        if is_absolute {
            has_non_in_flow_children = true;
            absolute_count = absolute_count.saturating_add(1);
            continue;
        }
        let css_order = child_style.order();
        has_nonzero_order |= css_order != 0;
        items.push(resolve_item(
            source,
            child_style,
            ItemKey {
                node: child,
                document_index,
                css_order,
                // Temporarily retain the number of preceding effective-order
                // zero absolute siblings. After sorting this is exactly the
                // merge offset for a zero-order in-flow item.
                layout_order: u32::try_from(absolute_count).unwrap_or(u32::MAX),
            },
            percentage_basis,
            container_cross,
            align_items,
            axes,
        ));
    }
    if has_nonzero_order {
        // The document index makes the key unique. An allocation-free unstable
        // sort therefore has exactly the required stable-within-equal-order
        // result.
        items.sort_unstable_by(
            |left, right| match left.key.css_order.cmp(&right.key.css_order) {
                Ordering::Equal => left.key.document_index.cmp(&right.key.document_index),
                ordering => ordering,
            },
        );
    }
    for (in_flow_order, item) in items.iter_mut().enumerate() {
        let absolute_before = match item.key.css_order.cmp(&0) {
            Ordering::Less => 0,
            Ordering::Equal => usize::try_from(item.key.layout_order).unwrap_or(usize::MAX),
            Ordering::Greater => absolute_count,
        };
        item.key.layout_order =
            u32::try_from(in_flow_order.saturating_add(absolute_before)).unwrap_or(u32::MAX);
    }

    size_items(
        source,
        session,
        &mut items,
        axes,
        percentage_basis,
        definite_inner_size,
        inner_available_space,
        main_was_definite,
        weight_sum,
    );
    let natural = natural_content_size(&items, axes);
    if outer_size.width.is_none() {
        outer_size.width = Some(clamp_axis(
            natural.width + container_inset.width,
            min_size.width,
            max_size.width,
            container_inset.width,
        ));
    }
    if outer_size.height.is_none() {
        outer_size.height = Some(clamp_axis(
            natural.height + container_inset.height,
            min_size.height,
            max_size.height,
            container_inset.height,
        ));
    }
    let final_outer_size = outer_size.unwrap_or(Size::ZERO);
    let final_inner_size = Size::new(
        (final_outer_size.width - container_inset.width).max(0.0),
        (final_outer_size.height - container_inset.height).max(0.0),
    );

    // Cyclic percentages contribute zero while an inline size is intrinsic,
    // then resolve against the resulting used width. Avoid the second pass for
    // the overwhelmingly common all-absolute-length case.
    if !outer_definite.width && items.iter().any(|item| item.depends_on_inline_basis) {
        percentage_basis = final_inner_size.map(Some);
        for item in &mut items {
            let key = item.key;
            let item_style = source.linear_item_style(key.node);
            *item = resolve_item(
                source,
                item_style,
                key,
                percentage_basis,
                container_cross,
                align_items,
                axes,
            );
        }
        size_items(
            source,
            session,
            &mut items,
            axes,
            percentage_basis,
            definite_inner_size,
            final_inner_size.map(AvailableSpace::Definite),
            main_was_definite,
            weight_sum,
        );
        // The intrinsic container size remains fixed. Percentage-dependent
        // used values may overflow it, but cannot feed back into and shrink or
        // recursively inflate the basis that resolved them.
    }

    position_items(&mut items, axes, final_inner_size, main_gravity);
    let content_origin = Point::new(border.left + padding.left, border.top + padding.top);
    let provisional_baseline = container_baseline(&items, axes, final_inner_size, content_origin);
    if matches!(input.goal, LayoutGoal::Measure(_)) {
        return LayoutOutput::new(final_outer_size, final_outer_size)
            .with_first_baselines(Point::new(None, provisional_baseline));
    }

    let mut content_size = commit_in_flow(
        source,
        session,
        &mut items,
        axes,
        final_inner_size,
        final_outer_size,
        content_origin,
    );
    if has_non_in_flow_children {
        content_size = commit_non_in_flow_children(
            source,
            session,
            node,
            &items,
            axes,
            final_outer_size,
            border,
            container_cross,
            align_items,
            main_gravity,
            content_size,
        );
    }
    let baseline =
        container_baseline(&items, axes, final_inner_size, content_origin).or(provisional_baseline);
    LayoutOutput::new(final_outer_size, content_size)
        .with_first_baselines(Point::new(None, baseline))
}
