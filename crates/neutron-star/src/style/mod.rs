//! The style protocol: how the engine reads computed style.

pub mod containment;
pub mod flex;
pub mod grid;
pub mod linear;
pub mod relative;
pub mod text;

pub use containment::effective_containment;
pub use flex::{FlexContainerStyle, FlexItemStyle};
pub use grid::{GridContainerStyle, GridItemStyle};
pub use linear::{LinearContainerStyle, LinearItemStyle};
pub use relative::{RelativeContainerStyle, RelativeItemStyle};
pub use stylo::computed_values::{
    box_sizing, direction, flex_direction, flex_wrap, linear_direction, relative_center,
    relative_layout_once, text_wrap_mode, visibility, white_space_collapse,
};
pub use stylo::values::computed::length::NonNegativeLengthPercentageOrNormal;
pub use stylo::values::computed::lynx_layout::{RelativeAlign, RelativeReference};
pub use stylo::values::computed::{
    AspectRatio, Au, BorderSideWidth, Contain, ContainIntrinsicSize, ContentDistribution,
    ContentVisibility, Display, FlexBasis, FontFamily, FontFeatureSettings, FontStyle,
    FontVariationSettings, FontWeight, GridAutoFlow, GridLine, GridTemplateComponent,
    ImplicitGridTracks, Inset, ItemPlacement, JustifyItems, LengthPercentage, LetterSpacing,
    LineHeight, Margin, MaxSize, NonNegativeLengthPercentage, NonNegativeNumber, Overflow,
    PositionProperty, SelfAlignment, Size as StyleSize, TextAlign, TextIndent, WordBreak,
};
pub use stylo::values::specified::align::AlignFlags;
pub use text::{TextBrush, TextContainerStyle, TextRun, TextRunStyle};

use crate::geometry::{Edges, Point, Size};

pub(in crate::style) mod defaults {
    use std::sync::LazyLock;

    use stylo::Zero;

    use super::{Inset, Margin, MaxSize, NonNegativeLengthPercentage, StyleSize};

    pub(in crate::style) static INSET_AUTO: LazyLock<Inset> = LazyLock::new(Inset::auto);
    pub(in crate::style) static SIZE_AUTO: LazyLock<StyleSize> = LazyLock::new(StyleSize::auto);
    pub(in crate::style) static MAX_SIZE_NONE: LazyLock<MaxSize> = LazyLock::new(MaxSize::none);
    pub(in crate::style) static MARGIN_ZERO: LazyLock<Margin> = LazyLock::new(Margin::zero);
    pub(in crate::style) static PADDING_ZERO: LazyLock<NonNegativeLengthPercentage> =
        LazyLock::new(NonNegativeLengthPercentage::zero);
}

pub trait CoreStyle: Sized {
    fn display(&self) -> Display;

    fn visibility(&self) -> visibility::T {
        visibility::T::Visible
    }

    fn position(&self) -> PositionProperty {
        PositionProperty::Static
    }

    fn inset(&self) -> Edges<&Inset> {
        Edges::uniform(&*defaults::INSET_AUTO)
    }

    fn size(&self) -> Size<&StyleSize> {
        Size::new(&*defaults::SIZE_AUTO, &*defaults::SIZE_AUTO)
    }

    fn min_size(&self) -> Size<&StyleSize> {
        Size::new(&*defaults::SIZE_AUTO, &*defaults::SIZE_AUTO)
    }

    fn max_size(&self) -> Size<&MaxSize> {
        Size::new(&*defaults::MAX_SIZE_NONE, &*defaults::MAX_SIZE_NONE)
    }

    fn aspect_ratio(&self) -> AspectRatio {
        AspectRatio::auto()
    }

    fn margin(&self) -> Edges<&Margin> {
        Edges::uniform(&*defaults::MARGIN_ZERO)
    }

    fn padding(&self) -> Edges<&NonNegativeLengthPercentage> {
        Edges::uniform(&*defaults::PADDING_ZERO)
    }

    fn border(&self) -> Edges<BorderSideWidth> {
        Edges::uniform(BorderSideWidth(Au(0)))
    }

    fn overflow(&self) -> Point<Overflow> {
        Point::new(Overflow::Visible, Overflow::Visible)
    }

    fn box_sizing(&self) -> box_sizing::T {
        box_sizing::T::ContentBox
    }

    fn direction(&self) -> direction::T {
        direction::T::Ltr
    }

    fn containment(&self) -> Contain {
        Contain::empty()
    }

    fn contain_intrinsic_width(&self) -> ContainIntrinsicSize {
        ContainIntrinsicSize::None
    }

    fn contain_intrinsic_height(&self) -> ContainIntrinsicSize {
        ContainIntrinsicSize::None
    }

    fn skips_contents(&self) -> bool {
        false
    }
}

impl<S: CoreStyle> CoreStyle for &S {
    fn display(&self) -> Display {
        (**self).display()
    }

    fn visibility(&self) -> visibility::T {
        (**self).visibility()
    }

    fn position(&self) -> PositionProperty {
        (**self).position()
    }

    fn inset(&self) -> Edges<&Inset> {
        (**self).inset()
    }

    fn size(&self) -> Size<&StyleSize> {
        (**self).size()
    }

    fn min_size(&self) -> Size<&StyleSize> {
        (**self).min_size()
    }

    fn max_size(&self) -> Size<&MaxSize> {
        (**self).max_size()
    }

    fn aspect_ratio(&self) -> AspectRatio {
        (**self).aspect_ratio()
    }

    fn margin(&self) -> Edges<&Margin> {
        (**self).margin()
    }

    fn padding(&self) -> Edges<&NonNegativeLengthPercentage> {
        (**self).padding()
    }

    fn border(&self) -> Edges<BorderSideWidth> {
        (**self).border()
    }

    fn overflow(&self) -> Point<Overflow> {
        (**self).overflow()
    }

    fn box_sizing(&self) -> box_sizing::T {
        (**self).box_sizing()
    }

    fn direction(&self) -> direction::T {
        (**self).direction()
    }

    fn containment(&self) -> Contain {
        (**self).containment()
    }

    fn contain_intrinsic_width(&self) -> ContainIntrinsicSize {
        (**self).contain_intrinsic_width()
    }

    fn contain_intrinsic_height(&self) -> ContainIntrinsicSize {
        (**self).contain_intrinsic_height()
    }

    fn skips_contents(&self) -> bool {
        (**self).skips_contents()
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use stylo::Zero;

    use super::*;

    #[derive(Debug)]
    struct Defaults;

    impl CoreStyle for Defaults {
        fn display(&self) -> Display {
            Display::Flex
        }
    }

    #[test]
    fn core_style_defaults_are_fork_initial_values() {
        let style = Defaults;

        assert!(!style.display().is_none());
        assert_eq!(style.visibility(), visibility::T::Visible);
        assert_eq!(style.position(), PositionProperty::Static);
        assert_eq!(style.inset(), Edges::uniform(&Inset::auto()));
        assert_eq!(
            style.size(),
            Size::new(&StyleSize::auto(), &StyleSize::auto())
        );
        assert_eq!(
            style.min_size(),
            Size::new(&StyleSize::auto(), &StyleSize::auto())
        );
        assert_eq!(
            style.max_size(),
            Size::new(&MaxSize::none(), &MaxSize::none())
        );
        assert!(style.aspect_ratio().auto);
        assert_eq!(style.margin(), Edges::uniform(&Margin::zero()));
        assert_eq!(
            style.padding(),
            Edges::uniform(&NonNegativeLengthPercentage::zero())
        );
        assert_eq!(style.border(), Edges::uniform(BorderSideWidth(Au(0))));
        assert_eq!(
            style.overflow(),
            Point::new(Overflow::Visible, Overflow::Visible)
        );
        assert_eq!(style.box_sizing(), box_sizing::T::ContentBox);
        assert_eq!(style.direction(), direction::T::Ltr);
        assert_eq!(style.containment(), Contain::empty());
        assert_eq!(style.contain_intrinsic_width(), ContainIntrinsicSize::None);
        assert_eq!(style.contain_intrinsic_height(), ContainIntrinsicSize::None);
        assert!(!style.skips_contents());

        let view = &style;
        assert_eq!(view.containment(), Contain::empty());
        assert_eq!(view.contain_intrinsic_width(), ContainIntrinsicSize::None);
        assert_eq!(view.contain_intrinsic_height(), ContainIntrinsicSize::None);
        assert!(!view.skips_contents());
    }

    #[test]
    fn overflow_scroll_containers_follow_stylo_is_scrollable() {
        assert!(!Overflow::Visible.is_scrollable());
        assert!(Overflow::Hidden.is_scrollable());
    }

    #[test]
    fn reference_views_forward_core_accessors() {
        let style = Defaults;
        let view = &style;
        assert_eq!(CoreStyle::visibility(&view), visibility::T::Visible);
        assert_eq!(CoreStyle::position(&view), PositionProperty::Static);
        assert!(!CoreStyle::display(&view).is_none());
    }
}
