//! Shared styling-engine-free host for neutron-star integration tests and benchmarks.

// Every integration-test or benchmark target includes this module separately, and each target
// intentionally uses only a subset of the helpers.
#![allow(dead_code)]

use neutron_star::compute::{
    FnLeafMeasurer, LeafMeasureInput, LeafMetrics, compute_cached_layout, compute_flexbox_layout,
    compute_grid_layout, compute_leaf_layout, compute_linear_layout, compute_relative_layout,
    hide_subtree,
};
use neutron_star::prelude::*;
use neutron_star::style::{
    AlignContent, AlignItems, AlignSelf, BoxGenerationMode, BoxSizing, CalcHandle, Dimension,
    Direction, FlexDirection, FlexWrap, GridAutoFlow, GridPlacement, GridTemplateComponent,
    JustifyContent, JustifyItems, JustifySelf, LengthPercentage, LengthPercentageAuto,
    LinearContainerStyle, LinearCrossGravity, LinearGravity, LinearItemStyle, LinearLayoutGravity,
    LinearOrientation, Overflow, Position, RelativeCenter, RelativeContainerStyle,
    RelativeItemStyle, RelativeReference, RepetitionCount, TrackSizingFunction, Visibility,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TestDisplay {
    Flex,
    Grid,
    Linear,
    Relative,
    Leaf,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct NoRepetition;

impl GridTemplateRepetition for NoRepetition {
    type Tracks<'a> = std::iter::Empty<TrackSizingFunction>;

    fn count(&self) -> RepetitionCount {
        RepetitionCount::Count(1)
    }

    fn tracks(&self) -> Self::Tracks<'_> {
        std::iter::empty()
    }
}

fn single_track(track: TrackSizingFunction) -> GridTemplateComponent<NoRepetition> {
    GridTemplateComponent::Single(track)
}

type TemplateTracks<'a> = std::iter::Map<
    std::iter::Copied<std::slice::Iter<'a, TrackSizingFunction>>,
    fn(TrackSizingFunction) -> GridTemplateComponent<NoRepetition>,
>;

#[derive(Debug, Clone)]
pub(super) struct TestStyle {
    pub(super) box_generation_mode: BoxGenerationMode,
    pub(super) visibility: Visibility,
    pub(super) position: Position,
    pub(super) inset: Edges<LengthPercentageAuto>,
    pub(super) size: Size<Dimension>,
    pub(super) min_size: Size<Dimension>,
    pub(super) max_size: Size<Dimension>,
    pub(super) aspect_ratio: Option<f32>,
    pub(super) margin: Edges<LengthPercentageAuto>,
    pub(super) padding: Edges<LengthPercentage>,
    pub(super) border: Edges<LengthPercentage>,
    pub(super) overflow: Point<Overflow>,
    pub(super) scrollbar_width: f32,
    pub(super) box_sizing: BoxSizing,
    pub(super) direction: Direction,
    pub(super) linear_orientation: LinearOrientation,
    pub(super) linear_gravity: LinearGravity,
    pub(super) linear_cross_gravity: LinearCrossGravity,
    pub(super) linear_weight_sum: f32,
    pub(super) flex_direction: FlexDirection,
    pub(super) flex_wrap: FlexWrap,
    pub(super) gap: Size<LengthPercentage>,
    pub(super) align_content: Option<AlignContent>,
    pub(super) align_items: Option<AlignItems>,
    pub(super) justify_content: Option<JustifyContent>,
    pub(super) flex_basis: Dimension,
    pub(super) flex_grow: f32,
    pub(super) flex_shrink: f32,
    pub(super) linear_layout_gravity: LinearLayoutGravity,
    pub(super) linear_weight: f32,
    pub(super) align_self: Option<AlignSelf>,
    pub(super) order: i32,
    pub(super) template_rows: Vec<TrackSizingFunction>,
    pub(super) template_columns: Vec<TrackSizingFunction>,
    pub(super) auto_rows: Vec<TrackSizingFunction>,
    pub(super) auto_columns: Vec<TrackSizingFunction>,
    pub(super) auto_flow: GridAutoFlow,
    pub(super) justify_items: Option<JustifyItems>,
    pub(super) grid_row: Line<GridPlacement>,
    pub(super) grid_column: Line<GridPlacement>,
    pub(super) justify_self: Option<JustifySelf>,
    pub(super) relative_layout_once: bool,
    pub(super) relative_id: RelativeReference,
    pub(super) relative_align: Edges<RelativeReference>,
    pub(super) relative_adjacent: Edges<RelativeReference>,
    pub(super) relative_center: RelativeCenter,
}

impl Default for TestStyle {
    fn default() -> Self {
        Self {
            box_generation_mode: BoxGenerationMode::Normal,
            visibility: Visibility::Visible,
            position: Position::Relative,
            inset: Edges::uniform(LengthPercentageAuto::Auto),
            size: Size::new(Dimension::Auto, Dimension::Auto),
            min_size: Size::new(Dimension::Auto, Dimension::Auto),
            max_size: Size::new(Dimension::Auto, Dimension::Auto),
            aspect_ratio: None,
            margin: Edges::uniform(LengthPercentageAuto::ZERO),
            padding: Edges::uniform(LengthPercentage::ZERO),
            border: Edges::uniform(LengthPercentage::ZERO),
            overflow: Point::new(Overflow::Visible, Overflow::Visible),
            scrollbar_width: 0.0,
            box_sizing: BoxSizing::ContentBox,
            direction: Direction::Ltr,
            linear_orientation: LinearOrientation::Vertical,
            linear_gravity: LinearGravity::None,
            linear_cross_gravity: LinearCrossGravity::None,
            linear_weight_sum: 0.0,
            flex_direction: FlexDirection::Row,
            flex_wrap: FlexWrap::NoWrap,
            gap: Size::new(LengthPercentage::ZERO, LengthPercentage::ZERO),
            align_content: None,
            align_items: None,
            justify_content: None,
            flex_basis: Dimension::Auto,
            flex_grow: 0.0,
            flex_shrink: 1.0,
            linear_layout_gravity: LinearLayoutGravity::None,
            linear_weight: 0.0,
            align_self: None,
            order: 0,
            template_rows: Vec::new(),
            template_columns: Vec::new(),
            auto_rows: Vec::new(),
            auto_columns: Vec::new(),
            auto_flow: GridAutoFlow::Row,
            justify_items: None,
            grid_row: Line::new(GridPlacement::Auto, GridPlacement::Auto),
            grid_column: Line::new(GridPlacement::Auto, GridPlacement::Auto),
            justify_self: None,
            relative_layout_once: false,
            relative_id: RelativeReference::NONE,
            relative_align: Edges::uniform(RelativeReference::NONE),
            relative_adjacent: Edges::uniform(RelativeReference::NONE),
            relative_center: RelativeCenter::None,
        }
    }
}

impl CoreStyle for TestStyle {
    fn box_generation_mode(&self) -> BoxGenerationMode {
        self.box_generation_mode
    }

    fn visibility(&self) -> Visibility {
        self.visibility
    }

    fn position(&self) -> Position {
        self.position
    }

    fn inset(&self) -> Edges<LengthPercentageAuto> {
        self.inset
    }

    fn size(&self) -> Size<Dimension> {
        self.size
    }

    fn min_size(&self) -> Size<Dimension> {
        self.min_size
    }

    fn max_size(&self) -> Size<Dimension> {
        self.max_size
    }

    fn aspect_ratio(&self) -> Option<f32> {
        self.aspect_ratio
    }

    fn margin(&self) -> Edges<LengthPercentageAuto> {
        self.margin
    }

    fn padding(&self) -> Edges<LengthPercentage> {
        self.padding
    }

    fn border(&self) -> Edges<LengthPercentage> {
        self.border
    }

    fn overflow(&self) -> Point<Overflow> {
        self.overflow
    }

    fn scrollbar_width(&self) -> f32 {
        self.scrollbar_width
    }

    fn box_sizing(&self) -> BoxSizing {
        self.box_sizing
    }

    fn direction(&self) -> Direction {
        self.direction
    }
}

impl LinearContainerStyle for TestStyle {
    fn linear_orientation(&self) -> LinearOrientation {
        self.linear_orientation
    }

    fn linear_gravity(&self) -> LinearGravity {
        self.linear_gravity
    }

    fn linear_cross_gravity(&self) -> LinearCrossGravity {
        self.linear_cross_gravity
    }

    fn linear_weight_sum(&self) -> f32 {
        self.linear_weight_sum
    }

    fn justify_content(&self) -> Option<JustifyContent> {
        self.justify_content
    }

    fn align_items(&self) -> Option<AlignItems> {
        self.align_items
    }
}

impl LinearItemStyle for TestStyle {
    fn linear_layout_gravity(&self) -> LinearLayoutGravity {
        self.linear_layout_gravity
    }

    fn linear_weight(&self) -> f32 {
        self.linear_weight
    }

    fn align_self(&self) -> Option<AlignSelf> {
        self.align_self
    }

    fn order(&self) -> i32 {
        self.order
    }
}

impl FlexContainerStyle for TestStyle {
    fn flex_direction(&self) -> FlexDirection {
        self.flex_direction
    }

    fn flex_wrap(&self) -> FlexWrap {
        self.flex_wrap
    }

    fn gap(&self) -> Size<LengthPercentage> {
        self.gap
    }

    fn align_content(&self) -> Option<AlignContent> {
        self.align_content
    }

    fn align_items(&self) -> Option<AlignItems> {
        self.align_items
    }

    fn justify_content(&self) -> Option<JustifyContent> {
        self.justify_content
    }
}

impl FlexItemStyle for TestStyle {
    fn flex_basis(&self) -> Dimension {
        self.flex_basis
    }

    fn flex_grow(&self) -> f32 {
        self.flex_grow
    }

    fn flex_shrink(&self) -> f32 {
        self.flex_shrink
    }

    fn align_self(&self) -> Option<AlignSelf> {
        self.align_self
    }

    fn order(&self) -> i32 {
        self.order
    }
}

impl GridContainerStyle for TestStyle {
    type Repetition<'a> = NoRepetition;
    type TemplateTracks<'a> = TemplateTracks<'a>;
    type AutoTracks<'a> = std::iter::Copied<std::slice::Iter<'a, TrackSizingFunction>>;

    fn grid_template_rows(&self) -> Self::TemplateTracks<'_> {
        self.template_rows.iter().copied().map(single_track as _)
    }

    fn grid_template_columns(&self) -> Self::TemplateTracks<'_> {
        self.template_columns.iter().copied().map(single_track as _)
    }

    fn grid_auto_rows(&self) -> Self::AutoTracks<'_> {
        self.auto_rows.iter().copied()
    }

    fn grid_auto_columns(&self) -> Self::AutoTracks<'_> {
        self.auto_columns.iter().copied()
    }

    fn grid_auto_flow(&self) -> GridAutoFlow {
        self.auto_flow
    }

    fn gap(&self) -> Size<LengthPercentage> {
        self.gap
    }

    fn align_content(&self) -> Option<AlignContent> {
        self.align_content
    }

    fn justify_content(&self) -> Option<JustifyContent> {
        self.justify_content
    }

    fn align_items(&self) -> Option<AlignItems> {
        self.align_items
    }

    fn justify_items(&self) -> Option<JustifyItems> {
        self.justify_items
    }
}

impl GridItemStyle for TestStyle {
    fn grid_row(&self) -> Line<GridPlacement> {
        self.grid_row
    }

    fn grid_column(&self) -> Line<GridPlacement> {
        self.grid_column
    }

    fn align_self(&self) -> Option<AlignSelf> {
        self.align_self
    }

    fn justify_self(&self) -> Option<JustifySelf> {
        self.justify_self
    }

    fn order(&self) -> i32 {
        self.order
    }
}

impl RelativeContainerStyle for TestStyle {
    fn relative_layout_once(&self) -> bool {
        self.relative_layout_once
    }
}

impl RelativeItemStyle for TestStyle {
    fn relative_id(&self) -> RelativeReference {
        self.relative_id
    }

    fn relative_align(&self) -> Edges<RelativeReference> {
        self.relative_align
    }

    fn relative_adjacent(&self) -> Edges<RelativeReference> {
        self.relative_adjacent
    }

    fn relative_center(&self) -> RelativeCenter {
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

#[derive(Debug, Clone)]
pub(super) struct TestSourceNode {
    pub(super) display: TestDisplay,
    pub(super) style: TestStyle,
    pub(super) children: Vec<NodeId>,
    pub(super) measure: TestMeasure,
}

#[derive(Debug, Clone, Default)]
pub(super) struct TestSessionNode {
    pub(super) layout: Layout,
    pub(super) final_layout: Layout,
    pub(super) static_position: Option<Point<f32>>,
    pub(super) output: LayoutOutput,
    pub(super) measure_calls: Vec<TestMeasureCall>,
}

#[derive(Debug, Clone, Copy)]
struct TestCalc {
    length: f32,
    percentage: f32,
}

#[derive(Debug, Default)]
pub(super) struct TestSource {
    pub(super) nodes: Vec<TestSourceNode>,
    calcs: Vec<TestCalc>,
}

#[derive(Debug, Default)]
pub(super) struct TestSession {
    pub(super) nodes: Vec<TestSessionNode>,
    pub(super) child_layout_calls: usize,
    pub(super) layout_writes: usize,
    pub(super) leaf_measure_calls: usize,
}

/// Builder and assertion facade; layout receives its fields separately.
#[derive(Debug, Default)]
pub(super) struct TestTree {
    pub(super) source: TestSource,
    pub(super) session: TestSession,
}

impl TestTree {
    pub(super) fn push_leaf(
        &mut self,
        style: TestStyle,
        intrinsic_size: Size<f32>,
        first_baseline: Option<f32>,
    ) -> NodeId {
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
    ) -> NodeId {
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
    ) -> NodeId {
        self.push(TestSourceNode {
            display: TestDisplay::Leaf,
            style,
            children: Vec::new(),
            measure: TestMeasure::Function(measure),
        })
    }

    pub(super) fn set_leaf_measure(&mut self, node: NodeId, measure: TestMeasure) {
        let source_node = self.source_node_mut(node);
        assert_eq!(source_node.display, TestDisplay::Leaf);
        source_node.measure = measure;
    }

    pub(super) fn push_flex(&mut self, style: TestStyle, children: Vec<NodeId>) -> NodeId {
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

    pub(super) fn push_grid(&mut self, style: TestStyle, children: Vec<NodeId>) -> NodeId {
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

    pub(super) fn push_linear(&mut self, style: TestStyle, children: Vec<NodeId>) -> NodeId {
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

    pub(super) fn push_relative(&mut self, style: TestStyle, children: Vec<NodeId>) -> NodeId {
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

    pub(super) fn push(&mut self, node: TestSourceNode) -> NodeId {
        debug_assert_eq!(self.source.nodes.len(), self.session.nodes.len());
        let id = NodeId::from(self.source.nodes.len());
        self.source.nodes.push(node);
        self.session.nodes.push(TestSessionNode::default());
        id
    }

    /// Stores a test `calc()` expression and returns its source-local handle.
    ///
    /// `percentage` is a fraction, so `0.5` represents `50%`.
    pub(super) fn push_calc(&mut self, length: f32, percentage: f32) -> CalcHandle {
        let handle = CalcHandle::from_raw(self.source.calcs.len() as u64);
        self.source.calcs.push(TestCalc { length, percentage });
        handle
    }

    pub(super) fn source_node_mut(&mut self, id: NodeId) -> &mut TestSourceNode {
        &mut self.source.nodes[usize::from(id)]
    }

    pub(super) fn session_node(&self, id: NodeId) -> &TestSessionNode {
        &self.session.nodes[usize::from(id)]
    }

    pub(super) fn session_node_mut(&mut self, id: NodeId) -> &mut TestSessionNode {
        &mut self.session.nodes[usize::from(id)]
    }

    pub(super) fn layout(&self, id: NodeId) -> Layout {
        self.session_node(id).layout
    }

    pub(super) fn final_layout(&self, id: NodeId) -> Layout {
        self.session_node(id).final_layout
    }

    pub(super) fn static_position(&self, id: NodeId) -> Option<Point<f32>> {
        self.session_node(id).static_position
    }
}

impl TraverseTree for TestSource {
    type ChildIter<'a> = std::iter::Copied<std::slice::Iter<'a, NodeId>>;

    fn child_ids(&self, parent: NodeId) -> Self::ChildIter<'_> {
        self.nodes[usize::from(parent)].children.iter().copied()
    }

    fn child_count(&self, parent: NodeId) -> usize {
        self.nodes[usize::from(parent)].children.len()
    }

    fn child_id(&self, parent: NodeId, index: usize) -> NodeId {
        self.nodes[usize::from(parent)].children[index]
    }
}

impl LayoutSource for TestSource {
    type CoreStyle<'a> = &'a TestStyle;

    fn core_style(&self, node: NodeId) -> Self::CoreStyle<'_> {
        &self.nodes[usize::from(node)].style
    }

    fn resolve_calc(&self, calc: CalcHandle, basis: f32) -> f32 {
        let expression = self
            .calcs
            .get(usize::try_from(calc.raw()).expect("test calc handle fits usize"))
            .expect("test calc handle belongs to this source");
        expression.length + expression.percentage * basis
    }
}

impl FlexSource for TestSource {
    type ContainerStyle<'a> = &'a TestStyle;
    type ItemStyle<'a> = &'a TestStyle;

    fn flex_container_style(&self, container: NodeId) -> Self::ContainerStyle<'_> {
        &self.nodes[usize::from(container)].style
    }

    fn flex_item_style(&self, item: NodeId) -> Self::ItemStyle<'_> {
        &self.nodes[usize::from(item)].style
    }
}

impl GridSource for TestSource {
    type ContainerStyle<'a> = &'a TestStyle;
    type ItemStyle<'a> = &'a TestStyle;

    fn grid_container_style(&self, container: NodeId) -> Self::ContainerStyle<'_> {
        &self.nodes[usize::from(container)].style
    }

    fn grid_item_style(&self, item: NodeId) -> Self::ItemStyle<'_> {
        &self.nodes[usize::from(item)].style
    }
}

impl LinearSource for TestSource {
    type ContainerStyle<'a> = &'a TestStyle;
    type ItemStyle<'a> = &'a TestStyle;

    fn linear_container_style(&self, container: NodeId) -> Self::ContainerStyle<'_> {
        &self.nodes[usize::from(container)].style
    }

    fn linear_item_style(&self, item: NodeId) -> Self::ItemStyle<'_> {
        &self.nodes[usize::from(item)].style
    }
}

impl RelativeSource for TestSource {
    type ContainerStyle<'a> = &'a TestStyle;
    type ItemStyle<'a> = &'a TestStyle;

    fn relative_container_style(&self, container: NodeId) -> Self::ContainerStyle<'_> {
        &self.nodes[usize::from(container)].style
    }

    fn relative_item_style(&self, item: NodeId) -> Self::ItemStyle<'_> {
        &self.nodes[usize::from(item)].style
    }
}

impl LayoutState for TestSession {
    fn set_unrounded_layout(&mut self, node: NodeId, layout: &Layout) {
        self.layout_writes += 1;
        self.nodes[usize::from(node)].layout = *layout;
    }

    fn set_static_position(&mut self, child: NodeId, static_position: Point<f32>) {
        self.nodes[usize::from(child)].static_position = Some(static_position);
    }
}

impl CacheState for TestSession {
    fn cache_get(&self, _node: NodeId, _input: LayoutInput) -> Option<LayoutOutput> {
        None
    }

    fn cache_store(&mut self, _node: NodeId, _input: LayoutInput, _output: LayoutOutput) {}

    fn cache_clear(&mut self, _node: NodeId) {}
}

impl RoundState for TestSession {
    fn unrounded_layout(&self, node: NodeId) -> Layout {
        self.nodes[usize::from(node)].layout
    }

    fn set_final_layout(&mut self, node: NodeId, layout: &Layout) {
        self.nodes[usize::from(node)].final_layout = *layout;
    }
}

impl LayoutSession<TestSource> for TestSession {
    fn compute_child_layout(
        &mut self,
        source: &TestSource,
        child: NodeId,
        input: LayoutInput,
    ) -> LayoutOutput {
        self.child_layout_calls += 1;
        let node = &source.nodes[usize::from(child)];
        let display = node.display;

        if node.style.box_generation_mode == BoxGenerationMode::None {
            hide_subtree(source, self, child);
            return LayoutOutput::HIDDEN;
        }

        let output =
            compute_cached_layout(self, child, input, |session, child, input| match display {
                TestDisplay::Flex => compute_flexbox_layout(source, session, child, input),
                TestDisplay::Grid => compute_grid_layout(source, session, child, input),
                TestDisplay::Linear => compute_linear_layout(source, session, child, input),
                TestDisplay::Relative => compute_relative_layout(source, session, child, input),
                TestDisplay::Leaf => {
                    let style = &node.style;
                    let measure = node.measure;
                    let leaf_measure_calls = &mut session.leaf_measure_calls;
                    let measure_calls = &mut session.nodes[usize::from(child)].measure_calls;
                    let mut measurer = FnLeafMeasurer::new(|measure_input| {
                        *leaf_measure_calls += 1;
                        let (metrics, call) = measure.measure(measure_input);
                        measure_calls.extend(call);
                        metrics
                    });
                    compute_leaf_layout(
                        input,
                        style,
                        |calc, basis| source.resolve_calc(calc, basis),
                        &mut measurer,
                    )
                }
            });
        self.nodes[usize::from(child)].output = output;
        output
    }
}

pub(super) fn fixed_leaf_style(width: f32, height: f32) -> TestStyle {
    TestStyle {
        size: Size::new(Dimension::Length(width), Dimension::Length(height)),
        flex_basis: Dimension::Length(width),
        ..TestStyle::default()
    }
}

pub(super) fn fixed_leaf(tree: &mut TestTree, width: f32, height: f32) -> NodeId {
    tree.push_leaf(
        fixed_leaf_style(width, height),
        Size::new(width, height),
        None,
    )
}

pub(super) fn flex_container(tree: &mut TestTree, style: TestStyle, children: &[NodeId]) -> NodeId {
    tree.push_flex(style, children.to_vec())
}

pub(super) fn relative_container(
    tree: &mut TestTree,
    style: TestStyle,
    children: &[NodeId],
) -> NodeId {
    tree.push_relative(style, children.to_vec())
}

pub(super) fn linear_container(
    tree: &mut TestTree,
    style: TestStyle,
    children: &[NodeId],
) -> NodeId {
    tree.push_linear(style, children.to_vec())
}

pub(super) fn perform_layout(
    tree: &mut TestTree,
    root: NodeId,
    known_dimensions: Size<Option<f32>>,
    available_space: Size<AvailableSpace>,
) -> LayoutOutput {
    tree.session.compute_child_layout(
        &tree.source,
        root,
        LayoutInput::perform_layout(
            known_dimensions,
            available_space.into_options(),
            available_space,
        ),
    )
}

pub(super) fn definite_layout(
    tree: &mut TestTree,
    root: NodeId,
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
