//! Grid style protocol (CSS Grid Layout Module Level 2, minus subgrid).
//!
//! Track lists are the one place a style value is a *sequence*, so this is
//! where the protocol leans hardest on borrowing: [`GridContainerStyle`]
//! exposes `grid-template-*` and `grid-auto-*` as **borrowed stylo values**
//! (`&GridTemplateComponent`, `&ImplicitGridTracks`) lent from the style
//! view. A stylo-backed host returns references straight into its
//! `ComputedValues`; the algorithm expands `repeat()` groups into its own
//! scratch exactly once. Bind the style view first (`let style =
//! node.style();`) before borrowing — the discipline documented in
//! [`tree`](crate::tree).
//!
//! # Numeric lines only
//!
//! Placements are stylo [`GridLine`] values consumed numerically:
//! `line_num == 0` means the number is absent (`auto` unless `is_span`), and
//! line *names* — in placements and in template line-name lists — are
//! ignored. Named lines, named areas (`grid-template-areas`), and `subgrid`
//! are **not protocol**: name→number resolution is a host concern. Lynx never
//! implemented named lines/areas, so the lynx-vello adapter needs no such
//! resolution.

use stylo::values::computed::length::NonNegativeLengthPercentageOrNormal;
use stylo::values::computed::{
    ContentDistribution, GridAutoFlow, GridLine, GridTemplateComponent, ImplicitGridTracks,
    ItemPlacement, JustifyItems, SelfAlignment,
};

use crate::geometry::Size;
use crate::style::CoreStyle;

/// Style of a node *as a grid container*.
///
/// The borrowed track-list accessors have no defaults (they lend host
/// storage); everything else defaults to the CSS initial value.
/// [`GridTemplateComponent::None`] means "no explicit tracks in this axis",
/// and an empty [`ImplicitGridTracks`] means `auto`.
pub trait GridContainerStyle: CoreStyle {
    /// `grid-template-rows`.
    fn grid_template_rows(&self) -> &GridTemplateComponent;

    /// `grid-template-columns`.
    fn grid_template_columns(&self) -> &GridTemplateComponent;

    /// `grid-auto-rows` — sizing for implicitly-created rows. Cycled if more
    /// implicit tracks exist than entries (CSS Grid §7.6); empty means
    /// `auto`.
    fn grid_auto_rows(&self) -> &ImplicitGridTracks;

    /// `grid-auto-columns` — sizing for implicitly-created columns.
    fn grid_auto_columns(&self) -> &ImplicitGridTracks;

    /// `grid-auto-flow` (bitflags: `ROW`/`COLUMN` axis plus `DENSE`
    /// backfilling, CSS Grid §8.5).
    fn grid_auto_flow(&self) -> GridAutoFlow {
        GridAutoFlow::ROW
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

    /// `align-content` — block-axis distribution of tracks.
    fn align_content(&self) -> ContentDistribution {
        ContentDistribution::normal()
    }

    /// `justify-content` — inline-axis distribution of tracks.
    fn justify_content(&self) -> ContentDistribution {
        ContentDistribution::normal()
    }

    /// `align-items` — default block-axis alignment of items.
    fn align_items(&self) -> ItemPlacement {
        ItemPlacement::normal()
    }

    /// `justify-items` — default inline-axis alignment of items.
    fn justify_items(&self) -> JustifyItems {
        let specified = stylo::values::specified::align::JustifyItems(ItemPlacement::normal());
        JustifyItems {
            specified,
            computed: specified,
        }
    }
}

/// Style of a node *as a grid item*.
///
/// Defaults are the CSS initial values.
pub trait GridItemStyle: CoreStyle {
    /// `grid-row-start`.
    fn grid_row_start(&self) -> GridLine {
        GridLine::auto()
    }

    /// `grid-row-end`.
    fn grid_row_end(&self) -> GridLine {
        GridLine::auto()
    }

    /// `grid-column-start`.
    fn grid_column_start(&self) -> GridLine {
        GridLine::auto()
    }

    /// `grid-column-end`.
    fn grid_column_end(&self) -> GridLine {
        GridLine::auto()
    }

    /// `align-self`. `auto` defers to the container's `align-items`.
    fn align_self(&self) -> SelfAlignment {
        SelfAlignment::auto()
    }

    /// `justify-self`. `auto` defers to the container's `justify-items`.
    fn justify_self(&self) -> SelfAlignment {
        SelfAlignment::auto()
    }

    /// `order` — layout/paint reordering among siblings; lower comes first.
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

    fn gap(&self) -> Size<NonNegativeLengthPercentageOrNormal> {
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
    fn grid_row_start(&self) -> GridLine {
        (**self).grid_row_start()
    }

    fn grid_row_end(&self) -> GridLine {
        (**self).grid_row_end()
    }

    fn grid_column_start(&self) -> GridLine {
        (**self).grid_column_start()
    }

    fn grid_column_end(&self) -> GridLine {
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
