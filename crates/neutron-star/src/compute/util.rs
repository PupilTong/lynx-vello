//! Small, dependency-free value-resolution helpers shared by layout entry
//! points.

use crate::geometry::{Edges, Point, Size};
use crate::style::value::{CalcHandle, Dimension, LengthPercentage, LengthPercentageAuto};
use crate::style::{BoxSizing, CoreStyle, Overflow};
use crate::tree::{AvailableSpace, LayoutInput, LayoutSource, NodeId, SizingMode};

/// Stable host identity and order-modified paint index shared by Flex and
/// Grid scratch.
#[derive(Debug, Clone, Copy)]
pub(super) struct ItemKey {
    pub(super) node: NodeId,
    pub(super) layout_order: u32,
}

/// Algorithm-neutral ordering data collected before Flex/Grid item
/// classification. The field order intentionally packs this to 24 bytes on
/// 64-bit targets; both algorithms keep one record per generated child.
#[derive(Debug, Clone, Copy)]
pub(super) struct OrderedItem {
    pub(super) node: NodeId,
    pub(super) document_index: usize,
    pub(super) css_order: i32,
    pub(super) layout_order: u32,
}

impl OrderedItem {
    /// Materializes the compact identity copied into algorithm-specific
    /// scratch after ordering is complete.
    #[inline]
    pub(super) const fn key(self) -> ItemKey {
        ItemKey {
            node: self.node,
            layout_order: self.layout_order,
        }
    }
}

/// Box classification inputs and resolved values common to Flex/Grid items.
///
/// This is a short-lived resolver result. Each algorithm destructures it into
/// its own flat hot scratch so shared code does not constrain data layout.
/// Raw values needed by algorithm-specific classification are returned beside
/// their resolved forms to avoid calling lazy host style accessors twice.
#[derive(Debug, Clone, Copy)]
pub(super) struct ResolvedItemBox {
    pub(super) raw_size: Size<Dimension>,
    pub(super) raw_min_size: Size<Dimension>,
    pub(super) aspect_ratio: Option<f32>,
    pub(super) box_sizing: BoxSizing,
    pub(super) overflow: Point<Overflow>,
    pub(super) preferred_size: Size<Option<f32>>,
    pub(super) min_size: Size<Option<f32>>,
    pub(super) max_size: Size<Option<f32>>,
    pub(super) margin: Edges<f32>,
    pub(super) margin_auto: Edges<bool>,
    pub(super) padding: Edges<f32>,
    pub(super) border: Edges<f32>,
    pub(super) scrollbar: Size<f32>,
    pub(super) inset: Edges<Option<f32>>,
}

/// Algorithm-neutral resolved container box and sizing constraints.
#[derive(Debug, Clone, Copy)]
pub(super) struct ResolvedContainerBox {
    pub(super) padding: Edges<f32>,
    pub(super) border: Edges<f32>,
    pub(super) scrollbar: Size<f32>,
    pub(super) box_inset: Size<f32>,
    pub(super) min: Size<Option<f32>>,
    pub(super) max: Size<Option<f32>>,
    pub(super) outer: Size<Option<f32>>,
    pub(super) inner: Size<Option<f32>>,
    pub(super) available_inner: Size<AvailableSpace>,
}

#[inline]
fn checked(value: f32) -> f32 {
    debug_assert!(value.is_finite(), "layout values must be finite");
    value
}

/// Resolves a non-auto length against an optional percentage basis.
///
/// Percentages and `calc()` remain unresolved when their basis is
/// indefinite. Absolute lengths never need a basis.
#[inline]
pub(super) fn resolve_length_percentage(
    value: LengthPercentage,
    basis: Option<f32>,
    resolve_calc: &impl Fn(CalcHandle, f32) -> f32,
) -> Option<f32> {
    match value {
        LengthPercentage::Length(value) => Some(checked(value)),
        LengthPercentage::Percent(fraction) => {
            debug_assert!(fraction.is_finite(), "percentages must be finite");
            basis.map(|basis| checked(basis * fraction))
        }
        LengthPercentage::Calc(handle) => basis.map(|basis| checked(resolve_calc(handle, basis))),
    }
}

/// Resolves a possibly-auto length against an optional percentage basis.
#[inline]
pub(super) fn resolve_length_percentage_auto(
    value: LengthPercentageAuto,
    basis: Option<f32>,
    resolve_calc: &impl Fn(CalcHandle, f32) -> f32,
) -> Option<f32> {
    match value {
        LengthPercentageAuto::Length(value) => Some(checked(value)),
        LengthPercentageAuto::Percent(fraction) => {
            debug_assert!(fraction.is_finite(), "percentages must be finite");
            basis.map(|basis| checked(basis * fraction))
        }
        LengthPercentageAuto::Calc(handle) => {
            basis.map(|basis| checked(resolve_calc(handle, basis)))
        }
        LengthPercentageAuto::Auto => None,
    }
}

/// Resolves a quantitative sizing value.
///
/// Intrinsic keywords require content-contribution probes and therefore
/// intentionally remain unresolved here, just like `auto`.
#[inline]
pub(super) fn resolve_dimension(
    value: Dimension,
    basis: Option<f32>,
    resolve_calc: &impl Fn(CalcHandle, f32) -> f32,
) -> Option<f32> {
    match value {
        Dimension::Length(value) => Some(checked(value)),
        Dimension::Percent(fraction) => {
            debug_assert!(fraction.is_finite(), "percentages must be finite");
            basis.map(|basis| checked(basis * fraction))
        }
        Dimension::Calc(handle) => basis.map(|basis| checked(resolve_calc(handle, basis))),
        Dimension::Auto
        | Dimension::MinContent
        | Dimension::MaxContent
        | Dimension::FitContent(_) => None,
    }
}

#[inline]
pub(super) fn resolve_size(
    value: Size<Dimension>,
    basis: Size<Option<f32>>,
    resolve_calc: &impl Fn(CalcHandle, f32) -> f32,
) -> Size<Option<f32>> {
    Size::new(
        resolve_dimension(value.width, basis.width, resolve_calc),
        resolve_dimension(value.height, basis.height, resolve_calc),
    )
}

/// Resolves padding or border edges. CSS resolves percentages on all four
/// physical sides against the containing block's width.
#[inline]
pub(super) fn resolve_edges(
    value: Edges<LengthPercentage>,
    inline_basis: Option<f32>,
    resolve_calc: &impl Fn(CalcHandle, f32) -> f32,
) -> Edges<f32> {
    value.map(|side| {
        resolve_length_percentage(side, inline_basis, resolve_calc)
            .unwrap_or(0.0)
            .max(0.0)
    })
}

/// Resolves margins while retaining `auto` as `None`.
#[inline]
pub(super) fn resolve_optional_edges(
    value: Edges<LengthPercentageAuto>,
    inline_basis: Option<f32>,
    resolve_calc: &impl Fn(CalcHandle, f32) -> f32,
) -> Edges<Option<f32>> {
    value.map(|side| resolve_length_percentage_auto(side, inline_basis, resolve_calc))
}

#[inline]
pub(super) fn auto_edges_to_zero(value: Edges<Option<f32>>) -> Edges<f32> {
    value.map(|side| side.unwrap_or(0.0))
}

/// Resolves physical insets. Horizontal percentages use the containing
/// block width; vertical percentages use its height.
#[inline(always)]
#[allow(
    clippy::inline_always,
    reason = "avoids a per-item call after the shared box resolver is inlined"
)]
pub(super) fn resolve_insets(
    value: Edges<LengthPercentageAuto>,
    basis: Size<Option<f32>>,
    resolve_calc: &impl Fn(CalcHandle, f32) -> f32,
) -> Edges<Option<f32>> {
    Edges {
        left: resolve_length_percentage_auto(value.left, basis.width, resolve_calc),
        right: resolve_length_percentage_auto(value.right, basis.width, resolve_calc),
        top: resolve_length_percentage_auto(value.top, basis.height, resolve_calc),
        bottom: resolve_length_percentage_auto(value.bottom, basis.height, resolve_calc),
    }
}

#[inline]
pub(super) fn add_optional_sizes(value: Size<Option<f32>>, amount: Size<f32>) -> Size<Option<f32>> {
    Size::new(
        value.width.map(|value| value + amount.width),
        value.height.map(|value| value + amount.height),
    )
}

/// Converts quantitative content-box sizing properties to border-box sizes.
#[inline]
pub(super) fn apply_box_sizing(
    value: Size<Option<f32>>,
    box_sizing: BoxSizing,
    padding_border_size: Size<f32>,
) -> Size<Option<f32>> {
    if box_sizing == BoxSizing::ContentBox {
        add_optional_sizes(value, padding_border_size)
    } else {
        value
    }
}

/// Fills the ratio-dependent axis when exactly one axis is definite.
#[inline]
pub(super) fn apply_aspect_ratio(
    mut value: Size<Option<f32>>,
    aspect_ratio: Option<f32>,
) -> Size<Option<f32>> {
    let Some(ratio) = aspect_ratio else {
        return value;
    };
    debug_assert!(
        ratio.is_finite() && ratio > 0.0,
        "aspect-ratio must be positive and finite"
    );
    if !ratio.is_finite() || ratio <= 0.0 {
        return value;
    }

    match (value.width, value.height) {
        (Some(width), None) => value.height = Some(width / ratio),
        (None, Some(height)) => value.width = Some(height * ratio),
        _ => {}
    }
    value
}

#[inline]
pub(super) fn clamp(value: f32, min: Option<f32>, max: Option<f32>) -> f32 {
    // CSS gives the minimum precedence when max < min.
    value
        .min(max.unwrap_or(f32::INFINITY))
        .max(min.unwrap_or(0.0))
}

/// Size consumed by padding, borders, and classic scrollbars.
#[inline]
pub(super) fn box_inset_size(
    padding: Edges<f32>,
    border: Edges<f32>,
    scrollbar: Size<f32>,
) -> Size<f32> {
    Size::new(
        padding.horizontal_sum() + border.horizontal_sum() + scrollbar.width,
        padding.vertical_sum() + border.vertical_sum() + scrollbar.height,
    )
}

/// Resolves preferred/min/max quantitative sizes into border-box values.
#[inline]
pub(super) fn resolve_quantitative_sizes(
    value: Size<Dimension>,
    basis: Size<Option<f32>>,
    aspect_ratio: Option<f32>,
    box_sizing: BoxSizing,
    box_inset: Size<f32>,
    resolve_calc: &impl Fn(CalcHandle, f32) -> f32,
) -> Size<Option<f32>> {
    apply_box_sizing(
        apply_aspect_ratio(resolve_size(value, basis, resolve_calc), aspect_ratio),
        box_sizing,
        box_inset,
    )
}

/// Applies CSS min/max precedence and a border-box floor on one axis.
#[inline]
pub(super) fn clamp_axis(value: f32, min: Option<f32>, max: Option<f32>, floor: f32) -> f32 {
    clamp(value, min, max).max(floor)
}

#[inline]
pub(super) fn subtract_available_space(
    available_space: AvailableSpace,
    amount: f32,
) -> AvailableSpace {
    match available_space {
        AvailableSpace::Definite(value) => AvailableSpace::Definite((value - amount).max(0.0)),
        intrinsic => intrinsic,
    }
}

/// Space consumed by classic (non-overlay) scrollbars. The axes transpose:
/// vertical overflow consumes width and horizontal overflow consumes height.
#[inline]
pub(super) fn scrollbar_size(style: &impl CoreStyle) -> Size<f32> {
    scrollbar_size_from(style.overflow(), style.scrollbar_width())
}

#[inline]
fn scrollbar_size_from(overflow: Point<Overflow>, width: f32) -> Size<f32> {
    debug_assert!(
        width.is_finite() && width >= 0.0,
        "scrollbar width must be finite and non-negative"
    );
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

/// Resolves the algorithm-neutral box values of one Flex/Grid item.
#[inline(always)]
#[allow(
    clippy::inline_always,
    reason = "avoids a 216-byte resolver result and copy chain in release LLVM IR"
)]
pub(super) fn resolve_item_box<Source: LayoutSource>(
    source: &Source,
    style: &impl CoreStyle,
    percentage_basis: Size<Option<f32>>,
) -> ResolvedItemBox {
    let resolve_calc = |handle, basis| source.resolve_calc(handle, basis);
    let raw_size = style.size();
    let raw_min_size = style.min_size();
    let raw_max_size = style.max_size();
    let aspect_ratio = style.aspect_ratio();
    let box_sizing = style.box_sizing();
    let overflow = style.overflow();
    let padding = resolve_edges(style.padding(), percentage_basis.width, &resolve_calc);
    let border = resolve_edges(style.border(), percentage_basis.width, &resolve_calc);
    let scrollbar = scrollbar_size_from(overflow, style.scrollbar_width());
    let box_inset = box_inset_size(padding, border, scrollbar);
    let preferred_size = resolve_quantitative_sizes(
        raw_size,
        percentage_basis,
        aspect_ratio,
        box_sizing,
        box_inset,
        &resolve_calc,
    );
    let min_size = resolve_quantitative_sizes(
        raw_min_size,
        percentage_basis,
        aspect_ratio,
        box_sizing,
        box_inset,
        &resolve_calc,
    );
    let max_size = resolve_quantitative_sizes(
        raw_max_size,
        percentage_basis,
        aspect_ratio,
        box_sizing,
        box_inset,
        &resolve_calc,
    );
    let margin_value = style.margin();
    let optional_margin =
        resolve_optional_edges(margin_value, percentage_basis.width, &resolve_calc);

    ResolvedItemBox {
        raw_size,
        raw_min_size,
        aspect_ratio,
        box_sizing,
        overflow,
        preferred_size,
        min_size,
        max_size,
        margin: auto_edges_to_zero(optional_margin),
        margin_auto: margin_value.map(LengthPercentageAuto::is_auto),
        padding,
        border,
        scrollbar,
        inset: resolve_insets(style.inset(), percentage_basis, &resolve_calc),
    }
}

/// Resolves the common container box before Flex/Grid-specific sizing.
#[inline]
pub(super) fn resolve_container_box<Source: LayoutSource>(
    source: &Source,
    style: &impl CoreStyle,
    input: LayoutInput,
) -> ResolvedContainerBox {
    let resolve_calc = |handle, basis| source.resolve_calc(handle, basis);
    let padding = resolve_edges(style.padding(), input.parent_size.width, &resolve_calc);
    let border = resolve_edges(style.border(), input.parent_size.width, &resolve_calc);
    let scrollbar = scrollbar_size(style);
    let box_inset = box_inset_size(padding, border, scrollbar);
    let margin = auto_edges_to_zero(resolve_optional_edges(
        style.margin(),
        input.parent_size.width,
        &resolve_calc,
    ));
    let (preferred, min, max) = if input.sizing_mode == SizingMode::ContentSize {
        (Size::NONE, Size::NONE, Size::NONE)
    } else {
        let aspect_ratio = style.aspect_ratio();
        let box_sizing = style.box_sizing();
        (
            resolve_quantitative_sizes(
                style.size(),
                input.parent_size,
                aspect_ratio,
                box_sizing,
                box_inset,
                &resolve_calc,
            ),
            resolve_quantitative_sizes(
                style.min_size(),
                input.parent_size,
                aspect_ratio,
                box_sizing,
                box_inset,
                &resolve_calc,
            ),
            resolve_quantitative_sizes(
                style.max_size(),
                input.parent_size,
                aspect_ratio,
                box_sizing,
                box_inset,
                &resolve_calc,
            ),
        )
    };
    let preferred = Size::new(
        preferred
            .width
            .map(|value| clamp_axis(value, min.width, max.width, box_inset.width)),
        preferred
            .height
            .map(|value| clamp_axis(value, min.height, max.height, box_inset.height)),
    );
    let outer = input.known_dimensions.or(preferred);
    let inner = Size::new(
        outer.width.map(|value| (value - box_inset.width).max(0.0)),
        outer
            .height
            .map(|value| (value - box_inset.height).max(0.0)),
    );
    let available_inner = Size::new(
        inner.width.map_or_else(
            || {
                subtract_available_space(
                    input.available_space.width,
                    margin.horizontal_sum() + box_inset.width,
                )
            },
            AvailableSpace::Definite,
        ),
        inner.height.map_or_else(
            || {
                subtract_available_space(
                    input.available_space.height,
                    margin.vertical_sum() + box_inset.height,
                )
            },
            AvailableSpace::Definite,
        ),
    );

    ResolvedContainerBox {
        padding,
        border,
        scrollbar,
        box_inset,
        min,
        max,
        outer,
        inner,
        available_inner,
    }
}

/// Resolves non-negative row/column gaps against their respective bases.
#[inline]
pub(super) fn resolve_gap<Source: LayoutSource>(
    source: &Source,
    value: Size<LengthPercentage>,
    basis: Size<Option<f32>>,
) -> Size<f32> {
    let resolve_calc = |handle, basis| source.resolve_calc(handle, basis);
    Size::new(
        resolve_length_percentage(value.width, basis.width, &resolve_calc)
            .unwrap_or(0.0)
            .max(0.0),
        resolve_length_percentage(value.height, basis.height, &resolve_calc)
            .unwrap_or(0.0)
            .max(0.0),
    )
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use crate::geometry::Point;

    #[test]
    fn calc_values_use_the_host_resolver_only_with_a_definite_basis() {
        let handle = CalcHandle::from_raw(4);
        let resolve = |actual: CalcHandle, basis: f32| {
            assert_eq!(actual, handle);
            4.0 + basis
        };

        assert_eq!(
            resolve_length_percentage(LengthPercentage::Calc(handle), None, &resolve),
            None
        );
        assert_eq!(
            resolve_length_percentage(LengthPercentage::Calc(handle), Some(10.0), &resolve),
            Some(14.0)
        );
        assert_eq!(
            resolve_length_percentage_auto(LengthPercentageAuto::Calc(handle), None, &resolve),
            None
        );
        assert_eq!(
            resolve_length_percentage_auto(
                LengthPercentageAuto::Calc(handle),
                Some(20.0),
                &resolve
            ),
            Some(24.0)
        );
        assert_eq!(
            resolve_dimension(Dimension::Calc(handle), None, &resolve),
            None
        );
        assert_eq!(
            resolve_dimension(Dimension::Calc(handle), Some(30.0), &resolve),
            Some(34.0)
        );
    }

    struct ScrollingStyle(Point<Overflow>);

    impl CoreStyle for ScrollingStyle {
        fn overflow(&self) -> Point<Overflow> {
            self.0
        }

        fn scrollbar_width(&self) -> f32 {
            7.0
        }
    }

    #[test]
    fn classic_scrollbars_consume_the_opposite_physical_axes() {
        assert_eq!(
            scrollbar_size(&ScrollingStyle(Point::new(
                Overflow::Scroll,
                Overflow::Visible,
            ))),
            Size::new(0.0, 7.0)
        );
        assert_eq!(
            scrollbar_size(&ScrollingStyle(Point::new(
                Overflow::Visible,
                Overflow::Scroll,
            ))),
            Size::new(7.0, 0.0)
        );
        assert_eq!(
            scrollbar_size(&ScrollingStyle(Point::new(
                Overflow::Scroll,
                Overflow::Scroll,
            ))),
            Size::new(7.0, 7.0)
        );
    }

    #[cfg(target_pointer_width = "64")]
    #[test]
    fn ordered_item_stays_compact_on_64_bit_targets() {
        assert_eq!(core::mem::size_of::<OrderedItem>(), 24);
    }
}
