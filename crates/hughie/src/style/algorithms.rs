//! The per-algorithm computed-style protocols.
//!
//! Each container algorithm reads a set of properties no other algorithm
//! reads. Keeping those sets on their own traits lets an entry point demand
//! exactly what it consumes, so `compute_linear_layout` cannot name a grid
//! property. The accessors a second algorithm shares — the box model, the
//! containment triple, the alignment accessors and `order` — stay on `CoreStyle`.

use stylo::computed_values::{
    direction, flex_direction, flex_wrap, linear_direction, relative_center, relative_layout_once,
};
use stylo::values::computed::lynx_layout::{RelativeAlign, RelativeReference};
use stylo::values::computed::{
    FlexBasis, GridAutoFlow, GridLine, GridTemplateComponent, ImplicitGridTracks, JustifyItems,
    NonNegativeNumber, SelfAlignment,
};

use crate::geometry::Edges;
use crate::style::{CoreStyle, lower_relative_logical};

style_protocol! {
    pub trait FlexboxStyle: CoreStyle {
        defaults(style) {
            flex_direction -> flex_direction::T =
                style.computed_values().clone_flex_direction(),
            flex_wrap -> flex_wrap::T = style.computed_values().clone_flex_wrap(),
            flex_basis -> &FlexBasis = &style.computed_values().get_position().flex_basis,
            flex_grow -> NonNegativeNumber =
                style.computed_values().get_position().flex_grow,
            flex_shrink -> NonNegativeNumber =
                style.computed_values().get_position().flex_shrink,
        }
    }
}

style_protocol! {
    pub trait GridStyle: CoreStyle {
        defaults(style) {
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
        }
    }
}

style_protocol! {
    pub trait LinearStyle: CoreStyle {
        defaults(style) {
            linear_direction -> linear_direction::T =
                style.computed_values().clone_linear_direction(),
            linear_weight_sum -> NonNegativeNumber =
                style.computed_values().clone_linear_weight_sum(),
            linear_weight -> NonNegativeNumber =
                style.computed_values().clone_linear_weight(),
        }
    }
}

style_protocol! {
    pub trait RelativeStyle: CoreStyle {
        defaults(style) {
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

    use stylo::values::specified::align::AlignFlags;

    use super::*;
    use crate::style::RELATIVE_REFERENCE_NONE;

    #[derive(Debug)]
    struct Defaults;

    impl CoreStyle for Defaults {}
    impl FlexboxStyle for Defaults {}
    impl GridStyle for Defaults {}
    impl LinearStyle for Defaults {}
    impl RelativeStyle for Defaults {}

    #[test]
    fn algorithm_defaults_are_the_fork_initial_values() {
        let style = Defaults;

        assert_eq!(style.flex_direction(), flex_direction::T::Row);
        assert_eq!(style.flex_wrap(), flex_wrap::T::Nowrap);
        assert_eq!(style.flex_basis(), &FlexBasis::auto());
        assert_eq!(style.flex_grow().0, 0.0);
        assert_eq!(style.flex_shrink().0, 1.0);

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
}
