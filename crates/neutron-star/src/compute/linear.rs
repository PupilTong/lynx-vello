//! Starlight linear layout.

#![allow(clippy::cast_precision_loss)]

use core::cmp::Ordering;

use stylo::computed_values::{box_sizing, direction, linear_direction};
use stylo::values::computed::{
    AspectRatio, ContentDistribution, Inset, ItemPlacement, LengthPercentage, Margin, MaxSize,
    PositionProperty, SelfAlignment, Size as StyleSize,
};
use stylo::values::generics::position::PreferredRatio;
use stylo::values::specified::align::AlignFlags;

use super::util::{
    ResolvedContainerBox, ResolvedItemBox, accumulate_scrollable_overflow, apply_aspect_ratio,
    auto_edges_to_zero, clamp_axis, own_scrollable_overflow, resolve_container_box, resolve_insets,
    resolve_item_box, resolve_length_percentage, resolve_margins, resolve_padding,
};
use super::{compute_absolute_layout_with_static_position, hide_subtree, measure_absolute_layout};
use crate::geometry::{Edges, Point, Size};
use crate::style::containment::size_containment;
use crate::style::{Contain, CoreStyle, LinearContainerStyle, LinearItemStyle, Overflow};
use crate::tree::{
    AvailableSpace, Layout, LayoutGoal, LayoutInput, LayoutNode, LayoutOutput, RequestedAxis,
    SizingMode,
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
    fn new(linear_direction: linear_direction::T, inline_direction: direction::T) -> Self {
        let horizontal = matches!(
            linear_direction,
            linear_direction::T::Row | linear_direction::T::RowReverse
        );
        let reverse = matches!(
            linear_direction,
            linear_direction::T::RowReverse | linear_direction::T::ColumnReverse
        );
        let rtl = inline_direction == direction::T::Rtl;
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
            main_reverse: reverse ^ (horizontal && rtl),
            cross_reverse: !horizontal && rtl,
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct ItemKey<N> {
    node: N,
    document_index: usize,
    css_order: i32,
    layout_order: u32,
}

#[derive(Debug, Clone, Copy)]
enum NonFlowItem<N> {
    Hidden {
        node: N,
        document_index: usize,
    },
    Absolute {
        key: ItemKey<N>,
        position: PositionProperty,
        gravity: AlignFlags,
        static_axes: Size<bool>,
    },
}

#[derive(Debug, Clone, Copy)]
#[repr(transparent)]
struct LinearItemFlags(u8);

impl LinearItemFlags {
    const MARGIN_REFRESH: u8 = 1 << 0;
    const PADDING_REFRESH: u8 = 1 << 1;
    const RELATIVE_OFFSET: u8 = 1 << 2;
    const FROZEN: u8 = 1 << 3;
    const BOX_REFRESH: u8 = Self::MARGIN_REFRESH | Self::PADDING_REFRESH;

    #[inline]
    const fn needs_box_refresh(self) -> bool {
        self.0 & Self::BOX_REFRESH != 0
    }

    #[inline]
    const fn needs_relative_offset_refresh(self) -> bool {
        self.0 & Self::RELATIVE_OFFSET != 0
    }

    #[inline]
    const fn needs_margin_refresh(self) -> bool {
        self.0 & Self::MARGIN_REFRESH != 0
    }

    #[inline]
    const fn needs_padding_refresh(self) -> bool {
        self.0 & Self::PADDING_REFRESH != 0
    }

    #[inline]
    const fn is_frozen(self) -> bool {
        self.0 & Self::FROZEN != 0
    }

    #[inline]
    fn set_frozen(&mut self, frozen: bool) {
        if frozen {
            self.0 |= Self::FROZEN;
        } else {
            self.0 &= !Self::FROZEN;
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct LinearItemSeed<N> {
    key: ItemKey<N>,
    flags: LinearItemFlags,
}

/// One allocation-friendly scratch record per in-flow item. Raw style stays
/// behind the node handle — immutable for the layout epoch — and is
/// re-fetched only for intrinsic probes or a cyclic percentage re-resolution.
#[derive(Debug)]
#[allow(clippy::struct_excessive_bools)]
struct LinearItem<N> {
    key: ItemKey<N>,
    gravity: AlignFlags,
    weight: f32,
    size_is_auto: Size<bool>,
    size_is_intrinsic: Size<bool>,
    has_intrinsic_size: bool,
    preferred_size: Size<Option<f32>>,
    preferred_size_is_definite: Size<bool>,
    min_size: Size<Option<f32>>,
    max_size: Size<Option<f32>>,
    margin: Edges<f32>,
    margin_auto: Edges<bool>,
    padding: Edges<f32>,
    border: Edges<f32>,
    overflow: Point<Overflow>,
    relative_offset: Point<f32>,
    box_sizing: box_sizing::T,
    aspect_ratio: Option<f32>,
    main_size: f32,
    cross_size: f32,
    main_position: f32,
    cross_position: f32,
    baseline: Option<f32>,
    main_size_is_definite: bool,
    cross_size_is_definite: bool,
    violation: f32,
    flags: LinearItemFlags,
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

fn map_cross_flags(flags: AlignFlags, axes: LinearAxes) -> AlignFlags {
    if flags == AlignFlags::STRETCH || flags == AlignFlags::CENTER {
        flags
    } else if flags == AlignFlags::START || flags == AlignFlags::FLEX_START {
        AlignFlags::START
    } else if flags == AlignFlags::END || flags == AlignFlags::FLEX_END {
        AlignFlags::END
    } else if flags == AlignFlags::LEFT || flags == AlignFlags::RIGHT {
        if axes.cross == Axis::Horizontal {
            let end = (flags == AlignFlags::RIGHT) ^ axes.cross_reverse;
            if end {
                AlignFlags::END
            } else {
                AlignFlags::START
            }
        } else {
            AlignFlags::START
        }
    } else {
        AlignFlags::NORMAL
    }
}

fn computed_cross_gravity(
    align_self: SelfAlignment,
    align_items: ItemPlacement,
    axes: LinearAxes,
) -> AlignFlags {
    let self_flags = align_self.0.value();
    if self_flags != AlignFlags::AUTO {
        let mapped = map_cross_flags(self_flags, axes);
        if mapped != AlignFlags::NORMAL {
            return mapped;
        }
    }
    map_cross_flags(align_items.0.value(), axes)
}

fn computed_main_gravity(justify_content: ContentDistribution, axes: LinearAxes) -> AlignFlags {
    let flags = justify_content.primary().value();
    if flags == AlignFlags::END || flags == AlignFlags::FLEX_END {
        AlignFlags::END
    } else if flags == AlignFlags::CENTER || flags == AlignFlags::SPACE_BETWEEN {
        flags
    } else if (flags == AlignFlags::LEFT || flags == AlignFlags::RIGHT)
        && axes.main == Axis::Horizontal
    {
        let end = (flags == AlignFlags::RIGHT) ^ axes.main_reverse;
        if end {
            AlignFlags::END
        } else {
            AlignFlags::START
        }
    } else {
        AlignFlags::START
    }
}

#[inline]
fn lp_depends_on_basis(value: &LengthPercentage) -> bool {
    value.has_percentage()
}

#[inline]
fn margin_depends_on_basis(value: &Margin) -> bool {
    match value {
        Margin::LengthPercentage(lp) => lp_depends_on_basis(lp),
        Margin::Auto => false,
        Margin::AnchorSizeFunction(_) | Margin::AnchorContainingCalcFunction(_) => {
            unreachable!("anchor margins are pref-dead under the lynx feature")
        }
    }
}

#[inline]
fn inset_depends_on_basis(value: &Inset) -> bool {
    match value {
        Inset::LengthPercentage(lp) => lp_depends_on_basis(lp),
        Inset::Auto => false,
        Inset::AnchorFunction(_)
        | Inset::AnchorSizeFunction(_)
        | Inset::AnchorContainingCalcFunction(_) => {
            unreachable!("anchor insets are pref-dead under the lynx feature")
        }
    }
}

#[inline]
fn relative_offset(inset: Edges<Option<f32>>, direction: direction::T) -> Point<f32> {
    let x = match (inset.left, inset.right) {
        (Some(_), Some(right)) if direction == direction::T::Rtl => -right,
        (Some(left), _) => left,
        (None, Some(right)) => -right,
        (None, None) => 0.0,
    };
    let y = inset.top.unwrap_or_else(|| -inset.bottom.unwrap_or(0.0));
    Point::new(x, y)
}

#[inline]
fn padding_border_size(padding: Edges<f32>, border: Edges<f32>) -> Size<f32> {
    Size::new(
        padding.horizontal_sum() + border.horizontal_sum(),
        padding.vertical_sum() + border.vertical_sum(),
    )
}

#[inline]
fn used_aspect_ratio(value: AspectRatio) -> Option<f32> {
    match value.ratio {
        PreferredRatio::None => None,
        PreferredRatio::Ratio(ratio) => (!ratio.is_degenerate()).then(|| ratio.0.0 / ratio.1.0),
    }
}

#[inline]
fn style_size_is_auto(value: &StyleSize) -> bool {
    match value {
        StyleSize::Auto
        | StyleSize::FitContent
        | StyleSize::Stretch
        | StyleSize::WebkitFillAvailable => true,
        StyleSize::LengthPercentage(_)
        | StyleSize::MaxContent
        | StyleSize::MinContent
        | StyleSize::FitContentFunction(_) => false,
        StyleSize::AnchorSizeFunction(_) | StyleSize::AnchorContainingCalcFunction(_) => {
            unreachable!("anchor sizing is pref-dead under the lynx feature")
        }
    }
}

#[inline]
fn style_size_is_intrinsic(value: &StyleSize) -> bool {
    matches!(
        value,
        StyleSize::MinContent | StyleSize::MaxContent | StyleSize::FitContentFunction(_)
    )
}

#[inline]
fn max_size_is_intrinsic(value: &MaxSize) -> bool {
    matches!(
        value,
        MaxSize::MinContent | MaxSize::MaxContent | MaxSize::FitContentFunction(_)
    )
}

#[inline]
fn style_size_axis_is_definite(value: &StyleSize, parent_basis: Option<f32>) -> bool {
    match value {
        StyleSize::LengthPercentage(lp) => !lp_depends_on_basis(&lp.0) || parent_basis.is_some(),
        _ => false,
    }
}

#[inline]
fn size_definiteness(
    size: Size<&StyleSize>,
    parent_size: Size<Option<f32>>,
    aspect_ratio: Option<f32>,
) -> Size<bool> {
    let mut definite = Size::new(
        style_size_axis_is_definite(size.width, parent_size.width),
        style_size_axis_is_definite(size.height, parent_size.height),
    );
    if aspect_ratio.is_some() {
        if definite.width {
            definite.height = true;
        } else if definite.height {
            definite.width = true;
        }
    }
    definite
}

#[inline]
fn initial_item_flags(
    style: &impl LinearItemStyle,
    inline_basis: Option<f32>,
    nudges: bool,
) -> LinearItemFlags {
    let relative_offset = nudges && {
        let inset = style.inset();
        inset_depends_on_basis(inset.left)
            || inset_depends_on_basis(inset.right)
            || inset_depends_on_basis(inset.top)
            || inset_depends_on_basis(inset.bottom)
    };
    let (margin_refresh, padding_refresh) = if inline_basis.is_none() {
        let margin = style.margin();
        let padding = style.padding();
        (
            margin_depends_on_basis(margin.left)
                || margin_depends_on_basis(margin.right)
                || margin_depends_on_basis(margin.top)
                || margin_depends_on_basis(margin.bottom),
            lp_depends_on_basis(&padding.left.0)
                || lp_depends_on_basis(&padding.right.0)
                || lp_depends_on_basis(&padding.top.0)
                || lp_depends_on_basis(&padding.bottom.0),
        )
    } else {
        (false, false)
    };
    let mut flags = 0;
    if margin_refresh {
        flags |= LinearItemFlags::MARGIN_REFRESH;
    }
    if padding_refresh {
        flags |= LinearItemFlags::PADDING_REFRESH;
    }
    if relative_offset {
        flags |= LinearItemFlags::RELATIVE_OFFSET;
    }
    LinearItemFlags(flags)
}

fn resolve_item<N>(
    style: impl LinearItemStyle,
    seed: LinearItemSeed<N>,
    percentage_basis: Size<Option<f32>>,
    align_items: ItemPlacement,
    axes: LinearAxes,
    nudges: bool,
) -> LinearItem<N>
where
    N: LayoutNode,
    N::Style: LinearContainerStyle + LinearItemStyle,
{
    let weight = style.linear_weight().0;
    debug_assert!(
        weight.is_finite() && weight >= 0.0,
        "linear-weight must be finite and non-negative"
    );
    let direction = style.direction();
    let gravity = computed_cross_gravity(style.align_self(), align_items, axes);
    let ResolvedItemBox {
        raw_size,
        raw_min_size,
        raw_max_size,
        aspect_ratio,
        box_sizing,
        preferred_size,
        min_size,
        max_size,
        margin,
        margin_auto,
        padding,
        border,
        inset,
        overflow,
        ..
    } = resolve_item_box(&style, percentage_basis);
    let relative_offset = if nudges {
        relative_offset(inset, direction)
    } else {
        Point::ZERO
    };

    LinearItem {
        key: seed.key,
        gravity,
        weight,
        size_is_auto: Size::new(
            style_size_is_auto(raw_size.width),
            style_size_is_auto(raw_size.height),
        ),
        size_is_intrinsic: Size::new(
            style_size_is_intrinsic(raw_size.width),
            style_size_is_intrinsic(raw_size.height),
        ),
        has_intrinsic_size: style_size_is_intrinsic(raw_size.width)
            || style_size_is_intrinsic(raw_size.height)
            || style_size_is_intrinsic(raw_min_size.width)
            || style_size_is_intrinsic(raw_min_size.height)
            || max_size_is_intrinsic(raw_max_size.width)
            || max_size_is_intrinsic(raw_max_size.height),
        preferred_size,
        preferred_size_is_definite: size_definiteness(raw_size, percentage_basis, aspect_ratio),
        min_size,
        max_size,
        margin,
        margin_auto,
        padding,
        border,
        overflow,
        relative_offset,
        box_sizing,
        aspect_ratio,
        main_size: 0.0,
        cross_size: 0.0,
        main_position: 0.0,
        cross_position: 0.0,
        baseline: None,
        main_size_is_definite: false,
        cross_size_is_definite: false,
        violation: 0.0,
        flags: seed.flags,
    }
}

#[inline]
fn refresh_item_edges<N>(
    style: impl LinearItemStyle,
    item: &mut LinearItem<N>,
    percentage_basis: Size<Option<f32>>,
) {
    if item.flags.needs_padding_refresh() {
        item.padding = resolve_padding(style.padding(), percentage_basis.width);
    }
    if item.flags.needs_margin_refresh() {
        let margin_value = style.margin();
        let optional_margin = resolve_margins(margin_value, percentage_basis.width);
        item.margin = auto_edges_to_zero(optional_margin);
        item.margin_auto = margin_value.map(Margin::is_auto);
    }
}

#[allow(clippy::too_many_arguments)]
fn child_measurement<N>(
    node: N,
    known_dimensions: Size<Option<f32>>,
    definite_dimensions: Size<bool>,
    parent_size: Size<Option<f32>>,
    available_space: Size<AvailableSpace>,
    sizing_mode: SizingMode,
    requested_axis: RequestedAxis,
) -> LayoutOutput
where
    N: LayoutNode,
    N::Style: LinearContainerStyle + LinearItemStyle,
{
    let mut input = LayoutInput::measure(
        known_dimensions,
        parent_size,
        available_space,
        requested_axis,
    );
    input.definite_dimensions = definite_dimensions;
    input.sizing_mode = sizing_mode;
    node.compute_layout(input)
}

#[inline]
fn needs_min_content(size: &StyleSize, min_size: &StyleSize, max_size: &MaxSize) -> bool {
    matches!(
        size,
        StyleSize::MinContent | StyleSize::FitContentFunction(_)
    ) || matches!(
        min_size,
        StyleSize::MinContent | StyleSize::FitContentFunction(_)
    ) || matches!(
        max_size,
        MaxSize::MinContent | MaxSize::FitContentFunction(_)
    )
}

#[inline]
fn needs_max_content(size: &StyleSize, min_size: &StyleSize, max_size: &MaxSize) -> bool {
    matches!(
        size,
        StyleSize::MaxContent | StyleSize::FitContentFunction(_)
    ) || matches!(
        min_size,
        StyleSize::MaxContent | StyleSize::FitContentFunction(_)
    ) || matches!(
        max_size,
        MaxSize::MaxContent | MaxSize::FitContentFunction(_)
    )
}

fn intrinsic_measurement<N>(
    item: &LinearItem<N>,
    percentage_basis: Size<Option<f32>>,
    requested: Size<bool>,
    target_available: AvailableSpace,
) -> LayoutOutput
where
    N: LayoutNode,
    N::Style: LinearContainerStyle + LinearItemStyle,
{
    let inset = padding_border_size(item.padding, item.border);
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
        item.key.node,
        known,
        definite,
        percentage_basis,
        available,
        SizingMode::IgnoreSizeStyles,
        requested_axis,
    )
}

#[inline]
fn fit_content_axis_value(
    limit: &LengthPercentage,
    minimum: f32,
    maximum: f32,
    basis: Option<f32>,
    inset: f32,
    box_sizing: box_sizing::T,
) -> f32 {
    let mut limit = resolve_length_percentage(limit, basis).unwrap_or(maximum);
    if box_sizing == box_sizing::T::ContentBox {
        limit += inset;
    }
    maximum.min(limit.max(minimum))
}

#[inline]
#[allow(clippy::too_many_arguments)]
fn intrinsic_axis_value(
    value: &StyleSize,
    quantitative: Option<f32>,
    minimum: f32,
    maximum: f32,
    basis: Option<f32>,
    inset: f32,
    box_sizing: box_sizing::T,
) -> Option<f32> {
    match value {
        StyleSize::MinContent => Some(minimum),
        StyleSize::MaxContent => Some(maximum),
        StyleSize::FitContentFunction(limit) => Some(fit_content_axis_value(
            &limit.0, minimum, maximum, basis, inset, box_sizing,
        )),
        StyleSize::Auto
        | StyleSize::LengthPercentage(_)
        | StyleSize::FitContent
        | StyleSize::Stretch
        | StyleSize::WebkitFillAvailable => quantitative,
        StyleSize::AnchorSizeFunction(_) | StyleSize::AnchorContainingCalcFunction(_) => {
            unreachable!("anchor sizing is pref-dead under the lynx feature")
        }
    }
}

#[inline]
#[allow(clippy::too_many_arguments)]
fn intrinsic_max_axis_value(
    value: &MaxSize,
    quantitative: Option<f32>,
    minimum: f32,
    maximum: f32,
    basis: Option<f32>,
    inset: f32,
    box_sizing: box_sizing::T,
) -> Option<f32> {
    match value {
        MaxSize::MinContent => Some(minimum),
        MaxSize::MaxContent => Some(maximum),
        MaxSize::FitContentFunction(limit) => Some(fit_content_axis_value(
            &limit.0, minimum, maximum, basis, inset, box_sizing,
        )),
        MaxSize::None
        | MaxSize::LengthPercentage(_)
        | MaxSize::FitContent
        | MaxSize::Stretch
        | MaxSize::WebkitFillAvailable => quantitative,
        MaxSize::AnchorSizeFunction(_) | MaxSize::AnchorContainingCalcFunction(_) => {
            unreachable!("anchor sizing is pref-dead under the lynx feature")
        }
    }
}

#[allow(clippy::too_many_lines)]
fn resolve_intrinsic_sizes<N>(item: &mut LinearItem<N>, percentage_basis: Size<Option<f32>>)
where
    N: LayoutNode,
    N::Style: LinearContainerStyle + LinearItemStyle,
{
    if !item.has_intrinsic_size {
        return;
    }
    let style = item.key.node.style();
    let size = style.size();
    let min_size = style.min_size();
    let max_size = style.max_size();
    let need_min = Size::new(
        needs_min_content(size.width, min_size.width, max_size.width),
        needs_min_content(size.height, min_size.height, max_size.height),
    );
    let need_max = Size::new(
        needs_max_content(size.width, min_size.width, max_size.width),
        needs_max_content(size.height, min_size.height, max_size.height),
    );
    let min_content = if need_min.width || need_min.height {
        intrinsic_measurement(item, percentage_basis, need_min, AvailableSpace::MinContent).size
    } else {
        Size::ZERO
    };
    let max_content = if need_max.width || need_max.height {
        intrinsic_measurement(item, percentage_basis, need_max, AvailableSpace::MaxContent).size
    } else {
        Size::ZERO
    };
    let inset = padding_border_size(item.padding, item.border);

    item.preferred_size = Size::new(
        intrinsic_axis_value(
            size.width,
            item.preferred_size.width,
            min_content.width,
            max_content.width,
            percentage_basis.width,
            inset.width,
            item.box_sizing,
        ),
        intrinsic_axis_value(
            size.height,
            item.preferred_size.height,
            min_content.height,
            max_content.height,
            percentage_basis.height,
            inset.height,
            item.box_sizing,
        ),
    );
    item.min_size = Size::new(
        intrinsic_axis_value(
            min_size.width,
            item.min_size.width,
            min_content.width,
            max_content.width,
            percentage_basis.width,
            inset.width,
            item.box_sizing,
        ),
        intrinsic_axis_value(
            min_size.height,
            item.min_size.height,
            min_content.height,
            max_content.height,
            percentage_basis.height,
            inset.height,
            item.box_sizing,
        ),
    );
    item.max_size = Size::new(
        intrinsic_max_axis_value(
            max_size.width,
            item.max_size.width,
            min_content.width,
            max_content.width,
            percentage_basis.width,
            inset.width,
            item.box_sizing,
        ),
        intrinsic_max_axis_value(
            max_size.height,
            item.max_size.height,
            min_content.height,
            max_content.height,
            percentage_basis.height,
            inset.height,
            item.box_sizing,
        ),
    );
    item.preferred_size = apply_aspect_ratio(item.preferred_size, item.aspect_ratio);
}

#[inline]
fn ratio_cross_size<N>(item: &LinearItem<N>, axes: LinearAxes, forced_main: f32) -> Option<f32>
where
    N: LayoutNode,
    N::Style: LinearContainerStyle + LinearItemStyle,
{
    let ratio = item.aspect_ratio?;
    if !ratio.is_finite() || ratio <= 0.0 || !axes.cross.size(item.size_is_auto) {
        return None;
    }
    let inset = padding_border_size(item.padding, item.border);
    let sizing_main = if item.box_sizing == box_sizing::T::ContentBox {
        (forced_main - axes.main.size(inset)).max(0.0)
    } else {
        forced_main
    };
    let sizing_cross = if axes.main == Axis::Horizontal {
        sizing_main / ratio
    } else {
        sizing_main * ratio
    };
    Some(if item.box_sizing == box_sizing::T::ContentBox {
        sizing_cross + axes.cross.size(inset)
    } else {
        sizing_cross
    })
}

#[inline]
fn apply_border_box_ratio(
    mut size: Size<Option<f32>>,
    aspect_ratio: Option<f32>,
    box_sizing: box_sizing::T,
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
            let sizing_width = if box_sizing == box_sizing::T::ContentBox {
                (width - inset.width).max(0.0)
            } else {
                width
            };
            let sizing_height = sizing_width / ratio;
            size.height = Some(if box_sizing == box_sizing::T::ContentBox {
                sizing_height + inset.height
            } else {
                sizing_height
            });
        }
        (None, Some(height)) => {
            let sizing_height = if box_sizing == box_sizing::T::ContentBox {
                (height - inset.height).max(0.0)
            } else {
                height
            };
            let sizing_width = sizing_height * ratio;
            size.width = Some(if box_sizing == box_sizing::T::ContentBox {
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
fn measure_item<N>(
    item: &mut LinearItem<N>,
    axes: LinearAxes,
    percentage_basis: Size<Option<f32>>,
    constraint_inner_size: Size<Option<f32>>,
    available_space: Size<AvailableSpace>,
    forced_main: Option<f32>,
) where
    N: LayoutNode,
    N::Style: LinearContainerStyle + LinearItemStyle,
{
    let inset = padding_border_size(item.padding, item.border);
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

    let cross_constraint = axes.cross.size(constraint_inner_size);
    let ratio_fixed_cross = forced_main
        .and_then(|main| ratio_cross_size(item, axes, main))
        .is_some();
    let should_stretch = cross_constraint.is_some()
        && (item.gravity == AlignFlags::STRETCH
            || (item.gravity == AlignFlags::NORMAL
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
        item.key.node,
        known,
        known_definite,
        percentage_basis,
        child_available,
        SizingMode::ApplySizeStyles,
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
    item.baseline = output.first_baselines.y;
    item.main_size_is_definite = forced_main.is_some() || axes.main.size(known_definite);
    item.cross_size_is_definite = axes.cross.size(known_definite);
}

#[inline]
fn outer_main<N>(item: &LinearItem<N>, axes: LinearAxes) -> f32
where
    N: LayoutNode,
    N::Style: LinearContainerStyle + LinearItemStyle,
{
    item.main_size + axis_sum(item.margin, axes.main)
}

#[inline]
fn outer_cross<N>(item: &LinearItem<N>, axes: LinearAxes) -> f32
where
    N: LayoutNode,
    N::Style: LinearContainerStyle + LinearItemStyle,
{
    item.cross_size + axis_sum(item.margin, axes.cross)
}

fn distribute_weighted_items<N>(
    items: &mut [LinearItem<N>],
    axes: LinearAxes,
    inner_main: f32,
    weight_sum_override: f32,
) where
    N: LayoutNode,
    N::Style: LinearContainerStyle + LinearItemStyle,
{
    let mut total_weight = 0.0_f32;
    let mut fixed_outer = 0.0_f32;
    let mut weighted_margins = 0.0_f32;
    let mut weighted_count = 0usize;
    for item in &mut *items {
        if item.weight > 0.0 {
            total_weight += item.weight;
            weighted_margins += axis_sum(item.margin, axes.main);
            weighted_count = weighted_count.saturating_add(1);
            item.main_size = 0.0;
            item.flags.set_frozen(false);
            item.violation = 0.0;
        } else {
            fixed_outer += outer_main(item, axes);
        }
    }
    if total_weight <= 0.0 {
        return;
    }

    let initial_free_space = inner_main - fixed_outer - weighted_margins;
    let mut active_weight = total_weight;
    let mut frozen_size = 0.0_f32;
    for _ in 0..=weighted_count {
        if active_weight <= 0.0 {
            return;
        }
        let remaining_space = initial_free_space - frozen_size;
        let adjusted_space = if weight_sum_override > 0.0 {
            initial_free_space * total_weight / weight_sum_override
        } else {
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
            .filter(|item| item.weight > 0.0 && !item.flags.is_frozen())
        {
            let tentative = if free_space > 0.0 {
                free_space * item.weight / active_weight
            } else {
                0.0
            };
            let floor = axes
                .main
                .size(padding_border_size(item.padding, item.border));
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
            .filter(|item| item.weight > 0.0 && !item.flags.is_frozen())
        {
            let violating = if freeze_min {
                item.violation > 0.0
            } else {
                item.violation < 0.0
            };
            if violating {
                item.flags.set_frozen(true);
                active_weight -= item.weight;
                frozen_size += item.main_size;
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
fn size_items<N>(
    items: &mut [LinearItem<N>],
    axes: LinearAxes,
    percentage_basis: Size<Option<f32>>,
    constraint_inner_size: Size<Option<f32>>,
    available_space: Size<AvailableSpace>,
    weight_sum: f32,
) where
    N: LayoutNode,
    N::Style: LinearContainerStyle + LinearItemStyle,
{
    let constrained_main = axes.main.size(constraint_inner_size);
    for item in items.iter_mut() {
        resolve_intrinsic_sizes(item, percentage_basis);
        if !(constrained_main.is_some() && item.weight > 0.0) {
            measure_item(
                item,
                axes,
                percentage_basis,
                constraint_inner_size,
                available_space,
                None,
            );
        }
    }

    if let Some(inner_main) = constrained_main {
        distribute_weighted_items(items, axes, inner_main, weight_sum);
        for item in items.iter_mut().filter(|item| item.weight > 0.0) {
            let resolved_main = item.main_size;
            measure_item(
                item,
                axes,
                percentage_basis,
                constraint_inner_size,
                available_space,
                Some(resolved_main),
            );
        }
    }
}

#[inline]
fn natural_content_size<N>(items: &[LinearItem<N>], axes: LinearAxes) -> (Size<f32>, f32)
where
    N: LayoutNode,
    N::Style: LinearContainerStyle + LinearItemStyle,
{
    let (main, cross) = items.iter().fold((0.0_f32, 0.0_f32), |acc, item| {
        (
            acc.0 + outer_main(item, axes),
            acc.1.max(outer_cross(item, axes)),
        )
    });
    (size_from_axes(axes, main, cross), main)
}

#[inline]
fn main_axis_distribution(
    main_gravity: AlignFlags,
    free_main: f32,
    item_count: usize,
) -> (f32, f32) {
    if main_gravity == AlignFlags::END {
        (free_main, 0.0)
    } else if main_gravity == AlignFlags::CENTER {
        (free_main / 2.0, 0.0)
    } else if main_gravity == AlignFlags::SPACE_BETWEEN && item_count > 1 {
        (0.0, free_main.max(0.0) / (item_count - 1) as f32)
    } else {
        (0.0, 0.0)
    }
}

#[inline]
fn cross_alignment_offset(gravity: AlignFlags, free_cross: f32) -> f32 {
    if gravity == AlignFlags::END {
        free_cross
    } else if gravity == AlignFlags::CENTER {
        free_cross / 2.0
    } else {
        0.0
    }
}

fn position_items<N>(
    items: &mut [LinearItem<N>],
    axes: LinearAxes,
    inner_size: Size<f32>,
    main_gravity: AlignFlags,
    used_main: f32,
) where
    N: LayoutNode,
    N::Style: LinearContainerStyle + LinearItemStyle,
{
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
    gravity: AlignFlags,
    main_gravity: AlignFlags,
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

fn item_location<N>(
    item: &LinearItem<N>,
    axes: LinearAxes,
    inner_size: Size<f32>,
    content_origin: Point<f32>,
) -> Point<f32>
where
    N: LayoutNode,
    N::Style: LinearContainerStyle + LinearItemStyle,
{
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

fn container_baseline<N>(
    items: &[LinearItem<N>],
    axes: LinearAxes,
    inner_size: Size<f32>,
    content_origin: Point<f32>,
) -> Option<f32>
where
    N: LayoutNode,
    N::Style: LinearContainerStyle + LinearItemStyle,
{
    if axes.main == Axis::Horizontal {
        items
            .iter()
            .map(|item| {
                let location = item_location(item, axes, inner_size, content_origin);
                location.y + item.baseline.unwrap_or(item.cross_size)
            })
            .reduce(f32::max)
    } else {
        let first = items.first()?;
        let baseline = first.baseline.unwrap_or(first.main_size);
        Some(item_location(first, axes, inner_size, content_origin).y + baseline)
    }
}

#[allow(clippy::too_many_arguments)]
fn commit_in_flow<N>(
    items: &mut [LinearItem<N>],
    axes: LinearAxes,
    inner_size: Size<f32>,
    outer_size: Size<f32>,
    content_origin: Point<f32>,
) -> Size<f32>
where
    N: LayoutNode,
    N::Style: LinearContainerStyle + LinearItemStyle,
{
    let parent_size = inner_size.map(Some);
    let mut content_size = outer_size;
    for item in items {
        let target_size = size_from_axes(axes, item.main_size, item.cross_size);
        let mut input = LayoutInput::commit(
            target_size.map(Some),
            parent_size,
            target_size.map(AvailableSpace::Definite),
        );
        input.sizing_mode = SizingMode::IgnoreSizeStyles;
        input.definite_dimensions = size_from_axes(
            axes,
            item.main_size_is_definite,
            item.cross_size_is_definite,
        );
        let output = item.key.node.compute_layout(input);
        item.baseline = output.first_baselines.y;

        let offset = item.relative_offset;
        let mut location = item_location(item, axes, inner_size, content_origin);
        location.x += offset.x;
        location.y += offset.y;

        let mut layout = Layout::with_order(item.key.layout_order);
        layout.location = location;
        layout.size = output.size;
        layout.content_size = output.content_size;
        layout.border = item.border;
        layout.padding = item.padding;
        layout.margin = item.margin;
        item.key.node.set_unrounded_layout(layout);

        accumulate_scrollable_overflow(
            &mut content_size,
            location,
            output.size,
            output.content_size,
            item.overflow,
        );
    }
    content_size
}

#[inline]
fn measure_absolute_static_box<N>(
    node: N,
    containing_block: Size<f32>,
    static_axes: Size<bool>,
) -> Layout
where
    N: LayoutNode,
    N::Style: LinearContainerStyle + LinearItemStyle,
{
    let requested_axis = match (static_axes.width, static_axes.height) {
        (false, false) => return Layout::default(),
        (true, false) => RequestedAxis::Horizontal,
        (false, true) => RequestedAxis::Vertical,
        (true, true) => RequestedAxis::Both,
    };
    measure_absolute_layout(node, containing_block, requested_axis)
}

#[allow(clippy::too_many_arguments)]
fn commit_non_in_flow_children<N>(
    non_flow_items: &[NonFlowItem<N>],
    in_flow_items: &[LinearItem<N>],
    axes: LinearAxes,
    outer_size: Size<f32>,
    border: Edges<f32>,
    main_gravity: AlignFlags,
    mut content_size: Size<f32>,
) -> Size<f32>
where
    N: LayoutNode,
    N::Style: LinearContainerStyle + LinearItemStyle,
{
    let padding_box_size = Size::new(
        (outer_size.width - border.horizontal_sum()).max(0.0),
        (outer_size.height - border.vertical_sum()).max(0.0),
    );
    let padding_box_origin = Point::new(border.left, border.top);
    let mut absolute_before = 0usize;
    let mut in_flow_before = 0usize;

    for non_flow in non_flow_items {
        let (key, position, gravity, static_axes) = match *non_flow {
            NonFlowItem::Hidden {
                node,
                document_index,
            } => {
                hide_subtree(node);
                node.set_unrounded_layout(Layout::with_order(
                    u32::try_from(document_index).unwrap_or(u32::MAX),
                ));
                continue;
            }
            NonFlowItem::Absolute {
                key,
                position,
                gravity,
                static_axes,
            } => (key, position, gravity, static_axes),
        };
        let child = key.node;
        let document_index = key.document_index;
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

        match position {
            PositionProperty::Absolute => {
                let mut layout = compute_absolute_layout_with_static_position(
                    child,
                    padding_box_size,
                    |size, margin| {
                        let static_position = absolute_static_position(
                            axes,
                            padding_box_size,
                            padding_box_origin,
                            size,
                            margin,
                            gravity,
                            main_gravity,
                        );
                        Point::new(
                            static_position.x - border.left,
                            static_position.y - border.top,
                        )
                    },
                );
                layout.order = layout_order;
                layout.location.x += border.left;
                layout.location.y += border.top;
                accumulate_scrollable_overflow(
                    &mut content_size,
                    layout.location,
                    layout.size,
                    layout.content_size,
                    child.style().overflow(),
                );
                child.set_unrounded_layout(layout);
            }
            PositionProperty::Fixed => {
                let measured = measure_absolute_static_box(child, padding_box_size, static_axes);
                let static_position = absolute_static_position(
                    axes,
                    padding_box_size,
                    padding_box_origin,
                    measured.size,
                    measured.margin,
                    gravity,
                    main_gravity,
                );
                child.set_static_position(static_position);
            }
            _ => unreachable!(),
        }
    }
    content_size
}

#[allow(clippy::too_many_lines)]
pub fn compute_linear_layout<N>(node: N, input: LayoutInput) -> LayoutOutput
where
    N: LayoutNode,
    N::Style: LinearContainerStyle + LinearItemStyle,
{
    let style = node.style();
    let size_containment = size_containment(&style);
    let layout_contained = style.containment().contains(Contain::LAYOUT);
    let axes = LinearAxes::new(style.linear_direction(), style.direction());
    let align_items = style.align_items();
    let main_gravity = computed_main_gravity(style.justify_content(), axes);
    let commits_layout = input.goal == LayoutGoal::Commit;
    let weight_sum = style.linear_weight_sum().0;
    debug_assert!(
        weight_sum.is_finite() && weight_sum >= 0.0,
        "linear-weight-sum must be finite and non-negative"
    );
    let container_aspect_ratio = used_aspect_ratio(style.aspect_ratio());
    let container_box_sizing = style.box_sizing();
    let style_definite = if input.sizing_mode == SizingMode::IgnoreSizeStyles {
        Size::new(false, false)
    } else {
        size_definiteness(style.size(), input.parent_size, container_aspect_ratio)
    };
    let ResolvedContainerBox {
        padding,
        border,
        box_inset: container_inset,
        min: min_size,
        max: max_size,
        outer: mut outer_size,
        available_inner: initial_available_inner,
        ..
    } = resolve_container_box(&style, input);
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
    if input.sizing_mode != SizingMode::IgnoreSizeStyles {
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
    let inner_available_space = Size::new(
        inner_size
            .width
            .map_or(initial_available_inner.width, AvailableSpace::Definite),
        inner_size
            .height
            .map_or(initial_available_inner.height, AvailableSpace::Definite),
    );
    let mut percentage_basis = definite_inner_size;

    let child_count = node.child_count();
    let mut items = Vec::with_capacity(child_count);
    let mut non_flow_items = Vec::new();
    let mut has_nonzero_order = false;
    let mut has_box_basis_dependency = false;
    let mut has_relative_basis_dependency = false;
    let mut absolute_count = 0usize;
    for (document_index, child) in node.children().enumerate() {
        let child_style = child.style();
        let position = child_style.position();
        let is_absolute = matches!(
            position,
            PositionProperty::Absolute | PositionProperty::Fixed
        );
        if child_style.display().is_none() {
            if commits_layout {
                non_flow_items.push(NonFlowItem::Hidden {
                    node: child,
                    document_index,
                });
            }
            continue;
        }
        if is_absolute {
            if commits_layout {
                let inset = child_style.inset();
                non_flow_items.push(NonFlowItem::Absolute {
                    key: ItemKey {
                        node: child,
                        document_index,
                        css_order: 0,
                        layout_order: 0,
                    },
                    position,
                    gravity: computed_cross_gravity(child_style.align_self(), align_items, axes),
                    static_axes: Size::new(
                        inset.left.is_auto() && inset.right.is_auto(),
                        inset.top.is_auto() && inset.bottom.is_auto(),
                    ),
                });
                absolute_count = absolute_count.saturating_add(1);
            }
            continue;
        }
        let css_order = child_style.order();
        has_nonzero_order |= css_order != 0;
        let nudges = position == PositionProperty::Relative;
        let flags = initial_item_flags(&child_style, percentage_basis.width, nudges);
        let item = resolve_item(
            child_style,
            LinearItemSeed {
                key: ItemKey {
                    node: child,
                    document_index,
                    css_order,
                    layout_order: if commits_layout {
                        u32::try_from(absolute_count).unwrap_or(u32::MAX)
                    } else {
                        0
                    },
                },
                flags,
            },
            percentage_basis,
            align_items,
            axes,
            nudges,
        );
        has_box_basis_dependency |= item.flags.needs_box_refresh();
        has_relative_basis_dependency |= item.flags.needs_relative_offset_refresh();
        items.push(item);
    }
    if has_nonzero_order {
        items.sort_unstable_by(
            |left, right| match left.key.css_order.cmp(&right.key.css_order) {
                Ordering::Equal => left.key.document_index.cmp(&right.key.document_index),
                ordering => ordering,
            },
        );
    }
    if commits_layout {
        for (in_flow_order, item) in items.iter_mut().enumerate() {
            let absolute_before = match item.key.css_order.cmp(&0) {
                Ordering::Less => 0,
                Ordering::Equal => usize::try_from(item.key.layout_order).unwrap_or(usize::MAX),
                Ordering::Greater => absolute_count,
            };
            item.key.layout_order =
                u32::try_from(in_flow_order.saturating_add(absolute_before)).unwrap_or(u32::MAX);
        }
    }

    size_items(
        &mut items,
        axes,
        percentage_basis,
        inner_size,
        inner_available_space,
        weight_sum,
    );
    let (natural, used_main) = natural_content_size(&items, axes);
    let container_natural = match size_containment {
        Some(intrinsic) => Size::new(
            intrinsic.width.unwrap_or(0.0),
            intrinsic.height.unwrap_or(0.0),
        ),
        None => natural,
    };
    if outer_size.width.is_none() {
        outer_size.width = Some(clamp_axis(
            container_natural.width + container_inset.width,
            min_size.width,
            max_size.width,
            container_inset.width,
        ));
    }
    if outer_size.height.is_none() {
        outer_size.height = Some(clamp_axis(
            container_natural.height + container_inset.height,
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

    if !outer_definite.width && has_box_basis_dependency {
        let contained_basis = if size_containment.is_some() {
            final_inner_size
        } else {
            natural
        };
        percentage_basis = Size::new(
            inner_size.width.unwrap_or(contained_basis.width),
            inner_size.height.unwrap_or(contained_basis.height),
        )
        .map(Some);
        for item in &mut items {
            if !item.flags.needs_box_refresh() {
                continue;
            }
            let item_style = item.key.node.style();
            refresh_item_edges(item_style, item, percentage_basis);
        }
    }

    position_items(&mut items, axes, final_inner_size, main_gravity, used_main);
    let content_origin = Point::new(border.left + padding.left, border.top + padding.top);
    if !commits_layout {
        let baseline = if layout_contained {
            None
        } else {
            container_baseline(&items, axes, final_inner_size, content_origin)
        };
        return LayoutOutput::new(final_outer_size, final_outer_size)
            .with_first_baselines(Point::new(None, baseline));
    }

    if has_relative_basis_dependency {
        let final_percentage_basis = final_inner_size.map(Some);
        for item in &mut items {
            if !item.flags.needs_relative_offset_refresh() {
                continue;
            }
            let item_style = item.key.node.style();
            let inset = resolve_insets(item_style.inset(), final_percentage_basis);
            item.relative_offset = relative_offset(inset, item_style.direction());
        }
    }

    let mut content_size = commit_in_flow(
        &mut items,
        axes,
        final_inner_size,
        final_outer_size,
        content_origin,
    );
    if !non_flow_items.is_empty() {
        content_size = commit_non_in_flow_children(
            &non_flow_items,
            &items,
            axes,
            final_outer_size,
            border,
            main_gravity,
            content_size,
        );
    }
    let content_size = own_scrollable_overflow(&style, final_outer_size, content_size);
    let baseline = if layout_contained {
        None
    } else {
        container_baseline(&items, axes, final_inner_size, content_origin)
    };
    LayoutOutput::new(final_outer_size, content_size)
        .with_first_baselines(Point::new(None, baseline))
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use style_traits::values::specified::AllowedNumericType;
    use stylo::Zero;
    use stylo::values::computed::length_percentage::{CalcNode, ComputedLeaf};
    use stylo::values::computed::{Display, Length, NonNegativeLengthPercentage, Percentage};
    use stylo::values::generics::NonNegative;

    use super::*;

    fn uniform<T: Clone>(value: T) -> Edges<T> {
        Edges {
            left: value.clone(),
            right: value.clone(),
            top: value.clone(),
            bottom: value,
        }
    }

    fn percent(fraction: f32) -> LengthPercentage {
        LengthPercentage::new_percent(Percentage(fraction))
    }

    #[derive(Debug, Clone)]
    struct DependencyStyle {
        size: Size<StyleSize>,
        min_size: Size<StyleSize>,
        max_size: Size<MaxSize>,
        margin: Edges<Margin>,
        padding: Edges<NonNegativeLengthPercentage>,
        inset: Edges<Inset>,
    }

    impl Default for DependencyStyle {
        fn default() -> Self {
            Self {
                size: Size::new(StyleSize::auto(), StyleSize::auto()),
                min_size: Size::new(StyleSize::auto(), StyleSize::auto()),
                max_size: Size::new(MaxSize::none(), MaxSize::none()),
                margin: uniform(Margin::zero()),
                padding: uniform(NonNegativeLengthPercentage::zero()),
                inset: uniform(Inset::auto()),
            }
        }
    }

    impl CoreStyle for DependencyStyle {
        fn display(&self) -> Display {
            Display::Linear
        }

        fn inset(&self) -> Edges<&Inset> {
            self.inset.as_ref()
        }

        fn size(&self) -> Size<&StyleSize> {
            self.size.as_ref()
        }

        fn min_size(&self) -> Size<&StyleSize> {
            self.min_size.as_ref()
        }

        fn max_size(&self) -> Size<&MaxSize> {
            self.max_size.as_ref()
        }

        fn margin(&self) -> Edges<&Margin> {
            self.margin.as_ref()
        }

        fn padding(&self) -> Edges<&NonNegativeLengthPercentage> {
            self.padding.as_ref()
        }
    }

    impl LinearItemStyle for DependencyStyle {}

    #[test]
    fn linear_item_scratch_stays_cache_conscious() {
        let size = core::mem::size_of::<LinearItem<[usize; 2]>>();
        assert!(size <= 200, "LinearItem grew to {size} bytes");
    }

    #[test]
    fn edge_dependency_values_cover_percent_and_calc() {
        let length = LengthPercentage::new_length(Length::new(1.0));
        let mixed_calc = LengthPercentage::new_calc(
            CalcNode::Sum(
                vec![
                    CalcNode::Leaf(ComputedLeaf::Percentage(Percentage(0.5))),
                    CalcNode::Leaf(ComputedLeaf::Length(Length::new(10.0))),
                ]
                .into(),
            ),
            AllowedNumericType::All,
        );
        let folded_calc = LengthPercentage::new_calc(
            CalcNode::Sum(
                vec![
                    CalcNode::Leaf(ComputedLeaf::Length(Length::new(10.0))),
                    CalcNode::Leaf(ComputedLeaf::Length(Length::new(20.0))),
                ]
                .into(),
            ),
            AllowedNumericType::All,
        );
        assert!(!lp_depends_on_basis(&length));
        assert!(lp_depends_on_basis(&percent(0.5)));
        assert!(lp_depends_on_basis(&mixed_calc));
        assert!(!lp_depends_on_basis(&folded_calc));

        assert!(!margin_depends_on_basis(&Margin::Auto));
        assert!(!margin_depends_on_basis(&Margin::LengthPercentage(
            length.clone()
        )));
        assert!(margin_depends_on_basis(&Margin::LengthPercentage(percent(
            0.5
        ))));
        assert!(!inset_depends_on_basis(&Inset::Auto));
        assert!(inset_depends_on_basis(&Inset::LengthPercentage(mixed_calc)));
    }

    #[test]
    fn linear_dependency_policy_matches_the_two_refresh_phases() {
        let width_only = DependencyStyle {
            size: Size::new(
                StyleSize::LengthPercentage(NonNegative(percent(0.5))),
                StyleSize::auto(),
            ),
            ..DependencyStyle::default()
        };
        let width_dependencies = initial_item_flags(&width_only, None, true);
        assert!(!width_dependencies.needs_box_refresh());
        assert!(!width_dependencies.needs_relative_offset_refresh());

        for style in [
            DependencyStyle {
                min_size: Size::new(
                    StyleSize::LengthPercentage(NonNegative(percent(0.5))),
                    StyleSize::auto(),
                ),
                ..DependencyStyle::default()
            },
            DependencyStyle {
                max_size: Size::new(
                    MaxSize::LengthPercentage(NonNegative(percent(0.5))),
                    MaxSize::none(),
                ),
                ..DependencyStyle::default()
            },
        ] {
            assert!(!initial_item_flags(&style, None, true).needs_box_refresh());
        }

        for (style, expected_refresh) in [
            (
                DependencyStyle {
                    margin: Edges {
                        left: Margin::LengthPercentage(percent(0.5)),
                        ..uniform(Margin::zero())
                    },
                    ..DependencyStyle::default()
                },
                LinearItemFlags::MARGIN_REFRESH,
            ),
            (
                DependencyStyle {
                    padding: Edges {
                        left: NonNegative(percent(0.5)),
                        ..uniform(NonNegativeLengthPercentage::zero())
                    },
                    ..DependencyStyle::default()
                },
                LinearItemFlags::PADDING_REFRESH,
            ),
        ] {
            let flags = initial_item_flags(&style, None, true);
            assert_eq!(flags.0 & LinearItemFlags::BOX_REFRESH, expected_refresh);
            assert!(!initial_item_flags(&style, Some(100.0), true).needs_box_refresh());
        }

        for inset in [
            Edges {
                left: Inset::LengthPercentage(percent(0.5)),
                ..uniform(Inset::auto())
            },
            Edges {
                top: Inset::LengthPercentage(percent(0.5)),
                ..uniform(Inset::auto())
            },
        ] {
            let style = DependencyStyle {
                inset,
                ..DependencyStyle::default()
            };
            let dependencies = initial_item_flags(&style, None, true);
            assert!(!dependencies.needs_box_refresh());
            assert!(dependencies.needs_relative_offset_refresh());
            assert!(!initial_item_flags(&style, None, false).needs_relative_offset_refresh());
        }
    }

    #[test]
    fn flow_edge_writes_map_to_physical_edges_under_reversal() {
        let mut edges = Edges::uniform(0.0_f32);
        set_flow_start(&mut edges, Axis::Horizontal, false, 1.0);
        set_flow_start(&mut edges, Axis::Horizontal, true, 2.0);
        set_flow_start(&mut edges, Axis::Vertical, false, 3.0);
        set_flow_start(&mut edges, Axis::Vertical, true, 4.0);
        assert_eq!(
            (edges.left, edges.right, edges.top, edges.bottom),
            (1.0, 2.0, 3.0, 4.0)
        );

        let mut edges = Edges::uniform(0.0_f32);
        set_flow_end(&mut edges, Axis::Horizontal, false, 1.0);
        set_flow_end(&mut edges, Axis::Horizontal, true, 2.0);
        set_flow_end(&mut edges, Axis::Vertical, false, 3.0);
        set_flow_end(&mut edges, Axis::Vertical, true, 4.0);
        assert_eq!(
            (edges.left, edges.right, edges.top, edges.bottom),
            (2.0, 1.0, 4.0, 3.0)
        );

        let edges = Edges {
            left: 10.0,
            right: 20.0,
            top: 30.0,
            bottom: 40.0,
        };
        assert_eq!(
            (
                flow_start(edges, Axis::Horizontal, true),
                flow_end(edges, Axis::Horizontal, true),
                flow_start(edges, Axis::Vertical, true),
                flow_end(edges, Axis::Vertical, true),
            ),
            (20.0, 10.0, 40.0, 30.0)
        );
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn gravity_re_keying_matches_the_legacy_mappings() {
        let ltr_column = LinearAxes::new(linear_direction::T::Column, direction::T::Ltr);
        let rtl_column = LinearAxes::new(linear_direction::T::Column, direction::T::Rtl);
        let ltr_row = LinearAxes::new(linear_direction::T::Row, direction::T::Ltr);
        let ltr_row_reverse = LinearAxes::new(linear_direction::T::RowReverse, direction::T::Ltr);

        assert_eq!(
            computed_main_gravity(ContentDistribution::normal(), ltr_column),
            AlignFlags::START
        );
        assert_eq!(
            computed_main_gravity(ContentDistribution::new(AlignFlags::FLEX_END), ltr_column),
            AlignFlags::END
        );
        assert_eq!(
            computed_main_gravity(ContentDistribution::new(AlignFlags::CENTER), ltr_column),
            AlignFlags::CENTER
        );
        assert_eq!(
            computed_main_gravity(
                ContentDistribution::new(AlignFlags::SPACE_BETWEEN),
                ltr_column
            ),
            AlignFlags::SPACE_BETWEEN
        );
        assert_eq!(
            computed_main_gravity(
                ContentDistribution::new(AlignFlags::SPACE_AROUND),
                ltr_column
            ),
            AlignFlags::START
        );
        assert_eq!(
            computed_main_gravity(ContentDistribution::new(AlignFlags::RIGHT), ltr_row),
            AlignFlags::END
        );
        assert_eq!(
            computed_main_gravity(ContentDistribution::new(AlignFlags::RIGHT), ltr_row_reverse),
            AlignFlags::START
        );
        assert_eq!(
            computed_main_gravity(ContentDistribution::new(AlignFlags::RIGHT), ltr_column),
            AlignFlags::START
        );

        assert_eq!(
            computed_cross_gravity(SelfAlignment::auto(), ItemPlacement::normal(), ltr_column),
            AlignFlags::NORMAL
        );
        assert_eq!(
            computed_cross_gravity(
                SelfAlignment(AlignFlags::CENTER),
                ItemPlacement(AlignFlags::FLEX_END),
                ltr_column
            ),
            AlignFlags::CENTER
        );
        assert_eq!(
            computed_cross_gravity(
                SelfAlignment::auto(),
                ItemPlacement(AlignFlags::STRETCH),
                ltr_column
            ),
            AlignFlags::STRETCH
        );
        assert_eq!(
            computed_cross_gravity(
                SelfAlignment(AlignFlags::BASELINE),
                ItemPlacement(AlignFlags::END),
                ltr_column
            ),
            AlignFlags::END
        );
        assert_eq!(
            computed_cross_gravity(
                SelfAlignment(AlignFlags::LEFT),
                ItemPlacement::normal(),
                ltr_column
            ),
            AlignFlags::START
        );
        assert_eq!(
            computed_cross_gravity(
                SelfAlignment(AlignFlags::LEFT),
                ItemPlacement::normal(),
                rtl_column
            ),
            AlignFlags::END
        );
        assert_eq!(
            computed_cross_gravity(
                SelfAlignment(AlignFlags::RIGHT),
                ItemPlacement::normal(),
                ltr_row
            ),
            AlignFlags::START
        );
        assert_eq!(
            computed_cross_gravity(
                SelfAlignment(AlignFlags::SAFE | AlignFlags::CENTER),
                ItemPlacement::normal(),
                ltr_column
            ),
            AlignFlags::CENTER
        );
    }
}
