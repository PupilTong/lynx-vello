//! Shared styling-engine-free host for neutron-star integration tests and benchmarks.
//!
//! The host speaks the engine's stylo vocabulary directly: `TestStyle` fields
//! are stylo computed values, and the constructor helpers below absorb the
//! fixture-site boilerplate of building them (`px`/`pct`/`calc_lp`,
//! `size_*`/`max_*`, `margin_*`/`inset_*`, grid track builders, and the
//! alignment wrappers over `AlignFlags`).

// Every integration-test or benchmark target includes this module separately, and each target
// intentionally uses only a subset of the helpers.
#![allow(dead_code)]

use std::cell::{Cell, RefCell};
use std::fmt;

use neutron_star::cache::Cache;
use neutron_star::compute::{
    FnLeafMeasurer, LeafMeasureInput, LeafMetrics, compute_cached_layout, compute_flexbox_layout,
    compute_grid_layout, compute_leaf_layout, compute_linear_layout, compute_relative_layout,
    hide_subtree,
};
use neutron_star::prelude::*;
use style_traits::values::specified::AllowedNumericType;
use stylo::computed_values::{
    box_sizing, direction, flex_direction, flex_wrap, linear_direction, relative_center,
    relative_layout_once, visibility,
};
use stylo::values::computed::length::NonNegativeLengthPercentageOrNormal;
use stylo::values::computed::length_percentage::{CalcNode, ComputedLeaf};
use stylo::values::computed::lynx_layout::{RelativeAlign, RelativeReference};
use stylo::values::computed::{
    AspectRatio, Au, BorderSideWidth, ContentDistribution, Display, FlexBasis, GridAutoFlow,
    GridLine, GridTemplateComponent, ImplicitGridTracks, Inset, ItemPlacement, JustifyItems,
    Length, LengthPercentage, Margin, MaxSize, NonNegativeLengthPercentage, NonNegativeNumber,
    Overflow, Percentage, PositionProperty, Ratio, SelfAlignment, Size as StyleSize,
};
use stylo::values::generics::position::PreferredRatio;
use stylo::values::generics::{NonNegative, grid as generic_grid};
use stylo::values::specified::align::AlignFlags;

// ---------------------------------------------------------------------------
// Stylo computed-value constructor helpers
// ---------------------------------------------------------------------------

/// `<length>` in CSS pixels.
pub(super) fn px(value: f32) -> LengthPercentage {
    LengthPercentage::new_length(Length::new(value))
}

/// `<percentage>`; `0.5` is `50%`.
pub(super) fn pct(fraction: f32) -> LengthPercentage {
    LengthPercentage::new_percent(Percentage(fraction))
}

/// `calc(<length> + <percentage>)`; `percentage` is a fraction, so `0.5`
/// represents `50%`. Replaces the deleted host calc-handle arena.
pub(super) fn calc_lp(length: f32, percentage: f32) -> LengthPercentage {
    LengthPercentage::new_calc(
        CalcNode::Sum(
            vec![
                CalcNode::Leaf(ComputedLeaf::Length(Length::new(length))),
                CalcNode::Leaf(ComputedLeaf::Percentage(Percentage(percentage))),
            ]
            .into(),
        ),
        AllowedNumericType::All,
    )
}

/// Non-negative `<length>` (padding, gap inner value).
pub(super) fn npx(value: f32) -> NonNegativeLengthPercentage {
    NonNegative(px(value))
}

/// Non-negative `<percentage>`.
pub(super) fn npct(fraction: f32) -> NonNegativeLengthPercentage {
    NonNegative(pct(fraction))
}

/// Non-negative `calc()`.
pub(super) fn ncalc(length: f32, percentage: f32) -> NonNegativeLengthPercentage {
    NonNegative(calc_lp(length, percentage))
}

/// Non-negative `<number>` (flex factors, linear weights).
pub(super) fn nn(value: f32) -> NonNegativeNumber {
    NonNegativeNumber::from(value)
}

/// `width`/`height`/`min-*`: `auto`.
pub(super) fn size_auto() -> StyleSize {
    StyleSize::auto()
}

/// `width`/`height`/`min-*`: `<length>`.
pub(super) fn size_px(value: f32) -> StyleSize {
    StyleSize::LengthPercentage(npx(value))
}

/// `width`/`height`/`min-*`: `<percentage>`.
pub(super) fn size_pct(fraction: f32) -> StyleSize {
    StyleSize::LengthPercentage(npct(fraction))
}

/// `width`/`height`/`min-*`: `calc()`.
pub(super) fn size_calc(length: f32, percentage: f32) -> StyleSize {
    StyleSize::LengthPercentage(ncalc(length, percentage))
}

/// `width`/`height`/`min-*`: `min-content`.
pub(super) fn size_min_content() -> StyleSize {
    StyleSize::MinContent
}

/// `width`/`height`/`min-*`: `max-content`.
pub(super) fn size_max_content() -> StyleSize {
    StyleSize::MaxContent
}

/// `width`/`height`/`min-*`: `fit-content(<length>)`.
pub(super) fn size_fit_content_px(value: f32) -> StyleSize {
    StyleSize::FitContentFunction(npx(value))
}

/// `width`/`height`/`min-*`: `fit-content(<percentage>)`.
pub(super) fn size_fit_content_pct(fraction: f32) -> StyleSize {
    StyleSize::FitContentFunction(npct(fraction))
}

/// `max-width`/`max-height`: `none`.
pub(super) fn max_none() -> MaxSize {
    MaxSize::none()
}

/// `max-width`/`max-height`: `<length>`.
pub(super) fn max_px(value: f32) -> MaxSize {
    MaxSize::LengthPercentage(npx(value))
}

/// `max-width`/`max-height`: `<percentage>`.
pub(super) fn max_pct(fraction: f32) -> MaxSize {
    MaxSize::LengthPercentage(npct(fraction))
}

/// `max-width`/`max-height`: `calc()`.
pub(super) fn max_calc(length: f32, percentage: f32) -> MaxSize {
    MaxSize::LengthPercentage(ncalc(length, percentage))
}

/// `max-width`/`max-height`: `min-content`.
pub(super) fn max_min_content() -> MaxSize {
    MaxSize::MinContent
}

/// `max-width`/`max-height`: `max-content`.
pub(super) fn max_max_content() -> MaxSize {
    MaxSize::MaxContent
}

/// `max-width`/`max-height`: `fit-content(<length>)`.
pub(super) fn max_fit_content_px(value: f32) -> MaxSize {
    MaxSize::FitContentFunction(npx(value))
}

/// `margin-*`: `<length>`.
pub(super) fn margin_px(value: f32) -> Margin {
    Margin::LengthPercentage(px(value))
}

/// `margin-*`: `<percentage>`.
pub(super) fn margin_pct(fraction: f32) -> Margin {
    Margin::LengthPercentage(pct(fraction))
}

/// `margin-*`: `calc()`.
pub(super) fn margin_calc(length: f32, percentage: f32) -> Margin {
    Margin::LengthPercentage(calc_lp(length, percentage))
}

/// `margin-*`: `auto`.
pub(super) fn margin_auto() -> Margin {
    Margin::Auto
}

/// `top`/`right`/`bottom`/`left`: `auto`.
pub(super) fn inset_auto() -> Inset {
    Inset::Auto
}

/// `top`/`right`/`bottom`/`left`: `<length>`.
pub(super) fn inset_px(value: f32) -> Inset {
    Inset::LengthPercentage(px(value))
}

/// `top`/`right`/`bottom`/`left`: `<percentage>`.
pub(super) fn inset_pct(fraction: f32) -> Inset {
    Inset::LengthPercentage(pct(fraction))
}

/// A used border width in CSS pixels.
pub(super) fn border_px(value: f32) -> BorderSideWidth {
    BorderSideWidth(Au::from_f32_px(value))
}

/// `flex-basis: auto`.
pub(super) fn basis_auto() -> FlexBasis {
    FlexBasis::auto()
}

/// `flex-basis: <length>`.
pub(super) fn basis_px(value: f32) -> FlexBasis {
    FlexBasis::Size(size_px(value))
}

/// `flex-basis: <percentage>`.
pub(super) fn basis_pct(fraction: f32) -> FlexBasis {
    FlexBasis::Size(size_pct(fraction))
}

/// `flex-basis: calc()`.
pub(super) fn basis_calc(length: f32, percentage: f32) -> FlexBasis {
    FlexBasis::Size(size_calc(length, percentage))
}

/// `flex-basis: fit-content(<length>)`.
pub(super) fn basis_fit_content_px(value: f32) -> FlexBasis {
    FlexBasis::Size(size_fit_content_px(value))
}

/// `flex-basis: content`.
pub(super) fn basis_content() -> FlexBasis {
    FlexBasis::Content
}

/// One `gap` axis: `normal` (resolves to zero).
pub(super) fn gap_normal() -> NonNegativeLengthPercentageOrNormal {
    NonNegativeLengthPercentageOrNormal::Normal
}

/// One `gap` axis: `<length>`.
pub(super) fn gap_px(value: f32) -> NonNegativeLengthPercentageOrNormal {
    NonNegativeLengthPercentageOrNormal::LengthPercentage(npx(value))
}

/// One `gap` axis: `<percentage>`.
pub(super) fn gap_pct(fraction: f32) -> NonNegativeLengthPercentageOrNormal {
    NonNegativeLengthPercentageOrNormal::LengthPercentage(npct(fraction))
}

/// `aspect-ratio: <ratio>` (as width / height).
pub(super) fn ratio(value: f32) -> AspectRatio {
    AspectRatio {
        auto: false,
        ratio: PreferredRatio::Ratio(Ratio::new(value, 1.0)),
    }
}

/// `align-items` / the container half of the deleted cross-gravity channel.
pub(super) fn items(flags: AlignFlags) -> ItemPlacement {
    ItemPlacement(flags)
}

/// `align-self`/`justify-self` / the deleted per-item layout-gravity channel.
pub(super) fn self_align(flags: AlignFlags) -> SelfAlignment {
    SelfAlignment(flags)
}

/// `align-content`/`justify-content` / the deleted main-gravity channel.
pub(super) fn content(flags: AlignFlags) -> ContentDistribution {
    ContentDistribution::new(flags)
}

/// `justify-items`.
pub(super) fn justify_items(flags: AlignFlags) -> JustifyItems {
    let specified = stylo::values::specified::align::JustifyItems(ItemPlacement(flags));
    JustifyItems {
        specified,
        computed: specified,
    }
}

/// The reserved "no reference" relative-layout sentinel.
pub(super) const RELATIVE_NONE: i32 = -1;

/// The reserved "the parent" relative-layout sentinel.
pub(super) const RELATIVE_PARENT: i32 = 0;

// ---------------------------------------------------------------------------
// Grid track constructor helpers
// ---------------------------------------------------------------------------

/// One computed grid track sizing function.
pub(super) type TestTrack = generic_grid::TrackSize<LengthPercentage>;

/// One computed track breadth.
pub(super) type TestTrackBreadth = generic_grid::TrackBreadth<LengthPercentage>;

/// One computed track-list entry (a track or a repetition).
pub(super) type TestTrackListValue = generic_grid::TrackListValue<LengthPercentage, i32>;

/// A `repeat()` count.
pub(super) type TestRepeatCount = generic_grid::RepeatCount<i32>;

/// Track breadth: `<length>`.
pub(super) fn breadth_px(value: f32) -> TestTrackBreadth {
    generic_grid::TrackBreadth::Breadth(px(value))
}

/// Track breadth: `<percentage>`.
pub(super) fn breadth_pct(fraction: f32) -> TestTrackBreadth {
    generic_grid::TrackBreadth::Breadth(pct(fraction))
}

/// Track breadth: `calc()`.
pub(super) fn breadth_calc(length: f32, percentage: f32) -> TestTrackBreadth {
    generic_grid::TrackBreadth::Breadth(calc_lp(length, percentage))
}

/// Track breadth: `<flex>` (`fr`).
pub(super) fn breadth_fr(value: f32) -> TestTrackBreadth {
    generic_grid::TrackBreadth::Flex(generic_grid::Flex(value))
}

/// Track breadth: `auto`.
pub(super) fn breadth_auto() -> TestTrackBreadth {
    generic_grid::TrackBreadth::Auto
}

/// Track breadth: `min-content`.
pub(super) fn breadth_min_content() -> TestTrackBreadth {
    generic_grid::TrackBreadth::MinContent
}

/// Track breadth: `max-content`.
pub(super) fn breadth_max_content() -> TestTrackBreadth {
    generic_grid::TrackBreadth::MaxContent
}

/// Track: single breadth.
pub(super) fn track(breadth: TestTrackBreadth) -> TestTrack {
    generic_grid::TrackSize::Breadth(breadth)
}

/// Track: `<length>`.
pub(super) fn track_px(value: f32) -> TestTrack {
    track(breadth_px(value))
}

/// Track: `<percentage>`.
pub(super) fn track_pct(fraction: f32) -> TestTrack {
    track(breadth_pct(fraction))
}

/// Track: `<flex>` (`fr`).
pub(super) fn track_fr(value: f32) -> TestTrack {
    track(breadth_fr(value))
}

/// Track: `auto`.
pub(super) fn track_auto() -> TestTrack {
    track(breadth_auto())
}

/// Track: `min-content`.
pub(super) fn track_min_content() -> TestTrack {
    track(breadth_min_content())
}

/// Track: `max-content`.
pub(super) fn track_max_content() -> TestTrack {
    track(breadth_max_content())
}

/// Track: `minmax(<breadth>, <breadth>)`.
pub(super) fn track_minmax(min: TestTrackBreadth, max: TestTrackBreadth) -> TestTrack {
    generic_grid::TrackSize::Minmax(min, max)
}

/// Track: `fit-content(<length-percentage>)`.
pub(super) fn track_fit_content(limit: TestTrackBreadth) -> TestTrack {
    generic_grid::TrackSize::FitContent(limit)
}

/// A `repeat()` track-list entry (line names left empty, honoring the
/// `line_names.len() == track_sizes.len() + 1` invariant).
pub(super) fn track_repeat(count: TestRepeatCount, tracks: Vec<TestTrack>) -> TestTrackListValue {
    generic_grid::TrackListValue::TrackRepeat(generic_grid::TrackRepeat {
        count,
        line_names: vec![stylo::OwnedSlice::default(); tracks.len() + 1].into(),
        track_sizes: tracks.into(),
    })
}

/// A template from explicit track-list entries. `auto_repeat_index` is the
/// index of the `auto-fill`/`auto-fit` repetition in `values`, or
/// `usize::MAX` when there is none.
pub(super) fn template_values(
    values: Vec<TestTrackListValue>,
    auto_repeat_index: usize,
) -> GridTemplateComponent {
    let count = values.len();
    GridTemplateComponent::TrackList(Box::new(generic_grid::TrackList {
        auto_repeat_index,
        values: values.into(),
        line_names: vec![stylo::OwnedSlice::default(); count + 1].into(),
    }))
}

/// A template of plain tracks (no repetitions, no line names).
pub(super) fn track_list(tracks: Vec<TestTrack>) -> GridTemplateComponent {
    template_values(
        tracks
            .into_iter()
            .map(generic_grid::TrackListValue::TrackSize)
            .collect(),
        usize::MAX,
    )
}

/// `grid-template-rows`/`-columns`: `none`.
pub(super) fn template_none() -> GridTemplateComponent {
    GridTemplateComponent::None
}

/// `grid-auto-rows`/`-columns`.
pub(super) fn implicit_tracks(tracks: Vec<TestTrack>) -> ImplicitGridTracks {
    generic_grid::ImplicitGridTracks(tracks.into())
}

/// Grid placement: a 1-based (possibly negative) line number.
pub(super) fn grid_line(line: i32) -> GridLine {
    GridLine {
        line_num: line,
        ..GridLine::auto()
    }
}

/// Grid placement: `span <n>`.
pub(super) fn grid_span(span: i32) -> GridLine {
    GridLine {
        line_num: span,
        is_span: true,
        ..GridLine::auto()
    }
}

/// Grid placement: `auto`.
pub(super) fn grid_auto_placement() -> GridLine {
    GridLine::auto()
}

// ---------------------------------------------------------------------------
// Host tree
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TestDisplay {
    Flex,
    Grid,
    Linear,
    Relative,
    Leaf,
}

#[derive(Debug, Clone)]
pub(super) struct TestStyle {
    pub(super) display: Display,
    pub(super) visibility: visibility::T,
    pub(super) position: PositionProperty,
    pub(super) inset: Edges<Inset>,
    pub(super) size: Size<StyleSize>,
    pub(super) min_size: Size<StyleSize>,
    pub(super) max_size: Size<MaxSize>,
    pub(super) aspect_ratio: AspectRatio,
    pub(super) margin: Edges<Margin>,
    pub(super) padding: Edges<NonNegativeLengthPercentage>,
    pub(super) border: Edges<BorderSideWidth>,
    pub(super) overflow: Point<Overflow>,
    pub(super) box_sizing: box_sizing::T,
    pub(super) direction: direction::T,
    pub(super) linear_direction: linear_direction::T,
    pub(super) linear_weight_sum: NonNegativeNumber,
    pub(super) flex_direction: flex_direction::T,
    pub(super) flex_wrap: flex_wrap::T,
    pub(super) gap: Size<NonNegativeLengthPercentageOrNormal>,
    pub(super) align_content: ContentDistribution,
    pub(super) align_items: ItemPlacement,
    pub(super) justify_content: ContentDistribution,
    pub(super) flex_basis: FlexBasis,
    pub(super) flex_grow: NonNegativeNumber,
    pub(super) flex_shrink: NonNegativeNumber,
    pub(super) linear_weight: NonNegativeNumber,
    pub(super) align_self: SelfAlignment,
    pub(super) order: i32,
    pub(super) template_rows: GridTemplateComponent,
    pub(super) template_columns: GridTemplateComponent,
    pub(super) auto_rows: ImplicitGridTracks,
    pub(super) auto_columns: ImplicitGridTracks,
    pub(super) auto_flow: GridAutoFlow,
    pub(super) justify_items: JustifyItems,
    pub(super) grid_row: Line<GridLine>,
    pub(super) grid_column: Line<GridLine>,
    pub(super) justify_self: SelfAlignment,
    pub(super) relative_layout_once: relative_layout_once::T,
    pub(super) relative_id: RelativeReference,
    pub(super) relative_align: Edges<RelativeAlign>,
    pub(super) relative_adjacent: Edges<RelativeReference>,
    pub(super) relative_center: relative_center::T,
}

impl Default for TestStyle {
    fn default() -> Self {
        Self {
            display: Display::Flex,
            visibility: visibility::T::Visible,
            position: PositionProperty::Relative,
            inset: Edges::uniform(inset_auto()),
            size: Size::new(size_auto(), size_auto()),
            min_size: Size::new(size_auto(), size_auto()),
            max_size: Size::new(max_none(), max_none()),
            aspect_ratio: AspectRatio::auto(),
            margin: Edges::uniform(margin_px(0.0)),
            padding: Edges::uniform(npx(0.0)),
            border: Edges::uniform(border_px(0.0)),
            overflow: Point::new(Overflow::Visible, Overflow::Visible),
            box_sizing: box_sizing::T::ContentBox,
            direction: direction::T::Ltr,
            linear_direction: linear_direction::T::Column,
            linear_weight_sum: nn(0.0),
            flex_direction: flex_direction::T::Row,
            flex_wrap: flex_wrap::T::Nowrap,
            gap: Size::new(gap_normal(), gap_normal()),
            align_content: ContentDistribution::normal(),
            align_items: ItemPlacement::normal(),
            justify_content: ContentDistribution::normal(),
            flex_basis: basis_auto(),
            flex_grow: nn(0.0),
            flex_shrink: nn(1.0),
            linear_weight: nn(0.0),
            align_self: SelfAlignment::auto(),
            order: 0,
            template_rows: template_none(),
            template_columns: template_none(),
            auto_rows: implicit_tracks(Vec::new()),
            auto_columns: implicit_tracks(Vec::new()),
            auto_flow: GridAutoFlow::ROW,
            justify_items: justify_items(AlignFlags::NORMAL),
            grid_row: Line::new(grid_auto_placement(), grid_auto_placement()),
            grid_column: Line::new(grid_auto_placement(), grid_auto_placement()),
            justify_self: SelfAlignment::auto(),
            relative_layout_once: relative_layout_once::T::True,
            relative_id: RELATIVE_NONE,
            relative_align: Edges::uniform(RELATIVE_NONE),
            relative_adjacent: Edges::uniform(RELATIVE_NONE),
            relative_center: relative_center::T::None,
        }
    }
}

impl CoreStyle for TestStyle {
    fn display(&self) -> Display {
        self.display
    }

    fn visibility(&self) -> visibility::T {
        self.visibility
    }

    fn position(&self) -> PositionProperty {
        self.position
    }

    fn inset(&self) -> Edges<&Inset> {
        self.inset.as_ref()
    }

    fn size(&self) -> Size<&StyleSize> {
        self.size.as_ref()
    }

    fn min_size(&self) -> Size<&StyleSize> {
        self.min_size.as_ref()
    }

    fn max_size(&self) -> Size<&MaxSize> {
        self.max_size.as_ref()
    }

    fn aspect_ratio(&self) -> AspectRatio {
        self.aspect_ratio
    }

    fn margin(&self) -> Edges<&Margin> {
        self.margin.as_ref()
    }

    fn padding(&self) -> Edges<&NonNegativeLengthPercentage> {
        self.padding.as_ref()
    }

    fn border(&self) -> Edges<BorderSideWidth> {
        self.border.clone()
    }

    fn overflow(&self) -> Point<Overflow> {
        self.overflow
    }

    fn box_sizing(&self) -> box_sizing::T {
        self.box_sizing
    }

    fn direction(&self) -> direction::T {
        self.direction
    }
}

impl LinearContainerStyle for TestStyle {
    fn linear_direction(&self) -> linear_direction::T {
        self.linear_direction
    }

    fn linear_weight_sum(&self) -> NonNegativeNumber {
        self.linear_weight_sum
    }

    fn justify_content(&self) -> ContentDistribution {
        self.justify_content
    }

    fn align_items(&self) -> ItemPlacement {
        self.align_items
    }
}

impl LinearItemStyle for TestStyle {
    fn linear_weight(&self) -> NonNegativeNumber {
        self.linear_weight
    }

    fn align_self(&self) -> SelfAlignment {
        self.align_self
    }

    fn order(&self) -> i32 {
        self.order
    }
}

impl FlexContainerStyle for TestStyle {
    fn flex_direction(&self) -> flex_direction::T {
        self.flex_direction
    }

    fn flex_wrap(&self) -> flex_wrap::T {
        self.flex_wrap
    }

    fn gap(&self) -> Size<&NonNegativeLengthPercentageOrNormal> {
        self.gap.as_ref()
    }

    fn align_content(&self) -> ContentDistribution {
        self.align_content
    }

    fn align_items(&self) -> ItemPlacement {
        self.align_items
    }

    fn justify_content(&self) -> ContentDistribution {
        self.justify_content
    }
}

impl FlexItemStyle for TestStyle {
    fn flex_basis(&self) -> &FlexBasis {
        &self.flex_basis
    }

    fn flex_grow(&self) -> NonNegativeNumber {
        self.flex_grow
    }

    fn flex_shrink(&self) -> NonNegativeNumber {
        self.flex_shrink
    }

    fn align_self(&self) -> SelfAlignment {
        self.align_self
    }

    fn order(&self) -> i32 {
        self.order
    }
}

impl GridContainerStyle for TestStyle {
    fn grid_template_rows(&self) -> &GridTemplateComponent {
        &self.template_rows
    }

    fn grid_template_columns(&self) -> &GridTemplateComponent {
        &self.template_columns
    }

    fn grid_auto_rows(&self) -> &ImplicitGridTracks {
        &self.auto_rows
    }

    fn grid_auto_columns(&self) -> &ImplicitGridTracks {
        &self.auto_columns
    }

    fn grid_auto_flow(&self) -> GridAutoFlow {
        self.auto_flow
    }

    fn gap(&self) -> Size<&NonNegativeLengthPercentageOrNormal> {
        self.gap.as_ref()
    }

    fn align_content(&self) -> ContentDistribution {
        self.align_content
    }

    fn justify_content(&self) -> ContentDistribution {
        self.justify_content
    }

    fn align_items(&self) -> ItemPlacement {
        self.align_items
    }

    fn justify_items(&self) -> JustifyItems {
        self.justify_items
    }
}

impl GridItemStyle for TestStyle {
    fn grid_row_start(&self) -> &GridLine {
        &self.grid_row.start
    }

    fn grid_row_end(&self) -> &GridLine {
        &self.grid_row.end
    }

    fn grid_column_start(&self) -> &GridLine {
        &self.grid_column.start
    }

    fn grid_column_end(&self) -> &GridLine {
        &self.grid_column.end
    }

    fn align_self(&self) -> SelfAlignment {
        self.align_self
    }

    fn justify_self(&self) -> SelfAlignment {
        self.justify_self
    }

    fn order(&self) -> i32 {
        self.order
    }
}

impl RelativeContainerStyle for TestStyle {
    fn relative_layout_once(&self) -> relative_layout_once::T {
        self.relative_layout_once
    }
}

impl RelativeItemStyle for TestStyle {
    fn relative_id(&self) -> RelativeReference {
        self.relative_id
    }

    fn relative_align(&self) -> Edges<RelativeAlign> {
        self.relative_align
    }

    fn relative_adjacent(&self) -> Edges<RelativeReference> {
        self.relative_adjacent
    }

    fn relative_center(&self) -> relative_center::T {
        self.relative_center
    }

    fn order(&self) -> i32 {
        self.order
    }
}

/// A static test-leaf measurement strategy.
///
/// `Function` accepts a function pointer rather than a trait object, keeping the shared host
/// statically described while allowing measurements that depend on the normalized constraints.
#[derive(Debug, Clone, Copy)]
pub(super) enum TestMeasure {
    Intrinsic {
        min_content_size: Size<f32>,
        max_content_size: Size<f32>,
        first_baseline: Option<f32>,
    },
    Function(fn(LeafMeasureInput) -> LeafMetrics),
    ConstraintFunction {
        measure: fn(TestConstraints) -> Size<f32>,
        baseline: Option<fn(Size<f32>) -> f32>,
    },
    Profile(TestMeasureProfile),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) enum TestMeasureMode {
    #[default]
    Indefinite,
    Definite,
    AtMost,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct TestSideConstraint {
    pub(super) size: f32,
    pub(super) mode: TestMeasureMode,
}

impl TestSideConstraint {
    pub(super) const fn indefinite() -> Self {
        Self {
            size: 0.0,
            mode: TestMeasureMode::Indefinite,
        }
    }

    pub(super) const fn definite(size: f32) -> Self {
        Self {
            size,
            mode: TestMeasureMode::Definite,
        }
    }

    pub(super) const fn at_most(size: f32) -> Self {
        Self {
            size,
            mode: TestMeasureMode::AtMost,
        }
    }

    pub(super) const fn is_definite(self) -> bool {
        matches!(self.mode, TestMeasureMode::Definite)
    }

    pub(super) fn bounded_size(self) -> Option<f32> {
        match self.mode {
            TestMeasureMode::Indefinite => None,
            TestMeasureMode::Definite | TestMeasureMode::AtMost => Some(self.size),
        }
    }

    pub(super) fn near(self, other: Self) -> bool {
        (self.mode == TestMeasureMode::Indefinite && other.mode == TestMeasureMode::Indefinite)
            || (self.mode == other.mode && (self.size - other.size).abs() < 0.00001)
    }

    pub(super) fn clamp(self, value: f32) -> f32 {
        match self.mode {
            TestMeasureMode::AtMost => value.min(self.size),
            TestMeasureMode::Definite | TestMeasureMode::Indefinite => value,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct TestConstraints {
    pub(super) width: TestSideConstraint,
    pub(super) height: TestSideConstraint,
}

impl TestConstraints {
    pub(super) const fn new(width: TestSideConstraint, height: TestSideConstraint) -> Self {
        Self { width, height }
    }

    pub(super) const fn definite(width: f32, height: f32) -> Self {
        Self::new(
            TestSideConstraint::definite(width),
            TestSideConstraint::definite(height),
        )
    }

    pub(super) const fn indefinite() -> Self {
        Self::new(
            TestSideConstraint::indefinite(),
            TestSideConstraint::indefinite(),
        )
    }
}

#[derive(Debug, Clone, Copy)]
pub(super) enum TestRegularMeasure {
    Fixed(Size<f32>),
    WidthByHeightDefiniteness {
        at_most_width: f32,
        definite_width: f32,
        height: f32,
    },
    HeightFromWidth {
        intrinsic_width: f32,
        fallback_height: f32,
        height_ratio: f32,
    },
}

#[derive(Debug, Clone, Copy)]
pub(super) enum TestIntrinsicMeasure {
    Fixed(Size<f32>),
    WidthFromHeight {
        fallback_width: f32,
        width_ratio: f32,
        height: f32,
    },
    CrossAxis {
        fallback_width: f32,
        width_from_height_ratio: f32,
        fallback_height: f32,
        height_from_width_ratio: f32,
    },
}

#[derive(Debug, Clone, Copy, Default)]
pub(super) struct TestMeasureProfile {
    pub(super) regular: Option<TestRegularMeasure>,
    pub(super) min_content: Option<TestIntrinsicMeasure>,
    pub(super) max_content: Option<TestIntrinsicMeasure>,
    pub(super) first_baseline: Option<f32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TestMeasureCallKind {
    Regular,
    MinContent,
    MaxContent,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct TestMeasureCall {
    pub(super) kind: TestMeasureCallKind,
    pub(super) constraints: TestConstraints,
    pub(super) goal: LayoutGoal,
}

fn test_side_constraint(known: Option<f32>, available: AvailableSpace) -> TestSideConstraint {
    if let Some(value) = known {
        return TestSideConstraint::definite(value);
    }
    match available {
        AvailableSpace::Definite(value) => TestSideConstraint::at_most(value),
        AvailableSpace::MinContent | AvailableSpace::MaxContent => TestSideConstraint::indefinite(),
    }
}

impl TestMeasure {
    fn measure(self, input: LeafMeasureInput) -> (LeafMetrics, Option<TestMeasureCall>) {
        let measured = match self {
            Self::Intrinsic {
                min_content_size,
                max_content_size,
                first_baseline,
            } => {
                let size = Size::new(
                    if input.available_space.width == AvailableSpace::MinContent {
                        min_content_size.width
                    } else {
                        max_content_size.width
                    },
                    if input.available_space.height == AvailableSpace::MinContent {
                        min_content_size.height
                    } else {
                        max_content_size.height
                    },
                );
                LeafMetrics::new(size).with_first_baselines(Point::new(None, first_baseline))
            }
            Self::Function(measure) => measure(input),
            Self::ConstraintFunction { measure, baseline } => {
                let constraints = TestConstraints::new(
                    test_side_constraint(input.known_dimensions.width, input.available_space.width),
                    test_side_constraint(
                        input.known_dimensions.height,
                        input.available_space.height,
                    ),
                );
                let size = measure(constraints);
                LeafMetrics::new(size)
                    .with_first_baselines(Point::new(None, baseline.map(|baseline| baseline(size))))
            }
            Self::Profile(profile) => {
                let constraints = TestConstraints::new(
                    test_side_constraint(input.known_dimensions.width, input.available_space.width),
                    test_side_constraint(
                        input.known_dimensions.height,
                        input.available_space.height,
                    ),
                );
                let kind = if matches!(input.goal, LayoutGoal::Measure(_))
                    && (input.available_space.width == AvailableSpace::MinContent
                        || input.available_space.height == AvailableSpace::MinContent)
                {
                    TestMeasureCallKind::MinContent
                } else if matches!(input.goal, LayoutGoal::Measure(_))
                    && (input.available_space.width == AvailableSpace::MaxContent
                        || input.available_space.height == AvailableSpace::MaxContent)
                {
                    TestMeasureCallKind::MaxContent
                } else {
                    TestMeasureCallKind::Regular
                };
                let regular = profile
                    .regular
                    .map_or(Size::ZERO, |measure| measure.measure(constraints));
                let size = match kind {
                    TestMeasureCallKind::Regular => regular,
                    TestMeasureCallKind::MinContent => profile
                        .min_content
                        .map_or(regular, |measure| measure.measure(constraints)),
                    TestMeasureCallKind::MaxContent => profile
                        .max_content
                        .map_or(regular, |measure| measure.measure(constraints)),
                };
                let size = if size.width.is_finite() && size.height.is_finite() {
                    size
                } else {
                    regular
                };
                return (
                    LeafMetrics::new(size)
                        .with_first_baselines(Point::new(None, profile.first_baseline)),
                    Some(TestMeasureCall {
                        kind,
                        constraints,
                        goal: input.goal,
                    }),
                );
            }
        };

        (
            LeafMetrics::new(Size::new(
                input.known_dimensions.width.unwrap_or(measured.size.width),
                input
                    .known_dimensions
                    .height
                    .unwrap_or(measured.size.height),
            ))
            .with_first_baselines(measured.first_baselines),
            None,
        )
    }
}

impl TestRegularMeasure {
    fn measure(self, constraints: TestConstraints) -> Size<f32> {
        match self {
            Self::Fixed(size) => Size::new(
                constraints.width.clamp(size.width),
                constraints.height.clamp(size.height),
            ),
            Self::WidthByHeightDefiniteness {
                at_most_width,
                definite_width,
                height,
            } => {
                let width = if constraints.height.is_definite() {
                    definite_width
                } else {
                    at_most_width
                };
                Size::new(
                    constraints.width.clamp(width),
                    constraints.height.clamp(height),
                )
            }
            Self::HeightFromWidth {
                intrinsic_width,
                fallback_height,
                height_ratio,
            } => {
                let width = constraints.width.bounded_size().unwrap_or(intrinsic_width);
                let height = if constraints.width.bounded_size().is_some() {
                    width * height_ratio
                } else {
                    fallback_height
                };
                Size::new(
                    constraints.width.clamp(width),
                    constraints.height.clamp(height),
                )
            }
        }
    }
}

impl TestIntrinsicMeasure {
    fn measure(self, constraints: TestConstraints) -> Size<f32> {
        match self {
            Self::Fixed(size) => size,
            Self::WidthFromHeight {
                fallback_width,
                width_ratio,
                height,
            } => Size::new(
                constraints
                    .height
                    .bounded_size()
                    .map_or(fallback_width, |value| value * width_ratio),
                height,
            ),
            Self::CrossAxis {
                fallback_width,
                width_from_height_ratio,
                fallback_height,
                height_from_width_ratio,
            } => Size::new(
                constraints
                    .height
                    .bounded_size()
                    .map_or(fallback_width, |value| value * width_from_height_ratio),
                constraints
                    .width
                    .bounded_size()
                    .map_or(fallback_height, |value| value * height_from_width_ratio),
            ),
        }
    }
}

/// Test-local node identity: a dense index into [`TestTree`]. Builders hand
/// these out during the mutation phase; layout and assertions resolve them
/// to borrowed [`TestRef`] handles.
pub(super) type TestId = usize;

#[derive(Debug, Clone)]
pub(super) struct TestSourceNode {
    pub(super) display: TestDisplay,
    pub(super) style: TestStyle,
    pub(super) children: Vec<TestId>,
    pub(super) measure: TestMeasure,
}

/// Per-node mutable layout slots, written through [`TestRef`] handles.
/// Layout is single-threaded, so `Cell`/`RefCell` interior mutability is the
/// whole synchronization story.
#[derive(Debug, Default)]
pub(super) struct TestSessionNode {
    pub(super) layout: Cell<Layout>,
    pub(super) final_layout: Cell<Layout>,
    pub(super) static_position: Cell<Option<Point<f32>>>,
    pub(super) output: Cell<LayoutOutput>,
    pub(super) measure_inputs: RefCell<Vec<LeafMeasureInput>>,
    pub(super) measure_calls: RefCell<Vec<TestMeasureCall>>,
}

/// The one host tree: immutable node data plus interior-mutable session
/// slots and instrumentation. The session slots deliberately live in a
/// parallel `Vec` (not inline in `TestSourceNode`) so bench fixtures keep
/// the same memory layout the pre-handle host had.
#[derive(Debug)]
pub(super) struct TestTree {
    pub(super) nodes: Vec<TestSourceNode>,
    pub(super) session: Vec<TestSessionNode>,
    caches: Option<Vec<RefCell<Cache>>>,
    pub(super) child_layout_calls: Cell<usize>,
    pub(super) layout_writes: Cell<usize>,
    pub(super) static_position_writes: Cell<usize>,
    pub(super) leaf_measure_calls: Cell<usize>,
    pub(super) record_measure_inputs: Cell<bool>,
}

impl Default for TestTree {
    fn default() -> Self {
        Self {
            nodes: Vec::new(),
            session: Vec::new(),
            caches: None,
            child_layout_calls: Cell::new(0),
            layout_writes: Cell::new(0),
            static_position_writes: Cell::new(0),
            leaf_measure_calls: Cell::new(0),
            record_measure_inputs: Cell::new(true),
        }
    }
}

/// The `Copy` node handle: a borrow of the tree plus a node index.
#[derive(Clone, Copy)]
pub(super) struct TestRef<'t> {
    tree: &'t TestTree,
    index: TestId,
}

impl fmt::Debug for TestRef<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_tuple("TestRef").field(&self.index).finish()
    }
}

impl<'t> TestRef<'t> {
    fn source(self) -> &'t TestSourceNode {
        &self.tree.nodes[self.index]
    }

    fn slots(self) -> &'t TestSessionNode {
        &self.tree.session[self.index]
    }
}

pub(super) struct TestChildren<'t> {
    tree: &'t TestTree,
    ids: std::slice::Iter<'t, TestId>,
}

impl<'t> Iterator for TestChildren<'t> {
    type Item = TestRef<'t>;

    fn next(&mut self) -> Option<TestRef<'t>> {
        let index = *self.ids.next()?;
        Some(TestRef {
            tree: self.tree,
            index,
        })
    }
}

impl<'t> LayoutNode for TestRef<'t> {
    type Style = &'t TestStyle;
    type ChildIter = TestChildren<'t>;

    fn children(self) -> TestChildren<'t> {
        TestChildren {
            tree: self.tree,
            ids: self.source().children.iter(),
        }
    }

    fn child_count(self) -> usize {
        self.source().children.len()
    }

    fn style(self) -> &'t TestStyle {
        &self.source().style
    }

    fn compute_child_layout(self, input: LayoutInput) -> LayoutOutput {
        let tree = self.tree;
        tree.child_layout_calls
            .set(tree.child_layout_calls.get() + 1);
        let node = self.source();
        let display = node.display;

        if node.style.display.is_none() {
            hide_subtree(self);
            return LayoutOutput::HIDDEN;
        }

        let output = compute_cached_layout(self, input, |handle, input| match display {
            TestDisplay::Flex => compute_flexbox_layout(handle, input),
            TestDisplay::Grid => compute_grid_layout(handle, input),
            TestDisplay::Linear => compute_linear_layout(handle, input),
            TestDisplay::Relative => compute_relative_layout(handle, input),
            TestDisplay::Leaf => {
                let style = &node.style;
                let measure = node.measure;
                let slots = handle.slots();
                let mut measurer = FnLeafMeasurer::new(|measure_input| {
                    tree.leaf_measure_calls
                        .set(tree.leaf_measure_calls.get() + 1);
                    if tree.record_measure_inputs.get() {
                        slots.measure_inputs.borrow_mut().push(measure_input);
                    }
                    let (metrics, call) = measure.measure(measure_input);
                    if let Some(call) = call {
                        slots.measure_calls.borrow_mut().push(call);
                    }
                    metrics
                });
                compute_leaf_layout(input, style, &mut measurer)
            }
        });
        self.slots().output.set(output);
        output
    }

    fn set_unrounded_layout(self, layout: &Layout) {
        self.tree
            .layout_writes
            .set(self.tree.layout_writes.get() + 1);
        self.slots().layout.set(*layout);
    }

    fn unrounded_layout(self) -> Layout {
        self.slots().layout.get()
    }

    fn set_final_layout(self, layout: &Layout) {
        self.slots().final_layout.set(*layout);
    }

    fn set_static_position(self, static_position: Point<f32>) {
        self.tree
            .static_position_writes
            .set(self.tree.static_position_writes.get() + 1);
        self.slots().static_position.set(Some(static_position));
    }

    fn cache_get(self, input: LayoutInput) -> Option<LayoutOutput> {
        self.tree.caches.as_ref()?[self.index].borrow().get(input)
    }

    fn cache_store(self, input: LayoutInput, output: LayoutOutput) {
        if let Some(caches) = &self.tree.caches {
            caches[self.index].borrow_mut().store(input, output);
        }
    }

    fn cache_clear(self) {
        if let Some(caches) = &self.tree.caches {
            caches[self.index].borrow_mut().clear();
        }
    }
}

impl TestTree {
    /// Resolves a builder-returned id to a borrowed node handle.
    pub(super) fn node(&self, id: TestId) -> TestRef<'_> {
        TestRef {
            tree: self,
            index: id,
        }
    }

    /// Dispatches layout on `id` — the entry point tests use directly.
    pub(super) fn compute_child_layout(&self, id: TestId, input: LayoutInput) -> LayoutOutput {
        self.node(id).compute_child_layout(input)
    }

    pub(super) fn enable_cache(&mut self) {
        self.caches = Some(
            self.nodes
                .iter()
                .map(|_| RefCell::new(Cache::new()))
                .collect(),
        );
    }

    pub(super) fn push_leaf(
        &mut self,
        style: TestStyle,
        intrinsic_size: Size<f32>,
        first_baseline: Option<f32>,
    ) -> TestId {
        self.push(TestSourceNode {
            display: TestDisplay::Leaf,
            style,
            children: Vec::new(),
            measure: TestMeasure::Intrinsic {
                min_content_size: intrinsic_size,
                max_content_size: intrinsic_size,
                first_baseline,
            },
        })
    }

    pub(super) fn push_intrinsic_leaf(
        &mut self,
        style: TestStyle,
        min_content_size: Size<f32>,
        max_content_size: Size<f32>,
    ) -> TestId {
        self.push(TestSourceNode {
            display: TestDisplay::Leaf,
            style,
            children: Vec::new(),
            measure: TestMeasure::Intrinsic {
                min_content_size,
                max_content_size,
                first_baseline: None,
            },
        })
    }

    pub(super) fn push_measured_leaf(
        &mut self,
        style: TestStyle,
        measure: fn(LeafMeasureInput) -> LeafMetrics,
    ) -> TestId {
        self.push(TestSourceNode {
            display: TestDisplay::Leaf,
            style,
            children: Vec::new(),
            measure: TestMeasure::Function(measure),
        })
    }

    pub(super) fn set_leaf_measure(&mut self, node: TestId, measure: TestMeasure) {
        let source_node = self.source_node_mut(node);
        assert_eq!(source_node.display, TestDisplay::Leaf);
        source_node.measure = measure;
    }

    pub(super) fn push_flex(&mut self, style: TestStyle, children: Vec<TestId>) -> TestId {
        self.push(TestSourceNode {
            display: TestDisplay::Flex,
            style,
            children,
            measure: TestMeasure::Intrinsic {
                min_content_size: Size::ZERO,
                max_content_size: Size::ZERO,
                first_baseline: None,
            },
        })
    }

    pub(super) fn push_grid(&mut self, style: TestStyle, children: Vec<TestId>) -> TestId {
        self.push(TestSourceNode {
            display: TestDisplay::Grid,
            style,
            children,
            measure: TestMeasure::Intrinsic {
                min_content_size: Size::ZERO,
                max_content_size: Size::ZERO,
                first_baseline: None,
            },
        })
    }

    pub(super) fn push_linear(&mut self, style: TestStyle, children: Vec<TestId>) -> TestId {
        self.push(TestSourceNode {
            display: TestDisplay::Linear,
            style,
            children,
            measure: TestMeasure::Intrinsic {
                min_content_size: Size::ZERO,
                max_content_size: Size::ZERO,
                first_baseline: None,
            },
        })
    }

    pub(super) fn push_relative(&mut self, style: TestStyle, children: Vec<TestId>) -> TestId {
        self.push(TestSourceNode {
            display: TestDisplay::Relative,
            style,
            children,
            measure: TestMeasure::Intrinsic {
                min_content_size: Size::ZERO,
                max_content_size: Size::ZERO,
                first_baseline: None,
            },
        })
    }

    pub(super) fn push(&mut self, node: TestSourceNode) -> TestId {
        debug_assert_eq!(self.nodes.len(), self.session.len());
        let id = self.nodes.len();
        self.nodes.push(node);
        self.session.push(TestSessionNode::default());
        if let Some(caches) = &mut self.caches {
            caches.push(RefCell::new(Cache::new()));
        }
        id
    }

    pub(super) fn source_node_mut(&mut self, id: TestId) -> &mut TestSourceNode {
        &mut self.nodes[id]
    }

    /// The interior-mutable session slots of one node; tests mutate them
    /// through the `Cell`/`RefCell` fields.
    pub(super) fn session_node(&self, id: TestId) -> &TestSessionNode {
        &self.session[id]
    }

    pub(super) fn layout(&self, id: TestId) -> Layout {
        self.session_node(id).layout.get()
    }

    pub(super) fn final_layout(&self, id: TestId) -> Layout {
        self.session_node(id).final_layout.get()
    }

    pub(super) fn static_position(&self, id: TestId) -> Option<Point<f32>> {
        self.session_node(id).static_position.get()
    }

    pub(super) fn measure_inputs(&self, id: TestId) -> Vec<LeafMeasureInput> {
        self.session_node(id).measure_inputs.borrow().clone()
    }
}

pub(super) fn fixed_leaf_style(width: f32, height: f32) -> TestStyle {
    TestStyle {
        size: Size::new(size_px(width), size_px(height)),
        flex_basis: basis_px(width),
        ..TestStyle::default()
    }
}

pub(super) fn fixed_leaf(tree: &mut TestTree, width: f32, height: f32) -> TestId {
    tree.push_leaf(
        fixed_leaf_style(width, height),
        Size::new(width, height),
        None,
    )
}

pub(super) fn flex_container(tree: &mut TestTree, style: TestStyle, children: &[TestId]) -> TestId {
    tree.push_flex(style, children.to_vec())
}

pub(super) fn relative_container(
    tree: &mut TestTree,
    style: TestStyle,
    children: &[TestId],
) -> TestId {
    tree.push_relative(style, children.to_vec())
}

pub(super) fn linear_container(
    tree: &mut TestTree,
    style: TestStyle,
    children: &[TestId],
) -> TestId {
    tree.push_linear(style, children.to_vec())
}

pub(super) fn perform_layout(
    tree: &TestTree,
    root: TestId,
    known_dimensions: Size<Option<f32>>,
    available_space: Size<AvailableSpace>,
) -> LayoutOutput {
    tree.compute_child_layout(
        root,
        LayoutInput::perform_layout(
            known_dimensions,
            available_space.into_options(),
            available_space,
        ),
    )
}

pub(super) fn definite_layout(
    tree: &TestTree,
    root: TestId,
    width: f32,
    height: f32,
) -> LayoutOutput {
    perform_layout(
        tree,
        root,
        Size::new(Some(width), Some(height)),
        Size::new(
            AvailableSpace::Definite(width),
            AvailableSpace::Definite(height),
        ),
    )
}

pub(super) fn measure_layout(
    tree: &TestTree,
    root: TestId,
    known_dimensions: Size<Option<f32>>,
    available_space: Size<AvailableSpace>,
) -> LayoutOutput {
    tree.compute_child_layout(
        root,
        LayoutInput::compute_size(
            known_dimensions,
            available_space.into_options(),
            available_space,
            RequestedAxis::Both,
        ),
    )
}

pub(super) fn assert_close(actual: f32, expected: f32) {
    let error = (actual - expected).abs();
    assert!(
        error <= 0.01,
        "expected {expected}, got {actual} (absolute error {error})"
    );
}

pub(super) fn assert_point(actual: Point<f32>, expected: Point<f32>) {
    assert_close(actual.x, expected.x);
    assert_close(actual.y, expected.y);
}

pub(super) fn assert_size(actual: Size<f32>, expected: Size<f32>) {
    assert_close(actual.width, expected.width);
    assert_close(actual.height, expected.height);
}
