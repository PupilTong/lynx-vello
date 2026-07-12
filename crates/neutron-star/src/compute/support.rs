//! Shared, dependency-free building blocks for host-provided layout algorithms.
//!
//! neutron-star deliberately keeps formatting-algorithm dispatch open. Host
//! algorithms such as Lynx `display: linear` still need the same box-model
//! arithmetic used internally by Flex and Grid. These thin, inlined adapters
//! expose that arithmetic without exposing engine scratch types or duplicating
//! its implementation.

use crate::geometry::{Edges, Point, Size};
use crate::style::value::{CalcHandle, Dimension, LengthPercentage, LengthPercentageAuto};
use crate::style::{BoxSizing, CoreStyle, Direction};
use crate::tree::AvailableSpace;

/// Resolves a non-auto length against an optional percentage basis.
///
/// Percentages and `calc()` remain unresolved when their basis is indefinite.
#[inline]
pub fn resolve_length_percentage(
    value: LengthPercentage,
    basis: Option<f32>,
    resolve_calc: &impl Fn(CalcHandle, f32) -> f32,
) -> Option<f32> {
    super::util::resolve_length_percentage(value, basis, resolve_calc)
}

/// Resolves padding or border edges against the containing block's width.
#[inline]
pub fn resolve_edges(
    value: Edges<LengthPercentage>,
    inline_basis: Option<f32>,
    resolve_calc: &impl Fn(CalcHandle, f32) -> f32,
) -> Edges<f32> {
    super::util::resolve_edges(value, inline_basis, resolve_calc)
}

/// Resolves margin-like edges while preserving `auto` as `None`.
#[inline]
pub fn resolve_optional_edges(
    value: Edges<LengthPercentageAuto>,
    inline_basis: Option<f32>,
    resolve_calc: &impl Fn(CalcHandle, f32) -> f32,
) -> Edges<Option<f32>> {
    super::util::resolve_optional_edges(value, inline_basis, resolve_calc)
}

/// Resolves physical insets against their corresponding containing-block axes.
#[inline]
pub fn resolve_insets(
    value: Edges<LengthPercentageAuto>,
    basis: Size<Option<f32>>,
    resolve_calc: &impl Fn(CalcHandle, f32) -> f32,
) -> Edges<Option<f32>> {
    super::util::resolve_insets(value, basis, resolve_calc)
}

/// Fills the ratio-dependent axis when exactly one axis is definite.
#[inline]
#[must_use]
pub fn apply_aspect_ratio(
    value: Size<Option<f32>>,
    aspect_ratio: Option<f32>,
) -> Size<Option<f32>> {
    super::util::apply_aspect_ratio(value, aspect_ratio)
}

/// Returns which preferred-size axes establish a definite percentage basis.
#[inline]
#[must_use]
pub fn preferred_size_definiteness(
    size: Size<Dimension>,
    parent_size: Size<Option<f32>>,
    aspect_ratio: Option<f32>,
) -> Size<bool> {
    super::util::preferred_size_definiteness(size, parent_size, aspect_ratio)
}

/// Size consumed by padding, borders, and classic scrollbars.
#[inline]
#[must_use]
pub fn padding_border_size(
    padding: Edges<f32>,
    border: Edges<f32>,
    scrollbar: Size<f32>,
) -> Size<f32> {
    super::util::box_inset_size(padding, border, scrollbar)
}

/// Resolves preferred/min/max quantitative sizes into border-box values.
#[inline]
pub fn resolve_quantitative_sizes(
    value: Size<Dimension>,
    basis: Size<Option<f32>>,
    aspect_ratio: Option<f32>,
    box_sizing: BoxSizing,
    box_inset: Size<f32>,
    resolve_calc: &impl Fn(CalcHandle, f32) -> f32,
) -> Size<Option<f32>> {
    super::util::resolve_quantitative_sizes(
        value,
        basis,
        aspect_ratio,
        box_sizing,
        box_inset,
        resolve_calc,
    )
}

/// Applies CSS min/max precedence and a border-box floor on one axis.
#[inline]
#[must_use]
pub fn clamp_axis(value: f32, min: Option<f32>, max: Option<f32>, floor: f32) -> f32 {
    super::util::clamp_axis(value, min, max, floor)
}

/// Subtracts box-model space from a definite constraint.
#[inline]
#[must_use]
pub fn subtract_available_space(available_space: AvailableSpace, amount: f32) -> AvailableSpace {
    super::util::subtract_available_space(available_space, amount)
}

/// Space consumed by classic (non-overlay) scrollbars.
#[inline]
pub fn scrollbar_size(style: &impl CoreStyle) -> Size<f32> {
    super::util::scrollbar_size(style)
}

/// Resolves relative-position insets to a physical visual offset.
#[inline]
#[must_use]
pub fn relative_offset(inset: Edges<Option<f32>>, direction: Direction) -> Point<f32> {
    let x = match (inset.left, inset.right) {
        (Some(_), Some(right)) if direction == Direction::Rtl => -right,
        (Some(left), _) => left,
        (None, Some(right)) => -right,
        (None, None) => 0.0,
    };
    let y = inset.top.unwrap_or_else(|| -inset.bottom.unwrap_or(0.0));
    Point::new(x, y)
}
