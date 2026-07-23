//! Flexbox style protocol (CSS Flexible Box Layout Module Level 1).

use std::sync::LazyLock;

use stylo::computed_values::{flex_direction, flex_wrap};
use stylo::values::computed::length::NonNegativeLengthPercentageOrNormal;
use stylo::values::computed::{
    ContentDistribution, FlexBasis, ItemPlacement, NonNegativeNumber, SelfAlignment,
};

use crate::geometry::Size;
use crate::style::CoreStyle;

static FLEX_BASIS_AUTO: LazyLock<FlexBasis> = LazyLock::new(FlexBasis::auto);
static GAP_NORMAL: NonNegativeLengthPercentageOrNormal =
    NonNegativeLengthPercentageOrNormal::Normal;

pub trait FlexContainerStyle: CoreStyle {
    fn flex_direction(&self) -> flex_direction::T {
        flex_direction::T::Row
    }

    fn flex_wrap(&self) -> flex_wrap::T {
        flex_wrap::T::Nowrap
    }

    fn gap(&self) -> Size<&NonNegativeLengthPercentageOrNormal> {
        Size::new(&GAP_NORMAL, &GAP_NORMAL)
    }

    fn align_content(&self) -> ContentDistribution {
        ContentDistribution::normal()
    }

    fn align_items(&self) -> ItemPlacement {
        ItemPlacement::normal()
    }

    fn justify_content(&self) -> ContentDistribution {
        ContentDistribution::normal()
    }
}

pub trait FlexItemStyle: CoreStyle {
    fn flex_basis(&self) -> &FlexBasis {
        &FLEX_BASIS_AUTO
    }

    fn flex_grow(&self) -> NonNegativeNumber {
        NonNegativeNumber::from(0.0)
    }

    fn flex_shrink(&self) -> NonNegativeNumber {
        NonNegativeNumber::from(1.0)
    }

    fn align_self(&self) -> SelfAlignment {
        SelfAlignment::auto()
    }

    fn order(&self) -> i32 {
        0
    }
}

impl<S: FlexContainerStyle> FlexContainerStyle for &S {
    fn flex_direction(&self) -> flex_direction::T {
        (**self).flex_direction()
    }

    fn flex_wrap(&self) -> flex_wrap::T {
        (**self).flex_wrap()
    }

    fn gap(&self) -> Size<&NonNegativeLengthPercentageOrNormal> {
        (**self).gap()
    }

    fn align_content(&self) -> ContentDistribution {
        (**self).align_content()
    }

    fn align_items(&self) -> ItemPlacement {
        (**self).align_items()
    }

    fn justify_content(&self) -> ContentDistribution {
        (**self).justify_content()
    }
}

impl<S: FlexItemStyle> FlexItemStyle for &S {
    fn flex_basis(&self) -> &FlexBasis {
        (**self).flex_basis()
    }

    fn flex_grow(&self) -> NonNegativeNumber {
        (**self).flex_grow()
    }

    fn flex_shrink(&self) -> NonNegativeNumber {
        (**self).flex_shrink()
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
            Display::Flex
        }
    }
    impl FlexContainerStyle for Defaults {}
    impl FlexItemStyle for Defaults {}

    #[test]
    fn flex_container_defaults_are_css_initial_values() {
        let style = Defaults;

        assert_eq!(style.flex_direction(), flex_direction::T::Row);
        assert_eq!(style.flex_wrap(), flex_wrap::T::Nowrap);
        assert_eq!(
            style.gap(),
            Size::new(
                &NonNegativeLengthPercentageOrNormal::Normal,
                &NonNegativeLengthPercentageOrNormal::Normal,
            )
        );
        assert_eq!(style.align_content(), ContentDistribution::normal());
        assert_eq!(style.align_items(), ItemPlacement::normal());
        assert_eq!(style.justify_content(), ContentDistribution::normal());
    }

    #[test]
    fn flex_item_defaults_are_css_initial_values() {
        let style = Defaults;

        assert_eq!(style.flex_basis(), &FlexBasis::auto());
        assert_eq!(style.flex_grow().0, 0.0);
        assert_eq!(style.flex_shrink().0, 1.0);
        assert_eq!(FlexItemStyle::align_self(&style), SelfAlignment::auto());
        assert_eq!(style.order(), 0);
    }
}
