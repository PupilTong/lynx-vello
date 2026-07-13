//! Starlight linear layout.
//!
//! Linear is a Lynx-only, single-axis formatting context implemented as a
//! first-class neutron-star algorithm alongside Flexbox and Grid.

// Item counts are transient `Vec` lengths and layout coordinates are `f32`.
// Converting a practical child count to `f32` for space distribution is safe.
#![allow(clippy::cast_precision_loss)]

use core::cmp::Ordering;

use super::util::{
    ResolvedContainerBox, ResolvedItemBox, apply_aspect_ratio, auto_edges_to_zero, box_inset_size,
    clamp_axis, preferred_size_definiteness, relative_offset, resolve_container_box, resolve_edges,
    resolve_insets, resolve_item_box, resolve_length_percentage, resolve_optional_edges,
};
use super::{compute_absolute_layout_with_static_position, hide_subtree, measure_absolute_layout};
use crate::geometry::{Edges, Point, Size};
use crate::style::{
    AlignItems, BoxGenerationMode, BoxSizing, CoreStyle, Dimension, Direction, JustifyContent,
    LengthPercentage, LengthPercentageAuto, LinearContainerStyle, LinearCrossGravity,
    LinearGravity, LinearItemStyle, LinearLayoutGravity, LinearOrientation, Position,
};
use crate::tree::{
    AvailableSpace, Layout, LayoutGoal, LayoutInput, LayoutOutput, LayoutSession, LinearSource,
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

#[derive(Debug, Clone, Copy)]
enum NonFlowItem {
    Hidden {
        node: NodeId,
        document_index: usize,
    },
    Absolute {
        key: ItemKey,
        position: Position,
        gravity: CrossGravity,
        static_axes: Size<bool>,
    },
}

#[derive(Debug, Clone, Copy)]
#[repr(transparent)]
struct LinearItemFlags(u8);

impl LinearItemFlags {
    const MARGIN_REFRESH: u8 = 1 << 0;
    const PADDING_BORDER_REFRESH: u8 = 1 << 1;
    const RELATIVE_OFFSET: u8 = 1 << 2;
    const FROZEN: u8 = 1 << 3;
    const BOX_REFRESH: u8 = Self::MARGIN_REFRESH | Self::PADDING_BORDER_REFRESH;

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
    const fn needs_padding_border_refresh(self) -> bool {
        self.0 & Self::PADDING_BORDER_REFRESH != 0
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
struct LinearItemSeed {
    key: ItemKey,
    flags: LinearItemFlags,
}

/// One allocation-friendly scratch record per in-flow item. Raw style remains
/// in the immutable source and is reborrowed only for intrinsic probes or a
/// cyclic percentage re-resolution.
#[derive(Debug)]
#[allow(clippy::struct_excessive_bools)]
struct LinearItem {
    key: ItemKey,
    gravity: CrossGravity,
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
    scrollbar: Size<f32>,
    relative_offset: Point<f32>,
    box_sizing: BoxSizing,
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

#[inline]
fn dimension_is_intrinsic(value: Dimension) -> bool {
    matches!(
        value,
        Dimension::MinContent | Dimension::MaxContent | Dimension::FitContent(_)
    )
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
fn initial_item_flags(style: &impl LinearItemStyle, inline_basis: Option<f32>) -> LinearItemFlags {
    let inset = style.inset();
    let relative_offset = auto_length_depends_on_basis(inset.left)
        || auto_length_depends_on_basis(inset.right)
        || auto_length_depends_on_basis(inset.top)
        || auto_length_depends_on_basis(inset.bottom);
    let (margin_refresh, padding_border_refresh) = if inline_basis.is_none() {
        // Only used edges can affect the remaining layout phases. Starlight's
        // internal min/max rewrite has no downstream consumer after sizing,
        // while relative insets resolve independently against the final
        // clamped containing block.
        let margin = style.margin();
        let padding = style.padding();
        let border = style.border();
        (
            auto_length_depends_on_basis(margin.left)
                || auto_length_depends_on_basis(margin.right)
                || auto_length_depends_on_basis(margin.top)
                || auto_length_depends_on_basis(margin.bottom),
            length_depends_on_basis(padding.left)
                || length_depends_on_basis(padding.right)
                || length_depends_on_basis(padding.top)
                || length_depends_on_basis(padding.bottom)
                || length_depends_on_basis(border.left)
                || length_depends_on_basis(border.right)
                || length_depends_on_basis(border.top)
                || length_depends_on_basis(border.bottom),
        )
    } else {
        (false, false)
    };
    let mut flags = 0;
    if margin_refresh {
        flags |= LinearItemFlags::MARGIN_REFRESH;
    }
    if padding_border_refresh {
        flags |= LinearItemFlags::PADDING_BORDER_REFRESH;
    }
    if relative_offset {
        flags |= LinearItemFlags::RELATIVE_OFFSET;
    }
    LinearItemFlags(flags)
}

fn resolve_item<Source: LinearSource>(
    source: &Source,
    style: impl LinearItemStyle,
    seed: LinearItemSeed,
    percentage_basis: Size<Option<f32>>,
    container_cross: LinearCrossGravity,
    align_items: Option<AlignItems>,
    axes: LinearAxes,
) -> LinearItem {
    let weight = style.linear_weight();
    debug_assert!(
        weight.is_finite() && weight >= 0.0,
        "linear-weight must be finite and non-negative"
    );
    let direction = style.direction();
    let gravity = computed_cross_gravity(
        style.linear_layout_gravity(),
        style.align_self(),
        container_cross,
        align_items,
        axes,
    );
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
        scrollbar,
        inset,
        ..
    } = resolve_item_box(source, &style, percentage_basis);
    let relative_offset = relative_offset(inset, direction);

    LinearItem {
        key: seed.key,
        gravity,
        weight,
        size_is_auto: raw_size.map(Dimension::is_auto),
        size_is_intrinsic: raw_size.map(dimension_is_intrinsic),
        has_intrinsic_size: has_intrinsic_dimension([
            raw_size.width,
            raw_min_size.width,
            raw_max_size.width,
        ]) || has_intrinsic_dimension([
            raw_size.height,
            raw_min_size.height,
            raw_max_size.height,
        ]),
        preferred_size,
        preferred_size_is_definite: preferred_size_definiteness(
            raw_size,
            percentage_basis,
            aspect_ratio,
        ),
        min_size,
        max_size,
        margin,
        margin_auto,
        padding,
        border,
        scrollbar,
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
fn refresh_item_edges<Source: LinearSource>(
    source: &Source,
    style: impl LinearItemStyle,
    item: &mut LinearItem,
    percentage_basis: Size<Option<f32>>,
) {
    let resolve_calc = |handle, basis| source.resolve_calc(handle, basis);
    if item.flags.needs_padding_border_refresh() {
        item.padding = resolve_edges(style.padding(), percentage_basis.width, &resolve_calc);
        item.border = resolve_edges(style.border(), percentage_basis.width, &resolve_calc);
    }
    if item.flags.needs_margin_refresh() {
        let margin_value = style.margin();
        let optional_margin =
            resolve_optional_edges(margin_value, percentage_basis.width, &resolve_calc);
        item.margin = auto_edges_to_zero(optional_margin);
        item.margin_auto = margin_value.map(LengthPercentageAuto::is_auto);
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
fn needs_min_content(values: [Dimension; 3]) -> bool {
    values
        .into_iter()
        .any(|value| matches!(value, Dimension::MinContent | Dimension::FitContent(_)))
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
    let inset = box_inset_size(item.padding, item.border, item.scrollbar);
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
    if !item.has_intrinsic_size {
        return;
    }
    let (size, min_size, max_size) = {
        let style = source.linear_item_style(item.key.node);
        (style.size(), style.min_size(), style.max_size())
    };
    let need_min = Size::new(
        needs_min_content([size.width, min_size.width, max_size.width]),
        needs_min_content([size.height, min_size.height, max_size.height]),
    );
    let need_max = Size::new(
        needs_max_content([size.width, min_size.width, max_size.width]),
        needs_max_content([size.height, min_size.height, max_size.height]),
    );
    let min_content = if need_min.width || need_min.height {
        intrinsic_measurement(
            source,
            session,
            item,
            percentage_basis,
            need_min,
            AvailableSpace::MinContent,
        )
        .size
    } else {
        Size::ZERO
    };
    let max_content = if need_max.width || need_max.height {
        intrinsic_measurement(
            source,
            session,
            item,
            percentage_basis,
            need_max,
            AvailableSpace::MaxContent,
        )
        .size
    } else {
        Size::ZERO
    };
    let inset = box_inset_size(item.padding, item.border, item.scrollbar);

    item.preferred_size = Size::new(
        intrinsic_axis_value(
            source,
            size.width,
            item.preferred_size.width,
            min_content.width,
            max_content.width,
            percentage_basis.width,
            inset.width,
            item.box_sizing,
        ),
        intrinsic_axis_value(
            source,
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
            source,
            min_size.width,
            item.min_size.width,
            min_content.width,
            max_content.width,
            percentage_basis.width,
            inset.width,
            item.box_sizing,
        ),
        intrinsic_axis_value(
            source,
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
        intrinsic_axis_value(
            source,
            max_size.width,
            item.max_size.width,
            min_content.width,
            max_content.width,
            percentage_basis.width,
            inset.width,
            item.box_sizing,
        ),
        intrinsic_axis_value(
            source,
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
fn ratio_cross_size(item: &LinearItem, axes: LinearAxes, forced_main: f32) -> Option<f32> {
    let ratio = item.aspect_ratio?;
    if !ratio.is_finite() || ratio <= 0.0 || !axes.cross.size(item.size_is_auto) {
        return None;
    }
    let inset = box_inset_size(item.padding, item.border, item.scrollbar);
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
    constraint_inner_size: Size<Option<f32>>,
    available_space: Size<AvailableSpace>,
    forced_main: Option<f32>,
) where
    Source: LinearSource,
    Session: LayoutSession<Source>,
{
    let inset = box_inset_size(item.padding, item.border, item.scrollbar);
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
    item.baseline = output.first_baselines.y;
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
            .filter(|item| item.weight > 0.0 && !item.flags.is_frozen())
        {
            let tentative = if free_space > 0.0 {
                free_space * item.weight / active_weight
            } else {
                0.0
            };
            let floor = axes
                .main
                .size(box_inset_size(item.padding, item.border, item.scrollbar));
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
fn size_items<Source, Session>(
    source: &Source,
    session: &mut Session,
    items: &mut [LinearItem],
    axes: LinearAxes,
    percentage_basis: Size<Option<f32>>,
    constraint_inner_size: Size<Option<f32>>,
    available_space: Size<AvailableSpace>,
    weight_sum: f32,
) where
    Source: LinearSource,
    Session: LayoutSession<Source>,
{
    let constrained_main = axes.main.size(constraint_inner_size);
    for item in items.iter_mut() {
        resolve_intrinsic_sizes(source, session, item, percentage_basis);
        if !(constrained_main.is_some() && item.weight > 0.0) {
            measure_item(
                source,
                session,
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
                source,
                session,
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
fn natural_content_size(items: &[LinearItem], axes: LinearAxes) -> (Size<f32>, f32) {
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
    used_main: f32,
) {
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
                location.y + item.baseline.unwrap_or(item.cross_size)
            })
            .reduce(f32::max)
    } else {
        let first = items.first()?;
        let baseline = first.baseline?;
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
        item.baseline = output.first_baselines.y;

        let offset = item.relative_offset;
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

#[inline]
fn measure_absolute_static_box<Source, Session>(
    source: &Source,
    session: &mut Session,
    node: NodeId,
    containing_block: Size<f32>,
    static_axes: Size<bool>,
) -> Layout
where
    Source: LinearSource,
    Session: LayoutSession<Source>,
{
    let requested_axis = match (static_axes.width, static_axes.height) {
        (false, false) => return Layout::default(),
        (true, false) => RequestedAxis::Horizontal,
        (false, true) => RequestedAxis::Vertical,
        (true, true) => RequestedAxis::Both,
    };
    measure_absolute_layout(source, session, node, containing_block, requested_axis)
}

#[allow(clippy::too_many_arguments)]
fn commit_non_in_flow_children<Source, Session>(
    source: &Source,
    session: &mut Session,
    non_flow_items: &[NonFlowItem],
    in_flow_items: &[LinearItem],
    axes: LinearAxes,
    outer_size: Size<f32>,
    border: Edges<f32>,
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
    let mut absolute_before = 0usize;
    let mut in_flow_before = 0usize;

    for non_flow in non_flow_items {
        let (key, position, gravity, static_axes) = match *non_flow {
            NonFlowItem::Hidden {
                node,
                document_index,
            } => {
                hide_subtree(source, session, node);
                session.set_unrounded_layout(
                    node,
                    &Layout::with_order(u32::try_from(document_index).unwrap_or(u32::MAX)),
                );
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

        match position {
            Position::Absolute => {
                // The common positioned algorithm already knows the final
                // border-box size and used margins before it consumes the
                // static fallback. Derive Linear alignment there so this
                // committing path lays out the child exactly once.
                let mut layout = compute_absolute_layout_with_static_position(
                    source,
                    session,
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
                content_size.width = content_size
                    .width
                    .max(layout.location.x + layout.size.width.max(layout.content_size.width));
                content_size.height = content_size
                    .height
                    .max(layout.location.y + layout.size.height.max(layout.content_size.height));
                session.set_unrounded_layout(child, &layout);
            }
            Position::AbsoluteHoisted => {
                let measured = measure_absolute_static_box(
                    source,
                    session,
                    child,
                    padding_box_size,
                    static_axes,
                );
                // Static-position fallback is gravity-driven. Out-of-flow
                // automatic margins are resolved by the common positioned
                // algorithm, not by Linear's in-flow auto-margin rule.
                let static_position = absolute_static_position(
                    axes,
                    padding_box_size,
                    padding_box_origin,
                    measured.size,
                    measured.margin,
                    gravity,
                    main_gravity,
                );
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
    let commits_layout = input.goal == LayoutGoal::Commit;
    let weight_sum = style.linear_weight_sum();
    debug_assert!(
        weight_sum.is_finite() && weight_sum >= 0.0,
        "linear-weight-sum must be finite and non-negative"
    );
    let container_aspect_ratio = style.aspect_ratio();
    let container_box_sizing = style.box_sizing();
    let style_definite = if input.sizing_mode == SizingMode::ContentSize {
        Size::new(false, false)
    } else {
        preferred_size_definiteness(style.size(), input.parent_size, container_aspect_ratio)
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
    } = resolve_container_box(source, &style, input);
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
    // Starlight gates Linear weight distribution and default cross stretch on
    // the incoming constraint mode, not CSS percentage definiteness. Flex can
    // decide an item's geometry while §9.8 still marks that size indefinite;
    // keep that distinction for descendant percentage bases, but treat the
    // decided inner size as a definite Linear constraint.
    let inner_available_space = Size::new(
        inner_size
            .width
            .map_or(initial_available_inner.width, AvailableSpace::Definite),
        inner_size
            .height
            .map_or(initial_available_inner.height, AvailableSpace::Definite),
    );
    let mut percentage_basis = definite_inner_size;

    let child_count = source.child_count(node);
    let mut items = Vec::with_capacity(child_count);
    let mut non_flow_items = Vec::new();
    let mut has_nonzero_order = false;
    let mut has_box_basis_dependency = false;
    let mut has_relative_basis_dependency = false;
    let mut absolute_count = 0usize;
    for document_index in 0..child_count {
        let child = source.child_id(node, document_index);
        let child_style = source.linear_item_style(child);
        let box_generation_mode = child_style.box_generation_mode();
        let position = child_style.position();
        let is_absolute = matches!(position, Position::Absolute | Position::AbsoluteHoisted);
        if box_generation_mode == BoxGenerationMode::None {
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
                    gravity: computed_cross_gravity(
                        child_style.linear_layout_gravity(),
                        child_style.align_self(),
                        container_cross,
                        align_items,
                        axes,
                    ),
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
        let flags = initial_item_flags(&child_style, percentage_basis.width);
        let item = resolve_item(
            source,
            child_style,
            LinearItemSeed {
                key: ItemKey {
                    node: child,
                    document_index,
                    css_order,
                    // Temporarily retain the number of preceding effective-order
                    // zero absolute siblings. After sorting this is exactly the
                    // merge offset for a zero-order in-flow item.
                    layout_order: if commits_layout {
                        u32::try_from(absolute_count).unwrap_or(u32::MAX)
                    } else {
                        0
                    },
                },
                flags,
            },
            percentage_basis,
            container_cross,
            align_items,
            axes,
        );
        has_box_basis_dependency |= item.flags.needs_box_refresh();
        has_relative_basis_dependency |= item.flags.needs_relative_offset_refresh();
        items.push(item);
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
        source,
        session,
        &mut items,
        axes,
        percentage_basis,
        inner_size,
        inner_available_space,
        weight_sum,
    );
    let (natural, used_main) = natural_content_size(&items, axes);
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
    if !outer_definite.width && has_box_basis_dependency {
        // Starlight refreshes BoxInfo before the container's provisional
        // border-box size is clamped by min/max. A previously constrained axis
        // keeps that constraint; an intrinsic axis uses its natural content
        // size. Keep the final, clamped inner size solely for container
        // geometry below.
        percentage_basis = Size::new(
            inner_size.width.unwrap_or(natural.width),
            inner_size.height.unwrap_or(natural.height),
        )
        .map(Some);
        for item in &mut items {
            if !item.flags.needs_box_refresh() {
                continue;
            }
            // Starlight's UpdateContainerSize refreshes percentage-dependent
            // used edges after an intrinsic container becomes definite, but
            // it deliberately does not measure the child again. Preserve the
            // already-used size/baseline/definiteness and weight-freeze state.
            // Min/max has no consumer after sizing, so re-resolving it here
            // would only create dead stores.
            let item_style = source.linear_item_style(item.key.node);
            refresh_item_edges(source, item_style, item, percentage_basis);
        }
        // The intrinsic container size remains fixed. Percentage-dependent
        // used values may overflow it, but neither item measurement nor the
        // already-computed main total feeds back into the basis. This mirrors
        // Starlight's UpdateContainerSize/UpdateBoxData split exactly.
    }

    position_items(&mut items, axes, final_inner_size, main_gravity, used_main);
    let content_origin = Point::new(border.left + padding.left, border.top + padding.top);
    if !commits_layout {
        let baseline = container_baseline(&items, axes, final_inner_size, content_origin);
        return LayoutOutput::new(final_outer_size, final_outer_size)
            .with_first_baselines(Point::new(None, baseline));
    }

    // Starlight applies relative positioning during alignment, after the
    // container's own min/max clamp. Re-resolve percentage-dependent in-flow
    // insets against that final containing block here, independently of the
    // earlier provisional BoxInfo refresh. Absolute-length/auto insets retain
    // their already-resolved fast-path value.
    if has_relative_basis_dependency {
        let final_percentage_basis = final_inner_size.map(Some);
        let resolve_calc = |handle, basis| source.resolve_calc(handle, basis);
        for item in &mut items {
            if !item.flags.needs_relative_offset_refresh() {
                continue;
            }
            let item_style = source.linear_item_style(item.key.node);
            let inset = resolve_insets(item_style.inset(), final_percentage_basis, &resolve_calc);
            item.relative_offset = relative_offset(inset, item_style.direction());
        }
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
    if !non_flow_items.is_empty() {
        content_size = commit_non_in_flow_children(
            source,
            session,
            &non_flow_items,
            &items,
            axes,
            final_outer_size,
            border,
            main_gravity,
            content_size,
        );
    }
    let baseline = container_baseline(&items, axes, final_inner_size, content_origin);
    LayoutOutput::new(final_outer_size, content_size)
        .with_first_baselines(Point::new(None, baseline))
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[derive(Debug, Clone, Copy)]
    struct DependencyStyle {
        size: Size<Dimension>,
        min_size: Size<Dimension>,
        max_size: Size<Dimension>,
        margin: Edges<LengthPercentageAuto>,
        padding: Edges<LengthPercentage>,
        border: Edges<LengthPercentage>,
        inset: Edges<LengthPercentageAuto>,
    }

    impl Default for DependencyStyle {
        fn default() -> Self {
            Self {
                size: Size::new(Dimension::Auto, Dimension::Auto),
                min_size: Size::new(Dimension::Auto, Dimension::Auto),
                max_size: Size::new(Dimension::Auto, Dimension::Auto),
                margin: Edges::uniform(LengthPercentageAuto::ZERO),
                padding: Edges::uniform(LengthPercentage::ZERO),
                border: Edges::uniform(LengthPercentage::ZERO),
                inset: Edges::uniform(LengthPercentageAuto::Auto),
            }
        }
    }

    impl CoreStyle for DependencyStyle {
        fn inset(&self) -> Edges<LengthPercentageAuto> {
            self.inset
        }

        fn size(&self) -> Size<Dimension> {
            self.size
        }

        fn min_size(&self) -> Size<Dimension> {
            self.min_size
        }

        fn max_size(&self) -> Size<Dimension> {
            self.max_size
        }

        fn margin(&self) -> Edges<LengthPercentageAuto> {
            self.margin
        }

        fn padding(&self) -> Edges<LengthPercentage> {
            self.padding
        }

        fn border(&self) -> Edges<LengthPercentage> {
            self.border
        }
    }

    impl LinearItemStyle for DependencyStyle {}

    #[test]
    fn linear_item_scratch_stays_cache_conscious() {
        let size = core::mem::size_of::<LinearItem>();
        assert!(size <= 192, "LinearItem grew to {size} bytes");
    }

    #[test]
    fn edge_dependency_values_cover_percent_and_calc() {
        let calc = crate::style::CalcHandle::from_raw(1);
        assert!(!length_depends_on_basis(LengthPercentage::Length(1.0)));
        assert!(length_depends_on_basis(LengthPercentage::Percent(0.5)));
        assert!(length_depends_on_basis(LengthPercentage::Calc(calc)));
        assert!(!auto_length_depends_on_basis(LengthPercentageAuto::Length(
            1.0
        )));
        assert!(!auto_length_depends_on_basis(LengthPercentageAuto::Auto));
        assert!(auto_length_depends_on_basis(LengthPercentageAuto::Percent(
            0.5
        )));
        assert!(auto_length_depends_on_basis(LengthPercentageAuto::Calc(
            calc
        )));
    }

    #[test]
    fn linear_dependency_policy_matches_the_two_refresh_phases() {
        let width_only = DependencyStyle {
            size: Size::new(Dimension::Percent(0.5), Dimension::Auto),
            ..DependencyStyle::default()
        };
        let width_dependencies = initial_item_flags(&width_only, None);
        assert!(!width_dependencies.needs_box_refresh());
        assert!(!width_dependencies.needs_relative_offset_refresh());

        for style in [
            DependencyStyle {
                min_size: Size::new(Dimension::Percent(0.5), Dimension::Auto),
                ..DependencyStyle::default()
            },
            DependencyStyle {
                max_size: Size::new(Dimension::Percent(0.5), Dimension::Auto),
                ..DependencyStyle::default()
            },
        ] {
            assert!(!initial_item_flags(&style, None).needs_box_refresh());
        }

        for (style, expected_refresh) in [
            (
                DependencyStyle {
                    margin: Edges {
                        left: LengthPercentageAuto::Percent(0.5),
                        ..Edges::uniform(LengthPercentageAuto::ZERO)
                    },
                    ..DependencyStyle::default()
                },
                LinearItemFlags::MARGIN_REFRESH,
            ),
            (
                DependencyStyle {
                    padding: Edges {
                        left: LengthPercentage::Percent(0.5),
                        ..Edges::uniform(LengthPercentage::ZERO)
                    },
                    ..DependencyStyle::default()
                },
                LinearItemFlags::PADDING_BORDER_REFRESH,
            ),
            (
                DependencyStyle {
                    border: Edges {
                        left: LengthPercentage::Percent(0.5),
                        ..Edges::uniform(LengthPercentage::ZERO)
                    },
                    ..DependencyStyle::default()
                },
                LinearItemFlags::PADDING_BORDER_REFRESH,
            ),
        ] {
            let flags = initial_item_flags(&style, None);
            assert_eq!(flags.0 & LinearItemFlags::BOX_REFRESH, expected_refresh);
            assert!(!initial_item_flags(&style, Some(100.0)).needs_box_refresh());
        }

        for inset in [
            Edges {
                left: LengthPercentageAuto::Percent(0.5),
                ..Edges::uniform(LengthPercentageAuto::Auto)
            },
            Edges {
                top: LengthPercentageAuto::Percent(0.5),
                ..Edges::uniform(LengthPercentageAuto::Auto)
            },
        ] {
            let dependencies = initial_item_flags(
                &DependencyStyle {
                    inset,
                    ..DependencyStyle::default()
                },
                None,
            );
            assert!(!dependencies.needs_box_refresh());
            assert!(dependencies.needs_relative_offset_refresh());
        }
    }
}
