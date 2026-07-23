//! Grid style protocol (CSS Grid Layout Module Level 2, minus subgrid).

use std::sync::LazyLock;

use stylo::values::computed::length::NonNegativeLengthPercentageOrNormal;
use stylo::values::computed::{
    ContentDistribution, GridAutoFlow, GridLine, GridTemplateComponent, ImplicitGridTracks,
    ItemPlacement, JustifyItems, SelfAlignment,
};

use crate::geometry::Size;
use crate::style::CoreStyle;

static GRID_LINE_AUTO: LazyLock<GridLine> = LazyLock::new(GridLine::auto);
static GAP_NORMAL: NonNegativeLengthPercentageOrNormal =
    NonNegativeLengthPercentageOrNormal::Normal;

pub trait GridContainerStyle: CoreStyle {
    fn grid_template_rows(&self) -> &GridTemplateComponent;

    fn grid_template_columns(&self) -> &GridTemplateComponent;

    fn grid_auto_rows(&self) -> &ImplicitGridTracks;

    fn grid_auto_columns(&self) -> &ImplicitGridTracks;

    fn grid_auto_flow(&self) -> GridAutoFlow {
        GridAutoFlow::ROW
    }

    fn gap(&self) -> Size<&NonNegativeLengthPercentageOrNormal> {
        Size::new(&GAP_NORMAL, &GAP_NORMAL)
    }

    fn align_content(&self) -> ContentDistribution {
        ContentDistribution::normal()
    }

    fn justify_content(&self) -> ContentDistribution {
        ContentDistribution::normal()
    }

    fn align_items(&self) -> ItemPlacement {
        ItemPlacement::normal()
    }

    fn justify_items(&self) -> JustifyItems {
        let specified = stylo::values::specified::align::JustifyItems(ItemPlacement::normal());
        JustifyItems {
            specified,
            computed: specified,
        }
    }
}

pub trait GridItemStyle: CoreStyle {
    fn grid_row_start(&self) -> &GridLine {
        &GRID_LINE_AUTO
    }

    fn grid_row_end(&self) -> &GridLine {
        &GRID_LINE_AUTO
    }

    fn grid_column_start(&self) -> &GridLine {
        &GRID_LINE_AUTO
    }

    fn grid_column_end(&self) -> &GridLine {
        &GRID_LINE_AUTO
    }

    fn align_self(&self) -> SelfAlignment {
        SelfAlignment::auto()
    }

    fn justify_self(&self) -> SelfAlignment {
        SelfAlignment::auto()
    }

    fn order(&self) -> i32 {
        0
    }
}

impl<S: GridContainerStyle> GridContainerStyle for &S {
    fn grid_template_rows(&self) -> &GridTemplateComponent {
        (**self).grid_template_rows()
    }

    fn grid_template_columns(&self) -> &GridTemplateComponent {
        (**self).grid_template_columns()
    }

    fn grid_auto_rows(&self) -> &ImplicitGridTracks {
        (**self).grid_auto_rows()
    }

    fn grid_auto_columns(&self) -> &ImplicitGridTracks {
        (**self).grid_auto_columns()
    }

    fn grid_auto_flow(&self) -> GridAutoFlow {
        (**self).grid_auto_flow()
    }

    fn gap(&self) -> Size<&NonNegativeLengthPercentageOrNormal> {
        (**self).gap()
    }

    fn align_content(&self) -> ContentDistribution {
        (**self).align_content()
    }

    fn justify_content(&self) -> ContentDistribution {
        (**self).justify_content()
    }

    fn align_items(&self) -> ItemPlacement {
        (**self).align_items()
    }

    fn justify_items(&self) -> JustifyItems {
        (**self).justify_items()
    }
}

impl<S: GridItemStyle> GridItemStyle for &S {
    fn grid_row_start(&self) -> &GridLine {
        (**self).grid_row_start()
    }

    fn grid_row_end(&self) -> &GridLine {
        (**self).grid_row_end()
    }

    fn grid_column_start(&self) -> &GridLine {
        (**self).grid_column_start()
    }

    fn grid_column_end(&self) -> &GridLine {
        (**self).grid_column_end()
    }

    fn align_self(&self) -> SelfAlignment {
        (**self).align_self()
    }

    fn justify_self(&self) -> SelfAlignment {
        (**self).justify_self()
    }

    fn order(&self) -> i32 {
        (**self).order()
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use stylo::values::computed::Display;
    use stylo::values::specified::align::AlignFlags;

    use super::*;

    #[derive(Debug)]
    struct Defaults {
        template: GridTemplateComponent,
        auto_tracks: ImplicitGridTracks,
    }

    impl Default for Defaults {
        fn default() -> Self {
            Self {
                template: GridTemplateComponent::None,
                auto_tracks: stylo::values::generics::grid::ImplicitGridTracks(Vec::new().into()),
            }
        }
    }

    impl CoreStyle for Defaults {
        fn display(&self) -> Display {
            Display::Grid
        }
    }

    impl GridContainerStyle for Defaults {
        fn grid_template_rows(&self) -> &GridTemplateComponent {
            &self.template
        }

        fn grid_template_columns(&self) -> &GridTemplateComponent {
            &self.template
        }

        fn grid_auto_rows(&self) -> &ImplicitGridTracks {
            &self.auto_tracks
        }

        fn grid_auto_columns(&self) -> &ImplicitGridTracks {
            &self.auto_tracks
        }
    }

    impl GridItemStyle for Defaults {}

    #[test]
    fn grid_container_defaults_are_css_initial_values() {
        let style = Defaults::default();

        assert!(matches!(
            style.grid_template_rows(),
            GridTemplateComponent::None
        ));
        assert!(matches!(
            style.grid_template_columns(),
            GridTemplateComponent::None
        ));
        assert!(style.grid_auto_rows().0.is_empty());
        assert!(style.grid_auto_columns().0.is_empty());
        assert_eq!(style.grid_auto_flow(), GridAutoFlow::ROW);
        assert!(!style.grid_auto_flow().contains(GridAutoFlow::DENSE));
        let gap = GridContainerStyle::gap(&style);
        assert!(matches!(
            gap.width,
            NonNegativeLengthPercentageOrNormal::Normal
        ));
        assert!(matches!(
            gap.height,
            NonNegativeLengthPercentageOrNormal::Normal
        ));
        assert_eq!(
            GridContainerStyle::align_content(&style),
            ContentDistribution::normal()
        );
        assert_eq!(
            GridContainerStyle::justify_content(&style),
            ContentDistribution::normal()
        );
        assert_eq!(
            GridContainerStyle::align_items(&style),
            ItemPlacement::normal()
        );
        assert_eq!(
            style.justify_items().computed.0.0.value(),
            AlignFlags::NORMAL
        );
    }

    #[test]
    fn grid_item_defaults_are_automatic_placements() {
        let style = Defaults::default();

        assert!(style.grid_row_start().is_auto());
        assert!(style.grid_row_end().is_auto());
        assert!(style.grid_column_start().is_auto());
        assert!(style.grid_column_end().is_auto());
        assert_eq!(GridItemStyle::align_self(&style), SelfAlignment::auto());
        assert_eq!(style.justify_self(), SelfAlignment::auto());
        assert_eq!(style.order(), 0);
    }
}
