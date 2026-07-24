//! The unified computed-style protocol: how every layout algorithm reads style.

use std::sync::LazyLock;

use stylo::properties::ComputedValues;
use stylo::properties::style_structs::Font;
use stylo::servo_arc::Arc;

use crate::geometry::{Edges, Point, Size};

/// Declares a source-backed style protocol and its zero-cost reference view.
///
/// The full forwarding implementation matters: cascade-less hosts commonly
/// return `&Style` from [`LayoutTree::style`](crate::tree::LayoutTree::style),
/// and forwarding only the computed-value source would discard any accessor
/// overrides on the underlying style.
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

pub mod containment;
pub mod text;

pub use containment::effective_containment;
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

pub const RELATIVE_REFERENCE_NONE: RelativeReference = -1;
pub const RELATIVE_REFERENCE_PARENT: RelativeReference = 0;

static INITIAL_VALUES: LazyLock<Arc<ComputedValues>> =
    LazyLock::new(|| ComputedValues::initial_values_with_font_override(Font::initial_values()));

#[inline]
pub(in crate::style) fn initial_values() -> &'static ComputedValues {
    &INITIAL_VALUES
}

#[inline]
fn lower_relative_logical(physical: RelativeReference, logical: RelativeReference) -> i32 {
    if physical == RELATIVE_REFERENCE_NONE {
        logical
    } else {
        physical
    }
}

// One borrowed view of all box-layout computed values. A Stylo-backed host
// only supplies `computed_values` and genuinely host-dependent lowering such
// as `position`; cascade-less hosts can still override individual accessors.
style_protocol! {
    pub trait CoreStyle: Sized {
        defaults(style) {
            computed_values -> &ComputedValues = initial_values(),
            inherited_values -> &ComputedValues = style.computed_values(),

            display -> Display = style.computed_values().clone_display(),
            visibility -> visibility::T = style.computed_values().clone_visibility(),
            position -> PositionProperty = style.computed_values().clone_position(),
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
            aspect_ratio -> AspectRatio = style.computed_values().clone_aspect_ratio(),
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
                style.computed_values().clone_overflow_x(),
                style.computed_values().clone_overflow_y(),
            ),
            box_sizing -> box_sizing::T = style.computed_values().clone_box_sizing(),
            direction -> direction::T = style.inherited_values().clone_direction(),
            containment -> Contain = effective_containment(
                style.computed_values().clone_contain(),
                style.computed_values().clone_content_visibility(),
                style.skips_contents(),
            ),
            contain_intrinsic_width -> ContainIntrinsicSize =
                style.computed_values().clone_contain_intrinsic_width(),
            contain_intrinsic_height -> ContainIntrinsicSize =
                style.computed_values().clone_contain_intrinsic_height(),
            skips_contents -> bool =
                style.computed_values().clone_content_visibility() == ContentVisibility::Hidden,

            flex_direction -> flex_direction::T =
                style.computed_values().clone_flex_direction(),
            flex_wrap -> flex_wrap::T = style.computed_values().clone_flex_wrap(),
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

            flex_basis -> &FlexBasis = &style.computed_values().get_position().flex_basis,
            flex_grow -> NonNegativeNumber =
                style.computed_values().get_position().flex_grow,
            flex_shrink -> NonNegativeNumber =
                style.computed_values().get_position().flex_shrink,
            align_self -> SelfAlignment =
                style.computed_values().get_position().align_self,
            order -> i32 = style.computed_values().get_position().order,

            grid_template_rows -> &GridTemplateComponent =
                &style.computed_values().get_position().grid_template_rows,
            grid_template_columns -> &GridTemplateComponent =
                &style.computed_values().get_position().grid_template_columns,
            grid_auto_rows -> &ImplicitGridTracks =
                &style.computed_values().get_position().grid_auto_rows,
            grid_auto_columns -> &ImplicitGridTracks =
                &style.computed_values().get_position().grid_auto_columns,
            grid_auto_flow -> GridAutoFlow =
                style.computed_values().get_position().grid_auto_flow,
            justify_items -> JustifyItems =
                style.computed_values().get_position().justify_items,
            grid_row_start -> &GridLine =
                &style.computed_values().get_position().grid_row_start,
            grid_row_end -> &GridLine =
                &style.computed_values().get_position().grid_row_end,
            grid_column_start -> &GridLine =
                &style.computed_values().get_position().grid_column_start,
            grid_column_end -> &GridLine =
                &style.computed_values().get_position().grid_column_end,
            justify_self -> SelfAlignment =
                style.computed_values().get_position().justify_self,

            linear_direction -> linear_direction::T =
                style.computed_values().clone_linear_direction(),
            linear_weight_sum -> NonNegativeNumber =
                style.computed_values().clone_linear_weight_sum(),
            linear_weight -> NonNegativeNumber =
                style.computed_values().clone_linear_weight(),

            relative_layout_once -> relative_layout_once::T =
                style.computed_values().clone_relative_layout_once(),
            relative_id -> RelativeReference =
                style.computed_values().clone_relative_id(),
            relative_align -> Edges<RelativeAlign> = {
                let values = style.computed_values();
                let (inline_start, inline_end) = (
                    values.clone_relative_align_inline_start(),
                    values.clone_relative_align_inline_end(),
                );
                let (logical_left, logical_right) =
                    if values.clone_direction() == direction::T::Ltr {
                        (inline_start, inline_end)
                    } else {
                        (inline_end, inline_start)
                    };
                Edges {
                    left: lower_relative_logical(
                        values.clone_relative_align_left(),
                        logical_left,
                    ),
                    right: lower_relative_logical(
                        values.clone_relative_align_right(),
                        logical_right,
                    ),
                    top: values.clone_relative_align_top(),
                    bottom: values.clone_relative_align_bottom(),
                }
            },
            relative_adjacent -> Edges<RelativeReference> = {
                let values = style.computed_values();
                let (inline_start, inline_end) = (
                    values.clone_relative_inline_start_of(),
                    values.clone_relative_inline_end_of(),
                );
                let (logical_left, logical_right) =
                    if values.clone_direction() == direction::T::Ltr {
                        (inline_start, inline_end)
                    } else {
                        (inline_end, inline_start)
                    };
                Edges {
                    left: lower_relative_logical(values.clone_relative_left_of(), logical_left),
                    right: lower_relative_logical(
                        values.clone_relative_right_of(),
                        logical_right,
                    ),
                    top: values.clone_relative_top_of(),
                    bottom: values.clone_relative_bottom_of(),
                }
            },
            relative_center -> relative_center::T =
                style.computed_values().clone_relative_center(),
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    #![allow(clippy::float_cmp)]

    use stylo::Zero;
    use stylo::values::specified::align::AlignFlags;

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

        assert_eq!(style.flex_direction(), flex_direction::T::Row);
        assert_eq!(style.flex_wrap(), flex_wrap::T::Nowrap);
        assert!(matches!(
            style.gap().width,
            NonNegativeLengthPercentageOrNormal::Normal
        ));
        assert_eq!(style.align_content(), ContentDistribution::normal());
        assert_eq!(style.align_items(), ItemPlacement::normal());
        assert_eq!(style.justify_content(), ContentDistribution::normal());
        assert_eq!(style.flex_basis(), &FlexBasis::auto());
        assert_eq!(style.flex_grow().0, 0.0);
        assert_eq!(style.flex_shrink().0, 1.0);
        assert_eq!(style.align_self(), SelfAlignment::auto());
        assert_eq!(style.order(), 0);

        assert!(matches!(
            style.grid_template_rows(),
            GridTemplateComponent::None
        ));
        assert!(style.grid_auto_rows().0.is_empty());
        assert_eq!(style.grid_auto_flow(), GridAutoFlow::ROW);
        assert_eq!(
            style.justify_items().computed.0.0.value(),
            AlignFlags::NORMAL
        );
        assert!(style.grid_row_start().is_auto());
        assert_eq!(style.justify_self(), SelfAlignment::auto());

        assert_eq!(style.linear_direction(), linear_direction::T::Column);
        assert_eq!(style.linear_weight_sum().0, 0.0);
        assert_eq!(style.linear_weight().0, 0.0);
        assert_eq!(style.relative_layout_once(), relative_layout_once::T::True);
        assert_eq!(style.relative_id(), RELATIVE_REFERENCE_NONE);
        assert_eq!(
            style.relative_align(),
            Edges::uniform(RELATIVE_REFERENCE_NONE)
        );
        assert_eq!(
            style.relative_adjacent(),
            Edges::uniform(RELATIVE_REFERENCE_NONE)
        );
        assert_eq!(style.relative_center(), relative_center::T::None);
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
}
