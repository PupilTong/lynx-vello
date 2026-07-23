//! Style protocol for Starlight's `display: linear` layout algorithm.

use stylo::computed_values::linear_direction;
use stylo::values::computed::{
    ContentDistribution, ItemPlacement, NonNegativeNumber, SelfAlignment,
};

use crate::style::CoreStyle;

pub trait LinearContainerStyle: CoreStyle {
    fn linear_direction(&self) -> linear_direction::T {
        linear_direction::T::Column
    }

    fn linear_weight_sum(&self) -> NonNegativeNumber {
        NonNegativeNumber::from(0.0)
    }

    fn justify_content(&self) -> ContentDistribution {
        ContentDistribution::normal()
    }

    fn align_items(&self) -> ItemPlacement {
        ItemPlacement::normal()
    }
}

pub trait LinearItemStyle: CoreStyle {
    fn linear_weight(&self) -> NonNegativeNumber {
        NonNegativeNumber::from(0.0)
    }

    fn align_self(&self) -> SelfAlignment {
        SelfAlignment::auto()
    }

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
