//! Small, dependency-free value-resolution helpers shared by layout entry
//! points.

use crate::geometry::{Edges, Size};
use crate::style::value::{CalcHandle, Dimension, LengthPercentage, LengthPercentageAuto};
use crate::style::{BoxSizing, CoreStyle, Overflow};
use crate::tree::AvailableSpace;

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
#[inline]
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
    let overflow = style.overflow();
    let width = style.scrollbar_width();
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
