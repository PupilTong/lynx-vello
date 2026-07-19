//! Flexbox style protocol (CSS Flexible Box Layout Module Level 1).
//!
//! Two traits, mirroring the spec's split of responsibilities: the
//! **container** decides axes, wrapping, and distribution
//! ([`FlexContainerStyle`]); each **item** decides its own flexibility and
//! self-alignment ([`FlexItemStyle`]). The L1 flexbox algorithm reads both
//! views through [`LayoutNode::style`], narrowing the node's style type with
//! `FlexContainerStyle + FlexItemStyle` bounds.
//!
//! Alignment values are stylo's `AlignFlags`-based wrappers
//! ([`ContentDistribution`], [`ItemPlacement`], [`SelfAlignment`]); the
//! `normal`/`auto` keywords are encoded in the flags and normalized by the
//! algorithm at style-read time.
//!
//! [`LayoutNode::style`]: crate::tree::LayoutNode::style

use stylo::computed_values::{flex_direction, flex_wrap};
use stylo::values::computed::length::NonNegativeLengthPercentageOrNormal;
use stylo::values::computed::{
    ContentDistribution, FlexBasis, ItemPlacement, NonNegativeNumber, SelfAlignment,
};

use crate::geometry::Size;
use crate::style::CoreStyle;

/// Style of a node *as a flex container*.
///
/// Defaults are the CSS initial values.
pub trait FlexContainerStyle: CoreStyle {
    /// `flex-direction`. Note `Row`/`RowReverse` are additionally flipped
    /// when [`CoreStyle::direction`] is `Rtl` — the flip is applied inside
    /// the algorithm.
    fn flex_direction(&self) -> flex_direction::T {
        flex_direction::T::Row
    }

    /// `flex-wrap`.
    fn flex_wrap(&self) -> flex_wrap::T {
        flex_wrap::T::Nowrap
    }

    /// `gap` (`column-gap` is `width`, `row-gap` is `height`); `normal`
    /// resolves to zero.
    ///
    /// Percentage basis: the container's content-box size in the gap's axis.
    fn gap(&self) -> Size<NonNegativeLengthPercentageOrNormal> {
        Size::new(
            NonNegativeLengthPercentageOrNormal::Normal,
            NonNegativeLengthPercentageOrNormal::Normal,
        )
    }

    /// `align-content` — cross-axis distribution of lines.
    fn align_content(&self) -> ContentDistribution {
        ContentDistribution::normal()
    }

    /// `align-items` — default cross-axis alignment of items (`normal`
    /// behaves as `stretch` here).
    fn align_items(&self) -> ItemPlacement {
        ItemPlacement::normal()
    }

    /// `justify-content` — main-axis distribution of items.
    fn justify_content(&self) -> ContentDistribution {
        ContentDistribution::normal()
    }
}

/// Style of a node *as a flex item* (i.e. as read by its parent container's
/// layout).
///
/// Defaults are the CSS initial values.
pub trait FlexItemStyle: CoreStyle {
    /// `flex-basis`. `content` has no Starlight sizing behavior and defers
    /// to [`CoreStyle::size`] (documented behavior delta of the stylo
    /// vocabulary swap).
    ///
    /// Percentage basis: the container's content-box main-axis size.
    fn flex_basis(&self) -> FlexBasis {
        FlexBasis::auto()
    }

    /// `flex-grow`.
    fn flex_grow(&self) -> NonNegativeNumber {
        NonNegativeNumber::from(0.0)
    }

    /// `flex-shrink`.
    fn flex_shrink(&self) -> NonNegativeNumber {
        NonNegativeNumber::from(1.0)
    }

    /// `align-self`. `auto` defers to the container's `align-items`.
    fn align_self(&self) -> SelfAlignment {
        SelfAlignment::auto()
    }

    /// `order` — layout/paint reordering among siblings; lower comes first.
    /// Lynx supports this standard property, so it is first-class protocol.
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

    fn gap(&self) -> Size<NonNegativeLengthPercentageOrNormal> {
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
    fn flex_basis(&self) -> FlexBasis {
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
                NonNegativeLengthPercentageOrNormal::Normal,
                NonNegativeLengthPercentageOrNormal::Normal,
            )
        );
        assert_eq!(style.align_content(), ContentDistribution::normal());
        assert_eq!(style.align_items(), ItemPlacement::normal());
        assert_eq!(style.justify_content(), ContentDistribution::normal());
    }

    #[test]
    fn flex_item_defaults_are_css_initial_values() {
        let style = Defaults;

        assert_eq!(style.flex_basis(), FlexBasis::auto());
        assert_eq!(style.flex_grow().0, 0.0);
        assert_eq!(style.flex_shrink().0, 1.0);
        assert_eq!(FlexItemStyle::align_self(&style), SelfAlignment::auto());
        assert_eq!(style.order(), 0);
    }
}
