//! The computed-style protocol: how every layout algorithm reads style. The
//! box-model core lives here, the per-algorithm surfaces in `algorithms`, and
//! the text surfaces in `text`.

use std::sync::LazyLock;

use stylo::properties::ComputedValues;
use stylo::properties::style_structs::Font;
use stylo::servo_arc::Arc;

use crate::geometry::{Edges, Point, Size};

macro_rules! style_protocol {
    (
        pub trait $trait:ident: $super:path {
            defaults($receiver:ident) {
                $($method:ident -> $return:ty = $value:expr),* $(,)?
            }
        }
    ) => {
        pub trait $trait: $super {
            $(
                #[inline]
                fn $method(&self) -> $return {
                    let $receiver = self;
                    let _ = $receiver;
                    $value
                }
            )*
        }

        impl<S: $trait> $trait for &S {
            $(
                #[inline]
                fn $method(&self) -> $return {
                    <S as $trait>::$method(*self)
                }
            )*
        }
    };
}

pub mod algorithms;
pub mod containment;
pub mod text;

pub use algorithms::{FlexboxStyle, GridLanesStyle, GridStyle, LinearStyle, RelativeStyle};
use containment::effective_containment;
pub use stylo::computed_values::{
    box_sizing, direction, flex_direction, flex_wrap, linear_direction, relative_center,
    relative_layout_once, text_wrap_mode, visibility,
};
pub use stylo::logical_geometry::PhysicalAxis;
pub use stylo::values::DashedIdent;
pub use stylo::values::computed::length::NonNegativeLengthPercentageOrNormal;
pub use stylo::values::computed::lynx_layout::{RelativeAlign, RelativeReference};
pub use stylo::values::computed::{
    AspectRatio, Au, BorderSideWidth, Contain, ContainIntrinsicSize, ContainerType,
    ContentDistribution, ContentVisibility, Display, FitTolerance, FlexBasis, FontFamily,
    FontFeatureSettings, FontStyle, FontVariationSettings, FontWeight, GridAutoFlow, GridLine,
    GridTemplateComponent, ImplicitGridTracks, Inset, ItemPlacement, JustifyItems,
    LengthPercentage, LetterSpacing, LineHeight, Margin, MaxSize, NonNegativeLengthPercentage,
    NonNegativeNumber, Overflow, PositionArea, PositionProperty, PositionTryOrder, SelfAlignment,
    Size as StyleSize, TextAlign, TextIndent, WordBreak,
};
pub use stylo::values::generics::position::TreeScoped;
pub use stylo::values::specified::align::AlignFlags;
use stylo::values::specified::position::PositionAnchorKeyword;
pub use text::{LynxTextStyle, TextBrush, TextContainerStyle, TextRunStyle};

pub const RELATIVE_REFERENCE_NONE: RelativeReference = -1;
pub const RELATIVE_REFERENCE_PARENT: RelativeReference = 0;

static INITIAL_VALUES: LazyLock<Arc<ComputedValues>> =
    LazyLock::new(|| ComputedValues::initial_values_with_font_override(Font::initial_values()));

#[inline]
pub(in crate::style) fn initial_values() -> &'static ComputedValues {
    &INITIAL_VALUES
}

#[inline]
pub(in crate::style) fn lower_relative_logical(
    physical: RelativeReference,
    logical: RelativeReference,
) -> i32 {
    if physical == RELATIVE_REFERENCE_NONE {
        logical
    } else {
        physical
    }
}

style_protocol! {
    pub trait CoreStyle: Sized {
        defaults(style) {
            computed_values -> &ComputedValues = initial_values(),
            inherited_values -> &ComputedValues = style.computed_values(),

            display -> Display = *style.computed_values().get_display(),
            position -> PositionProperty = *style.computed_values().get_box().get_position(),
            inset -> Edges<&Inset> = {
                let position = style.computed_values().get_position();
                Edges {
                    left: &position.left,
                    right: &position.right,
                    top: &position.top,
                    bottom: &position.bottom,
                }
            },
            size -> Size<&StyleSize> = {
                let position = style.computed_values().get_position();
                Size::new(&position.width, &position.height)
            },
            min_size -> Size<&StyleSize> = {
                let position = style.computed_values().get_position();
                Size::new(&position.min_width, &position.min_height)
            },
            max_size -> Size<&MaxSize> = {
                let position = style.computed_values().get_position();
                Size::new(&position.max_width, &position.max_height)
            },
            aspect_ratio -> AspectRatio = *style.computed_values().get_aspect_ratio(),
            natural_size -> Size<Option<f32>> = Size::NONE,
            margin -> Edges<&Margin> = {
                let margin = style.computed_values().get_margin();
                Edges {
                    left: &margin.margin_left,
                    right: &margin.margin_right,
                    top: &margin.margin_top,
                    bottom: &margin.margin_bottom,
                }
            },
            padding -> Edges<&NonNegativeLengthPercentage> = {
                let padding = style.computed_values().get_padding();
                Edges {
                    left: &padding.padding_left,
                    right: &padding.padding_right,
                    top: &padding.padding_top,
                    bottom: &padding.padding_bottom,
                }
            },
            border -> Edges<BorderSideWidth> = {
                let border = style.computed_values().get_border();
                let used = |width: Au, border_style: stylo::values::specified::BorderStyle| {
                    BorderSideWidth(if border_style.none_or_hidden() { Au(0) } else { width })
                };
                Edges {
                    left: used(border.border_left_width.0, border.border_left_style),
                    right: used(border.border_right_width.0, border.border_right_style),
                    top: used(border.border_top_width.0, border.border_top_style),
                    bottom: used(border.border_bottom_width.0, border.border_bottom_style),
                }
            },
            overflow -> Point<Overflow> = Point::new(
                *style.computed_values().get_overflow_x(),
                *style.computed_values().get_overflow_y(),
            ),
            box_sizing -> box_sizing::T = *style.computed_values().get_box_sizing(),
            direction -> direction::T = *style.inherited_values().get_direction(),
            container_type -> ContainerType =
                *style.computed_values().get_box().get_container_type(),
            containment -> Contain = {
                let box_style = style.computed_values().get_box();
                let container_type = style.container_type();
                let uses_containment_defaults = box_style.contain.is_empty()
                    && box_style.content_visibility == ContentVisibility::Visible
                    && container_type.is_normal();
                if uses_containment_defaults || style.display().is_contents() {
                    Contain::empty()
                } else {
                    effective_containment(
                        box_style.contain,
                        box_style.content_visibility,
                        style.skips_contents(),
                        container_type,
                    )
                }
            },
            contain_intrinsic_width -> ContainIntrinsicSize =
                style.computed_values().slow_clone_contain_intrinsic_width(),
            contain_intrinsic_height -> ContainIntrinsicSize =
                style.computed_values().slow_clone_contain_intrinsic_height(),
            skips_contents -> bool =
                *style.computed_values().get_content_visibility() == ContentVisibility::Hidden
                    && !style.display().is_contents(),

            gap -> Size<&NonNegativeLengthPercentageOrNormal> = {
                let position = style.computed_values().get_position();
                Size::new(&position.column_gap, &position.row_gap)
            },
            align_content -> ContentDistribution =
                style.computed_values().get_position().align_content,
            align_items -> ItemPlacement =
                style.computed_values().get_position().align_items,
            justify_content -> ContentDistribution =
                style.computed_values().get_position().justify_content,
            align_self -> SelfAlignment =
                style.computed_values().get_position().align_self,
            justify_self -> SelfAlignment =
                style.computed_values().get_position().justify_self,
            position_area -> PositionArea =
                style.computed_values().get_position().position_area,
            position_try_order -> PositionTryOrder =
                style.computed_values().get_position().position_try_order,
            names_position_anchor -> bool = !matches!(
                style.computed_values().get_position().position_anchor.value,
                PositionAnchorKeyword::Normal
                    | PositionAnchorKeyword::None
                    | PositionAnchorKeyword::Auto
            ),
            has_position_try_fallbacks -> bool =
                !style.computed_values().get_position().position_try_fallbacks.value.is_none(),
            order -> i32 = style.computed_values().get_position().order,
            reads_percentage_basis -> bool = box_model_reads_percentage_basis(style),
        }
    }
}

#[inline]
pub fn box_model_reads_percentage_basis(style: &impl CoreStyle) -> bool {
    let size = style.size();
    let min_size = style.min_size();
    let max_size = style.max_size();
    style_size_depends_on_basis(size.width)
        || style_size_depends_on_basis(size.height)
        || style_size_depends_on_basis(min_size.width)
        || style_size_depends_on_basis(min_size.height)
        || max_size_depends_on_basis(max_size.width)
        || max_size_depends_on_basis(max_size.height)
        || edges_depend_on_inline_basis(&style.margin(), &style.padding())
}

#[inline]
pub(crate) fn style_size_depends_on_basis(value: &StyleSize) -> bool {
    match value {
        StyleSize::LengthPercentage(lp) => lp.0.has_percentage(),
        StyleSize::FitContentFunction(limit) => limit.0.has_percentage(),
        StyleSize::AnchorSizeFunction(_) | StyleSize::AnchorContainingCalcFunction(_) => true,
        _ => false,
    }
}

#[inline]
pub(crate) fn max_size_depends_on_basis(value: &MaxSize) -> bool {
    match value {
        MaxSize::LengthPercentage(lp) => lp.0.has_percentage(),
        MaxSize::FitContentFunction(limit) => limit.0.has_percentage(),
        MaxSize::AnchorSizeFunction(_) | MaxSize::AnchorContainingCalcFunction(_) => true,
        _ => false,
    }
}

#[inline]
pub(crate) fn edges_depend_on_inline_basis(
    margin: &Edges<&Margin>,
    padding: &Edges<&NonNegativeLengthPercentage>,
) -> bool {
    let margin_depends = |value: &Margin| match value {
        Margin::LengthPercentage(lp) => lp.has_percentage(),
        Margin::Auto => false,
        Margin::AnchorSizeFunction(_) | Margin::AnchorContainingCalcFunction(_) => true,
    };
    margin_depends(margin.left)
        || margin_depends(margin.right)
        || margin_depends(margin.top)
        || margin_depends(margin.bottom)
        || padding.left.0.has_percentage()
        || padding.right.0.has_percentage()
        || padding.top.0.has_percentage()
        || padding.bottom.0.has_percentage()
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    #![allow(clippy::float_cmp)]

    use stylo::Zero;

    use super::*;

    #[derive(Debug)]
    struct Defaults;

    impl CoreStyle for Defaults {}

    #[derive(Debug)]
    struct Overrides;

    impl CoreStyle for Overrides {
        fn display(&self) -> Display {
            Display::Flex
        }

        fn order(&self) -> i32 {
            7
        }
    }

    #[test]
    fn defaults_are_the_fork_initial_values() {
        let style = Defaults;

        assert!(!style.display().is_none());
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
        assert!(style.container_type().is_normal());
        assert_eq!(style.contain_intrinsic_width(), ContainIntrinsicSize::None);
        assert_eq!(style.contain_intrinsic_height(), ContainIntrinsicSize::None);
        assert!(!style.skips_contents());

        assert!(matches!(
            style.gap().width,
            NonNegativeLengthPercentageOrNormal::Normal
        ));
        assert_eq!(style.align_content(), ContentDistribution::normal());
        assert_eq!(style.align_items(), ItemPlacement::normal());
        assert_eq!(style.justify_content(), ContentDistribution::normal());
        assert_eq!(style.align_self(), SelfAlignment::auto());
        assert_eq!(style.justify_self(), SelfAlignment::auto());
        assert!(style.position_area().is_none());
        assert!(style.position_try_order().is_normal());
        assert!(!style.names_position_anchor());
        assert_eq!(style.order(), 0);
    }

    #[test]
    fn reference_views_preserve_accessor_overrides() {
        let style = Overrides;
        let view = &style;
        assert_eq!(view.display(), Display::Flex);
        assert_eq!(view.order(), 7);
    }

    #[test]
    fn overflow_scroll_containers_follow_stylo_is_scrollable() {
        assert!(!Overflow::Visible.is_scrollable());
        assert!(Overflow::Hidden.is_scrollable());
    }

    #[derive(Debug)]
    struct BoxLess {
        display: Display,
    }

    impl CoreStyle for BoxLess {
        fn display(&self) -> Display {
            self.display
        }

        fn computed_values(&self) -> &ComputedValues {
            static CONTAINED: LazyLock<Arc<ComputedValues>> = LazyLock::new(|| {
                let mut values = ComputedValues::clone(initial_values());
                let box_style = values.mutate_box();
                box_style.contain = Contain::STRICT;
                box_style.content_visibility = ContentVisibility::Hidden;
                Arc::new(values)
            });
            &CONTAINED
        }
    }

    #[derive(Debug)]
    struct SizeQueryContainer;

    impl CoreStyle for SizeQueryContainer {
        fn container_type(&self) -> ContainerType {
            ContainerType::INLINE_SIZE
        }
    }

    #[test]
    fn a_size_query_container_is_contained_without_an_authored_contain() {
        let style = SizeQueryContainer;
        assert_eq!(
            style.containment(),
            Contain::LAYOUT | Contain::STYLE | Contain::INLINE_SIZE
        );
    }

    #[test]
    fn an_element_generating_no_box_claims_no_containment() {
        let contained = BoxLess {
            display: Display::Flex,
        };
        assert!(
            contained
                .containment()
                .contains(Contain::SIZE | Contain::LAYOUT)
        );
        assert!(contained.skips_contents());

        let box_less = BoxLess {
            display: Display::Contents,
        };
        assert_eq!(box_less.containment(), Contain::empty());
        assert!(!box_less.skips_contents());
    }
}
