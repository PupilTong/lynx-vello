//! Style protocol for Starlight's `display: linear` layout algorithm.
//!
//! The fork's grammar is the source of truth for the Lynx property surface:
//! there are no `linear-gravity`/`linear-cross-gravity`/
//! `linear-layout-gravity` longhands. Gravity is expressed through the
//! standard alignment properties — main-axis packing through
//! [`justify_content`](LinearContainerStyle::justify_content), cross-axis
//! alignment through [`align_items`](LinearContainerStyle::align_items) with
//! a per-item [`align_self`](LinearItemStyle::align_self) override — and the
//! algorithm re-keys the legacy gravity semantics onto those values (the
//! legacy `fill-*` gravities map to `stretch`). The legacy
//! `linear-orientation` spelling is host-lowered into `linear-direction`.

use stylo::computed_values::linear_direction;
use stylo::values::computed::{
    ContentDistribution, ItemPlacement, NonNegativeNumber, SelfAlignment,
};

use crate::style::CoreStyle;

/// Computed style of a node as a linear container.
///
/// Defaults are the fork's initial values (`linear-direction: column`).
pub trait LinearContainerStyle: CoreStyle {
    /// `linear-direction`: which physical axis is the main axis, and its
    /// direction (`column` is vertical, `row` horizontal; `row` variants are
    /// additionally flipped when [`CoreStyle::direction`] is `Rtl`).
    fn linear_direction(&self) -> linear_direction::T {
        linear_direction::T::Column
    }

    /// `linear-weight-sum`; a positive value overrides the distribution
    /// denominator, while zero requests the sum of participating item weights.
    fn linear_weight_sum(&self) -> NonNegativeNumber {
        NonNegativeNumber::from(0.0)
    }

    /// `justify-content` — main-axis packing (the legacy `linear-gravity`
    /// channel).
    fn justify_content(&self) -> ContentDistribution {
        ContentDistribution::normal()
    }

    /// `align-items` — cross-axis alignment fallback (the legacy
    /// `linear-cross-gravity` channel).
    fn align_items(&self) -> ItemPlacement {
        ItemPlacement::normal()
    }
}

/// Computed style of a node as an item of a linear container.
pub trait LinearItemStyle: CoreStyle {
    /// `linear-weight`; only positive values participate in weighted
    /// main-size distribution.
    fn linear_weight(&self) -> NonNegativeNumber {
        NonNegativeNumber::from(0.0)
    }

    /// `align-self` — per-item cross-axis alignment (the legacy
    /// `linear-layout-gravity` channel); `auto` defers to the container's
    /// `align-items`.
    fn align_self(&self) -> SelfAlignment {
        SelfAlignment::auto()
    }

    /// Standard `order`; lower values are laid out first, stably within ties.
    fn order(&self) -> i32 {
        0
    }
}

impl<S: LinearContainerStyle> LinearContainerStyle for &S {
    fn linear_direction(&self) -> linear_direction::T {
        (**self).linear_direction()
    }

    fn linear_weight_sum(&self) -> NonNegativeNumber {
        (**self).linear_weight_sum()
    }

    fn justify_content(&self) -> ContentDistribution {
        (**self).justify_content()
    }

    fn align_items(&self) -> ItemPlacement {
        (**self).align_items()
    }
}

impl<S: LinearItemStyle> LinearItemStyle for &S {
    fn linear_weight(&self) -> NonNegativeNumber {
        (**self).linear_weight()
    }

    fn align_self(&self) -> SelfAlignment {
        (**self).align_self()
    }

    fn order(&self) -> i32 {
        (**self).order()
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    #![allow(clippy::float_cmp)]

    use stylo::values::computed::Display;

    use super::*;

    #[derive(Debug)]
    struct Defaults;

    impl CoreStyle for Defaults {
        fn display(&self) -> Display {
            Display::Linear
        }
    }
    impl LinearContainerStyle for Defaults {}
    impl LinearItemStyle for Defaults {}

    #[test]
    fn linear_defaults_match_the_fork_initial_values() {
        let style = Defaults;
        assert_eq!(style.linear_direction(), linear_direction::T::Column);
        assert_eq!(style.linear_weight_sum().0, 0.0);
        assert_eq!(
            LinearContainerStyle::justify_content(&style),
            ContentDistribution::normal()
        );
        assert_eq!(
            LinearContainerStyle::align_items(&style),
            ItemPlacement::normal()
        );
        assert_eq!(style.linear_weight().0, 0.0);
        assert_eq!(LinearItemStyle::align_self(&style), SelfAlignment::auto());
        assert_eq!(style.order(), 0);
    }
}
