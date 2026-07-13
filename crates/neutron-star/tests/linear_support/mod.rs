//! Styling-engine-free host shared by neutron-star Linear tests and benchmarks.

// Every integration-test or benchmark target includes this module separately and intentionally
// uses only part of the fixture surface.
#![allow(dead_code)]

use neutron_star::cache::Cache;
use neutron_star::compute::{
    FnLeafMeasurer, LeafMeasureInput, LeafMetrics, compute_cached_layout, compute_flexbox_layout,
    compute_grid_layout, compute_leaf_layout, compute_linear_layout, hide_subtree,
};
use neutron_star::prelude::*;
use neutron_star::style::{
    AlignItems, AlignSelf, BoxGenerationMode, BoxSizing, CalcHandle, CoreStyle, Dimension,
    Direction, GridTemplateComponent, JustifyContent, LengthPercentage, LengthPercentageAuto,
    LinearContainerStyle, LinearCrossGravity, LinearGravity, LinearItemStyle, LinearLayoutGravity,
    LinearOrientation, Overflow, Position, RepetitionCount, TrackSizingFunction, Visibility,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TestDisplay {
    Linear,
    Flex,
    Grid,
    Leaf,
}

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
    pub(super) justify_content: Option<JustifyContent>,
    pub(super) align_items: Option<AlignItems>,
    pub(super) linear_layout_gravity: LinearLayoutGravity,
    pub(super) linear_weight: f32,
    pub(super) align_self: Option<AlignSelf>,
    pub(super) order: i32,
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
            justify_content: None,
            align_items: None,
            linear_layout_gravity: LinearLayoutGravity::None,
            linear_weight: 0.0,
            align_self: None,
            order: 0,
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

impl FlexContainerStyle for TestStyle {}
impl FlexItemStyle for TestStyle {
    fn align_self(&self) -> Option<AlignSelf> {
        self.align_self
    }

    fn order(&self) -> i32 {
        self.order
    }
}

#[derive(Debug, Clone, Copy)]
pub(super) struct EmptyGridRepetition;

impl GridTemplateRepetition for EmptyGridRepetition {
    type Tracks<'a> = core::iter::Empty<TrackSizingFunction>;

    fn count(&self) -> RepetitionCount {
        RepetitionCount::Count(1)
    }

    fn tracks(&self) -> Self::Tracks<'_> {
        core::iter::empty()
    }
}

impl GridContainerStyle for TestStyle {
    type Repetition<'a> = EmptyGridRepetition;
    type TemplateTracks<'a> = core::iter::Empty<GridTemplateComponent<EmptyGridRepetition>>;
    type AutoTracks<'a> = core::iter::Empty<TrackSizingFunction>;

    fn grid_template_rows(&self) -> Self::TemplateTracks<'_> {
        core::iter::empty()
    }

    fn grid_template_columns(&self) -> Self::TemplateTracks<'_> {
        core::iter::empty()
    }

    fn grid_auto_rows(&self) -> Self::AutoTracks<'_> {
        core::iter::empty()
    }

    fn grid_auto_columns(&self) -> Self::AutoTracks<'_> {
        core::iter::empty()
    }

    fn justify_content(&self) -> Option<JustifyContent> {
        self.justify_content
    }

    fn align_items(&self) -> Option<AlignItems> {
        self.align_items
    }
}

impl GridItemStyle for TestStyle {
    fn align_self(&self) -> Option<AlignSelf> {
        self.align_self
    }

    fn order(&self) -> i32 {
        self.order
    }
}

/// Deterministic test-leaf measurement strategy.
#[derive(Debug, Clone, Copy)]
pub(super) enum TestMeasure {
    Intrinsic {
        min_content_size: Size<f32>,
        max_content_size: Size<f32>,
        first_baseline: Option<f32>,
    },
    Function(fn(LeafMeasureInput) -> LeafMetrics),
}

impl TestMeasure {
    fn measure(self, input: LeafMeasureInput) -> LeafMetrics {
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
        };

        LeafMetrics::new(Size::new(
            input.known_dimensions.width.unwrap_or(measured.size.width),
            input
                .known_dimensions
                .height
                .unwrap_or(measured.size.height),
        ))
        .with_first_baselines(measured.first_baselines)
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
    pub(super) static_position: Option<Point<f32>>,
    pub(super) output: LayoutOutput,
    pub(super) measure_inputs: Vec<LeafMeasureInput>,
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

#[derive(Debug)]
pub(super) struct TestSession {
    pub(super) nodes: Vec<TestSessionNode>,
    pub(super) layout_writes: usize,
    pub(super) static_position_writes: usize,
    pub(super) record_measure_inputs: bool,
    caches: Option<Vec<Cache>>,
}

impl Default for TestSession {
    fn default() -> Self {
        Self {
            nodes: Vec::new(),
            layout_writes: 0,
            static_position_writes: 0,
            record_measure_inputs: true,
            caches: None,
        }
    }
}

impl TestSession {
    pub(super) fn enable_cache(&mut self) {
        self.caches = Some(vec![Cache::new(); self.nodes.len()]);
    }
}

/// Builder and assertion facade; layout receives source and session separately.
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
        first_baseline: Option<f32>,
    ) -> NodeId {
        self.push(TestSourceNode {
            display: TestDisplay::Leaf,
            style,
            children: Vec::new(),
            measure: TestMeasure::Intrinsic {
                min_content_size,
                max_content_size,
                first_baseline,
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

    pub(super) fn push(&mut self, node: TestSourceNode) -> NodeId {
        debug_assert_eq!(self.source.nodes.len(), self.session.nodes.len());
        let id = NodeId::from(self.source.nodes.len());
        self.source.nodes.push(node);
        self.session.nodes.push(TestSessionNode::default());
        if let Some(caches) = &mut self.session.caches {
            caches.push(Cache::new());
        }
        id
    }

    /// Stores `length + percentage * basis`; percentage is a fraction.
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

    pub(super) fn layout(&self, id: NodeId) -> Layout {
        self.session_node(id).layout
    }

    pub(super) fn output(&self, id: NodeId) -> LayoutOutput {
        self.session_node(id).output
    }

    pub(super) fn static_position(&self, id: NodeId) -> Option<Point<f32>> {
        self.session_node(id).static_position
    }

    pub(super) fn measure_inputs(&self, id: NodeId) -> &[LeafMeasureInput] {
        &self.session_node(id).measure_inputs
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

impl LayoutState for TestSession {
    fn set_unrounded_layout(&mut self, node: NodeId, layout: &Layout) {
        self.layout_writes += 1;
        self.nodes[usize::from(node)].layout = *layout;
    }

    fn set_static_position(&mut self, child: NodeId, static_position: Point<f32>) {
        self.static_position_writes += 1;
        self.nodes[usize::from(child)].static_position = Some(static_position);
    }
}

impl CacheState for TestSession {
    fn cache_get(&self, node: NodeId, input: LayoutInput) -> Option<LayoutOutput> {
        self.caches.as_ref()?[usize::from(node)].get(input)
    }

    fn cache_store(&mut self, node: NodeId, input: LayoutInput, output: LayoutOutput) {
        if let Some(caches) = &mut self.caches {
            caches[usize::from(node)].store(input, output);
        }
    }

    fn cache_clear(&mut self, node: NodeId) {
        if let Some(caches) = &mut self.caches {
            caches[usize::from(node)].clear();
        }
    }
}

impl LayoutSession<TestSource> for TestSession {
    fn compute_child_layout(
        &mut self,
        source: &TestSource,
        child: NodeId,
        input: LayoutInput,
    ) -> LayoutOutput {
        let index = usize::from(child);
        if source.nodes[index].style.box_generation_mode == BoxGenerationMode::None {
            hide_subtree(source, self, child);
            return LayoutOutput::HIDDEN;
        }
        let display = source.nodes[index].display;

        let output =
            compute_cached_layout(self, child, input, |session, child, input| match display {
                TestDisplay::Linear => compute_linear_layout(source, session, child, input),
                TestDisplay::Flex => compute_flexbox_layout(source, session, child, input),
                TestDisplay::Grid => compute_grid_layout(source, session, child, input),
                TestDisplay::Leaf => {
                    let style = &source.nodes[index].style;
                    let measure = source.nodes[index].measure;
                    let record_measure_inputs = session.record_measure_inputs;
                    let measure_inputs = &mut session.nodes[index].measure_inputs;
                    let mut measurer = FnLeafMeasurer::new(|measure_input| {
                        if record_measure_inputs {
                            measure_inputs.push(measure_input);
                        }
                        measure.measure(measure_input)
                    });
                    compute_leaf_layout(
                        input,
                        style,
                        |calc, basis| source.resolve_calc(calc, basis),
                        &mut measurer,
                    )
                }
            });
        self.nodes[index].output = output;
        output
    }
}

pub(super) fn fixed_leaf_style(width: f32, height: f32) -> TestStyle {
    TestStyle {
        size: Size::new(Dimension::Length(width), Dimension::Length(height)),
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

pub(super) fn measure_layout(
    tree: &mut TestTree,
    root: NodeId,
    known_dimensions: Size<Option<f32>>,
    available_space: Size<AvailableSpace>,
) -> LayoutOutput {
    tree.session.compute_child_layout(
        &tree.source,
        root,
        LayoutInput::compute_size(
            known_dimensions,
            available_space.into_options(),
            available_space,
            RequestedAxis::Both,
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
