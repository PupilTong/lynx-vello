//! Grid style protocol (CSS Grid Layout Module Level 2, minus subgrid).
//!
//! Track lists are the one place a style value is a *sequence*, so this is
//! where the protocol works hardest to stay allocation-free and `dyn`-free:
//! [`GridContainerStyle`] exposes template/auto track lists as **GAT
//! iterators** borrowed from the style view, and `repeat(...)` groups as a
//! nested [`GridTemplateRepetition`] value. Hosts adapt whatever their style
//! engine stores (stylo's `GenericGridTemplateComponent`, a plain `Vec`, …)
//! without materializing engine-side copies; the algorithm collects into its
//! own scratch exactly once, after `repeat()` expansion.
//!
//! # Numeric lines only
//!
//! Placements are numeric lines and spans. Named lines, named areas
//! (`grid-template-areas`), and `subgrid` are **not protocol**: name→number
//! resolution is a style-system concern and must be done host-side. (Lynx
//! never implemented named lines/areas, so the lynx-vello host needs no such
//! resolution; a browser-grade host would do it in its style adapter.)

use crate::geometry::{Line, Size};
use crate::style::CoreStyle;
use crate::style::alignment::{
    AlignContent, AlignItems, AlignSelf, JustifyContent, JustifyItems, JustifySelf,
};
use crate::style::value::LengthPercentage;

/// `grid-auto-flow`: the axis and packing mode of the auto-placement
/// algorithm (CSS Grid §8.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum GridAutoFlow {
    /// Fill each row in turn, adding new rows as needed (sparse packing).
    #[default]
    Row,
    /// Fill each column in turn, adding new columns as needed (sparse).
    Column,
    /// Row flow with dense backfilling of earlier holes.
    RowDense,
    /// Column flow with dense backfilling of earlier holes.
    ColumnDense,
}

impl GridAutoFlow {
    /// Is this a `dense` packing mode?
    #[must_use]
    pub const fn is_dense(self) -> bool {
        matches!(self, Self::RowDense | Self::ColumnDense)
    }

    /// Is the primary placement axis the row axis (items fill rows)?
    #[must_use]
    pub const fn is_row_flow(self) -> bool {
        matches!(self, Self::Row | Self::RowDense)
    }
}

/// A 1-based grid line number.
///
/// Positive counts from the start (line 1 is the start edge), negative from
/// the end (line -1 is the end edge). `0` is invalid per CSS grammar — hosts
/// must never produce it; the engine treats a placement carrying it as
/// [`GridPlacement::Auto`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct GridLine(i16);

impl GridLine {
    /// Wraps a 1-based (possibly negative) line number.
    #[must_use]
    pub const fn new(line: i16) -> Self {
        Self(line)
    }

    /// The raw 1-based line number.
    #[must_use]
    pub const fn as_i16(self) -> i16 {
        self.0
    }
}

/// One side of a `grid-row` / `grid-column` placement.
///
/// A full placement is a [`Line<GridPlacement>`]; resolution of the
/// start/end/span combinations is CSS Grid §8.3 (the "grid placement
/// conflict handling" rules) and happens inside the algorithm.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum GridPlacement {
    /// `auto`: placed by the auto-placement algorithm.
    #[default]
    Auto,
    /// A specific line.
    Line(GridLine),
    /// `span <n>` relative to the opposite side. `n` is clamped to ≥ 1 by
    /// the algorithm (CSS treats `span 0` as invalid).
    Span(u16),
}

/// The repetition count of a `repeat(...)` in a template track list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RepetitionCount {
    /// `repeat(<n>, …)` — a fixed count.
    Count(u16),
    /// `repeat(auto-fill, …)` — as many tracks as fit the definite axis size.
    AutoFill,
    /// `repeat(auto-fit, …)` — like `auto-fill`, then collapse empty tracks.
    AutoFit,
}

/// The *minimum* half of a track sizing function (CSS Grid §7.2).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum MinTrackSizingFunction {
    /// A fixed `<length-percentage>` breadth.
    Fixed(LengthPercentage),
    /// The `min-content` intrinsic size of the track's items.
    MinContent,
    /// The `max-content` intrinsic size of the track's items.
    MaxContent,
    /// `auto`: largest item minimum, growable.
    #[default]
    Auto,
}

/// The *maximum* half of a track sizing function (CSS Grid §7.2).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum MaxTrackSizingFunction {
    /// A fixed `<length-percentage>` breadth.
    Fixed(LengthPercentage),
    /// The `min-content` intrinsic size of the track's items.
    MinContent,
    /// The `max-content` intrinsic size of the track's items.
    MaxContent,
    /// `auto`: `max-content`, but stretchable by `align/justify-content`.
    #[default]
    Auto,
    /// `<flex>` (`fr`) — a share of the leftover space.
    Fr(f32),
    /// `fit-content(<length-percentage>)`.
    FitContent(LengthPercentage),
}

/// A full track sizing function: `minmax(min, max)`, or the single-value
/// forms which set both halves (per CSS Grid §7.2's expansion rules).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct TrackSizingFunction {
    /// The minimum sizing function.
    pub min: MinTrackSizingFunction,
    /// The maximum sizing function.
    pub max: MaxTrackSizingFunction,
}

impl TrackSizingFunction {
    /// `auto` (i.e. `minmax(auto, auto)`).
    pub const AUTO: Self = Self {
        min: MinTrackSizingFunction::Auto,
        max: MaxTrackSizingFunction::Auto,
    };

    /// An explicit `minmax(min, max)`.
    #[must_use]
    pub const fn minmax(min: MinTrackSizingFunction, max: MaxTrackSizingFunction) -> Self {
        Self { min, max }
    }

    /// A fixed breadth: `minmax(fixed, fixed)`.
    #[must_use]
    pub const fn fixed(breadth: LengthPercentage) -> Self {
        Self {
            min: MinTrackSizingFunction::Fixed(breadth),
            max: MaxTrackSizingFunction::Fixed(breadth),
        }
    }

    /// `<flex>` single-value form: `minmax(auto, <flex>)` per spec.
    #[must_use]
    pub const fn fr(flex: f32) -> Self {
        Self {
            min: MinTrackSizingFunction::Auto,
            max: MaxTrackSizingFunction::Fr(flex),
        }
    }

    /// `fit-content(limit)`: `minmax(auto, fit-content(limit))` per spec.
    #[must_use]
    pub const fn fit_content(limit: LengthPercentage) -> Self {
        Self {
            min: MinTrackSizingFunction::Auto,
            max: MaxTrackSizingFunction::FitContent(limit),
        }
    }
}

/// One `repeat(count, <track list>)` group inside a template track list,
/// borrowed from the host's style storage.
///
/// The track iterator must be `Clone` (the algorithm iterates a repetition
/// once per expansion) and `ExactSizeIterator` (auto-fill/auto-fit need the
/// per-repetition track count up front to solve the repetition count).
pub trait GridTemplateRepetition {
    /// Borrowed iterator over the tracks inside this repetition.
    type Tracks<'a>: Iterator<Item = TrackSizingFunction> + ExactSizeIterator + Clone
    where
        Self: 'a;

    /// How many times the group repeats.
    fn count(&self) -> RepetitionCount;

    /// The tracks repeated by this group.
    fn tracks(&self) -> Self::Tracks<'_>;
}

/// One component of a `grid-template-rows`/`grid-template-columns` list:
/// either a single track or a `repeat(...)` group.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum GridTemplateComponent<R> {
    /// A single track sizing function.
    Single(TrackSizingFunction),
    /// A `repeat(...)` group (a borrowed host value implementing
    /// [`GridTemplateRepetition`]).
    Repeat(R),
}

/// Style of a node *as a grid container*.
///
/// Track-list accessors have no defaults (a default would have to conjure an
/// iterator type); everything else defaults to the CSS initial value. An
/// empty template iterator means "no explicit tracks in this axis".
pub trait GridContainerStyle: CoreStyle {
    /// The borrowed `repeat(...)` view yielded by template track lists.
    type Repetition<'a>: GridTemplateRepetition
    where
        Self: 'a;

    /// Borrowed iterator over a template track list
    /// (`grid-template-rows`/`-columns`).
    type TemplateTracks<'a>: Iterator<Item = GridTemplateComponent<Self::Repetition<'a>>>
    where
        Self: 'a;

    /// Borrowed iterator over an auto track list
    /// (`grid-auto-rows`/`-columns`).
    type AutoTracks<'a>: Iterator<Item = TrackSizingFunction> + Clone
    where
        Self: 'a;

    /// `grid-template-rows`.
    fn grid_template_rows(&self) -> Self::TemplateTracks<'_>;

    /// `grid-template-columns`.
    fn grid_template_columns(&self) -> Self::TemplateTracks<'_>;

    /// `grid-auto-rows` — sizing for implicitly-created rows. Cycled if more
    /// implicit tracks exist than entries (CSS Grid §7.6); empty means
    /// `auto`.
    fn grid_auto_rows(&self) -> Self::AutoTracks<'_>;

    /// `grid-auto-columns` — sizing for implicitly-created columns.
    fn grid_auto_columns(&self) -> Self::AutoTracks<'_>;

    /// `grid-auto-flow`.
    fn grid_auto_flow(&self) -> GridAutoFlow {
        GridAutoFlow::Row
    }

    /// `gap` (`column-gap` is `width`, `row-gap` is `height`).
    ///
    /// Percentage basis: the container's content-box size in the gap's axis.
    fn gap(&self) -> Size<LengthPercentage> {
        Size::new(LengthPercentage::ZERO, LengthPercentage::ZERO)
    }

    /// `align-content` — block-axis distribution of tracks. `None` =
    /// `normal`.
    fn align_content(&self) -> Option<AlignContent> {
        None
    }

    /// `justify-content` — inline-axis distribution of tracks. `None` =
    /// `normal`.
    fn justify_content(&self) -> Option<JustifyContent> {
        None
    }

    /// `align-items` — default block-axis alignment of items. `None` =
    /// `normal`.
    fn align_items(&self) -> Option<AlignItems> {
        None
    }

    /// `justify-items` — default inline-axis alignment of items. `None` =
    /// `normal`.
    fn justify_items(&self) -> Option<JustifyItems> {
        None
    }
}

/// Style of a node *as a grid item*.
///
/// Defaults are the CSS initial values.
pub trait GridItemStyle: CoreStyle {
    /// `grid-row` (`grid-row-start` / `grid-row-end`).
    fn grid_row(&self) -> Line<GridPlacement> {
        Line::new(GridPlacement::Auto, GridPlacement::Auto)
    }

    /// `grid-column` (`grid-column-start` / `grid-column-end`).
    fn grid_column(&self) -> Line<GridPlacement> {
        Line::new(GridPlacement::Auto, GridPlacement::Auto)
    }

    /// `align-self`. `None` = `auto` (defer to the container's
    /// `align-items`).
    fn align_self(&self) -> Option<AlignSelf> {
        None
    }

    /// `justify-self`. `None` = `auto` (defer to the container's
    /// `justify-items`).
    fn justify_self(&self) -> Option<JustifySelf> {
        None
    }

    /// `order` — layout/paint reordering among siblings; lower comes first.
    fn order(&self) -> i32 {
        0
    }
}

impl<R: GridTemplateRepetition> GridTemplateRepetition for &R {
    type Tracks<'a>
        = R::Tracks<'a>
    where
        Self: 'a;

    fn count(&self) -> RepetitionCount {
        (**self).count()
    }

    fn tracks(&self) -> Self::Tracks<'_> {
        (**self).tracks()
    }
}

impl<S: GridContainerStyle> GridContainerStyle for &S {
    type Repetition<'a>
        = S::Repetition<'a>
    where
        Self: 'a;
    type TemplateTracks<'a>
        = S::TemplateTracks<'a>
    where
        Self: 'a;
    type AutoTracks<'a>
        = S::AutoTracks<'a>
    where
        Self: 'a;

    fn grid_template_rows(&self) -> Self::TemplateTracks<'_> {
        S::grid_template_rows(&**self)
    }

    fn grid_template_columns(&self) -> Self::TemplateTracks<'_> {
        S::grid_template_columns(&**self)
    }

    fn grid_auto_rows(&self) -> Self::AutoTracks<'_> {
        S::grid_auto_rows(&**self)
    }

    fn grid_auto_columns(&self) -> Self::AutoTracks<'_> {
        S::grid_auto_columns(&**self)
    }

    fn grid_auto_flow(&self) -> GridAutoFlow {
        (**self).grid_auto_flow()
    }

    fn gap(&self) -> Size<LengthPercentage> {
        (**self).gap()
    }

    fn align_content(&self) -> Option<AlignContent> {
        (**self).align_content()
    }

    fn justify_content(&self) -> Option<JustifyContent> {
        (**self).justify_content()
    }

    fn align_items(&self) -> Option<AlignItems> {
        (**self).align_items()
    }

    fn justify_items(&self) -> Option<JustifyItems> {
        (**self).justify_items()
    }
}

impl<S: GridItemStyle> GridItemStyle for &S {
    fn grid_row(&self) -> Line<GridPlacement> {
        (**self).grid_row()
    }

    fn grid_column(&self) -> Line<GridPlacement> {
        (**self).grid_column()
    }

    fn align_self(&self) -> Option<AlignSelf> {
        (**self).align_self()
    }

    fn justify_self(&self) -> Option<JustifySelf> {
        (**self).justify_self()
    }

    fn order(&self) -> i32 {
        (**self).order()
    }
}
