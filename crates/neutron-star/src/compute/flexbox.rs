//! CSS Flexible Box Layout Module Level 1 layout algorithm.
//!
//! The implementation follows the pass ordering in Flexbox §9. Topology and
//! styles are immutable for the layout epoch, while recursive measurement and
//! durable writes go through the [`LayoutNode`] handle into host-owned
//! interior-mutable per-node slots. This lets borrowed style views remain
//! live across child layout without raw style snapshots or self-referential
//! scratch structures.
//!
//! The current protocol deliberately leaves formatting-tree preprocessing
//! (anonymous item generation) to the host and has no representation for
//! replaced-vs-non-replaced automatic minimums or non-horizontal writing
//! modes. `flex-basis: content` defers to the item's `size` (documented
//! vocabulary-swap delta), and the lynx grammar has no `visibility: collapse`
//! so no collapse-strut pass exists. The algorithm is spec-oriented over the
//! representable surface; ordinary items use the non-replaced §4.5
//! automatic-minimum rule.

// Item and line counts are transient Vec lengths. A flex container cannot
// practically approach f32's exact-integer limit, while alignment division
// necessarily operates in the engine's f32 coordinate space.
#![allow(clippy::cast_precision_loss)]

use stylo::computed_values::{box_sizing, direction, flex_direction, flex_wrap};
use stylo::values::computed::{FlexBasis, MaxSize, PositionProperty, Size as StyleSize};
use stylo::values::specified::align::AlignFlags;

use super::compute_absolute_layout;
use super::util::{
    ItemKey, OrderedItem, ResolvedContainerBox, ResolvedItemBox, box_inset_size, clamp_axis,
    preferred_size_definiteness, relative_offset, resolve_container_box, resolve_gap,
    resolve_gap_axis, resolve_item_box, resolve_length_percentage, resolve_style_size,
    sort_and_assign_layout_order, used_aspect_ratio,
};
use crate::geometry::{Edges, Point, Size};
use crate::style::{CoreStyle, FlexContainerStyle, FlexItemStyle};
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
    fn size_ref<T>(self, size: &Size<T>) -> &T {
        match self {
            Self::Horizontal => &size.width,
            Self::Vertical => &size.height,
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

/// Engine-private normalized item self-alignment (the flexbox value space of
/// `align-items`/`align-self` after `AlignFlags` interpretation).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AlignItems {
    Start,
    End,
    FlexStart,
    FlexEnd,
    Center,
    Baseline,
    Stretch,
}

/// Engine-private normalized content distribution (the flexbox value space
/// of `align-content`/`justify-content` after `AlignFlags` interpretation).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AlignContent {
    Start,
    End,
    FlexStart,
    FlexEnd,
    Center,
    Stretch,
    SpaceBetween,
    SpaceEvenly,
    SpaceAround,
}

/// Interprets one `AlignFlags` value as flexbox item self-alignment.
///
/// `None` means `auto`/`normal` (the caller applies its contextual default).
/// The engine interprets the flags it understands; `SAFE`/`UNSAFE` are
/// stripped by [`AlignFlags::value`] (safe fallback ignored, as before the
/// vocabulary swap); last-baseline uses its specified fallback (the end
/// edge); `LEFT`/`RIGHT` are physical and map through the container's inline
/// direction where the aligned axis is horizontal, else to start; unknown
/// fabricated values fall back to start rather than crashing (design
/// amendment F).
fn normalize_item_alignment(
    flags: AlignFlags,
    horizontal_axis: bool,
    rtl: bool,
) -> Option<AlignItems> {
    let value = flags.value();
    if value == AlignFlags::AUTO || value == AlignFlags::NORMAL {
        None
    } else if value == AlignFlags::START {
        Some(AlignItems::Start)
    } else if value == AlignFlags::END {
        Some(AlignItems::End)
    } else if value == AlignFlags::FLEX_START {
        Some(AlignItems::FlexStart)
    } else if value == AlignFlags::FLEX_END {
        Some(AlignItems::FlexEnd)
    } else if value == AlignFlags::CENTER {
        Some(AlignItems::Center)
    } else if value == AlignFlags::BASELINE {
        Some(AlignItems::Baseline)
    } else if value == AlignFlags::LAST_BASELINE {
        // Last-baseline sharing is not implemented; its specified fallback
        // alignment is the end edge (CSS Box Alignment §4.2).
        Some(AlignItems::End)
    } else if value == AlignFlags::STRETCH {
        Some(AlignItems::Stretch)
    } else if value == AlignFlags::LEFT && horizontal_axis {
        Some(if rtl {
            AlignItems::End
        } else {
            AlignItems::Start
        })
    } else if value == AlignFlags::RIGHT && horizontal_axis {
        Some(if rtl {
            AlignItems::Start
        } else {
            AlignItems::End
        })
    } else {
        Some(AlignItems::Start)
    }
}

/// Interprets one `AlignFlags` value as flexbox content distribution.
///
/// `None` means `normal`; the caller applies the property's flexbox default
/// (`stretch` for `align-content`, `flex-start` for `justify-content`). The
/// flag policy matches [`normalize_item_alignment`].
fn normalize_content_alignment(
    flags: AlignFlags,
    horizontal_axis: bool,
    rtl: bool,
) -> Option<AlignContent> {
    let value = flags.value();
    if value == AlignFlags::NORMAL || value == AlignFlags::AUTO {
        None
    } else if value == AlignFlags::START {
        Some(AlignContent::Start)
    } else if value == AlignFlags::END {
        Some(AlignContent::End)
    } else if value == AlignFlags::FLEX_START {
        Some(AlignContent::FlexStart)
    } else if value == AlignFlags::FLEX_END {
        Some(AlignContent::FlexEnd)
    } else if value == AlignFlags::CENTER {
        Some(AlignContent::Center)
    } else if value == AlignFlags::STRETCH {
        Some(AlignContent::Stretch)
    } else if value == AlignFlags::SPACE_BETWEEN {
        Some(AlignContent::SpaceBetween)
    } else if value == AlignFlags::SPACE_AROUND {
        Some(AlignContent::SpaceAround)
    } else if value == AlignFlags::SPACE_EVENLY {
        Some(AlignContent::SpaceEvenly)
    } else if value == AlignFlags::LEFT && horizontal_axis {
        Some(if rtl {
            AlignContent::End
        } else {
            AlignContent::Start
        })
    } else if value == AlignFlags::RIGHT && horizontal_axis {
        Some(if rtl {
            AlignContent::Start
        } else {
            AlignContent::End
        })
    } else {
        Some(AlignContent::Start)
    }
}

#[inline]
const fn direction_is_row(direction: flex_direction::T) -> bool {
    matches!(
        direction,
        flex_direction::T::Row | flex_direction::T::RowReverse
    )
}

#[inline]
const fn direction_is_reverse(direction: flex_direction::T) -> bool {
    matches!(
        direction,
        flex_direction::T::RowReverse | flex_direction::T::ColumnReverse
    )
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
    fn new(
        direction: flex_direction::T,
        wrap: flex_wrap::T,
        inline_direction: direction::T,
    ) -> Self {
        let is_row = direction_is_row(direction);
        let main = if is_row {
            Axis::Horizontal
        } else {
            Axis::Vertical
        };
        let cross = if is_row {
            Axis::Vertical
        } else {
            Axis::Horizontal
        };
        let rtl = inline_direction == direction::T::Rtl;
        let main_base_reverse = is_row && rtl;
        let main_reverse = main_base_reverse ^ direction_is_reverse(direction);
        let cross_base_reverse = !is_row && rtl;
        let cross_reverse = cross_base_reverse ^ (wrap == flex_wrap::T::WrapReverse);
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

/// Transient per-item state accumulated across the Flexbox §9 sizing,
/// flexing, cross-size, and alignment passes. It stores only resolved values
/// and compact hot style fields; raw CSS values are reborrowed from `node`.
#[derive(Debug)]
struct FlexItem<N> {
    key: ItemKey<N>,
    direction: direction::T,
    /// The item's positioning scheme. In-flow schemes lay out identically
    /// except that only `relative` applies the definite-inset visual nudge
    /// (`sticky` is nudged by the host at scroll time, `static` never).
    position: PositionProperty,
    align_self: AlignItems,
    size_is_auto: Size<bool>,
    flex_grow: f32,
    flex_shrink: f32,
    preferred_size: Size<Option<f32>>,
    preferred_size_is_definite: Size<bool>,
    min_size: Size<Option<f32>>,
    max_size: Size<Option<f32>>,
    margin: Edges<f32>,
    margin_auto: Edges<bool>,
    padding: Edges<f32>,
    border: Edges<f32>,
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
    main_size_is_definite: bool,
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

/// Whether a preferred-size value behaves as `auto`. The lynx-parseable
/// keywords Starlight has no sizing behavior for (bare `fit-content`,
/// `stretch`, `-webkit-fill-available`) are treated as `auto` (documented
/// vocabulary-swap delta).
#[inline]
fn style_size_behaves_auto(value: &StyleSize) -> bool {
    match value {
        StyleSize::Auto
        | StyleSize::FitContent
        | StyleSize::Stretch
        | StyleSize::WebkitFillAvailable => true,
        StyleSize::LengthPercentage(_)
        | StyleSize::MinContent
        | StyleSize::MaxContent
        | StyleSize::FitContentFunction(_) => false,
        StyleSize::AnchorSizeFunction(_) | StyleSize::AnchorContainingCalcFunction(_) => {
            unreachable!("anchor sizing is pref-dead under the lynx feature")
        }
    }
}

/// Whether a `flex-basis` value behaves as `auto` (defer to the item's
/// `size`). `content` has no protocol representation of its own and also
/// defers to the size (documented vocabulary-swap delta), as do the
/// treated-as-auto sizing keywords.
#[inline]
fn flex_basis_behaves_auto(value: &FlexBasis) -> bool {
    match value {
        FlexBasis::Content => true,
        FlexBasis::Size(size) => style_size_behaves_auto(size),
    }
}

/// Resolves the quantitative part of `flex-basis` against the main-axis
/// percentage basis.
#[inline]
fn resolve_flex_basis(value: &FlexBasis, basis: Option<f32>) -> Option<f32> {
    match value {
        FlexBasis::Content => None,
        FlexBasis::Size(size) => resolve_style_size(size, basis),
    }
}

fn resolve_item<N>(
    key: ItemKey<N>,
    container_inner_size: Size<Option<f32>>,
    axes: Axes,
    rtl: bool,
    default_alignment: AlignItems,
) -> FlexItem<N>
where
    N: LayoutNode,
    N::Style: FlexContainerStyle + FlexItemStyle,
{
    let style = key.node.style();
    let flex_grow = style.flex_grow().0;
    let flex_shrink = style.flex_shrink().0;
    debug_assert!(
        flex_grow.is_finite() && flex_grow >= 0.0,
        "flex-grow must be finite and non-negative"
    );
    debug_assert!(
        flex_shrink.is_finite() && flex_shrink >= 0.0,
        "flex-shrink must be finite and non-negative"
    );
    let ResolvedItemBox {
        raw_size,
        preferred_size,
        min_size,
        max_size,
        margin,
        margin_auto,
        padding,
        border,
        inset,
        ..
    } = resolve_item_box(key.node, &style, container_inner_size);
    let preferred_size_is_definite =
        preferred_size_definiteness(&style.size(), container_inner_size, style.aspect_ratio());

    FlexItem {
        key,
        direction: style.direction(),
        position: style.position(),
        align_self: normalize_item_alignment(
            FlexItemStyle::align_self(&style).0.value(),
            axes.cross == Axis::Horizontal,
            rtl,
        )
        .unwrap_or(default_alignment),
        size_is_auto: Size::new(
            style_size_behaves_auto(&raw_size.width),
            style_size_behaves_auto(&raw_size.height),
        ),
        flex_grow,
        flex_shrink,
        preferred_size,
        preferred_size_is_definite,
        min_size,
        max_size,
        margin,
        margin_auto,
        padding,
        border,
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
        main_size_is_definite: false,
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
    N::Style: FlexContainerStyle + FlexItemStyle,
{
    let mut input = LayoutInput::compute_size(
        known_dimensions,
        parent_size,
        available_space,
        requested_axis,
    );
    input.definite_dimensions = definite_dimensions;
    input.sizing_mode = sizing_mode;
    node.compute_child_layout(input)
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
fn determine_flex_base_sizes<N>(
    items: &mut [FlexItem<N>],
    axes: Axes,
    container_inner_size: Size<Option<f32>>,
    available_space: Size<AvailableSpace>,
    flex_basis_percentage_basis: Option<f32>,
    container_main_is_definite: bool,
) where
    N: LayoutNode,
    N::Style: FlexContainerStyle + FlexItemStyle,
{
    let container_main = axes.main.size(container_inner_size);
    let available_main = axes.main.size(available_space);

    for item in items {
        let node = item.key.node;
        // Recursive measurement mutates only host-owned per-node slots, so
        // this borrowed style view remains valid across both recursive
        // measurements below.
        let style = node.style();
        let inset_size = box_inset_size(item.padding, item.border);
        let main_floor = axes.main.size(inset_size);
        let cross_preferred = axes.cross.size(item.preferred_size);
        let mut known = Size::NONE;
        axes.cross.set_size(&mut known, cross_preferred);
        let mut known_is_definite = Size::new(false, false);
        axes.cross.set_size(
            &mut known_is_definite,
            axes.cross.size(item.preferred_size_is_definite),
        );

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
                node,
                known,
                known_is_definite,
                contribution_parent_size,
                min_available,
                SizingMode::ContentSize,
                axes.main.requested(),
            )
            .size,
        );
        let max_content = axes.main.size(
            child_measurement(
                node,
                known,
                known_is_definite,
                contribution_parent_size,
                max_available,
                SizingMode::ContentSize,
                axes.main.requested(),
            )
            .size,
        );
        let available_content = if matches!(available_main, AvailableSpace::Definite(_)) {
            axes.main.size(
                child_measurement(
                    node,
                    known,
                    known_is_definite,
                    contribution_parent_size,
                    available_space,
                    SizingMode::ContentSize,
                    axes.main.requested(),
                )
                .size,
            )
        } else {
            max_content
        };

        let raw_size = style.size();
        let raw_min_size = style.min_size();
        let raw_max_size = style.max_size();
        let resolve_intrinsic_size = |value: &StyleSize| -> Option<f32> {
            match value {
                StyleSize::MinContent => Some(min_content),
                StyleSize::MaxContent => Some(max_content),
                StyleSize::FitContentFunction(limit) => {
                    let limit =
                        resolve_length_percentage(&limit.0, container_main).unwrap_or(max_content);
                    Some(max_content.min(limit.max(min_content)))
                }
                _ => None,
            }
        };
        let resolve_intrinsic_max = |value: &MaxSize| -> Option<f32> {
            match value {
                MaxSize::MinContent => Some(min_content),
                MaxSize::MaxContent => Some(max_content),
                MaxSize::FitContentFunction(limit) => {
                    let limit =
                        resolve_length_percentage(&limit.0, container_main).unwrap_or(max_content);
                    Some(max_content.min(limit.max(min_content)))
                }
                _ => None,
            }
        };
        if axes.main.size(item.preferred_size).is_none()
            && let Some(value) = resolve_intrinsic_size(axes.main.size_ref(&raw_size))
        {
            axes.main.set_size(&mut item.preferred_size, Some(value));
        }
        if axes.main.size(item.min_size).is_none()
            && let Some(value) = resolve_intrinsic_size(axes.main.size_ref(&raw_min_size))
        {
            axes.main.set_size(&mut item.min_size, Some(value));
        }
        if axes.main.size(item.max_size).is_none()
            && let Some(value) = resolve_intrinsic_max(axes.main.size_ref(&raw_max_size))
        {
            axes.main.set_size(&mut item.max_size, Some(value));
        }

        let raw_flex_basis = style.flex_basis();
        let flex_basis_is_auto = flex_basis_behaves_auto(&raw_flex_basis);
        let resolved_basis =
            resolve_flex_basis(&raw_flex_basis, flex_basis_percentage_basis).map(|basis| {
                if style.box_sizing() == box_sizing::T::ContentBox {
                    basis + main_floor
                } else {
                    basis
                }
            });

        let preferred_main = axes.main.size(item.preferred_size);
        let preferred_flex_basis = if flex_basis_is_auto {
            preferred_main
        } else {
            None
        };
        let definite_basis = resolved_basis.or(preferred_flex_basis);
        item.main_size_is_definite = container_main_is_definite || definite_basis.is_some();
        item.flex_basis = if let Some(basis) = definite_basis {
            basis
        } else {
            // `flex-basis: auto`/`content` defer to the preferred size; a
            // non-auto basis whose quantitative resolution failed keeps its
            // own value form.
            let content_basis: &StyleSize = if flex_basis_is_auto {
                axes.main.size_ref(&raw_size)
            } else {
                match &raw_flex_basis {
                    FlexBasis::Size(size) => size,
                    FlexBasis::Content => {
                        unreachable!("flex-basis: content behaves as auto and was handled above")
                    }
                }
            };
            match content_basis {
                StyleSize::MinContent => min_content,
                StyleSize::MaxContent => max_content,
                StyleSize::FitContentFunction(limit) => {
                    let limit = resolve_length_percentage(&limit.0, flex_basis_percentage_basis)
                        .unwrap_or(max_content);
                    max_content.min(limit.max(min_content))
                }
                StyleSize::LengthPercentage(lp) if lp.0.to_percentage().is_none() => {
                    // A calc() carrying a percentage without a definite basis
                    // sizes to max-content, exactly like the old symbolic
                    // `Calc` arm. (A plain percentage takes the arm below; a
                    // plain length always resolved above.)
                    max_content
                }
                StyleSize::Auto
                | StyleSize::LengthPercentage(_)
                | StyleSize::FitContent
                | StyleSize::Stretch
                | StyleSize::WebkitFillAvailable => {
                    if available_main == AvailableSpace::MinContent {
                        min_content
                    } else {
                        available_content
                    }
                }
                StyleSize::AnchorSizeFunction(_) | StyleSize::AnchorContainingCalcFunction(_) => {
                    unreachable!("anchor sizing is pref-dead under the lynx feature")
                }
            }
        };

        // Flexbox §9.2 deliberately allows a negative inner flex base.
        item.inner_flex_basis = item.flex_basis - main_floor;

        let explicit_min = axes.main.size(item.min_size);
        item.resolved_min_main = if let Some(minimum) = explicit_min {
            minimum.max(main_floor)
        } else if style.overflow().x.is_scrollable() || style.overflow().y.is_scrollable() {
            main_floor
        } else {
            let main_is_auto = style_size_behaves_auto(axes.main.size_ref(&raw_size));
            let cross_is_auto = style_size_behaves_auto(axes.cross.size_ref(&raw_size));
            let specified_suggestion = (!main_is_auto).then_some(preferred_main).flatten();
            let transferred_suggestion = (used_aspect_ratio(style.aspect_ratio()).is_some()
                && !cross_is_auto)
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
            let definite_basis = (!flex_basis_is_auto).then_some(item.flex_basis);
            let mut value = content
                .max(preferred_contribution)
                .max(definite_basis.unwrap_or(0.0));
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
fn item_outer_hypothetical_main<N>(item: &FlexItem<N>, axes: Axes) -> f32 {
    item.hypothetical_main + axis_sum(item.margin, axes.main)
}

#[inline]
fn item_outer_target_main<N>(item: &FlexItem<N>, axes: Axes) -> f32 {
    item.target_main + axis_sum(item.margin, axes.main)
}

fn collect_flex_lines<N>(
    items: &[FlexItem<N>],
    wrap: flex_wrap::T,
    available_main: AvailableSpace,
    gap: f32,
    axes: Axes,
) -> Vec<FlexLine> {
    if wrap == flex_wrap::T::Nowrap || available_main == AvailableSpace::MaxContent {
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
            .map(|start| FlexLine {
                start,
                end: start + 1,
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
        let mut prior_participants = 0;
        while end < items.len() {
            let item_size = item_outer_hypothetical_main(&items[end], axes);
            let candidate_gap = if prior_participants == 0 { 0.0 } else { gap };
            let candidate = occupied + candidate_gap + item_size;
            // The first item always establishes a line. A zero-sized item at
            // an exact boundary remains on the preceding line (§9.3 note).
            if prior_participants > 0
                && candidate > limit
                && !(item_size == 0.0 && candidate_gap == 0.0)
            {
                break;
            }
            occupied = candidate;
            prior_participants += 1;
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

fn line_intrinsic_main<N>(items: &[FlexItem<N>], line: FlexLine, gap: f32, axes: Axes) -> f32 {
    let line_items = &items[line.start..line.end];
    let item_sum = line_items
        .iter()
        .map(|item| item.flex_basis.max(item.resolved_min_main) + axis_sum(item.margin, axes.main))
        .sum::<f32>();
    item_sum + gap * line_items.len().saturating_sub(1) as f32
}

fn line_content_contribution<N>(
    items: &[FlexItem<N>],
    line: FlexLine,
    gap: f32,
    maximum: bool,
) -> f32 {
    let line_items = &items[line.start..line.end];
    let item_sum = line_items
        .iter()
        .map(|item| {
            if maximum {
                item.max_content_contribution
            } else {
                item.min_content_contribution
            }
        })
        .sum::<f32>();
    item_sum + gap * line_items.len().saturating_sub(1) as f32
}

#[allow(clippy::too_many_arguments)]
fn determine_auto_main_size<N>(
    items: &[FlexItem<N>],
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
    clamp_axis(content + inset_main, min_outer, max_outer, inset_main)
}

#[allow(clippy::too_many_lines)]
fn resolve_flexible_lengths<N>(
    items: &mut [FlexItem<N>],
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
            let floor = axes.main.size(box_inset_size(item.padding, item.border));
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

fn determine_hypothetical_cross_sizes<N>(
    items: &mut [FlexItem<N>],
    lines: &[FlexLine],
    axes: Axes,
    container_inner_size: Size<Option<f32>>,
    available_space: Size<AvailableSpace>,
) where
    N: LayoutNode,
    N::Style: FlexContainerStyle + FlexItemStyle,
{
    for line in lines {
        for item in &mut items[line.start..line.end] {
            let mut known = Size::NONE;
            axes.main.set_size(&mut known, Some(item.target_main));
            axes.cross
                .set_size(&mut known, axes.cross.size(item.preferred_size));
            let mut known_is_definite = item.preferred_size_is_definite;
            axes.main
                .set_size(&mut known_is_definite, item.main_size_is_definite);
            let child_available = size_from_axes(
                axes,
                AvailableSpace::Definite(item.target_main),
                axes.cross.size(available_space),
            );
            let output = child_measurement(
                item.key.node,
                known,
                known_is_definite,
                container_inner_size,
                child_available,
                SizingMode::InherentSize,
                RequestedAxis::Both,
            );
            let inset_size = box_inset_size(item.padding, item.border);
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

fn calculate_line_cross_sizes<N>(
    items: &[FlexItem<N>],
    lines: &mut [FlexLine],
    axes: Axes,
    wrap: flex_wrap::T,
    known_inner_cross: Option<f32>,
) {
    if wrap == flex_wrap::T::Nowrap
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
    wrap: flex_wrap::T,
    align_content: AlignContent,
    inner_cross: f32,
    cross_gap: f32,
) {
    if wrap == flex_wrap::T::Nowrap || align_content != AlignContent::Stretch || lines.is_empty() {
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

fn determine_used_cross_sizes<N>(items: &mut [FlexItem<N>], lines: &[FlexLine], axes: Axes) {
    for line in lines {
        for item in &mut items[line.start..line.end] {
            let inset_size = box_inset_size(item.padding, item.border);
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

fn distribute_main_axis<N>(
    items: &mut [FlexItem<N>],
    lines: &[FlexLine],
    axes: Axes,
    inner_main: f32,
    main_gap: f32,
    justify_content: AlignContent,
) {
    for line in lines {
        let line_items = &mut items[line.start..line.end];
        let participant_count = line_items.len();
        let fixed_gap = main_gap * participant_count.saturating_sub(1) as f32;
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
                participant_count,
                axes.main_reverse,
                axes.main_base_reverse,
            )
        };

        let mut cursor = leading;
        let mut participant_index = 0;
        for item in line_items.iter_mut() {
            cursor += flow_start(item.margin, axes.main, axes.main_reverse);
            item.main_position = cursor;
            cursor += item.target_main + flow_end(item.margin, axes.main, axes.main_reverse);
            participant_index += 1;
            if participant_index < participant_count {
                cursor += main_gap + distributed_gap;
            }
        }
    }
}

fn align_lines(
    lines: &mut [FlexLine],
    axes: Axes,
    wrap: flex_wrap::T,
    align_content: AlignContent,
    inner_cross: f32,
    cross_gap: f32,
) {
    let used = lines.iter().map(|line| line.cross_size).sum::<f32>()
        + cross_gap * lines.len().saturating_sub(1) as f32;
    let free_space = inner_cross - used;
    let effective_alignment = if wrap == flex_wrap::T::Nowrap {
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

fn align_items_cross_axis<N>(items: &mut [FlexItem<N>], lines: &[FlexLine], axes: Axes) {
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

fn item_border_box_location<N>(
    item: &FlexItem<N>,
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

fn first_container_baseline<N>(
    items: &[FlexItem<N>],
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
fn perform_in_flow_layout<N>(
    items: &mut [FlexItem<N>],
    lines: &[FlexLine],
    axes: Axes,
    inner_size: Size<f32>,
    content_origin: Point<f32>,
    container_size: Size<f32>,
) -> (Size<f32>, Option<f32>)
where
    N: LayoutNode,
    N::Style: FlexContainerStyle + FlexItemStyle,
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
            axes.main
                .set_size(&mut input.definite_dimensions, item.main_size_is_definite);
            // The parent has already applied the flex item's own sizing,
            // min/max and aspect-ratio rules to both target axes.
            input.sizing_mode = SizingMode::ContentSize;
            let output = item.key.node.compute_child_layout(input);

            // Only `relative` nudges at layout time; `static` has no offsets
            // and `sticky` is a host scroll-time post-pass.
            let offset = if item.position == PositionProperty::Relative {
                relative_offset(item.inset, item.direction)
            } else {
                Point::ZERO
            };
            let mut location =
                item_border_box_location(item, *line, axes, inner_size, content_origin);
            location.x += offset.x;
            location.y += offset.y;

            let mut layout = Layout::with_order(item.key.layout_order);
            layout.location = location;
            layout.size = output.size;
            layout.content_size = output.content_size;
            layout.border = item.border;
            layout.padding = item.padding;
            layout.margin = item.margin;
            item.key.node.set_unrounded_layout(&layout);

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

fn static_position_for_absolute<N>(
    item: &FlexItem<N>,
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
fn perform_absolute_children<N>(
    absolute_items: &[OrderedItem<N>],
    axes: Axes,
    rtl: bool,
    inner_size: Size<f32>,
    container_size: Size<f32>,
    padding: Edges<f32>,
    border: Edges<f32>,
    justify_content: AlignContent,
    default_alignment: AlignItems,
) -> Size<f32>
where
    N: LayoutNode,
    N::Style: FlexContainerStyle + FlexItemStyle,
{
    let content_origin = Point::new(border.left + padding.left, border.top + padding.top);
    let parent_size = inner_size.map(Some);
    let mut content_size = container_size;
    let padding_box_size = Size::new(
        (container_size.width - border.horizontal_sum()).max(0.0),
        (container_size.height - border.vertical_sum()).max(0.0),
    );

    for pending in absolute_items {
        let key = pending.key();
        // The borrowed view is safe across recursive child layout; only
        // host-owned layout/cache slots mutate while topology and styles
        // stay immutable for the flush.
        let style = key.node.style();
        let mut item = resolve_item(key, parent_size, axes, rtl, default_alignment);
        let mut known = item.preferred_size;
        let available = inner_size.map(AvailableSpace::Definite);
        let output = child_measurement(
            key.node,
            known,
            item.preferred_size_is_definite,
            parent_size,
            available,
            SizingMode::InherentSize,
            RequestedAxis::Both,
        );
        let inset_size = box_inset_size(item.padding, item.border);
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
            PositionProperty::Absolute => {
                let static_in_padding_space = Point::new(
                    static_position.x - border.left,
                    static_position.y - border.top,
                );
                let mut layout =
                    compute_absolute_layout(key.node, padding_box_size, static_in_padding_space);
                layout.order = key.layout_order;
                layout.location.x += border.left;
                layout.location.y += border.top;
                content_size.width = content_size
                    .width
                    .max(layout.location.x + layout.size.width.max(layout.content_size.width));
                content_size.height = content_size
                    .height
                    .max(layout.location.y + layout.size.height.max(layout.content_size.height));
                key.node.set_unrounded_layout(&layout);
            }
            // The containing block is not the layout parent (CSS `fixed`):
            // record the static position; the host completes layout in its
            // positioned pass.
            PositionProperty::Fixed => {
                key.node.set_static_position(static_position);
            }
            PositionProperty::Static | PositionProperty::Relative | PositionProperty::Sticky => {}
        }
    }
    content_size
}

/// Computes one flex container according to CSS Flexible Box Layout §9.
///
/// Style and child topology are read through the node handle and stay
/// immutable for the flush; recursive layout and durable geometry writes go
/// through the handle into host-owned per-node slots. Child layouts are
/// stored only for [`LayoutGoal::Commit`].
#[allow(clippy::too_many_lines)]
pub fn compute_flexbox_layout<N>(node: N, input: LayoutInput) -> LayoutOutput
where
    N: LayoutNode,
    N::Style: FlexContainerStyle + FlexItemStyle,
{
    // This borrowed style view remains live for the whole algorithm;
    // recursive calls mutate only host-owned per-node layout slots.
    let style = node.style();
    let flex_wrap = style.flex_wrap();
    let axes = Axes::new(style.flex_direction(), flex_wrap, style.direction());
    let rtl = style.direction() == direction::T::Rtl;
    let main_horizontal = axes.main == Axis::Horizontal;
    let cross_horizontal = axes.cross == Axis::Horizontal;
    let align_content = normalize_content_alignment(
        style.align_content().primary().value(),
        cross_horizontal,
        rtl,
    )
    .unwrap_or(AlignContent::Stretch);
    let align_items = normalize_item_alignment(
        FlexContainerStyle::align_items(&style).0.value(),
        cross_horizontal,
        rtl,
    )
    .unwrap_or(AlignItems::Stretch);
    let justify_content = normalize_content_alignment(
        FlexContainerStyle::justify_content(&style)
            .primary()
            .value(),
        main_horizontal,
        rtl,
    )
    .unwrap_or(AlignContent::FlexStart);
    let style_definite = if input.sizing_mode == SizingMode::ContentSize {
        Size::new(false, false)
    } else {
        preferred_size_definiteness(&style.size(), input.parent_size, style.aspect_ratio())
    };
    let outer_definite = Size::new(
        input.definite_dimensions.width || style_definite.width,
        input.definite_dimensions.height || style_definite.height,
    );
    let ResolvedContainerBox {
        padding,
        border,
        box_inset: container_inset_size,
        min: min_size,
        max: max_size,
        outer: mut outer_size,
        inner: mut inner_size,
        available_inner: inner_available_space,
        ..
    } = resolve_container_box(node, &style, input);
    let item_inline_basis_was_indefinite = !outer_definite.width;
    let main_percentage_basis_was_indefinite = !axes.main.size(outer_definite);
    let gap_value = style.gap();
    let mut gap = resolve_gap(&gap_value, inner_size);
    let mut generated = Vec::new();
    let mut absolute_items = Vec::new();
    let mut hidden = Vec::new();
    for (document_index, child) in node.children().enumerate() {
        let child_style = child.style();
        if child_style.display().is_none() {
            hidden.push((document_index, child));
            continue;
        }
        let pending = OrderedItem {
            node: child,
            document_index,
            css_order: FlexItemStyle::order(&child_style),
            layout_order: u32::try_from(document_index).unwrap_or(u32::MAX),
        };
        if matches!(
            child_style.position(),
            PositionProperty::Absolute | PositionProperty::Fixed
        ) {
            absolute_items.push(pending);
        } else {
            generated.push(pending);
        }
    }
    sort_and_assign_layout_order(&mut generated, &mut absolute_items);

    let mut items = generated
        .into_iter()
        .map(|item| {
            let mut percentage_basis = inner_size;
            if !outer_definite.width {
                percentage_basis.width = None;
            }
            if !outer_definite.height {
                percentage_basis.height = None;
            }
            resolve_item(item.key(), percentage_basis, axes, rtl, align_items)
        })
        .collect::<Vec<_>>();
    determine_flex_base_sizes(
        &mut items,
        axes,
        inner_size,
        inner_available_space,
        (!main_percentage_basis_was_indefinite)
            .then(|| axes.main.size(inner_size))
            .flatten(),
        !main_percentage_basis_was_indefinite,
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
        let resolved_main_gap =
            resolve_gap_axis(axes.main.size_ref(&gap_value), axes.main.size(inner_size));
        axes.main.set_size(&mut gap, resolved_main_gap);
    }
    let inner_main = axes.main.size(inner_size).unwrap_or(0.0);
    let mut main_gap = axes.main.size(gap);
    for line in lines.iter().copied() {
        resolve_flexible_lengths(&mut items, line, inner_main, main_gap, axes);
    }

    determine_hypothetical_cross_sizes(&mut items, &lines, axes, inner_size, inner_available_space);
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
        let resolved_cross_gap =
            resolve_gap_axis(axes.cross.size_ref(&gap_value), axes.cross.size(inner_size));
        axes.cross.set_size(&mut gap, resolved_cross_gap);
    }
    let inner_cross = axes.cross.size(inner_size).unwrap_or(0.0);
    if item_inline_basis_was_indefinite {
        // Cyclic percentages contribute zero to intrinsic sizing, but their
        // used values resolve against the resulting content-box width. Run
        // the item/line phases once more with that now-definite basis while
        // keeping the intrinsic container size fixed.
        gap = resolve_gap(&gap_value, inner_size);
        main_gap = axes.main.size(gap);
        // Re-resolve compact scratch in place. Raw style is refetched through
        // the node handle; no second full-style snapshot or parallel style
        // Vec is needed.
        for item in &mut items {
            let key = item.key;
            *item = resolve_item(key, inner_size, axes, rtl, align_items);
        }
        let final_available_space = Size::new(
            AvailableSpace::Definite(inner_size.width.unwrap_or(0.0)),
            AvailableSpace::Definite(inner_size.height.unwrap_or(0.0)),
        );
        determine_flex_base_sizes(
            &mut items,
            axes,
            inner_size,
            final_available_space,
            if main_percentage_basis_was_indefinite {
                None
            } else {
                axes.main.size(inner_size)
            },
            !main_percentage_basis_was_indefinite,
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
            &mut items,
            &lines,
            axes,
            inner_size,
            final_available_space,
        );
        calculate_line_cross_sizes(&items, &mut lines, axes, flex_wrap, Some(inner_cross));
    }
    let cross_gap = axes.cross.size(gap);
    if flex_wrap == flex_wrap::T::Nowrap
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
        &mut items,
        &lines,
        axes,
        inner_size,
        content_origin,
        outer_size,
    );
    for (document_index, child) in hidden {
        super::hide_subtree(child);
        child.set_unrounded_layout(&Layout::with_order(
            u32::try_from(document_index).unwrap_or(u32::MAX),
        ));
    }
    let absolute_content_size = perform_absolute_children(
        &absolute_items,
        axes,
        rtl,
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

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::float_cmp)]
mod tests {
    use stylo::values::computed::Display;

    use super::*;

    #[derive(Debug)]
    struct TestStyle;

    impl CoreStyle for TestStyle {
        fn display(&self) -> Display {
            Display::Flex
        }
    }
    impl FlexContainerStyle for TestStyle {}
    impl FlexItemStyle for TestStyle {}

    /// Minimal zero-sized handle for the line-math scratch: these tests
    /// fabricate `FlexItem` records directly, so the handle only has to
    /// satisfy the `LayoutNode` bounds — no accessor is ever reached.
    #[derive(Debug, Clone, Copy)]
    struct TestRef;

    impl LayoutNode for TestRef {
        type Style = &'static TestStyle;
        type ChildIter = core::iter::Empty<Self>;

        fn children(self) -> Self::ChildIter {
            core::iter::empty()
        }

        fn style(self) -> &'static TestStyle {
            &TestStyle
        }

        fn compute_child_layout(self, _input: LayoutInput) -> LayoutOutput {
            unreachable!("line-math tests never recurse into children")
        }

        fn set_unrounded_layout(self, _layout: &Layout) {
            unreachable!("line-math tests never store layouts")
        }

        fn unrounded_layout(self) -> Layout {
            unreachable!("line-math tests never read layouts")
        }

        fn set_final_layout(self, _layout: &Layout) {
            unreachable!("line-math tests never round layouts")
        }

        fn set_static_position(self, _static_position: Point<f32>) {
            unreachable!("line-math tests never hoist items")
        }

        // Caching deliberately disabled.
        fn cache_get(self, _input: LayoutInput) -> Option<LayoutOutput> {
            None
        }

        fn cache_store(self, _input: LayoutInput, _output: LayoutOutput) {}

        fn cache_clear(self) {}
    }

    fn item(main: f32, cross: f32) -> FlexItem<TestRef> {
        FlexItem {
            key: ItemKey {
                node: TestRef,
                layout_order: 0,
            },
            direction: direction::T::Ltr,
            position: PositionProperty::Relative,
            align_self: AlignItems::Stretch,
            size_is_auto: Size::new(true, true),
            flex_grow: 0.0,
            flex_shrink: 1.0,
            preferred_size: Size::NONE,
            preferred_size_is_definite: Size::new(false, false),
            min_size: Size::NONE,
            max_size: Size::NONE,
            margin: Edges::ZERO,
            margin_auto: Edges::uniform(false),
            padding: Edges::ZERO,
            border: Edges::ZERO,
            inset: Edges::uniform(None),
            flex_basis: main,
            inner_flex_basis: main,
            min_content_contribution: main,
            max_content_contribution: main,
            resolved_min_main: 0.0,
            hypothetical_main: main,
            target_main: main,
            hypothetical_cross: cross,
            target_cross: cross,
            baseline: cross,
            measured_baselines: Point::NONE,
            frozen: false,
            violation: 0.0,
            main_position: 0.0,
            cross_position: 0.0,
            main_size_is_definite: false,
        }
    }

    #[test]
    fn alignment_normalization_covers_flags_defaults_and_physical_keywords() {
        assert_eq!(
            normalize_item_alignment(AlignFlags::AUTO, true, false),
            None
        );
        assert_eq!(
            normalize_item_alignment(AlignFlags::NORMAL, true, false),
            None
        );
        assert_eq!(
            normalize_item_alignment(AlignFlags::SAFE | AlignFlags::CENTER, true, false),
            Some(AlignItems::Center)
        );
        assert_eq!(
            normalize_item_alignment(AlignFlags::LAST_BASELINE, true, false),
            Some(AlignItems::End)
        );
        assert_eq!(
            normalize_item_alignment(AlignFlags::LEFT, true, true),
            Some(AlignItems::End)
        );
        assert_eq!(
            normalize_item_alignment(AlignFlags::LEFT, false, true),
            Some(AlignItems::Start)
        );
        assert_eq!(
            normalize_content_alignment(AlignFlags::NORMAL, true, false),
            None
        );
        assert_eq!(
            normalize_content_alignment(AlignFlags::SPACE_BETWEEN, true, false),
            Some(AlignContent::SpaceBetween)
        );
        assert_eq!(
            normalize_content_alignment(AlignFlags::RIGHT, true, false),
            Some(AlignContent::End)
        );
        assert_eq!(
            normalize_content_alignment(AlignFlags::RIGHT, false, false),
            Some(AlignContent::Start)
        );
        // Fabricated/unknown flag values fall back to start (amendment F).
        assert_eq!(
            normalize_item_alignment(AlignFlags::SELF_START, true, false),
            Some(AlignItems::Start)
        );
    }

    #[test]
    fn physical_edge_and_alignment_helpers_cover_reverse_and_overflow_rules() {
        let mut edges = Edges::ZERO;
        set_flow_start(&mut edges, Axis::Horizontal, true, 1.0);
        set_flow_start(&mut edges, Axis::Vertical, true, 2.0);
        set_flow_end(&mut edges, Axis::Horizontal, true, 3.0);
        set_flow_end(&mut edges, Axis::Vertical, true, 4.0);
        assert_eq!(
            edges,
            Edges {
                left: 3.0,
                right: 1.0,
                top: 4.0,
                bottom: 2.0
            }
        );

        assert_eq!(
            alignment_distribution(AlignContent::Start, 12.0, 2, false, false),
            (0.0, 0.0)
        );
        assert_eq!(
            alignment_distribution(AlignContent::Start, 12.0, 2, true, false),
            (12.0, 0.0)
        );
        assert_eq!(
            alignment_distribution(AlignContent::End, 12.0, 2, false, false),
            (12.0, 0.0)
        );
        assert_eq!(
            alignment_distribution(AlignContent::End, 12.0, 2, true, false),
            (0.0, 0.0)
        );
        assert_eq!(
            alignment_distribution(AlignContent::FlexEnd, 12.0, 2, false, false),
            (12.0, 0.0)
        );
        assert_eq!(
            alignment_distribution(AlignContent::Center, 12.0, 2, false, false),
            (6.0, 0.0)
        );
        assert_eq!(
            alignment_distribution(AlignContent::SpaceBetween, 12.0, 3, false, false),
            (0.0, 6.0)
        );
        assert_eq!(
            alignment_distribution(AlignContent::SpaceAround, 12.0, 3, false, false),
            (2.0, 4.0)
        );
        assert_eq!(
            alignment_distribution(AlignContent::SpaceEvenly, 12.0, 3, false, false),
            (3.0, 3.0)
        );
        assert_eq!(
            alignment_distribution(AlignContent::SpaceAround, -12.0, 3, false, false),
            (-6.0, 0.0)
        );
        assert_eq!(
            alignment_distribution(AlignContent::SpaceBetween, -12.0, 3, false, false),
            (0.0, 0.0)
        );
        assert_eq!(
            alignment_distribution(AlignContent::Center, 12.0, 0, false, false),
            (0.0, 0.0)
        );

        let opposing = Edges {
            left: Some(7.0),
            right: Some(11.0),
            top: None,
            bottom: Some(5.0),
        };
        assert_eq!(
            relative_offset(opposing, direction::T::Ltr),
            Point::new(7.0, -5.0)
        );
        assert_eq!(
            relative_offset(opposing, direction::T::Rtl),
            Point::new(-11.0, -5.0)
        );
    }

    #[test]
    fn intrinsic_line_helpers_cover_empty_min_max_and_definite_constraints() {
        let axes = Axes::new(
            flex_direction::T::Row,
            flex_wrap::T::Wrap,
            direction::T::Ltr,
        );
        assert!(
            collect_flex_lines::<TestRef>(
                &[],
                flex_wrap::T::Wrap,
                AvailableSpace::Definite(10.0),
                0.0,
                axes
            )
            .is_empty()
        );

        let mut items = vec![item(10.0, 5.0), item(20.0, 7.0)];
        items[0].min_content_contribution = 8.0;
        items[1].min_content_contribution = 15.0;
        items[0].max_content_contribution = 12.0;
        items[1].max_content_contribution = 24.0;
        let lines = collect_flex_lines(
            &items,
            flex_wrap::T::Wrap,
            AvailableSpace::MinContent,
            2.0,
            axes,
        );
        assert_eq!(lines.len(), 2);
        assert_eq!(
            line_intrinsic_main(
                &items,
                FlexLine {
                    start: 0,
                    end: 2,
                    cross_size: 0.0,
                    cross_position: 0.0
                },
                2.0,
                axes
            ),
            32.0
        );
        assert_eq!(
            determine_auto_main_size(
                &items,
                &lines,
                2.0,
                axes,
                AvailableSpace::MinContent,
                1.0,
                None,
                None
            ),
            16.0
        );
        assert_eq!(
            determine_auto_main_size(
                &items,
                &lines,
                2.0,
                axes,
                AvailableSpace::MaxContent,
                1.0,
                None,
                None
            ),
            25.0
        );
        assert_eq!(
            determine_auto_main_size(
                &items,
                &lines,
                2.0,
                axes,
                AvailableSpace::Definite(100.0),
                1.0,
                None,
                None
            ),
            21.0
        );
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn baseline_and_cross_alignment_cover_auto_margin_overflow_and_reversal() {
        let normal = Axes::new(
            flex_direction::T::Row,
            flex_wrap::T::Wrap,
            direction::T::Ltr,
        );
        let reversed = Axes::new(
            flex_direction::T::Row,
            flex_wrap::T::WrapReverse,
            direction::T::Ltr,
        );
        let mut baseline_items = vec![item(10.0, 20.0), item(10.0, 15.0)];
        baseline_items[0].align_self = AlignItems::Baseline;
        baseline_items[0].baseline = 12.0;
        baseline_items[0].margin = Edges {
            left: 0.0,
            right: 0.0,
            top: 2.0,
            bottom: 1.0,
        };
        baseline_items[1].align_self = AlignItems::Baseline;
        baseline_items[1].baseline = 5.0;
        baseline_items[1].margin = Edges {
            left: 0.0,
            right: 0.0,
            top: 1.0,
            bottom: 3.0,
        };
        let mut lines = [FlexLine {
            start: 0,
            end: 2,
            cross_size: 0.0,
            cross_position: 0.0,
        }];
        calculate_line_cross_sizes(
            &baseline_items,
            &mut lines,
            normal,
            flex_wrap::T::Wrap,
            None,
        );
        assert_eq!(lines[0].cross_size, 27.0);

        let mut positive_auto = item(10.0, 10.0);
        positive_auto.margin_auto.bottom = true;
        let mut positive = [positive_auto];
        align_items_cross_axis(
            &mut positive,
            &[FlexLine {
                start: 0,
                end: 1,
                cross_size: 20.0,
                cross_position: 0.0,
            }],
            normal,
        );
        assert_eq!(positive[0].margin.bottom, 10.0);

        let mut overflowing_auto = item(10.0, 30.0);
        overflowing_auto.margin_auto.bottom = true;
        let mut overflowing = [overflowing_auto];
        align_items_cross_axis(
            &mut overflowing,
            &[FlexLine {
                start: 0,
                end: 1,
                cross_size: 20.0,
                cross_position: 0.0,
            }],
            reversed,
        );
        assert_eq!(overflowing[0].margin.bottom, -10.0);

        let mut baseline = item(10.0, 20.0);
        baseline.align_self = AlignItems::Baseline;
        baseline.baseline = 8.0;
        let mut baseline = [baseline];
        align_items_cross_axis(
            &mut baseline,
            &[FlexLine {
                start: 0,
                end: 1,
                cross_size: 40.0,
                cross_position: 0.0,
            }],
            reversed,
        );
        assert_eq!(baseline[0].cross_position, 20.0);

        let mut end = item(10.0, 20.0);
        end.align_self = AlignItems::End;
        let mut normal_end = [end];
        align_items_cross_axis(
            &mut normal_end,
            &[FlexLine {
                start: 0,
                end: 1,
                cross_size: 40.0,
                cross_position: 0.0,
            }],
            normal,
        );
        assert_eq!(normal_end[0].cross_position, 20.0);
        let mut end = item(10.0, 20.0);
        end.align_self = AlignItems::End;
        let mut reversed_end = [end];
        align_items_cross_axis(
            &mut reversed_end,
            &[FlexLine {
                start: 0,
                end: 1,
                cross_size: 40.0,
                cross_position: 0.0,
            }],
            reversed,
        );
        assert_eq!(reversed_end[0].cross_position, 0.0);
    }

    #[test]
    fn absolute_static_cross_alignment_uses_logical_start_and_end() {
        let normal = Axes::new(
            flex_direction::T::Row,
            flex_wrap::T::Wrap,
            direction::T::Ltr,
        );
        let reversed = Axes::new(
            flex_direction::T::Row,
            flex_wrap::T::WrapReverse,
            direction::T::Ltr,
        );
        let inner = Size::new(100.0, 50.0);
        let origin = Point::new(5.0, 7.0);
        let mut candidate = item(20.0, 10.0);

        candidate.align_self = AlignItems::Start;
        assert_eq!(
            static_position_for_absolute(
                &candidate,
                normal,
                inner,
                origin,
                AlignContent::FlexStart
            ),
            origin
        );
        assert_eq!(
            static_position_for_absolute(
                &candidate,
                reversed,
                inner,
                origin,
                AlignContent::FlexStart
            ),
            origin
        );
        candidate.align_self = AlignItems::End;
        assert_eq!(
            static_position_for_absolute(
                &candidate,
                normal,
                inner,
                origin,
                AlignContent::FlexStart
            ),
            Point::new(5.0, 47.0)
        );
        assert_eq!(
            static_position_for_absolute(
                &candidate,
                reversed,
                inner,
                origin,
                AlignContent::FlexStart
            ),
            Point::new(5.0, 47.0)
        );
        candidate.align_self = AlignItems::FlexEnd;
        assert_eq!(
            static_position_for_absolute(
                &candidate,
                normal,
                inner,
                origin,
                AlignContent::FlexStart
            ),
            Point::new(5.0, 47.0)
        );
    }
}
