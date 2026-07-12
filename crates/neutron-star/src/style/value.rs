//! Style **value types** — the engine-owned vocabulary style traits speak in.
//!
//! Hosts translate their own computed-style representation (in lynx-vello:
//! stylo `ComputedValues`) into these types *lazily, per accessor call* — the
//! engine never asks the host to materialize a whole style struct. All types
//! are small `Copy` enums; every length is a resolved **CSS pixel** `f32`
//! (device-pixel scaling, `rpx`/`rem`/viewport units etc. are resolved by the
//! host's style system before layout ever runs). Percentages are stored as
//! fractions in `0.0..=1.0`; *what* they are a percentage of is documented on
//! each style-trait accessor.
//!
//! # `calc()` without a dependency
//!
//! `calc()` expressions mixing lengths and percentages cannot be resolved
//! until layout knows the percentage basis, and their parsed representation
//! lives in the host's style engine. The protocol therefore carries an opaque
//! [`CalcHandle`] token; whenever an algorithm needs the value it calls back
//! through [`LayoutSource::resolve_calc`] with the basis. This keeps
//! neutron-star free of any CSS-parser dependency while supporting full
//! `calc()`.
//!
//! [`LayoutSource::resolve_calc`]: crate::tree::LayoutSource::resolve_calc

/// An opaque reference to a host-owned `calc()` expression.
///
/// The engine never inspects the value — it only passes it back to
/// [`LayoutSource::resolve_calc`](crate::tree::LayoutSource::resolve_calc)
/// together with a percentage basis. Hosts typically encode an index or a
/// (suitably guaranteed-live) pointer into their computed-style storage.
///
/// A handle is only meaningful to the [`LayoutSource`](crate::tree::LayoutSource)
/// that produced it, and only for that immutable source epoch's layout run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CalcHandle(u64);

impl CalcHandle {
    /// Wraps a host-chosen identifier.
    #[must_use]
    pub const fn from_raw(raw: u64) -> Self {
        Self(raw)
    }

    /// Returns the host-chosen identifier back.
    #[must_use]
    pub const fn raw(self) -> u64 {
        self.0
    }
}

/// A length, percentage, or `calc()` mixing the two.
///
/// The value type of properties that don't accept `auto`: `padding`,
/// `border-width`, `gap`, and the fixed parts of grid track sizing.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LengthPercentage {
    /// An absolute length in CSS pixels.
    Length(f32),
    /// A fraction (`0.0..=1.0`) of the percentage basis documented on the
    /// style accessor that produced this value.
    Percent(f32),
    /// A host-owned `calc()` expression; resolve via
    /// [`LayoutSource::resolve_calc`](crate::tree::LayoutSource::resolve_calc).
    Calc(CalcHandle),
}

impl LengthPercentage {
    /// Zero length.
    pub const ZERO: Self = Self::Length(0.0);

    /// An absolute length in CSS pixels.
    #[must_use]
    pub const fn length(value: f32) -> Self {
        Self::Length(value)
    }

    /// A percentage, as a fraction in `0.0..=1.0` (so `50%` is `0.5`).
    #[must_use]
    pub const fn percent(fraction: f32) -> Self {
        Self::Percent(fraction)
    }
}

impl Default for LengthPercentage {
    fn default() -> Self {
        Self::ZERO
    }
}

/// A [`LengthPercentage`] that may also be `auto`.
///
/// The value type of `margin` and `inset` (`top`/`right`/`bottom`/`left`),
/// where `auto` has per-context meaning (margin auto-centering, inset
/// non-placement).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum LengthPercentageAuto {
    /// An absolute length in CSS pixels.
    Length(f32),
    /// A fraction (`0.0..=1.0`) of the accessor-documented percentage basis.
    Percent(f32),
    /// A host-owned `calc()` expression.
    Calc(CalcHandle),
    /// The `auto` keyword.
    #[default]
    Auto,
}

impl LengthPercentageAuto {
    /// Zero length.
    pub const ZERO: Self = Self::Length(0.0);

    /// Returns `true` for the `auto` keyword.
    #[must_use]
    pub const fn is_auto(self) -> bool {
        matches!(self, Self::Auto)
    }
}

impl From<LengthPercentage> for LengthPercentageAuto {
    fn from(value: LengthPercentage) -> Self {
        match value {
            LengthPercentage::Length(l) => Self::Length(l),
            LengthPercentage::Percent(p) => Self::Percent(p),
            LengthPercentage::Calc(c) => Self::Calc(c),
        }
    }
}

/// The value of the sizing properties `width`/`height`, `min-*`/`max-*`, and
/// `flex-basis`.
///
/// Beyond [`LengthPercentageAuto`]'s values this includes the CSS Sizing
/// Level 3 intrinsic-sizing keywords, which Lynx's `starlight` also models
/// (`NLength::kNLengthMaxContent`/`kNLengthFitContent`). Intrinsic keywords
/// remain symbolic so each layout algorithm can resolve them from its
/// min-/max-content probes. Flexbox resolves them for main-axis preferred,
/// minimum, maximum, and flex-basis sizes; unsupported contexts leave them
/// unresolved for that context's fallback.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum Dimension {
    /// An absolute length in CSS pixels.
    Length(f32),
    /// A fraction (`0.0..=1.0`) of the accessor-documented percentage basis.
    Percent(f32),
    /// A host-owned `calc()` expression.
    Calc(CalcHandle),
    /// The `auto` keyword: resolve from context (stretch, content, …).
    #[default]
    Auto,
    /// The `min-content` intrinsic size keyword.
    MinContent,
    /// The `max-content` intrinsic size keyword.
    MaxContent,
    /// `fit-content(<length-percentage>)`: clamp `max-content` by the given
    /// limit (and by `min-content` from below).
    FitContent(LengthPercentage),
}

impl Dimension {
    /// Zero length.
    pub const ZERO: Self = Self::Length(0.0);

    /// Returns `true` for the `auto` keyword.
    #[must_use]
    pub const fn is_auto(self) -> bool {
        matches!(self, Self::Auto)
    }
}

impl From<LengthPercentage> for Dimension {
    fn from(value: LengthPercentage) -> Self {
        match value {
            LengthPercentage::Length(l) => Self::Length(l),
            LengthPercentage::Percent(p) => Self::Percent(p),
            LengthPercentage::Calc(c) => Self::Calc(c),
        }
    }
}

impl From<LengthPercentageAuto> for Dimension {
    fn from(value: LengthPercentageAuto) -> Self {
        match value {
            LengthPercentageAuto::Length(l) => Self::Length(l),
            LengthPercentageAuto::Percent(p) => Self::Percent(p),
            LengthPercentageAuto::Calc(c) => Self::Calc(c),
            LengthPercentageAuto::Auto => Self::Auto,
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn calc_handles_round_trip_host_identifiers() {
        let handle = CalcHandle::from_raw(u64::MAX - 7);
        assert_eq!(handle.raw(), u64::MAX - 7);
    }

    #[test]
    fn length_percentage_conversions_preserve_every_variant() {
        let handle = CalcHandle::from_raw(9);
        let cases = [
            (
                LengthPercentage::Length(12.0),
                LengthPercentageAuto::Length(12.0),
                Dimension::Length(12.0),
            ),
            (
                LengthPercentage::Percent(0.25),
                LengthPercentageAuto::Percent(0.25),
                Dimension::Percent(0.25),
            ),
            (
                LengthPercentage::Calc(handle),
                LengthPercentageAuto::Calc(handle),
                Dimension::Calc(handle),
            ),
        ];

        for (source, expected_auto, expected_dimension) in cases {
            assert_eq!(LengthPercentageAuto::from(source), expected_auto);
            assert_eq!(Dimension::from(source), expected_dimension);
        }
    }

    #[test]
    fn auto_length_percentage_converts_to_all_dimension_variants() {
        let handle = CalcHandle::from_raw(11);
        let cases = [
            (LengthPercentageAuto::Length(4.0), Dimension::Length(4.0)),
            (
                LengthPercentageAuto::Percent(0.75),
                Dimension::Percent(0.75),
            ),
            (LengthPercentageAuto::Calc(handle), Dimension::Calc(handle)),
            (LengthPercentageAuto::Auto, Dimension::Auto),
        ];

        for (source, expected) in cases {
            assert_eq!(Dimension::from(source), expected);
        }
        assert!(LengthPercentageAuto::Auto.is_auto());
        assert!(!LengthPercentageAuto::ZERO.is_auto());
        assert!(Dimension::Auto.is_auto());
        assert!(!Dimension::ZERO.is_auto());
    }
}
