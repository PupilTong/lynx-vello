//! Compatibility facade for Rust layout fixtures migrated from PupilTong/lynx#25.
//!
//! The source PR used a monolithic `SimpleTree` API. This facade preserves its
//! fixture vocabulary while lowering every run into neutron-star's immutable
//! `TestSource` plus mutable `TestSession`; no C++ baseline or styling engine is
//! involved.

#![allow(dead_code)]

use std::collections::BTreeMap;

use neutron_star::compute::compute_absolute_layout;
use neutron_star::geometry::Size as NSize;
use neutron_star::prelude::*;
pub(crate) use neutron_star::style::{
    AlignContent, AlignItems, BoxSizing, Direction, FlexDirection, FlexWrap, JustifyContent,
    LinearCrossGravity, LinearGravity, LinearLayoutGravity, RelativeCenter,
};
use neutron_star::style::{
    BoxGenerationMode, CalcHandle, Dimension, GridAutoFlow as NeutronGridAutoFlow, GridLine,
    GridPlacement, LengthPercentage, LengthPercentageAuto, MaxTrackSizingFunction,
    MinTrackSizingFunction, Position, RelativeReference, TrackSizingFunction,
};

use crate::support::{
    TestConstraints, TestDisplay, TestMeasure, TestMeasureMode, TestSideConstraint, TestSourceNode,
    TestStyle, TestTree,
};
#[allow(unused_imports)] // Individual PR migration targets use different trace subsets.
pub(crate) use crate::support::{
    TestConstraints as Constraints, TestIntrinsicMeasure as IntrinsicMeasureSpec,
    TestMeasureCall as MeasureCall, TestMeasureCallKind as MeasureCallKind,
    TestMeasureMode as MeasureMode, TestMeasureProfile as MeasurementProfile,
    TestRegularMeasure as RegularMeasure, TestSideConstraint as SideConstraint,
};
pub(crate) type Point = neutron_star::geometry::Point<f32>;
pub(crate) type Size = neutron_star::geometry::Size<f32>;
pub(crate) const RELATIVE_ALIGN_NONE: i32 = -1;
pub(crate) const RELATIVE_ALIGN_PARENT: i32 = 0;
/// Starlight's exported value for an authored `auto` sticky inset.
pub(crate) const STICKY_AUTO_INSET: f32 = -1e10;

pub(crate) trait DirectionExt {
    fn is_rtl(&self) -> bool;
}

impl DirectionExt for Direction {
    fn is_rtl(&self) -> bool {
        *self == Self::Rtl
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum Display {
    None,
    #[default]
    Block,
    Flex,
    Linear,
    Relative,
    Grid,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum PositionType {
    #[default]
    Static,
    Relative,
    Absolute,
    Fixed,
    Sticky,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum Visibility {
    #[default]
    Visible,
    Hidden,
    Collapse,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum JustifyItems {
    Auto,
    #[default]
    Stretch,
    Start,
    Center,
    End,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum GridAutoFlow {
    #[default]
    Row,
    Column,
    Dense,
    RowDense,
    ColumnDense,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum LinearOrientation {
    Horizontal,
    HorizontalReverse,
    #[default]
    Vertical,
    VerticalReverse,
    Row,
    RowReverse,
    Column,
    ColumnReverse,
}

impl LinearOrientation {
    pub(crate) fn is_row(self) -> bool {
        matches!(
            self,
            Self::Horizontal | Self::HorizontalReverse | Self::Row | Self::RowReverse
        )
    }

    pub(crate) fn is_reverse(self) -> bool {
        matches!(
            self,
            Self::HorizontalReverse
                | Self::VerticalReverse
                | Self::RowReverse
                | Self::ColumnReverse
        )
    }

    fn neutron(self) -> neutron_star::style::LinearOrientation {
        match self {
            Self::Horizontal => neutron_star::style::LinearOrientation::Horizontal,
            Self::HorizontalReverse => neutron_star::style::LinearOrientation::HorizontalReverse,
            Self::Vertical => neutron_star::style::LinearOrientation::Vertical,
            Self::VerticalReverse => neutron_star::style::LinearOrientation::VerticalReverse,
            Self::Row => neutron_star::style::LinearOrientation::Row,
            Self::RowReverse => neutron_star::style::LinearOrientation::RowReverse,
            Self::Column => neutron_star::style::LinearOrientation::Column,
            Self::ColumnReverse => neutron_star::style::LinearOrientation::ColumnReverse,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct BaseLength {
    fixed: f32,
    percentage: f32,
    has_percentage: bool,
}

impl BaseLength {
    pub(crate) const fn fixed(fixed: f32) -> Self {
        Self {
            fixed,
            percentage: 0.0,
            has_percentage: false,
        }
    }

    pub(crate) const fn fixed_and_percent(fixed: f32, percentage: f32) -> Self {
        Self {
            fixed,
            percentage,
            has_percentage: true,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) enum Length {
    #[default]
    Auto,
    Points(f32),
    Percent(f32),
    Calc {
        fixed: f32,
        percent: f32,
    },
    Fr(f32),
    MinContent,
    MaxContent,
    FitContent(Option<BaseLength>),
}

impl Length {
    pub(crate) const ZERO: Self = Self::Points(0.0);

    pub(crate) const fn points(value: f32) -> Self {
        Self::Points(value)
    }

    pub(crate) const fn percent(value: f32) -> Self {
        Self::Percent(value)
    }

    pub(crate) const fn calc(fixed: f32, percent: f32) -> Self {
        Self::Calc { fixed, percent }
    }

    pub(crate) const fn fr(value: f32) -> Self {
        Self::Fr(value)
    }

    pub(crate) const fn min_content() -> Self {
        Self::MinContent
    }

    pub(crate) const fn max_content() -> Self {
        Self::MaxContent
    }

    pub(crate) const fn fit_content(base: Option<BaseLength>) -> Self {
        Self::FitContent(base)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Rect<T> {
    pub(crate) left: T,
    pub(crate) right: T,
    pub(crate) top: T,
    pub(crate) bottom: T,
}

impl<T: Copy> Rect<T> {
    pub(crate) const fn all(value: T) -> Self {
        Self {
            left: value,
            right: value,
            top: value,
            bottom: value,
        }
    }

    pub(crate) const fn new(left: T, right: T, top: T, bottom: T) -> Self {
        Self {
            left,
            right,
            top,
            bottom,
        }
    }
}

impl<T: Default + Copy> Default for Rect<T> {
    fn default() -> Self {
        Self::all(T::default())
    }
}

pub(crate) type Edges = Rect<f32>;

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Style {
    pub(crate) display: Display,
    pub(crate) position: PositionType,
    pub(crate) box_sizing: BoxSizing,
    pub(crate) direction: Direction,
    pub(crate) visibility: Visibility,
    pub(crate) width: Length,
    pub(crate) height: Length,
    pub(crate) min_width: Length,
    pub(crate) min_height: Length,
    pub(crate) max_width: Length,
    pub(crate) max_height: Length,
    pub(crate) aspect_ratio: Option<f32>,
    pub(crate) left: Length,
    pub(crate) right: Length,
    pub(crate) top: Length,
    pub(crate) bottom: Length,
    pub(crate) margin: Rect<Length>,
    pub(crate) padding: Rect<Length>,
    pub(crate) border: Rect<f32>,
    pub(crate) flex_direction: FlexDirection,
    pub(crate) flex_wrap: FlexWrap,
    pub(crate) justify_content: JustifyContent,
    pub(crate) align_items: AlignItems,
    pub(crate) align_self: Option<AlignItems>,
    pub(crate) align_content: AlignContent,
    pub(crate) justify_items: JustifyItems,
    pub(crate) justify_self: JustifyItems,
    pub(crate) flex_grow: f32,
    pub(crate) flex_shrink: f32,
    pub(crate) flex_basis: Length,
    pub(crate) order: i32,
    pub(crate) row_gap: Length,
    pub(crate) column_gap: Length,
    pub(crate) linear_orientation: LinearOrientation,
    pub(crate) linear_gravity: LinearGravity,
    pub(crate) linear_layout_gravity: LinearLayoutGravity,
    pub(crate) linear_cross_gravity: LinearCrossGravity,
    pub(crate) linear_weight: f32,
    pub(crate) linear_weight_sum: f32,
    pub(crate) grid_template_columns: Vec<Length>,
    pub(crate) grid_template_rows: Vec<Length>,
    pub(crate) grid_template_columns_max: Vec<Length>,
    pub(crate) grid_template_rows_max: Vec<Length>,
    pub(crate) grid_auto_columns: Vec<Length>,
    pub(crate) grid_auto_rows: Vec<Length>,
    pub(crate) grid_auto_columns_max: Vec<Length>,
    pub(crate) grid_auto_rows_max: Vec<Length>,
    pub(crate) grid_auto_flow: GridAutoFlow,
    pub(crate) grid_column_start: Option<i32>,
    pub(crate) grid_column_end: Option<i32>,
    pub(crate) grid_row_start: Option<i32>,
    pub(crate) grid_row_end: Option<i32>,
    pub(crate) grid_column_span: usize,
    pub(crate) grid_row_span: usize,
    pub(crate) relative_id: i32,
    pub(crate) relative_align_top: i32,
    pub(crate) relative_align_right: i32,
    pub(crate) relative_align_bottom: i32,
    pub(crate) relative_align_left: i32,
    pub(crate) relative_top_of: i32,
    pub(crate) relative_right_of: i32,
    pub(crate) relative_bottom_of: i32,
    pub(crate) relative_left_of: i32,
    pub(crate) relative_layout_once: bool,
    pub(crate) relative_center: RelativeCenter,
}

impl Default for Style {
    fn default() -> Self {
        Self {
            display: Display::Block,
            position: PositionType::Static,
            box_sizing: BoxSizing::ContentBox,
            direction: Direction::Ltr,
            visibility: Visibility::Visible,
            width: Length::Auto,
            height: Length::Auto,
            min_width: Length::Auto,
            min_height: Length::Auto,
            max_width: Length::Auto,
            max_height: Length::Auto,
            aspect_ratio: None,
            left: Length::Auto,
            right: Length::Auto,
            top: Length::Auto,
            bottom: Length::Auto,
            margin: Rect::all(Length::ZERO),
            padding: Rect::all(Length::ZERO),
            border: Rect::all(0.0),
            flex_direction: FlexDirection::Row,
            flex_wrap: FlexWrap::NoWrap,
            justify_content: JustifyContent::Stretch,
            align_items: AlignItems::Stretch,
            align_self: None,
            align_content: AlignContent::Stretch,
            justify_items: JustifyItems::Stretch,
            justify_self: JustifyItems::Auto,
            flex_grow: 0.0,
            flex_shrink: 1.0,
            flex_basis: Length::Auto,
            order: 0,
            row_gap: Length::ZERO,
            column_gap: Length::ZERO,
            linear_orientation: LinearOrientation::Vertical,
            linear_gravity: LinearGravity::None,
            linear_layout_gravity: LinearLayoutGravity::None,
            linear_cross_gravity: LinearCrossGravity::None,
            linear_weight: 0.0,
            linear_weight_sum: 0.0,
            grid_template_columns: Vec::new(),
            grid_template_rows: Vec::new(),
            grid_template_columns_max: Vec::new(),
            grid_template_rows_max: Vec::new(),
            grid_auto_columns: Vec::new(),
            grid_auto_rows: Vec::new(),
            grid_auto_columns_max: Vec::new(),
            grid_auto_rows_max: Vec::new(),
            grid_auto_flow: GridAutoFlow::Row,
            grid_column_start: None,
            grid_column_end: None,
            grid_row_start: None,
            grid_row_end: None,
            grid_column_span: 1,
            grid_row_span: 1,
            relative_id: RELATIVE_ALIGN_NONE,
            relative_align_top: RELATIVE_ALIGN_NONE,
            relative_align_right: RELATIVE_ALIGN_NONE,
            relative_align_bottom: RELATIVE_ALIGN_NONE,
            relative_align_left: RELATIVE_ALIGN_NONE,
            relative_top_of: RELATIVE_ALIGN_NONE,
            relative_right_of: RELATIVE_ALIGN_NONE,
            relative_bottom_of: RELATIVE_ALIGN_NONE,
            relative_left_of: RELATIVE_ALIGN_NONE,
            relative_layout_once: false,
            relative_center: RelativeCenter::None,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct LayoutResult {
    pub(crate) offset: Point,
    pub(crate) size: Size,
    pub(crate) baseline: Option<f32>,
    pub(crate) padding: Edges,
    pub(crate) border: Edges,
    pub(crate) margin: Edges,
    /// Resolved sticky insets retained by this test-only host facade.
    ///
    /// Sticky positioning remains a host post-pass in neutron-star. The core
    /// layout therefore receives an in-flow node with auto visual insets,
    /// while this field preserves the source PR's exported metadata for the
    /// later scroll-time pass.
    pub(crate) sticky_pos: Edges,
}

pub(crate) type SimpleMeasureFunc = fn(Constraints) -> Size;

#[derive(Clone, Debug)]
pub(crate) struct SimpleNode {
    pub(crate) style: Style,
    pub(crate) layout: LayoutResult,
    pub(crate) children: Vec<usize>,
    has_measure: bool,
    measured_size: Option<Size>,
    measure_func: Option<SimpleMeasureFunc>,
    baseline: Option<f32>,
    measurement_profile: Option<MeasurementProfile>,
    measure_trace: Vec<MeasureCall>,
}

impl SimpleNode {
    pub(crate) fn new(style: Style) -> Self {
        Self {
            style,
            layout: LayoutResult::default(),
            children: Vec::new(),
            has_measure: false,
            measured_size: None,
            measure_func: None,
            baseline: None,
            measurement_profile: None,
            measure_trace: Vec::new(),
        }
    }

    pub(crate) fn with_measured_size(style: Style, measured_size: Size) -> Self {
        Self {
            has_measure: true,
            measured_size: Some(measured_size),
            ..Self::new(style)
        }
    }

    pub(crate) fn with_measure_func(style: Style, measure_func: SimpleMeasureFunc) -> Self {
        Self {
            has_measure: true,
            measure_func: Some(measure_func),
            ..Self::new(style)
        }
    }

    /// A measured node whose host callback declines to provide a size.
    ///
    /// PR #25 still routes such a node through measured layout and uses its
    /// zero-size fallback; it must not fall through to display dispatch.
    pub(crate) fn with_null_measure(style: Style) -> Self {
        Self {
            has_measure: true,
            ..Self::new(style)
        }
    }

    pub(crate) fn with_measured_size_and_baseline(
        style: Style,
        measured_size: Size,
        baseline: f32,
    ) -> Self {
        Self {
            has_measure: true,
            measured_size: Some(measured_size),
            baseline: Some(baseline),
            ..Self::new(style)
        }
    }
}

#[derive(Clone, Debug, Default)]
pub(crate) struct SimpleTree {
    pub(crate) nodes: Vec<SimpleNode>,
}

impl SimpleTree {
    pub(crate) fn push(&mut self, node: SimpleNode) -> usize {
        let id = self.nodes.len();
        self.nodes.push(node);
        id
    }

    pub(crate) fn append_child(&mut self, parent: usize, child: usize) {
        self.nodes[parent].children.push(child);
    }
}

pub(crate) trait LayoutTree {
    type NodeId: Copy + Ord;
    type Children<'a>: Iterator<Item = Self::NodeId>
    where
        Self: 'a;

    fn children(&self, node: Self::NodeId) -> Self::Children<'_>;
    fn style(&self, node: Self::NodeId) -> &Style;
    fn set_layout(&mut self, node: Self::NodeId, layout: LayoutResult);
    fn layout(&self, _node: Self::NodeId) -> Option<LayoutResult> {
        None
    }

    fn measure(&mut self, _node: Self::NodeId, _constraints: Constraints) -> Option<Size> {
        None
    }

    fn measure_min_content(
        &mut self,
        node: Self::NodeId,
        constraints: Constraints,
    ) -> Option<Size> {
        self.measure(node, constraints)
    }

    fn measure_max_content(
        &mut self,
        node: Self::NodeId,
        constraints: Constraints,
    ) -> Option<Size> {
        self.measure(node, constraints)
    }

    fn has_measure(&self, _node: Self::NodeId) -> bool {
        false
    }

    fn measure_func(&self, _node: Self::NodeId) -> Option<SimpleMeasureFunc> {
        None
    }

    fn baseline(&self, _node: Self::NodeId, _content_size: Size) -> Option<f32> {
        None
    }

    fn measurement_profile(&self, _node: Self::NodeId) -> Option<MeasurementProfile> {
        None
    }

    fn set_measure_trace(&mut self, _node: Self::NodeId, _trace: &[MeasureCall]) {}
}

impl LayoutTree for SimpleTree {
    type NodeId = usize;
    type Children<'a> = std::iter::Copied<std::slice::Iter<'a, usize>>;

    fn children(&self, node: Self::NodeId) -> Self::Children<'_> {
        self.nodes[node].children.iter().copied()
    }

    fn style(&self, node: Self::NodeId) -> &Style {
        &self.nodes[node].style
    }

    fn set_layout(&mut self, node: Self::NodeId, layout: LayoutResult) {
        self.nodes[node].layout = layout;
    }

    fn layout(&self, node: Self::NodeId) -> Option<LayoutResult> {
        Some(self.nodes[node].layout)
    }

    fn measure(&mut self, node: Self::NodeId, constraints: Constraints) -> Option<Size> {
        let node = &self.nodes[node];
        node.measured_size
            .or_else(|| node.measure_func.map(|measure| measure(constraints)))
    }

    fn has_measure(&self, node: Self::NodeId) -> bool {
        self.nodes[node].has_measure
    }

    fn measure_func(&self, node: Self::NodeId) -> Option<SimpleMeasureFunc> {
        self.nodes[node].measure_func
    }

    fn baseline(&self, node: Self::NodeId, _content_size: Size) -> Option<f32> {
        self.nodes[node].baseline
    }

    fn measurement_profile(&self, node: Self::NodeId) -> Option<MeasurementProfile> {
        self.nodes[node].measurement_profile
    }

    fn set_measure_trace(&mut self, node: Self::NodeId, trace: &[MeasureCall]) {
        self.nodes[node].measure_trace.clear();
        self.nodes[node].measure_trace.extend_from_slice(trace);
    }
}

fn dimension(tree: &mut TestTree, value: Length) -> Dimension {
    match value {
        // `fr` outside grid tracks is a Lynx raw-value extension. The host
        // lowering used by these CSS-focused fixtures treats it as `auto`.
        Length::Auto | Length::Fr(_) => Dimension::Auto,
        Length::Points(value) => Dimension::Length(value),
        Length::Percent(value) => Dimension::Percent(value / 100.0),
        Length::Calc { fixed, percent } => Dimension::Calc(tree.push_calc(fixed, percent / 100.0)),
        Length::MinContent => Dimension::MinContent,
        // The legacy argument-less keyword behaves as an uncapped intrinsic
        // size in the source Flex fixtures.
        Length::MaxContent | Length::FitContent(None) => Dimension::MaxContent,
        Length::FitContent(Some(base)) => {
            Dimension::FitContent(length_percentage_from_base(tree, base))
        }
    }
}

fn minimum_dimension(tree: &mut TestTree, value: Length) -> Dimension {
    if value == Length::FitContent(None) {
        Dimension::Auto
    } else {
        dimension(tree, value)
    }
}

fn maximum_dimension(tree: &mut TestTree, value: Length) -> Dimension {
    if value == Length::FitContent(None) {
        Dimension::Auto
    } else {
        dimension(tree, value)
    }
}

fn preferred_dimension(tree: &mut TestTree, value: Length, position: PositionType) -> Dimension {
    if matches!(position, PositionType::Absolute | PositionType::Fixed)
        && matches!(value, Length::FitContent(_))
    {
        // PR #25's positioned Grid path treats `fit-content(...)` as the
        // legacy uncapped intrinsic keyword. Keep that source-fixture quirk
        // in the compatibility adapter; neutron-star's production protocol
        // retains standards-oriented `Dimension::FitContent` semantics.
        Dimension::MaxContent
    } else {
        dimension(tree, value)
    }
}

fn length_percentage_from_base(tree: &mut TestTree, base: BaseLength) -> LengthPercentage {
    if base.has_percentage {
        LengthPercentage::Calc(tree.push_calc(base.fixed, base.percentage / 100.0))
    } else {
        LengthPercentage::Length(base.fixed)
    }
}

fn length_percentage(tree: &mut TestTree, value: Length) -> LengthPercentage {
    match value {
        Length::Points(value) => LengthPercentage::Length(value),
        Length::Percent(value) => LengthPercentage::Percent(value / 100.0),
        Length::Calc { fixed, percent } => {
            LengthPercentage::Calc(tree.push_calc(fixed, percent / 100.0))
        }
        Length::Auto
        | Length::Fr(_)
        | Length::MinContent
        | Length::MaxContent
        | Length::FitContent(_) => LengthPercentage::ZERO,
    }
}

fn length_percentage_auto(tree: &mut TestTree, value: Length) -> LengthPercentageAuto {
    match value {
        Length::Auto => LengthPercentageAuto::Auto,
        Length::Points(value) => LengthPercentageAuto::Length(value),
        Length::Percent(value) => LengthPercentageAuto::Percent(value / 100.0),
        Length::Calc { fixed, percent } => {
            LengthPercentageAuto::Calc(tree.push_calc(fixed, percent / 100.0))
        }
        Length::Fr(_) | Length::MinContent | Length::MaxContent | Length::FitContent(_) => {
            LengthPercentageAuto::ZERO
        }
    }
}

fn min_track_sizing(tree: &mut TestTree, value: Length) -> MinTrackSizingFunction {
    match value {
        Length::Points(_) | Length::Percent(_) | Length::Calc { .. } => {
            MinTrackSizingFunction::Fixed(length_percentage(tree, value))
        }
        Length::MinContent => MinTrackSizingFunction::MinContent,
        Length::MaxContent => MinTrackSizingFunction::MaxContent,
        // `<flex>` and `fit-content()` are invalid minimums in `minmax()`;
        // source fixtures only use them in the single/max position, whose
        // expansion is handled by `track_sizing` below.
        Length::Auto | Length::Fr(_) | Length::FitContent(_) => MinTrackSizingFunction::Auto,
    }
}

fn max_track_sizing(tree: &mut TestTree, value: Length) -> MaxTrackSizingFunction {
    match value {
        Length::Points(_) | Length::Percent(_) | Length::Calc { .. } => {
            MaxTrackSizingFunction::Fixed(length_percentage(tree, value))
        }
        Length::Auto | Length::FitContent(None) => MaxTrackSizingFunction::Auto,
        Length::MinContent => MaxTrackSizingFunction::MinContent,
        Length::MaxContent => MaxTrackSizingFunction::MaxContent,
        Length::Fr(factor) => MaxTrackSizingFunction::Fr(factor),
        Length::FitContent(Some(base)) => {
            MaxTrackSizingFunction::FitContent(length_percentage_from_base(tree, base))
        }
    }
}

fn track_sizing(
    tree: &mut TestTree,
    minimum: Length,
    maximum: Option<Length>,
) -> TrackSizingFunction {
    if let Some(maximum) = maximum {
        return TrackSizingFunction::minmax(
            min_track_sizing(tree, minimum),
            max_track_sizing(tree, maximum),
        );
    }
    match minimum {
        Length::Points(_) | Length::Percent(_) | Length::Calc { .. } => {
            TrackSizingFunction::fixed(length_percentage(tree, minimum))
        }
        Length::Auto | Length::FitContent(None) => TrackSizingFunction::AUTO,
        Length::MinContent => TrackSizingFunction::minmax(
            MinTrackSizingFunction::MinContent,
            MaxTrackSizingFunction::MinContent,
        ),
        Length::MaxContent => TrackSizingFunction::minmax(
            MinTrackSizingFunction::MaxContent,
            MaxTrackSizingFunction::MaxContent,
        ),
        Length::Fr(factor) => TrackSizingFunction::fr(factor),
        Length::FitContent(Some(base)) => {
            TrackSizingFunction::fit_content(length_percentage_from_base(tree, base))
        }
    }
}

fn track_list(
    tree: &mut TestTree,
    minimums: &[Length],
    maximums: &[Length],
) -> Vec<TrackSizingFunction> {
    minimums
        .iter()
        .copied()
        .enumerate()
        .map(|(index, minimum)| track_sizing(tree, minimum, maximums.get(index).copied()))
        .collect()
}

fn grid_flow(flow: GridAutoFlow) -> NeutronGridAutoFlow {
    match flow {
        GridAutoFlow::Row => NeutronGridAutoFlow::Row,
        GridAutoFlow::Column => NeutronGridAutoFlow::Column,
        GridAutoFlow::Dense | GridAutoFlow::RowDense => NeutronGridAutoFlow::RowDense,
        GridAutoFlow::ColumnDense => NeutronGridAutoFlow::ColumnDense,
    }
}

fn grid_line(value: i32) -> GridPlacement {
    let value = i16::try_from(value).expect("PR #25 numeric grid line fits i16");
    GridPlacement::Line(GridLine::new(value))
}

fn grid_span(value: usize) -> GridPlacement {
    GridPlacement::Span(u16::try_from(value).expect("PR #25 numeric grid span fits u16"))
}

fn grid_placement(
    start: Option<i32>,
    end: Option<i32>,
    span: usize,
    position: PositionType,
) -> Line<GridPlacement> {
    let positioned = matches!(position, PositionType::Absolute | PositionType::Fixed);
    let start_placement = start.map_or_else(
        || {
            if end.is_some() && span > 1 {
                grid_span(span)
            } else {
                GridPlacement::Auto
            }
        },
        grid_line,
    );
    let end_placement = end.map_or_else(
        || {
            if span > 1 || (start.is_some() && !positioned) {
                grid_span(span)
            } else {
                GridPlacement::Auto
            }
        },
        grid_line,
    );
    Line::new(start_placement, end_placement)
}

fn justify_item(value: JustifyItems) -> Option<AlignItems> {
    match value {
        JustifyItems::Auto => None,
        JustifyItems::Stretch => Some(AlignItems::Stretch),
        JustifyItems::Start => Some(AlignItems::Start),
        JustifyItems::Center => Some(AlignItems::Center),
        JustifyItems::End => Some(AlignItems::End),
    }
}

#[allow(clippy::too_many_lines)]
fn convert_style(
    tree: &mut TestTree,
    style: &Style,
    has_children: bool,
    hoist_fixed: bool,
) -> TestStyle {
    let display_mode = match style.display {
        Display::None => BoxGenerationMode::None,
        _ => BoxGenerationMode::Normal,
    };
    let position = match style.position {
        // PR #25's Rust Grid suite exercises fixed children through Grid's
        // direct out-of-flow area path. Lower that source fixture to
        // `Absolute`; neutron-star's real `fixed` integration remains the
        // host-owned hoisted positioned pass and is tested separately.
        PositionType::Fixed if hoist_fixed => Position::AbsoluteHoisted,
        PositionType::Absolute | PositionType::Fixed => Position::Absolute,
        PositionType::Static | PositionType::Relative | PositionType::Sticky => Position::Relative,
    };
    // Sticky participates in normal flow and its authored insets belong to a
    // host scroll-time post-pass. Passing them to neutron-star here would
    // reinterpret Sticky as `position: relative` and visually shift the box.
    // Keep the source values on `Style`; `run_simple_layout` resolves and
    // exports them through the test facade's `LayoutResult::sticky_pos`.
    let inset = if style.position == PositionType::Sticky {
        neutron_star::geometry::Edges {
            left: LengthPercentageAuto::Auto,
            right: LengthPercentageAuto::Auto,
            top: LengthPercentageAuto::Auto,
            bottom: LengthPercentageAuto::Auto,
        }
    } else {
        neutron_star::geometry::Edges {
            left: length_percentage_auto(tree, style.left),
            right: length_percentage_auto(tree, style.right),
            top: length_percentage_auto(tree, style.top),
            bottom: length_percentage_auto(tree, style.bottom),
        }
    };
    let flex_direction = match style.display {
        Display::Block | Display::Relative | Display::Grid if has_children => FlexDirection::Column,
        _ => style.flex_direction,
    };
    let template_columns = track_list(
        tree,
        &style.grid_template_columns,
        &style.grid_template_columns_max,
    );
    let template_rows = track_list(
        tree,
        &style.grid_template_rows,
        &style.grid_template_rows_max,
    );
    let auto_columns = track_list(tree, &style.grid_auto_columns, &style.grid_auto_columns_max);
    let auto_rows = track_list(tree, &style.grid_auto_rows, &style.grid_auto_rows_max);
    TestStyle {
        box_generation_mode: display_mode,
        visibility: match style.visibility {
            Visibility::Visible => neutron_star::style::Visibility::Visible,
            Visibility::Hidden => neutron_star::style::Visibility::Hidden,
            Visibility::Collapse => neutron_star::style::Visibility::Collapse,
        },
        position,
        inset,
        size: NSize::new(
            preferred_dimension(tree, style.width, style.position),
            preferred_dimension(tree, style.height, style.position),
        ),
        min_size: NSize::new(
            minimum_dimension(tree, style.min_width),
            minimum_dimension(tree, style.min_height),
        ),
        max_size: NSize::new(
            maximum_dimension(tree, style.max_width),
            maximum_dimension(tree, style.max_height),
        ),
        aspect_ratio: style.aspect_ratio,
        margin: neutron_star::geometry::Edges {
            left: length_percentage_auto(tree, style.margin.left),
            right: length_percentage_auto(tree, style.margin.right),
            top: length_percentage_auto(tree, style.margin.top),
            bottom: length_percentage_auto(tree, style.margin.bottom),
        },
        padding: neutron_star::geometry::Edges {
            left: length_percentage(tree, style.padding.left),
            right: length_percentage(tree, style.padding.right),
            top: length_percentage(tree, style.padding.top),
            bottom: length_percentage(tree, style.padding.bottom),
        },
        border: neutron_star::geometry::Edges {
            left: LengthPercentage::Length(style.border.left),
            right: LengthPercentage::Length(style.border.right),
            top: LengthPercentage::Length(style.border.top),
            bottom: LengthPercentage::Length(style.border.bottom),
        },
        box_sizing: style.box_sizing,
        direction: style.direction,
        linear_orientation: style.linear_orientation.neutron(),
        linear_gravity: style.linear_gravity,
        linear_layout_gravity: style.linear_layout_gravity,
        linear_cross_gravity: style.linear_cross_gravity,
        linear_weight: style.linear_weight,
        linear_weight_sum: style.linear_weight_sum,
        flex_direction,
        flex_wrap: style.flex_wrap,
        gap: NSize::new(
            length_percentage(tree, style.column_gap),
            length_percentage(tree, style.row_gap),
        ),
        align_content: Some(style.align_content),
        // The source fixtures model Lynx computed defaults (`overflow:
        // hidden`) rather than CSS parser initials. Supplying that value at
        // the host boundary keeps automatic flex minimums faithful without
        // baking a Lynx default into neutron-star.
        overflow: neutron_star::geometry::Point::new(
            neutron_star::style::Overflow::Hidden,
            neutron_star::style::Overflow::Hidden,
        ),
        align_items: Some(style.align_items),
        justify_content: Some(style.justify_content),
        flex_basis: dimension(tree, style.flex_basis),
        flex_grow: style.flex_grow,
        flex_shrink: style.flex_shrink,
        align_self: style.align_self,
        order: style.order,
        template_columns,
        template_rows,
        auto_columns,
        auto_rows,
        auto_flow: grid_flow(style.grid_auto_flow),
        justify_items: justify_item(style.justify_items),
        justify_self: justify_item(style.justify_self),
        grid_column: grid_placement(
            style.grid_column_start,
            style.grid_column_end,
            style.grid_column_span,
            style.position,
        ),
        grid_row: grid_placement(
            style.grid_row_start,
            style.grid_row_end,
            style.grid_row_span,
            style.position,
        ),
        relative_layout_once: style.relative_layout_once,
        relative_id: RelativeReference::new(style.relative_id),
        relative_align: neutron_star::geometry::Edges {
            left: RelativeReference::new(style.relative_align_left),
            right: RelativeReference::new(style.relative_align_right),
            top: RelativeReference::new(style.relative_align_top),
            bottom: RelativeReference::new(style.relative_align_bottom),
        },
        relative_adjacent: neutron_star::geometry::Edges {
            left: RelativeReference::new(style.relative_left_of),
            right: RelativeReference::new(style.relative_right_of),
            top: RelativeReference::new(style.relative_top_of),
            bottom: RelativeReference::new(style.relative_bottom_of),
        },
        relative_center: style.relative_center,
        ..TestStyle::default()
    }
}

fn lower_tree(tree: &SimpleTree) -> TestTree {
    let mut lowered = TestTree::default();
    let hoisted_fixed = hoisted_fixed_mask(tree);
    let mut grid_child = vec![false; tree.nodes.len()];
    for node in &tree.nodes {
        if node.style.display == Display::Grid {
            for &child in &node.children {
                grid_child[child] = true;
            }
        }
    }
    for (node_index, node) in tree.nodes.iter().enumerate() {
        let style = convert_style(
            &mut lowered,
            &node.style,
            !node.children.is_empty(),
            hoisted_fixed[node_index],
        );
        // PR #25 selects the measured path before display dispatch. Otherwise
        // its effective-style step maps every Block box, including an empty
        // one, to Linear. Mirror that ordering instead of inferring leafness
        // from child count or the authored display value.
        let has_measure =
            node.has_measure || node.measure_func.is_some() || node.measurement_profile.is_some();
        let display = if has_measure {
            TestDisplay::Leaf
        } else {
            match node.style.display {
                Display::Grid => TestDisplay::Grid,
                Display::Flex => TestDisplay::Flex,
                Display::Linear | Display::Block => TestDisplay::Linear,
                Display::Relative => TestDisplay::Relative,
                Display::None => TestDisplay::Leaf,
            }
        };
        let measure = if let Some(profile) = node.measurement_profile {
            TestMeasure::Profile(profile)
        } else if let Some(measure) = node.measure_func {
            TestMeasure::ConstraintFunction {
                measure,
                baseline: None,
            }
        } else if let Some(intrinsic) = node.measured_size
            && grid_child[node_index]
        {
            let minimum = Size::new(
                match node.style.min_width {
                    Length::Points(value) => value,
                    _ => 0.0,
                },
                match node.style.min_height {
                    Length::Points(value) => value,
                    _ => 0.0,
                },
            );
            TestMeasure::Profile(MeasurementProfile {
                regular: Some(RegularMeasure::Fixed(intrinsic)),
                min_content: Some(IntrinsicMeasureSpec::Fixed(minimum)),
                max_content: Some(IntrinsicMeasureSpec::Fixed(intrinsic)),
                first_baseline: node.baseline,
            })
        } else if let Some(intrinsic) = node.measured_size {
            // Preserve the pre-Grid PR adapter contract for Flex and foreign
            // layout children: a fixed measurement is also their min- and
            // max-content contribution.
            TestMeasure::Intrinsic {
                min_content_size: intrinsic,
                max_content_size: intrinsic,
                first_baseline: node.baseline,
            }
        } else {
            TestMeasure::Intrinsic {
                min_content_size: Size::ZERO,
                max_content_size: Size::ZERO,
                first_baseline: node.baseline,
            }
        };
        lowered.push(TestSourceNode {
            display,
            style,
            children: Vec::new(),
            measure,
        });
    }
    for (index, node) in tree.nodes.iter().enumerate() {
        lowered.source.nodes[index].children =
            node.children.iter().copied().map(NodeId::from).collect();
        if node.style.display == Display::Grid {
            for &child in &node.children {
                // PR #25's Grid fixture surface has no overflow property.
                // Use CSS's visible initial value so the automatic-minimum
                // rules remain Grid-correct; Flex fixtures retain the Lynx
                // computed hidden default supplied above.
                lowered.source.nodes[child].style.overflow = neutron_star::geometry::Point::new(
                    neutron_star::style::Overflow::Visible,
                    neutron_star::style::Overflow::Visible,
                );
            }
        }
    }
    lowered
}

fn hoisted_fixed_mask(tree: &SimpleTree) -> Vec<bool> {
    fn visit(tree: &SimpleTree, node: usize, inside_host_pass: bool, mask: &mut [bool]) {
        let inside_host_pass = inside_host_pass
            || matches!(
                tree.nodes[node].style.display,
                Display::Block | Display::Linear | Display::Relative
            );
        for &child in &tree.nodes[node].children {
            mask[child] =
                inside_host_pass && tree.nodes[child].style.position == PositionType::Fixed;
            visit(tree, child, inside_host_pass, mask);
        }
    }

    let mut is_child = vec![false; tree.nodes.len()];
    for node in &tree.nodes {
        for &child in &node.children {
            is_child[child] = true;
        }
    }
    let mut mask = vec![false; tree.nodes.len()];
    for (root, is_child) in is_child.iter().copied().enumerate() {
        if !is_child {
            visit(tree, root, false, &mut mask);
        }
    }
    mask
}

fn finish_relative_fixed_pass(
    tree: &SimpleTree,
    lowered: &mut TestTree,
    root: usize,
    root_size: Size,
) -> Vec<Option<Layout>> {
    fn collect(
        tree: &SimpleTree,
        lowered: &TestTree,
        node: usize,
        global: Point,
        parents: &mut [Option<usize>],
        globals: &mut [Point],
    ) {
        globals[node] = global;
        for &child in &tree.nodes[node].children {
            parents[child] = Some(node);
            let location = lowered.session.nodes[child].layout.location;
            collect(
                tree,
                lowered,
                child,
                Point::new(global.x + location.x, global.y + location.y),
                parents,
                globals,
            );
        }
    }

    let mut parents = vec![None; tree.nodes.len()];
    let mut globals = vec![Point::ZERO; tree.nodes.len()];
    collect(tree, lowered, root, Point::ZERO, &mut parents, &mut globals);

    let root_border = tree.nodes[root].style.border;
    let containing_size = Size::new(
        (root_size.width - root_border.left - root_border.right).max(0.0),
        (root_size.height - root_border.top - root_border.bottom).max(0.0),
    );
    let root_padding_origin = Point::new(root_border.left, root_border.top);
    let mut exposed = vec![None; tree.nodes.len()];

    for index in 0..tree.nodes.len() {
        if tree.nodes[index].style.position != PositionType::Fixed
            || lowered.source.nodes[index].style.position != Position::AbsoluteHoisted
            || lowered.source.nodes[index].style.box_generation_mode == BoxGenerationMode::None
        {
            continue;
        }
        let Some(static_position) = lowered.session.nodes[index].static_position else {
            continue;
        };
        let parent_global = parents[index].map_or(Point::ZERO, |parent| globals[parent]);
        let static_in_root_padding = Point::new(
            parent_global.x + static_position.x - root_padding_origin.x,
            parent_global.y + static_position.y - root_padding_origin.y,
        );
        let layout = compute_absolute_layout(
            &lowered.source,
            &mut lowered.session,
            NodeId::from(index),
            containing_size,
            static_in_root_padding,
        );
        exposed[index] = Some(layout);

        let mut stored = layout;
        stored.location = Point::new(
            root_padding_origin.x + layout.location.x - parent_global.x,
            root_padding_origin.y + layout.location.y - parent_global.y,
        );
        lowered
            .session
            .set_unrounded_layout(NodeId::from(index), &stored);
        globals[index] = Point::new(
            parent_global.x + stored.location.x,
            parent_global.y + stored.location.y,
        );
    }
    exposed
}

fn constraint_space(side: SideConstraint) -> AvailableSpace {
    match side.mode {
        MeasureMode::Indefinite => AvailableSpace::MaxContent,
        MeasureMode::Definite | MeasureMode::AtMost => AvailableSpace::Definite(side.size),
    }
}

fn known_dimensions(
    constraints: Constraints,
    owner_constraints: bool,
    style: &Style,
) -> NSize<Option<f32>> {
    if owner_constraints {
        return NSize::NONE;
    }
    NSize::new(
        (constraints.width.mode == MeasureMode::Definite
            && (style.display != Display::Grid || style.width == Length::Auto))
            .then_some(constraints.width.size),
        (constraints.height.mode == MeasureMode::Definite
            && (style.display != Display::Grid || style.height == Length::Auto))
            .then_some(constraints.height.size),
    )
}

fn resolve_authored_length(length: Length, percent_base: Option<f32>) -> Option<f32> {
    match length {
        Length::Auto | Length::MinContent | Length::MaxContent | Length::FitContent(None) => None,
        Length::Points(value) | Length::Fr(value) => Some(value),
        Length::Percent(value) => percent_base.map(|base| base * (value / 100.0)),
        Length::Calc { fixed, percent } => percent_base
            .map(|base| fixed + base * (percent / 100.0))
            .or_else(|| (percent == 0.0).then_some(fixed)),
        Length::FitContent(Some(base)) => {
            if base.has_percentage {
                percent_base.map(|value| base.fixed + value * (base.percentage / 100.0))
            } else {
                Some(base.fixed)
            }
        }
    }
}

fn resolve_sticky_inset(length: Length, percent_base: Option<f32>) -> f32 {
    resolve_authored_length(length, percent_base).unwrap_or(STICKY_AUTO_INSET)
}

fn resolved_sticky_pos(style: &Style, percent_base: NSize<Option<f32>>) -> Edges {
    if style.position != PositionType::Sticky {
        return Edges::default();
    }
    Edges {
        left: resolve_sticky_inset(style.left, percent_base.width),
        right: resolve_sticky_inset(style.right, percent_base.width),
        top: resolve_sticky_inset(style.top, percent_base.height),
        bottom: resolve_sticky_inset(style.bottom, percent_base.height),
    }
}

fn content_percent_base(layout: &Layout) -> NSize<Option<f32>> {
    NSize::new(
        Some(
            (layout.size.width
                - layout.padding.left
                - layout.padding.right
                - layout.border.left
                - layout.border.right)
                .max(0.0),
        ),
        Some(
            (layout.size.height
                - layout.padding.top
                - layout.padding.bottom
                - layout.border.top
                - layout.border.bottom)
                .max(0.0),
        ),
    )
}

fn root_sticky_percent_base(constraints: Constraints) -> NSize<Option<f32>> {
    NSize::new(
        constraints
            .width
            .is_definite()
            .then_some(constraints.width.size),
        constraints
            .height
            .is_definite()
            .then_some(constraints.height.size),
    )
}

fn root_content_percent_base(
    style: &Style,
    border_box_size: Size,
    constraints: Constraints,
) -> NSize<Option<f32>> {
    let inline_basis = constraints
        .width
        .is_definite()
        .then_some(constraints.width.size);
    let padding_left = resolve_authored_length(style.padding.left, inline_basis).unwrap_or(0.0);
    let padding_right = resolve_authored_length(style.padding.right, inline_basis).unwrap_or(0.0);
    let padding_top = resolve_authored_length(style.padding.top, inline_basis).unwrap_or(0.0);
    let padding_bottom = resolve_authored_length(style.padding.bottom, inline_basis).unwrap_or(0.0);
    NSize::new(
        Some(
            (border_box_size.width
                - padding_left
                - padding_right
                - style.border.left
                - style.border.right)
                .max(0.0),
        ),
        Some(
            (border_box_size.height
                - padding_top
                - padding_bottom
                - style.border.top
                - style.border.bottom)
                .max(0.0),
        ),
    )
}

fn resolve_lowered_length_percentage(
    tree: &TestTree,
    value: LengthPercentage,
    inline_basis: Option<f32>,
) -> f32 {
    match value {
        LengthPercentage::Length(value) => value,
        LengthPercentage::Percent(fraction) => inline_basis.map_or(0.0, |basis| basis * fraction),
        LengthPercentage::Calc(handle) => {
            inline_basis.map_or(0.0, |basis| tree.source.resolve_calc(handle, basis))
        }
    }
}

fn resolve_lowered_length_percentage_auto(
    tree: &TestTree,
    value: LengthPercentageAuto,
    inline_basis: Option<f32>,
) -> f32 {
    match value {
        LengthPercentageAuto::Length(value) => value,
        LengthPercentageAuto::Percent(fraction) => {
            inline_basis.map_or(0.0, |basis| basis * fraction)
        }
        LengthPercentageAuto::Calc(handle) => {
            inline_basis.map_or(0.0, |basis| tree.source.resolve_calc(handle, basis))
        }
        LengthPercentageAuto::Auto => 0.0,
    }
}

fn commit_root_box(
    tree: &mut TestTree,
    root: usize,
    output: LayoutOutput,
    inline_basis: Option<f32>,
) {
    if tree.source.nodes[root].style.box_generation_mode == BoxGenerationMode::None {
        return;
    }

    let style = &tree.source.nodes[root].style;
    let margin = style
        .margin
        .map(|value| resolve_lowered_length_percentage_auto(tree, value, inline_basis));
    let padding = style
        .padding
        .map(|value| resolve_lowered_length_percentage(tree, value, inline_basis).max(0.0));
    let border = style
        .border
        .map(|value| resolve_lowered_length_percentage(tree, value, inline_basis).max(0.0));

    let mut layout = Layout::default();
    layout.size = output.size;
    layout.content_size = output.content_size;
    layout.margin = margin;
    layout.padding = padding;
    layout.border = border;
    tree.session.nodes[root].layout = layout;
}

fn hidden_subtree_mask(tree: &SimpleTree, root: usize) -> Vec<bool> {
    fn visit(tree: &SimpleTree, node: usize, ancestor_hidden: bool, hidden: &mut [bool]) {
        let node_hidden = ancestor_hidden || tree.nodes[node].style.display == Display::None;
        hidden[node] = node_hidden;
        for &child in &tree.nodes[node].children {
            visit(tree, child, node_hidden, hidden);
        }
    }

    // Nodes outside the requested root's subtree were not laid out either;
    // suppress host metadata for them just as for an explicitly hidden box.
    let mut hidden = vec![true; tree.nodes.len()];
    visit(tree, root, false, &mut hidden);
    hidden
}

fn run_simple_layout(
    tree: &mut SimpleTree,
    root: usize,
    constraints: Constraints,
    owner_constraints: bool,
) -> Size {
    let mut lowered = lower_tree(tree);
    let available = NSize::new(
        constraint_space(constraints.width),
        constraint_space(constraints.height),
    );
    let root_id = NodeId::from(root);
    let known_dimensions =
        known_dimensions(constraints, owner_constraints, &tree.nodes[root].style);
    let parent_size = available.into_options();
    let output = lowered.session.compute_child_layout(
        &lowered.source,
        root_id,
        LayoutInput::perform_layout(known_dimensions, parent_size, available),
    );
    commit_root_box(&mut lowered, root, output, parent_size.width);
    let exposed_fixed = finish_relative_fixed_pass(tree, &mut lowered, root, output.size);
    let root_child_sticky_percent_base =
        root_content_percent_base(&tree.nodes[root].style, output.size, constraints);
    let mut relative_item = vec![false; tree.nodes.len()];
    let mut parents = vec![None; tree.nodes.len()];
    let hidden_subtree = hidden_subtree_mask(tree, root);
    for (parent_index, parent) in tree.nodes.iter().enumerate() {
        if parent.style.display == Display::Relative {
            for &child in &parent.children {
                relative_item[child] = true;
            }
        }
        for &child in &parent.children {
            parents[child] = Some(parent_index);
        }
    }

    for (index, node) in tree.nodes.iter_mut().enumerate() {
        let session = &lowered.session.nodes[index];
        let sticky_percent_base = match parents[index] {
            None => root_sticky_percent_base(constraints),
            Some(parent) if parent == root => root_child_sticky_percent_base,
            Some(parent) => content_percent_base(&lowered.session.nodes[parent].layout),
        };
        node.layout = LayoutResult {
            offset: exposed_fixed[index].map_or(session.layout.location, |layout| layout.location),
            size: session.layout.size,
            baseline: if hidden_subtree[index] {
                None
            } else {
                session
                    .output
                    .first_baselines
                    .y
                    .or_else(|| (!relative_item[index]).then_some(session.layout.size.height))
            },
            padding: Rect::new(
                session.layout.padding.left,
                session.layout.padding.right,
                session.layout.padding.top,
                session.layout.padding.bottom,
            ),
            border: Rect::new(
                session.layout.border.left,
                session.layout.border.right,
                session.layout.border.top,
                session.layout.border.bottom,
            ),
            margin: Rect::new(
                session.layout.margin.left,
                session.layout.margin.right,
                session.layout.margin.top,
                session.layout.margin.bottom,
            ),
            sticky_pos: if hidden_subtree[index] {
                Edges::default()
            } else {
                resolved_sticky_pos(&node.style, sticky_percent_base)
            },
        };
        node.measure_trace.clear();
        node.measure_trace
            .extend_from_slice(&lowered.session.nodes[index].measure_calls);
    }
    tree.nodes[root].layout.size = output.size;
    tree.nodes[root].layout.baseline = output.first_baselines.y;
    output.size
}

pub(crate) fn run_rust_layout<T: LayoutTree>(
    tree: &mut T,
    root: T::NodeId,
    constraints: Constraints,
) -> Size {
    run_layout_tree(tree, root, constraints, false)
}

#[derive(Debug, Default)]
pub(crate) struct LayoutEngine;

impl LayoutEngine {
    pub(crate) const fn new() -> Self {
        Self
    }

    #[allow(clippy::unused_self)] // Mirrors the source fixture's engine-shaped API.
    pub(crate) fn layout_with_owner_constraints<T: LayoutTree>(
        &mut self,
        tree: &mut T,
        root: T::NodeId,
        constraints: Constraints,
    ) -> Size {
        run_layout_tree(tree, root, constraints, true)
    }
}

fn run_layout_tree<T: LayoutTree>(
    tree: &mut T,
    root: T::NodeId,
    constraints: Constraints,
    owner_constraints: bool,
) -> Size {
    fn copy_subtree<T: LayoutTree>(
        tree: &mut T,
        node: T::NodeId,
        snapshot: &mut SimpleTree,
        node_map: &mut BTreeMap<T::NodeId, usize>,
    ) -> usize {
        if let Some(&mapped) = node_map.get(&node) {
            return mapped;
        }

        let style = tree.style(node).clone();
        let children = tree.children(node).collect::<Vec<_>>();
        let has_measure = tree.has_measure(node);
        let measure_func = tree.measure_func(node);
        let measurement_profile = tree.measurement_profile(node);
        let measured_size =
            if has_measure && measure_func.is_none() && measurement_profile.is_none() {
                tree.measure(node, Constraints::indefinite())
            } else {
                None
            };
        let baseline = measured_size.and_then(|size| tree.baseline(node, size));
        let mapped = snapshot.push(SimpleNode {
            style,
            layout: LayoutResult::default(),
            children: Vec::new(),
            has_measure,
            measured_size,
            measure_func,
            baseline,
            measurement_profile,
            measure_trace: Vec::new(),
        });
        node_map.insert(node, mapped);

        for child in children {
            let mapped_child = copy_subtree(tree, child, snapshot, node_map);
            snapshot.append_child(mapped, mapped_child);
        }
        mapped
    }

    let mut snapshot = SimpleTree::default();
    let mut node_map = BTreeMap::new();
    let mapped_root = copy_subtree(tree, root, &mut snapshot, &mut node_map);
    let size = run_simple_layout(&mut snapshot, mapped_root, constraints, owner_constraints);
    for (node, mapped) in node_map {
        tree.set_layout(node, snapshot.nodes[mapped].layout);
        tree.set_measure_trace(node, &snapshot.nodes[mapped].measure_trace);
    }
    size
}

// Keep the imported names visible so the compatibility type aliases are
// checked against the shared host's exact constraint representation.
const _: Option<(
    TestConstraints,
    TestSideConstraint,
    TestMeasureMode,
    CalcHandle,
)> = None;
