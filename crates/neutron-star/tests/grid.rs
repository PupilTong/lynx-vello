//! Spec-focused CSS Grid integration tests over a plain `Vec`-backed host.
//!
//! There is deliberately no styling engine here: `TestStyle` is already a
//! computed-style view, track-list access stays borrowed through the Grid
//! GATs, and display dispatch is static all the way into the generic Grid and
//! leaf entry points.

use neutron_star::compute::{
    FnLeafMeasurer, LeafMetrics, compute_absolute_layout, compute_cached_layout,
    compute_flexbox_layout, compute_grid_layout, compute_leaf_layout, hide_subtree,
};
use neutron_star::prelude::*;
use neutron_star::style::{
    AlignContent, AlignItems, AlignSelf, BoxGenerationMode, BoxSizing, CalcHandle, Dimension,
    Direction, GridAutoFlow, GridLine, GridPlacement, GridTemplateComponent, JustifyContent,
    JustifyItems, JustifySelf, LengthPercentage, LengthPercentageAuto, MaxTrackSizingFunction,
    MinTrackSizingFunction, Overflow, Position, RepetitionCount, TrackSizingFunction,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TestDisplay {
    Flex,
    Grid,
    Leaf,
}

#[derive(Debug, Clone)]
struct TestRepetition {
    count: RepetitionCount,
    tracks: Vec<TrackSizingFunction>,
}

impl GridTemplateRepetition for TestRepetition {
    type Tracks<'a>
        = std::iter::Copied<std::slice::Iter<'a, TrackSizingFunction>>
    where
        Self: 'a;

    fn count(&self) -> RepetitionCount {
        self.count
    }

    fn tracks(&self) -> Self::Tracks<'_> {
        self.tracks.iter().copied()
    }
}

#[derive(Debug, Clone)]
enum TestTemplateComponent {
    Single(TrackSizingFunction),
    Repeat(TestRepetition),
}

fn template_component(component: &TestTemplateComponent) -> GridTemplateComponent<&TestRepetition> {
    match component {
        TestTemplateComponent::Single(track) => GridTemplateComponent::Single(*track),
        TestTemplateComponent::Repeat(repetition) => GridTemplateComponent::Repeat(repetition),
    }
}

#[derive(Debug, Clone)]
struct TestStyle {
    box_generation_mode: BoxGenerationMode,
    position: Position,
    inset: Edges<LengthPercentageAuto>,
    size: Size<Dimension>,
    min_size: Size<Dimension>,
    max_size: Size<Dimension>,
    aspect_ratio: Option<f32>,
    margin: Edges<LengthPercentageAuto>,
    padding: Edges<LengthPercentage>,
    border: Edges<LengthPercentage>,
    overflow: Point<Overflow>,
    scrollbar_width: f32,
    box_sizing: BoxSizing,
    direction: Direction,
    template_rows: Vec<TestTemplateComponent>,
    template_columns: Vec<TestTemplateComponent>,
    auto_rows: Vec<TrackSizingFunction>,
    auto_columns: Vec<TrackSizingFunction>,
    auto_flow: GridAutoFlow,
    gap: Size<LengthPercentage>,
    align_content: Option<AlignContent>,
    justify_content: Option<JustifyContent>,
    align_items: Option<AlignItems>,
    justify_items: Option<JustifyItems>,
    grid_row: Line<GridPlacement>,
    grid_column: Line<GridPlacement>,
    align_self: Option<AlignSelf>,
    justify_self: Option<JustifySelf>,
    order: i32,
}

impl Default for TestStyle {
    fn default() -> Self {
        Self {
            box_generation_mode: BoxGenerationMode::Normal,
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
            template_rows: Vec::new(),
            template_columns: Vec::new(),
            auto_rows: Vec::new(),
            auto_columns: Vec::new(),
            auto_flow: GridAutoFlow::Row,
            gap: Size::new(LengthPercentage::ZERO, LengthPercentage::ZERO),
            align_content: None,
            justify_content: None,
            align_items: None,
            justify_items: None,
            grid_row: Line::new(GridPlacement::Auto, GridPlacement::Auto),
            grid_column: Line::new(GridPlacement::Auto, GridPlacement::Auto),
            align_self: None,
            justify_self: None,
            order: 0,
        }
    }
}

impl CoreStyle for TestStyle {
    fn box_generation_mode(&self) -> BoxGenerationMode {
        self.box_generation_mode
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

impl GridContainerStyle for TestStyle {
    type Repetition<'a>
        = &'a TestRepetition
    where
        Self: 'a;
    type TemplateTracks<'a>
        = std::iter::Map<
        std::slice::Iter<'a, TestTemplateComponent>,
        fn(&'a TestTemplateComponent) -> GridTemplateComponent<&'a TestRepetition>,
    >
    where
        Self: 'a;
    type AutoTracks<'a>
        = std::iter::Copied<std::slice::Iter<'a, TrackSizingFunction>>
    where
        Self: 'a;

    fn grid_template_rows(&self) -> Self::TemplateTracks<'_> {
        self.template_rows.iter().map(template_component as _)
    }

    fn grid_template_columns(&self) -> Self::TemplateTracks<'_> {
        self.template_columns.iter().map(template_component as _)
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

impl FlexContainerStyle for TestStyle {
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
    fn align_self(&self) -> Option<AlignSelf> {
        self.align_self
    }

    fn order(&self) -> i32 {
        self.order
    }
}

#[derive(Debug, Clone)]
struct TestSourceNode {
    display: TestDisplay,
    style: TestStyle,
    children: Vec<NodeId>,
    min_content_size: Size<f32>,
    max_content_size: Size<f32>,
    first_baseline: Option<f32>,
}

#[derive(Debug, Clone, Copy, Default)]
struct TestSessionNode {
    layout: Layout,
    static_position: Option<Point<f32>>,
    layout_writes: usize,
    static_position_writes: usize,
    measure_calls: usize,
}

#[derive(Debug, Default)]
struct TestSource {
    nodes: Vec<TestSourceNode>,
}

#[derive(Debug, Default)]
struct TestSession {
    nodes: Vec<TestSessionNode>,
    layout_writes: usize,
    leaf_measure_calls: usize,
}

/// Builder and assertion facade; layout receives immutable source and
/// mutable session storage as physically separate values.
#[derive(Debug, Default)]
struct TestTree {
    source: TestSource,
    session: TestSession,
}

impl TestTree {
    fn push_leaf(
        &mut self,
        style: TestStyle,
        min_content_size: Size<f32>,
        max_content_size: Size<f32>,
    ) -> NodeId {
        self.push(TestSourceNode {
            display: TestDisplay::Leaf,
            style,
            children: Vec::new(),
            min_content_size,
            max_content_size,
            first_baseline: None,
        })
    }

    fn push_grid(&mut self, style: TestStyle, children: Vec<NodeId>) -> NodeId {
        self.push(TestSourceNode {
            display: TestDisplay::Grid,
            style,
            children,
            min_content_size: Size::ZERO,
            max_content_size: Size::ZERO,
            first_baseline: None,
        })
    }

    fn push_flex(&mut self, style: TestStyle, children: Vec<NodeId>) -> NodeId {
        self.push(TestSourceNode {
            display: TestDisplay::Flex,
            style,
            children,
            min_content_size: Size::ZERO,
            max_content_size: Size::ZERO,
            first_baseline: None,
        })
    }

    fn push(&mut self, node: TestSourceNode) -> NodeId {
        debug_assert_eq!(self.source.nodes.len(), self.session.nodes.len());
        let id = NodeId::from(self.source.nodes.len());
        self.source.nodes.push(node);
        self.session.nodes.push(TestSessionNode::default());
        id
    }

    fn source_node_mut(&mut self, id: NodeId) -> &mut TestSourceNode {
        &mut self.source.nodes[usize::from(id)]
    }

    fn session_node(&self, id: NodeId) -> &TestSessionNode {
        &self.session.nodes[usize::from(id)]
    }

    fn session_node_mut(&mut self, id: NodeId) -> &mut TestSessionNode {
        &mut self.session.nodes[usize::from(id)]
    }

    fn layout(&self, id: NodeId) -> Layout {
        self.session_node(id).layout
    }

    fn compute_child_layout(&mut self, child: NodeId, input: LayoutInput) -> LayoutOutput {
        self.session
            .compute_child_layout(&self.source, child, input)
    }
}

impl TraverseTree for TestSource {
    type ChildIter<'a> = std::iter::Copied<std::slice::Iter<'a, NodeId>>;

    #[inline]
    fn child_ids(&self, parent: NodeId) -> Self::ChildIter<'_> {
        self.nodes[usize::from(parent)].children.iter().copied()
    }

    #[inline]
    fn child_count(&self, parent: NodeId) -> usize {
        self.nodes[usize::from(parent)].children.len()
    }

    #[inline]
    fn child_id(&self, parent: NodeId, index: usize) -> NodeId {
        self.nodes[usize::from(parent)].children[index]
    }
}

impl LayoutSource for TestSource {
    type CoreStyle<'a> = &'a TestStyle;

    #[inline]
    fn core_style(&self, node: NodeId) -> Self::CoreStyle<'_> {
        &self.nodes[usize::from(node)].style
    }

    fn resolve_calc(&self, _calc: CalcHandle, _basis: f32) -> f32 {
        unreachable!("test styles contain no calc() values")
    }
}

impl LayoutState for TestSession {
    fn set_unrounded_layout(&mut self, node: NodeId, layout: &Layout) {
        self.layout_writes += 1;
        let node = &mut self.nodes[usize::from(node)];
        node.layout_writes += 1;
        node.layout = *layout;
    }

    fn set_static_position(&mut self, child: NodeId, static_position: Point<f32>) {
        let node = &mut self.nodes[usize::from(child)];
        node.static_position_writes += 1;
        node.static_position = Some(static_position);
    }
}

impl CacheState for TestSession {
    fn cache_get(&self, _node: NodeId, _input: LayoutInput) -> Option<LayoutOutput> {
        None
    }

    fn cache_store(&mut self, _node: NodeId, _input: LayoutInput, _output: LayoutOutput) {}

    fn cache_clear(&mut self, _node: NodeId) {}
}

impl LayoutSession<TestSource> for TestSession {
    fn compute_child_layout(
        &mut self,
        source: &TestSource,
        child: NodeId,
        input: LayoutInput,
    ) -> LayoutOutput {
        let node = &source.nodes[usize::from(child)];
        let display = node.display;

        if node.style.box_generation_mode == BoxGenerationMode::None {
            hide_subtree(source, self, child);
            return LayoutOutput::HIDDEN;
        }

        compute_cached_layout(self, child, input, |session, child, input| match display {
            TestDisplay::Flex => compute_flexbox_layout(source, session, child, input),
            TestDisplay::Grid => compute_grid_layout(source, session, child, input),
            TestDisplay::Leaf => {
                let style = &node.style;
                let min_content_size = node.min_content_size;
                let max_content_size = node.max_content_size;
                let first_baseline = node.first_baseline;
                let leaf_measure_calls = &mut session.leaf_measure_calls;
                let node_measure_calls = &mut session.nodes[usize::from(child)].measure_calls;
                let mut measurer = FnLeafMeasurer::new(|measure_input| {
                    *leaf_measure_calls += 1;
                    *node_measure_calls += 1;
                    let measured = Size::new(
                        if measure_input.available_space.width == AvailableSpace::MinContent {
                            min_content_size.width
                        } else {
                            max_content_size.width
                        },
                        if measure_input.available_space.height == AvailableSpace::MinContent {
                            min_content_size.height
                        } else {
                            max_content_size.height
                        },
                    );
                    let size = Size::new(
                        measure_input
                            .known_dimensions
                            .width
                            .unwrap_or(measured.width),
                        measure_input
                            .known_dimensions
                            .height
                            .unwrap_or(measured.height),
                    );
                    LeafMetrics::new(size).with_first_baselines(Point::new(None, first_baseline))
                });
                compute_leaf_layout(
                    input,
                    style,
                    |_calc, _basis| unreachable!("test styles contain no calc() values"),
                    &mut measurer,
                )
            }
        })
    }
}

impl GridSource for TestSource {
    type ContainerStyle<'a> = &'a TestStyle;
    type ItemStyle<'a> = &'a TestStyle;

    #[inline]
    fn grid_container_style(&self, container: NodeId) -> Self::ContainerStyle<'_> {
        &self.nodes[usize::from(container)].style
    }

    #[inline]
    fn grid_item_style(&self, item: NodeId) -> Self::ItemStyle<'_> {
        &self.nodes[usize::from(item)].style
    }
}

impl FlexSource for TestSource {
    type ContainerStyle<'a> = &'a TestStyle;
    type ItemStyle<'a> = &'a TestStyle;

    #[inline]
    fn flex_container_style(&self, container: NodeId) -> Self::ContainerStyle<'_> {
        &self.nodes[usize::from(container)].style
    }

    #[inline]
    fn flex_item_style(&self, item: NodeId) -> Self::ItemStyle<'_> {
        &self.nodes[usize::from(item)].style
    }
}

fn px(value: f32) -> TrackSizingFunction {
    TrackSizingFunction::fixed(LengthPercentage::length(value))
}

fn fr(value: f32) -> TrackSizingFunction {
    TrackSizingFunction::fr(value)
}

fn percent(fraction: f32) -> TrackSizingFunction {
    TrackSizingFunction::fixed(LengthPercentage::percent(fraction))
}

fn max_content_track() -> TrackSizingFunction {
    TrackSizingFunction::minmax(
        MinTrackSizingFunction::MaxContent,
        MaxTrackSizingFunction::MaxContent,
    )
}

fn repeat(count: RepetitionCount, tracks: Vec<TrackSizingFunction>) -> TestTemplateComponent {
    TestTemplateComponent::Repeat(TestRepetition { count, tracks })
}

fn tracks(tracks: impl IntoIterator<Item = TrackSizingFunction>) -> Vec<TestTemplateComponent> {
    tracks
        .into_iter()
        .map(TestTemplateComponent::Single)
        .collect()
}

fn grid_style(columns: &[TrackSizingFunction], rows: &[TrackSizingFunction]) -> TestStyle {
    TestStyle {
        template_columns: tracks(columns.iter().copied()),
        template_rows: tracks(rows.iter().copied()),
        ..TestStyle::default()
    }
}

fn fixed_leaf_style(width: f32, height: f32) -> TestStyle {
    TestStyle {
        size: Size::new(Dimension::Length(width), Dimension::Length(height)),
        ..TestStyle::default()
    }
}

fn fixed_leaf(tree: &mut TestTree, width: f32, height: f32) -> NodeId {
    tree.push_leaf(
        fixed_leaf_style(width, height),
        Size::new(width, height),
        Size::new(width, height),
    )
}

fn intrinsic_leaf(
    tree: &mut TestTree,
    min_content_size: Size<f32>,
    max_content_size: Size<f32>,
) -> NodeId {
    tree.push_leaf(TestStyle::default(), min_content_size, max_content_size)
}

fn line(number: i16) -> GridPlacement {
    GridPlacement::Line(GridLine::new(number))
}

fn placement(start: GridPlacement, end: GridPlacement) -> Line<GridPlacement> {
    Line::new(start, end)
}

fn definite_layout(tree: &mut TestTree, root: NodeId, width: f32, height: f32) -> LayoutOutput {
    tree.compute_child_layout(
        root,
        LayoutInput::perform_layout(
            Size::new(Some(width), Some(height)),
            Size::new(Some(width), Some(height)),
            Size::new(
                AvailableSpace::Definite(width),
                AvailableSpace::Definite(height),
            ),
        ),
    )
}

fn intrinsic_layout(tree: &mut TestTree, root: NodeId) -> LayoutOutput {
    tree.compute_child_layout(
        root,
        LayoutInput::perform_layout(Size::NONE, Size::NONE, Size::MAX_CONTENT),
    )
}

fn assert_close(actual: f32, expected: f32) {
    let error = (actual - expected).abs();
    assert!(
        error <= 0.01,
        "expected {expected}, got {actual} (absolute error {error})"
    );
}

fn assert_point(actual: Point<f32>, expected: Point<f32>) {
    assert_close(actual.x, expected.x);
    assert_close(actual.y, expected.y);
}

fn assert_size(actual: Size<f32>, expected: Size<f32>) {
    assert_close(actual.width, expected.width);
    assert_close(actual.height, expected.height);
}

#[test]
fn fixed_tracks_and_gaps_form_concrete_grid_areas() {
    let mut tree = TestTree::default();
    let children = [
        fixed_leaf(&mut tree, 10.0, 10.0),
        fixed_leaf(&mut tree, 10.0, 10.0),
        fixed_leaf(&mut tree, 10.0, 10.0),
        fixed_leaf(&mut tree, 10.0, 10.0),
    ];
    let mut style = grid_style(&[px(80.0), px(120.0)], &[px(30.0), px(50.0)]);
    style.gap = Size::new(
        LengthPercentage::length(10.0),
        LengthPercentage::length(5.0),
    );
    style.justify_items = Some(JustifyItems::Start);
    style.align_items = Some(AlignItems::Start);
    let root = tree.push_grid(style, children.to_vec());

    let output = definite_layout(&mut tree, root, 210.0, 85.0);

    assert_size(output.size, Size::new(210.0, 85.0));
    assert_point(tree.layout(children[0]).location, Point::new(0.0, 0.0));
    assert_point(tree.layout(children[1]).location, Point::new(90.0, 0.0));
    assert_point(tree.layout(children[2]).location, Point::new(0.0, 35.0));
    assert_point(tree.layout(children[3]).location, Point::new(90.0, 35.0));
}

#[test]
fn fractional_tracks_share_space_after_the_gap() {
    let mut tree = TestTree::default();
    let first = intrinsic_leaf(&mut tree, Size::ZERO, Size::ZERO);
    let second = intrinsic_leaf(&mut tree, Size::ZERO, Size::ZERO);
    let mut style = grid_style(&[fr(1.0), fr(2.0)], &[px(20.0)]);
    style.gap.width = LengthPercentage::length(30.0);
    let root = tree.push_grid(style, vec![first, second]);

    definite_layout(&mut tree, root, 300.0, 20.0);

    assert_size(tree.layout(first).size, Size::new(90.0, 20.0));
    assert_point(tree.layout(second).location, Point::new(120.0, 0.0));
    assert_size(tree.layout(second).size, Size::new(180.0, 20.0));
}

#[test]
fn cyclic_percentage_track_resolves_after_intrinsic_container_sizing() {
    let mut tree = TestTree::default();
    let child_style = TestStyle {
        min_size: Size::new(Dimension::ZERO, Dimension::ZERO),
        ..TestStyle::default()
    };
    let child = tree.push_leaf(child_style, Size::new(40.0, 10.0), Size::new(100.0, 10.0));
    let root = tree.push_grid(grid_style(&[percent(0.5)], &[px(20.0)]), vec![child]);

    let output = intrinsic_layout(&mut tree, root);

    // The cyclic percentage behaves as auto for intrinsic sizing (100px),
    // then resolves against that 100px container without resizing it again.
    assert_size(output.size, Size::new(100.0, 20.0));
    assert_size(tree.layout(child).size, Size::new(50.0, 20.0));
}

#[test]
fn cyclic_percentage_gap_resolves_after_intrinsic_container_sizing() {
    let mut tree = TestTree::default();
    let first = fixed_leaf(&mut tree, 10.0, 10.0);
    let second = fixed_leaf(&mut tree, 10.0, 10.0);
    let mut style = grid_style(&[px(40.0), px(40.0)], &[px(20.0)]);
    style.gap.width = LengthPercentage::percent(0.1);
    style.justify_items = Some(JustifyItems::Start);
    style.align_items = Some(AlignItems::Start);
    let root = tree.push_grid(style, vec![first, second]);

    let output = intrinsic_layout(&mut tree, root);

    // Percentage gaps contribute zero to the intrinsic width, then resolve
    // to 8px against the resulting 80px content box and may overflow it.
    assert_size(output.size, Size::new(80.0, 20.0));
    assert_point(tree.layout(second).location, Point::new(48.0, 0.0));
}

#[test]
fn minmax_and_fit_content_stop_at_their_growth_limits() {
    let mut minmax_tree = TestTree::default();
    let child = intrinsic_leaf(&mut minmax_tree, Size::ZERO, Size::ZERO);
    let bounded = TrackSizingFunction::minmax(
        MinTrackSizingFunction::Fixed(LengthPercentage::length(40.0)),
        MaxTrackSizingFunction::Fixed(LengthPercentage::length(80.0)),
    );
    let minmax_root = minmax_tree.push_grid(grid_style(&[bounded], &[px(20.0)]), vec![child]);

    definite_layout(&mut minmax_tree, minmax_root, 100.0, 20.0);
    assert_size(minmax_tree.layout(child).size, Size::new(80.0, 20.0));

    let mut fit_tree = TestTree::default();
    let intrinsic_style = TestStyle {
        grid_column: placement(line(1), line(2)),
        ..TestStyle::default()
    };
    let intrinsic = fit_tree.push_leaf(
        intrinsic_style,
        Size::new(20.0, 10.0),
        Size::new(100.0, 10.0),
    );
    let mut marker_style = fixed_leaf_style(10.0, 10.0);
    marker_style.grid_column = placement(line(2), line(3));
    marker_style.justify_self = Some(JustifySelf::Start);
    let marker = fit_tree.push_leaf(marker_style, Size::new(10.0, 10.0), Size::new(10.0, 10.0));
    let fit_root = fit_tree.push_grid(
        grid_style(
            &[
                TrackSizingFunction::fit_content(LengthPercentage::length(60.0)),
                px(10.0),
            ],
            &[px(20.0)],
        ),
        vec![intrinsic, marker],
    );

    definite_layout(&mut fit_tree, fit_root, 100.0, 20.0);
    assert_close(fit_tree.layout(marker).location.x, 60.0);
}

#[test]
fn zero_fr_and_fraction_sums_below_one_leave_unclaimed_space() {
    let mut tree = TestTree::default();
    let children = [
        intrinsic_leaf(&mut tree, Size::ZERO, Size::ZERO),
        intrinsic_leaf(&mut tree, Size::ZERO, Size::ZERO),
        intrinsic_leaf(&mut tree, Size::ZERO, Size::ZERO),
    ];
    let root = tree.push_grid(
        grid_style(&[fr(0.0), fr(0.25), fr(0.25)], &[px(20.0)]),
        children.to_vec(),
    );

    definite_layout(&mut tree, root, 200.0, 20.0);

    assert_size(tree.layout(children[0]).size, Size::new(0.0, 20.0));
    assert_point(tree.layout(children[1]).location, Point::new(0.0, 0.0));
    assert_size(tree.layout(children[1]).size, Size::new(50.0, 20.0));
    assert_point(tree.layout(children[2]).location, Point::new(50.0, 0.0));
    assert_size(tree.layout(children[2]).size, Size::new(50.0, 20.0));
}

#[test]
fn positive_negative_lines_and_spans_resolve_against_the_explicit_grid() {
    let mut tree = TestTree::default();
    let spanning_style = TestStyle {
        grid_column: placement(line(2), GridPlacement::Span(2)),
        grid_row: placement(line(1), GridPlacement::Span(2)),
        ..TestStyle::default()
    };
    let spanning = tree.push_leaf(spanning_style, Size::ZERO, Size::ZERO);

    let mut negative_style = fixed_leaf_style(10.0, 10.0);
    negative_style.grid_column = placement(line(-2), line(-1));
    negative_style.grid_row = placement(line(-2), line(-1));
    let negative = tree.push_leaf(negative_style, Size::new(10.0, 10.0), Size::new(10.0, 10.0));
    let mut style = grid_style(&[px(40.0), px(50.0), px(60.0)], &[px(30.0), px(40.0)]);
    style.gap = Size::new(
        LengthPercentage::length(10.0),
        LengthPercentage::length(5.0),
    );
    let root = tree.push_grid(style, vec![spanning, negative]);

    definite_layout(&mut tree, root, 170.0, 75.0);

    assert_point(tree.layout(spanning).location, Point::new(50.0, 0.0));
    assert_size(tree.layout(spanning).size, Size::new(120.0, 75.0));
    assert_point(tree.layout(negative).location, Point::new(110.0, 35.0));
}

#[test]
fn reversed_lines_are_swapped_and_equal_lines_fall_back_to_one_track() {
    let mut tree = TestTree::default();
    let reversed_style = TestStyle {
        grid_column: placement(line(3), line(1)),
        grid_row: placement(line(1), line(2)),
        ..TestStyle::default()
    };
    let reversed = tree.push_leaf(reversed_style, Size::ZERO, Size::ZERO);
    let equal_style = TestStyle {
        grid_column: placement(line(2), line(2)),
        grid_row: placement(line(1), line(2)),
        ..TestStyle::default()
    };
    let equal = tree.push_leaf(equal_style, Size::ZERO, Size::ZERO);
    let root = tree.push_grid(
        grid_style(&[px(40.0), px(40.0), px(40.0)], &[px(20.0)]),
        vec![reversed, equal],
    );

    definite_layout(&mut tree, root, 120.0, 20.0);

    assert_point(tree.layout(reversed).location, Point::new(0.0, 0.0));
    assert_size(tree.layout(reversed).size, Size::new(80.0, 20.0));
    assert_point(tree.layout(equal).location, Point::new(40.0, 0.0));
    assert_size(tree.layout(equal).size, Size::new(40.0, 20.0));
}

fn row_packing_layout(flow: GridAutoFlow) -> (Point<f32>, Point<f32>, Point<f32>) {
    let mut tree = TestTree::default();
    let mut wide = fixed_leaf_style(10.0, 10.0);
    wide.grid_column.end = GridPlacement::Span(2);
    let first = tree.push_leaf(wide.clone(), Size::ZERO, Size::new(10.0, 10.0));
    let second = tree.push_leaf(wide, Size::ZERO, Size::new(10.0, 10.0));
    let third = fixed_leaf(&mut tree, 10.0, 10.0);
    let mut style = grid_style(&[px(40.0), px(40.0), px(40.0)], &[px(30.0), px(30.0)]);
    style.auto_flow = flow;
    style.justify_items = Some(JustifyItems::Start);
    style.align_items = Some(AlignItems::Start);
    let root = tree.push_grid(style, vec![first, second, third]);
    definite_layout(&mut tree, root, 120.0, 60.0);
    (
        tree.layout(first).location,
        tree.layout(second).location,
        tree.layout(third).location,
    )
}

#[test]
fn row_dense_backfills_holes_that_sparse_flow_leaves_open() {
    let sparse = row_packing_layout(GridAutoFlow::Row);
    let dense = row_packing_layout(GridAutoFlow::RowDense);

    assert_point(sparse.0, Point::new(0.0, 0.0));
    assert_point(sparse.1, Point::new(0.0, 30.0));
    assert_point(sparse.2, Point::new(80.0, 30.0));
    assert_point(dense.0, Point::new(0.0, 0.0));
    assert_point(dense.1, Point::new(0.0, 30.0));
    assert_point(dense.2, Point::new(80.0, 0.0));
}

#[test]
fn column_auto_flow_fills_rows_before_advancing_columns() {
    let mut tree = TestTree::default();
    let children = [
        fixed_leaf(&mut tree, 10.0, 10.0),
        fixed_leaf(&mut tree, 10.0, 10.0),
        fixed_leaf(&mut tree, 10.0, 10.0),
    ];
    let mut style = grid_style(&[px(40.0), px(40.0)], &[px(30.0), px(30.0)]);
    style.auto_flow = GridAutoFlow::Column;
    style.justify_items = Some(JustifyItems::Start);
    style.align_items = Some(AlignItems::Start);
    let root = tree.push_grid(style, children.to_vec());

    definite_layout(&mut tree, root, 80.0, 60.0);

    assert_point(tree.layout(children[0]).location, Point::new(0.0, 0.0));
    assert_point(tree.layout(children[1]).location, Point::new(0.0, 30.0));
    assert_point(tree.layout(children[2]).location, Point::new(40.0, 0.0));
}

#[test]
fn implicit_auto_tracks_cycle_after_the_explicit_grid() {
    let mut tree = TestTree::default();
    let mut children = Vec::new();
    for column in 2_i16..=4 {
        let mut child_style = fixed_leaf_style(5.0, 5.0);
        child_style.grid_column = placement(line(column), line(column + 1));
        child_style.grid_row = placement(line(1), line(2));
        children.push(tree.push_leaf(child_style, Size::new(5.0, 5.0), Size::new(5.0, 5.0)));
    }
    let mut style = grid_style(&[px(10.0)], &[px(20.0)]);
    style.auto_columns = vec![px(30.0), px(50.0)];
    style.justify_items = Some(JustifyItems::Start);
    style.align_items = Some(AlignItems::Start);
    let root = tree.push_grid(style, children.clone());

    definite_layout(&mut tree, root, 120.0, 20.0);

    assert_close(tree.layout(children[0]).location.x, 10.0);
    assert_close(tree.layout(children[1]).location.x, 40.0);
    assert_close(tree.layout(children[2]).location.x, 90.0);
}

#[test]
fn leading_implicit_auto_tracks_cycle_backwards_from_the_explicit_grid() {
    let mut tree = TestTree::default();
    let mut children = Vec::new();
    for (start, end) in [(-5, -4), (-4, -3), (-3, -2), (1, 2)] {
        let mut child_style = fixed_leaf_style(5.0, 5.0);
        child_style.grid_column = placement(line(start), line(end));
        child_style.grid_row = placement(line(1), line(2));
        children.push(tree.push_leaf(child_style, Size::new(5.0, 5.0), Size::new(5.0, 5.0)));
    }
    let mut style = grid_style(&[px(10.0)], &[px(20.0)]);
    style.auto_columns = vec![px(20.0), px(30.0)];
    style.justify_items = Some(JustifyItems::Start);
    style.align_items = Some(AlignItems::Start);
    let root = tree.push_grid(style, children.clone());

    definite_layout(&mut tree, root, 90.0, 20.0);

    // Leading tracks consume the auto-track pattern backwards: 30, 20, 30.
    assert_close(tree.layout(children[0]).location.x, 0.0);
    assert_close(tree.layout(children[1]).location.x, 30.0);
    assert_close(tree.layout(children[2]).location.x, 50.0);
    assert_close(tree.layout(children[3]).location.x, 80.0);
}

fn automatic_repeat_layout(count: RepetitionCount) -> (Layout, Layout) {
    let mut tree = TestTree::default();
    let first = intrinsic_leaf(&mut tree, Size::ZERO, Size::ZERO);
    let second = intrinsic_leaf(&mut tree, Size::ZERO, Size::ZERO);
    let repeated_track = TrackSizingFunction::minmax(
        MinTrackSizingFunction::Fixed(LengthPercentage::length(40.0)),
        MaxTrackSizingFunction::Fr(1.0),
    );
    let mut style = grid_style(&[], &[px(20.0)]);
    style.template_columns = vec![repeat(count, vec![repeated_track])];
    style.gap.width = LengthPercentage::length(10.0);
    let root = tree.push_grid(style, vec![first, second]);
    definite_layout(&mut tree, root, 230.0, 20.0);
    (tree.layout(first), tree.layout(second))
}

#[test]
fn auto_fill_keeps_empty_tracks_while_auto_fit_collapses_them() {
    let fill = automatic_repeat_layout(RepetitionCount::AutoFill);
    let fit = automatic_repeat_layout(RepetitionCount::AutoFit);

    assert_size(fill.0.size, Size::new(50.0, 20.0));
    assert_point(fill.1.location, Point::new(60.0, 0.0));
    assert_size(fill.1.size, Size::new(50.0, 20.0));
    assert_size(fit.0.size, Size::new(110.0, 20.0));
    assert_point(fit.1.location, Point::new(120.0, 0.0));
    assert_size(fit.1.size, Size::new(110.0, 20.0));
}

#[test]
fn auto_fit_collapses_gutters_on_both_sides_of_an_empty_track() {
    let mut tree = TestTree::default();
    let mut first_style = fixed_leaf_style(10.0, 10.0);
    first_style.grid_column = placement(line(1), line(2));
    first_style.grid_row = placement(line(1), line(2));
    let first = tree.push_leaf(first_style, Size::new(10.0, 10.0), Size::new(10.0, 10.0));
    let mut third_style = fixed_leaf_style(10.0, 10.0);
    third_style.grid_column = placement(line(3), line(4));
    third_style.grid_row = placement(line(1), line(2));
    let third = tree.push_leaf(third_style, Size::new(10.0, 10.0), Size::new(10.0, 10.0));
    let mut style = grid_style(&[], &[px(20.0)]);
    style.template_columns = vec![repeat(RepetitionCount::AutoFit, vec![px(40.0)])];
    style.gap.width = LengthPercentage::length(10.0);
    style.justify_items = Some(JustifyItems::Start);
    style.align_items = Some(AlignItems::Start);
    let root = tree.push_grid(style, vec![first, third]);

    definite_layout(&mut tree, root, 190.0, 20.0);

    assert_close(tree.layout(first).location.x, 0.0);
    // Track 2 is empty and collapsed, including both adjoining gutters.
    assert_close(tree.layout(third).location.x, 40.0);
}

#[test]
fn max_content_track_uses_the_largest_single_track_contribution() {
    let mut tree = TestTree::default();
    let intrinsic_style = TestStyle {
        grid_column: placement(line(1), line(2)),
        justify_self: Some(JustifySelf::Start),
        align_self: Some(AlignSelf::Start),
        ..TestStyle::default()
    };
    let intrinsic = tree.push_leaf(
        intrinsic_style,
        Size::new(30.0, 10.0),
        Size::new(70.0, 10.0),
    );
    let mut marker_style = fixed_leaf_style(20.0, 10.0);
    marker_style.grid_column = placement(line(2), line(3));
    let marker = tree.push_leaf(marker_style, Size::new(20.0, 10.0), Size::new(20.0, 10.0));
    let root = tree.push_grid(
        grid_style(&[max_content_track(), px(20.0)], &[px(20.0)]),
        vec![intrinsic, marker],
    );

    definite_layout(&mut tree, root, 90.0, 20.0);

    assert_close(tree.layout(marker).location.x, 70.0);
    assert_size(tree.layout(intrinsic).size, Size::new(70.0, 10.0));
    assert!((2..=6).contains(&tree.session_node(intrinsic).measure_calls));
}

#[test]
fn intrinsic_growth_limit_uses_the_largest_item_in_a_track() {
    let mut tree = TestTree::default();
    let mut first_style = fixed_leaf_style(50.0, 10.0);
    first_style.grid_column = placement(line(1), line(2));
    let first = tree.push_leaf(first_style, Size::new(50.0, 10.0), Size::new(50.0, 10.0));
    let mut second_style = fixed_leaf_style(100.0, 10.0);
    second_style.grid_column = placement(line(1), line(2));
    let second = tree.push_leaf(second_style, Size::new(100.0, 10.0), Size::new(100.0, 10.0));
    let mut marker_style = fixed_leaf_style(0.0, 1.0);
    marker_style.position = Position::Absolute;
    marker_style.inset.left = LengthPercentageAuto::ZERO;
    marker_style.inset.top = LengthPercentageAuto::ZERO;
    marker_style.grid_column = placement(line(2), line(3));
    let marker = tree.push_leaf(marker_style, Size::ZERO, Size::ZERO);
    let intrinsic_max = TrackSizingFunction::minmax(
        MinTrackSizingFunction::Fixed(LengthPercentage::ZERO),
        MaxTrackSizingFunction::MaxContent,
    );
    let root = tree.push_grid(
        grid_style(&[intrinsic_max, px(0.0)], &[px(10.0)]),
        vec![first, second, marker],
    );

    definite_layout(&mut tree, root, 100.0, 10.0);

    assert_close(tree.layout(marker).location.x, 100.0);
}

#[test]
fn spanning_intrinsic_contribution_is_distributed_across_tracks() {
    let mut tree = TestTree::default();
    let spanning_style = TestStyle {
        grid_column: placement(line(1), line(3)),
        grid_row: placement(line(1), line(2)),
        ..TestStyle::default()
    };
    let spanning = tree.push_leaf(
        spanning_style,
        Size::new(100.0, 10.0),
        Size::new(100.0, 10.0),
    );
    let mut marker_style = fixed_leaf_style(0.0, 1.0);
    // Keep the line marker out of flow: an in-flow zero-sized item would
    // resolve this max-content track's growth limit to zero and legitimately
    // bias §12.5.1 redistribution toward the other track.
    marker_style.position = Position::Absolute;
    marker_style.inset.left = LengthPercentageAuto::ZERO;
    marker_style.inset.top = LengthPercentageAuto::ZERO;
    marker_style.grid_column = placement(line(2), line(3));
    marker_style.justify_self = Some(JustifySelf::Start);
    marker_style.align_self = Some(AlignSelf::Start);
    let marker = tree.push_leaf(marker_style, Size::ZERO, Size::ZERO);
    let root = tree.push_grid(
        grid_style(&[max_content_track(), max_content_track()], &[px(20.0)]),
        vec![spanning, marker],
    );

    definite_layout(&mut tree, root, 100.0, 20.0);

    assert_size(tree.layout(spanning).size, Size::new(100.0, 20.0));
    assert_close(tree.layout(marker).location.x, 50.0);
    assert!((2..=8).contains(&tree.session_node(spanning).measure_calls));
}

#[test]
fn content_alignment_distributes_the_track_grid_in_both_axes() {
    let mut tree = TestTree::default();
    let children = [
        fixed_leaf(&mut tree, 10.0, 10.0),
        fixed_leaf(&mut tree, 10.0, 10.0),
        fixed_leaf(&mut tree, 10.0, 10.0),
        fixed_leaf(&mut tree, 10.0, 10.0),
    ];
    let mut style = grid_style(&[px(40.0), px(40.0)], &[px(20.0), px(20.0)]);
    style.justify_content = Some(JustifyContent::SpaceBetween);
    style.align_content = Some(AlignContent::Center);
    style.justify_items = Some(JustifyItems::Start);
    style.align_items = Some(AlignItems::Start);
    let root = tree.push_grid(style, children.to_vec());

    definite_layout(&mut tree, root, 200.0, 100.0);

    assert_point(tree.layout(children[0]).location, Point::new(0.0, 30.0));
    assert_point(tree.layout(children[1]).location, Point::new(160.0, 30.0));
    assert_point(tree.layout(children[2]).location, Point::new(0.0, 50.0));
    assert_point(tree.layout(children[3]).location, Point::new(160.0, 50.0));
}

#[test]
fn self_alignment_positions_a_fixed_item_inside_its_area() {
    let mut tree = TestTree::default();
    let mut child_style = fixed_leaf_style(20.0, 10.0);
    child_style.justify_self = Some(JustifySelf::End);
    child_style.align_self = Some(AlignSelf::Center);
    let child = tree.push_leaf(child_style, Size::new(20.0, 10.0), Size::new(20.0, 10.0));
    let root = tree.push_grid(grid_style(&[px(100.0)], &[px(80.0)]), vec![child]);

    definite_layout(&mut tree, root, 100.0, 80.0);

    assert_point(tree.layout(child).location, Point::new(80.0, 35.0));
}

#[test]
fn baseline_group_aligns_items_and_sets_the_container_first_baseline() {
    let mut tree = TestTree::default();
    let first = fixed_leaf(&mut tree, 20.0, 20.0);
    tree.source_node_mut(first).first_baseline = Some(15.0);
    let second = fixed_leaf(&mut tree, 20.0, 30.0);
    tree.source_node_mut(second).first_baseline = Some(10.0);
    let mut style = grid_style(&[px(50.0), px(50.0)], &[px(40.0)]);
    style.align_items = Some(AlignItems::Baseline);
    style.justify_items = Some(JustifyItems::Start);
    let root = tree.push_grid(style, vec![first, second]);

    let output = definite_layout(&mut tree, root, 100.0, 40.0);

    assert_close(tree.layout(first).location.y, 0.0);
    assert_close(tree.layout(second).location.y, 5.0);
    assert_close(tree.layout(first).location.y + 15.0, 15.0);
    assert_close(tree.layout(second).location.y + 10.0, 15.0);
    assert_eq!(output.first_baselines.y, Some(15.0));
}

#[test]
fn block_axis_auto_margin_excludes_an_item_from_baseline_sharing() {
    let mut tree = TestTree::default();
    let first = fixed_leaf(&mut tree, 20.0, 20.0);
    tree.source_node_mut(first).first_baseline = Some(15.0);

    let mut second_style = fixed_leaf_style(20.0, 10.0);
    second_style.margin.top = LengthPercentageAuto::Auto;
    let second = tree.push_leaf(second_style, Size::new(20.0, 10.0), Size::new(20.0, 10.0));
    tree.source_node_mut(second).first_baseline = Some(5.0);

    let mut style = grid_style(&[px(50.0), px(50.0)], &[px(40.0)]);
    style.align_items = Some(AlignItems::Baseline);
    style.justify_items = Some(JustifyItems::Start);
    let root = tree.push_grid(style, vec![first, second]);

    definite_layout(&mut tree, root, 100.0, 40.0);

    assert_close(tree.layout(first).location.y, 0.0);
    assert_close(tree.layout(second).location.y, 30.0);
}

#[test]
fn container_baseline_comes_from_first_nonempty_row_with_synthesis() {
    let mut tree = TestTree::default();
    let first = fixed_leaf(&mut tree, 10.0, 10.0);
    let second = fixed_leaf(&mut tree, 10.0, 10.0);
    tree.source_node_mut(second).first_baseline = Some(5.0);
    let mut style = grid_style(&[px(20.0)], &[px(20.0), px(20.0)]);
    style.align_items = Some(AlignItems::Start);
    style.justify_items = Some(JustifyItems::Start);
    let root = tree.push_grid(style, vec![first, second]);

    let output = definite_layout(&mut tree, root, 20.0, 40.0);

    assert_eq!(output.first_baselines.y, Some(10.0));
    assert_close(tree.layout(second).location.y, 20.0);
}

#[test]
fn auto_sized_items_stretch_and_auto_margins_win_over_self_alignment() {
    let mut tree = TestTree::default();
    let stretch = intrinsic_leaf(&mut tree, Size::new(20.0, 10.0), Size::new(20.0, 10.0));
    let mut centered_style = fixed_leaf_style(20.0, 10.0);
    centered_style.grid_row = placement(line(2), line(3));
    centered_style.margin = Edges::uniform(LengthPercentageAuto::Auto);
    centered_style.justify_self = Some(JustifySelf::End);
    centered_style.align_self = Some(AlignSelf::End);
    let centered = tree.push_leaf(centered_style, Size::new(20.0, 10.0), Size::new(20.0, 10.0));
    let root = tree.push_grid(
        grid_style(&[px(100.0)], &[px(40.0), px(40.0)]),
        vec![stretch, centered],
    );

    definite_layout(&mut tree, root, 100.0, 80.0);

    assert_size(tree.layout(stretch).size, Size::new(100.0, 40.0));
    assert_point(tree.layout(centered).location, Point::new(40.0, 55.0));
    assert_close(tree.layout(centered).margin.left, 40.0);
    assert_close(tree.layout(centered).margin.right, 40.0);
    assert_close(tree.layout(centered).margin.top, 15.0);
    assert_close(tree.layout(centered).margin.bottom, 15.0);
}

#[test]
fn overflowing_auto_margins_zero_out_then_self_alignment_applies() {
    let mut tree = TestTree::default();
    let mut child_style = fixed_leaf_style(80.0, 10.0);
    child_style.margin.left = LengthPercentageAuto::Auto;
    child_style.margin.right = LengthPercentageAuto::Auto;
    child_style.justify_self = Some(JustifySelf::Center);
    child_style.align_self = Some(AlignSelf::Start);
    let child = tree.push_leaf(child_style, Size::new(80.0, 10.0), Size::new(80.0, 10.0));
    let root = tree.push_grid(grid_style(&[px(50.0)], &[px(20.0)]), vec![child]);

    definite_layout(&mut tree, root, 50.0, 20.0);

    assert_close(tree.layout(child).location.x, -15.0);
    assert_close(tree.layout(child).margin.left, 0.0);
    assert_close(tree.layout(child).margin.right, 0.0);
}

#[test]
fn rtl_flips_the_inline_track_axis_and_auto_placement_start() {
    let mut tree = TestTree::default();
    let children = [
        fixed_leaf(&mut tree, 10.0, 10.0),
        fixed_leaf(&mut tree, 10.0, 10.0),
        fixed_leaf(&mut tree, 10.0, 10.0),
    ];
    let mut style = grid_style(&[px(30.0), px(40.0), px(50.0)], &[px(20.0)]);
    style.direction = Direction::Rtl;
    style.gap.width = LengthPercentage::length(10.0);
    style.justify_items = Some(JustifyItems::Start);
    style.align_items = Some(AlignItems::Start);
    let root = tree.push_grid(style, children.to_vec());

    definite_layout(&mut tree, root, 140.0, 20.0);

    assert_close(tree.layout(children[0]).location.x, 130.0);
    assert_close(tree.layout(children[1]).location.x, 90.0);
    assert_close(tree.layout(children[2]).location.x, 40.0);
}

#[test]
fn order_sort_is_stable_and_recorded_in_layouts() {
    let mut tree = TestTree::default();
    let mut first_style = fixed_leaf_style(10.0, 10.0);
    first_style.order = 1;
    let first = tree.push_leaf(first_style, Size::ZERO, Size::new(10.0, 10.0));
    let mut earlier_style = fixed_leaf_style(10.0, 10.0);
    earlier_style.order = 0;
    let earlier = tree.push_leaf(earlier_style, Size::ZERO, Size::new(10.0, 10.0));
    let mut third_style = fixed_leaf_style(10.0, 10.0);
    third_style.order = 1;
    let third = tree.push_leaf(third_style, Size::ZERO, Size::new(10.0, 10.0));
    let root = tree.push_grid(
        grid_style(&[px(40.0), px(40.0), px(40.0)], &[px(20.0)]),
        vec![first, earlier, third],
    );

    definite_layout(&mut tree, root, 120.0, 20.0);

    assert_close(tree.layout(earlier).location.x, 0.0);
    assert_close(tree.layout(first).location.x, 40.0);
    assert_close(tree.layout(third).location.x, 80.0);
    assert_eq!(tree.layout(earlier).order, 0);
    assert_eq!(tree.layout(first).order, 1);
    assert_eq!(tree.layout(third).order, 2);
}

#[test]
fn measure_goal_probes_intrinsics_without_durable_writes() {
    let mut tree = TestTree::default();
    let child = intrinsic_leaf(&mut tree, Size::new(30.0, 10.0), Size::new(60.0, 20.0));
    let root = tree.push_grid(
        grid_style(&[max_content_track()], &[max_content_track()]),
        vec![child],
    );
    let mut sentinel = Layout::default();
    sentinel.location = Point::new(123.0, 456.0);
    sentinel.size = Size::new(7.0, 8.0);
    tree.session_node_mut(child).layout = sentinel;
    tree.session_node_mut(root).layout = sentinel;

    let output = tree.compute_child_layout(
        root,
        LayoutInput::compute_size(
            Size::new(Some(100.0), Some(40.0)),
            Size::new(Some(100.0), Some(40.0)),
            Size::new(
                AvailableSpace::Definite(100.0),
                AvailableSpace::Definite(40.0),
            ),
            RequestedAxis::Both,
        ),
    );

    assert_size(output.size, Size::new(100.0, 40.0));
    assert_eq!(tree.session.layout_writes, 0);
    assert_eq!(tree.layout(child), sentinel);
    assert_eq!(tree.layout(root), sentinel);
    assert!((1..=6).contains(&tree.session_node(child).measure_calls));
}

#[test]
fn hidden_and_out_of_flow_children_do_not_occupy_grid_cells() {
    let mut tree = TestTree::default();
    let first = fixed_leaf(&mut tree, 10.0, 10.0);
    let mut hidden_style = fixed_leaf_style(1_000.0, 1_000.0);
    hidden_style.box_generation_mode = BoxGenerationMode::None;
    let hidden = tree.push_leaf(hidden_style, Size::ZERO, Size::new(1_000.0, 1_000.0));
    tree.session_node_mut(hidden).layout.size = Size::new(999.0, 999.0);

    let mut absolute_style = fixed_leaf_style(20.0, 10.0);
    absolute_style.position = Position::Absolute;
    absolute_style.inset.left = LengthPercentageAuto::Length(7.0);
    absolute_style.inset.top = LengthPercentageAuto::Length(9.0);
    let absolute = tree.push_leaf(absolute_style, Size::new(20.0, 10.0), Size::new(20.0, 10.0));

    let mut hoisted_style = fixed_leaf_style(20.0, 10.0);
    hoisted_style.position = Position::AbsoluteHoisted;
    let hoisted = tree.push_leaf(hoisted_style, Size::new(20.0, 10.0), Size::new(20.0, 10.0));
    let second = fixed_leaf(&mut tree, 10.0, 10.0);
    let mut style = grid_style(&[px(50.0), px(50.0)], &[px(20.0)]);
    style.justify_items = Some(JustifyItems::Start);
    style.align_items = Some(AlignItems::Start);
    let root = tree.push_grid(style, vec![first, hidden, absolute, hoisted, second]);

    definite_layout(&mut tree, root, 100.0, 20.0);

    assert_close(tree.layout(first).location.x, 0.0);
    assert_close(tree.layout(second).location.x, 50.0);
    assert_eq!(tree.layout(hidden).size, Size::ZERO);
    assert_eq!(tree.session_node(hidden).measure_calls, 0);
    assert_point(tree.layout(absolute).location, Point::new(7.0, 9.0));
    assert_eq!(tree.session_node(hoisted).layout_writes, 0);
    assert_eq!(tree.session_node(hoisted).static_position_writes, 1);
}

#[test]
fn direct_absolute_child_uses_its_definite_grid_area_as_containing_block() {
    let mut tree = TestTree::default();
    let child_style = TestStyle {
        position: Position::Absolute,
        inset: Edges {
            left: LengthPercentageAuto::Length(5.0),
            right: LengthPercentageAuto::Length(10.0),
            top: LengthPercentageAuto::Length(2.0),
            bottom: LengthPercentageAuto::Length(3.0),
        },
        grid_column: placement(line(2), line(3)),
        grid_row: placement(line(2), line(3)),
        ..TestStyle::default()
    };
    let child = tree.push_leaf(child_style, Size::ZERO, Size::ZERO);
    let mut style = grid_style(&[px(50.0), px(70.0)], &[px(30.0), px(40.0)]);
    style.gap = Size::new(
        LengthPercentage::length(10.0),
        LengthPercentage::length(5.0),
    );
    let root = tree.push_grid(style, vec![child]);

    definite_layout(&mut tree, root, 130.0, 75.0);

    // The selected area starts at (60, 35) and is 70x40. Opposing insets
    // stretch the auto-sized absolute box within that area, not the grid root.
    assert_point(tree.layout(child).location, Point::new(65.0, 37.0));
    assert_size(tree.layout(child).size, Size::new(55.0, 35.0));
}

#[test]
fn absolute_auto_grid_lines_use_the_container_padding_edges() {
    let mut tree = TestTree::default();
    let child_style = TestStyle {
        position: Position::Absolute,
        inset: Edges::uniform(LengthPercentageAuto::ZERO),
        ..TestStyle::default()
    };
    let child = tree.push_leaf(child_style, Size::ZERO, Size::ZERO);
    let mut style = grid_style(&[], &[]);
    style.border = Edges::uniform(LengthPercentage::length(2.0));
    style.padding = Edges {
        left: LengthPercentage::length(10.0),
        right: LengthPercentage::length(20.0),
        top: LengthPercentage::length(5.0),
        bottom: LengthPercentage::length(15.0),
    };
    let root = tree.push_grid(style, vec![child]);

    definite_layout(&mut tree, root, 120.0, 80.0);

    // With both placement lines auto, Grid §10.1 uses the padding edges,
    // not the content edges, as the abspos containing block.
    assert_point(tree.layout(child).location, Point::new(2.0, 2.0));
    assert_size(tree.layout(child).size, Size::new(116.0, 76.0));
}

#[test]
fn absolute_static_fallback_uses_content_box_not_selected_grid_area() {
    let mut tree = TestTree::default();
    let mut child_style = fixed_leaf_style(20.0, 10.0);
    child_style.position = Position::Absolute;
    child_style.grid_column = placement(line(2), line(3));
    child_style.grid_row = placement(line(1), line(2));
    child_style.justify_self = Some(JustifySelf::Center);
    child_style.align_self = Some(AlignSelf::End);
    let child = tree.push_leaf(child_style, Size::new(20.0, 10.0), Size::new(20.0, 10.0));
    let root = tree.push_grid(grid_style(&[px(30.0), px(70.0)], &[px(50.0)]), vec![child]);

    definite_layout(&mut tree, root, 100.0, 50.0);

    // The selected area is x=30..100, but Grid §10.2 defines the static
    // position as if this were the sole item in the full content-edge area.
    assert_point(tree.layout(child).location, Point::new(40.0, 40.0));
}

#[test]
fn hoisted_absolute_records_grid_aware_static_position_for_positioned_pass() {
    let mut tree = TestTree::default();
    let mut child_style = fixed_leaf_style(20.0, 10.0);
    child_style.position = Position::AbsoluteHoisted;
    child_style.justify_self = Some(JustifySelf::Center);
    child_style.align_self = Some(AlignSelf::End);
    child_style.margin.left = LengthPercentageAuto::Length(5.0);
    child_style.margin.top = LengthPercentageAuto::Length(3.0);
    let child = tree.push_leaf(child_style, Size::new(20.0, 10.0), Size::new(20.0, 10.0));
    let root = tree.push_grid(grid_style(&[], &[]), vec![child]);

    definite_layout(&mut tree, root, 100.0, 50.0);

    assert_eq!(tree.session_node(child).layout_writes, 0);
    assert_eq!(
        tree.session_node(child).static_position,
        Some(Point::new(37.5, 37.0))
    );

    let static_position = tree.session_node(child).static_position.unwrap();
    let positioned = compute_absolute_layout(
        &tree.source,
        &mut tree.session,
        child,
        Size::new(100.0, 50.0),
        static_position,
    );
    assert_point(positioned.location, Point::new(42.5, 40.0));
    assert_size(positioned.size, Size::new(20.0, 10.0));
}

#[test]
fn hoisted_static_position_ignores_placement_and_measures_auto_content() {
    let mut tree = TestTree::default();
    let child_style = TestStyle {
        position: Position::AbsoluteHoisted,
        grid_column: placement(line(2), line(3)),
        grid_row: placement(line(1), line(2)),
        justify_self: Some(JustifySelf::Center),
        align_self: Some(AlignSelf::End),
        ..TestStyle::default()
    };
    let child = tree.push_leaf(child_style, Size::new(20.0, 10.0), Size::new(20.0, 10.0));
    let root = tree.push_grid(grid_style(&[px(30.0), px(70.0)], &[px(50.0)]), vec![child]);

    definite_layout(&mut tree, root, 100.0, 50.0);

    assert!(tree.session_node(child).measure_calls > 0);
    assert_eq!(tree.session_node(child).layout_writes, 0);
    assert_eq!(
        tree.session_node(child).static_position,
        Some(Point::new(40.0, 40.0))
    );
}

#[test]
fn nested_grid_dispatch_composes_without_erasure() {
    let mut tree = TestTree::default();
    let first = intrinsic_leaf(&mut tree, Size::ZERO, Size::ZERO);
    let second = intrinsic_leaf(&mut tree, Size::ZERO, Size::ZERO);
    let inner = tree.push_grid(
        grid_style(&[fr(1.0), fr(1.0)], &[px(20.0)]),
        vec![first, second],
    );
    let root = tree.push_grid(grid_style(&[px(120.0)], &[px(40.0)]), vec![inner]);

    definite_layout(&mut tree, root, 120.0, 40.0);

    assert_size(tree.layout(inner).size, Size::new(120.0, 40.0));
    assert_size(tree.layout(first).size, Size::new(60.0, 20.0));
    assert_point(tree.layout(second).location, Point::new(60.0, 0.0));
}

#[test]
fn grid_dispatch_composes_with_nested_flex_without_erasure() {
    let mut tree = TestTree::default();
    let first = fixed_leaf(&mut tree, 20.0, 10.0);
    let second = fixed_leaf(&mut tree, 20.0, 10.0);
    let inner = tree.push_flex(
        TestStyle {
            align_items: Some(AlignItems::Start),
            justify_content: Some(JustifyContent::SpaceBetween),
            ..TestStyle::default()
        },
        vec![first, second],
    );
    let root = tree.push_grid(grid_style(&[px(120.0)], &[px(40.0)]), vec![inner]);

    definite_layout(&mut tree, root, 120.0, 40.0);

    assert_size(tree.layout(inner).size, Size::new(120.0, 40.0));
    assert_point(tree.layout(first).location, Point::ZERO);
    assert_point(tree.layout(second).location, Point::new(100.0, 0.0));
}

#[test]
fn intrinsic_probe_count_stays_linear_in_item_count() {
    const ITEM_COUNT: usize = 24;
    const MAX_PROBES_PER_ITEM: usize = 6;

    let mut tree = TestTree::default();
    let mut children = Vec::with_capacity(ITEM_COUNT);
    for _ in 0..ITEM_COUNT {
        children.push(intrinsic_leaf(
            &mut tree,
            Size::new(5.0, 10.0),
            Size::new(10.0, 10.0),
        ));
    }
    let columns = std::iter::repeat_n(max_content_track(), ITEM_COUNT).collect::<Vec<_>>();
    let root = tree.push_grid(grid_style(&columns, &[px(20.0)]), children.clone());

    definite_layout(&mut tree, root, 240.0, 20.0);

    assert!(tree.session.leaf_measure_calls >= ITEM_COUNT);
    assert!(tree.session.leaf_measure_calls <= ITEM_COUNT * MAX_PROBES_PER_ITEM);
    for child in children {
        assert!((1..=MAX_PROBES_PER_ITEM).contains(&tree.session_node(child).measure_calls));
    }
}

fn min_content_layout(tree: &mut TestTree, root: NodeId) -> LayoutOutput {
    tree.compute_child_layout(
        root,
        LayoutInput::perform_layout(Size::NONE, Size::NONE, Size::MIN_CONTENT),
    )
}

#[test]
fn multitrack_auto_minimum_contributes_to_intrinsic_track_sizes() {
    let mut tree = TestTree::default();
    let item_style = TestStyle {
        grid_column: placement(line(1), line(3)),
        grid_row: placement(line(1), line(2)),
        ..TestStyle::default()
    };
    let item = tree.push_leaf(item_style, Size::new(200.0, 10.0), Size::new(200.0, 10.0));
    let root = tree.push_grid(
        grid_style(
            &[TrackSizingFunction::AUTO, TrackSizingFunction::AUTO],
            &[px(10.0)],
        ),
        vec![item],
    );

    let output = min_content_layout(&mut tree, root);

    // Grid §6.6 keeps the content-based automatic minimum for a multi-track
    // item when at least one minimum is `auto` and none of the tracks flex.
    assert_size(output.size, Size::new(200.0, 10.0));
    assert_size(tree.layout(item).size, Size::new(200.0, 10.0));
}

#[test]
fn spanning_item_minimum_grows_flexible_tracks_before_fr_expansion() {
    let mut tree = TestTree::default();
    let mut spanning_style = fixed_leaf_style(200.0, 10.0);
    spanning_style.grid_column = placement(line(1), line(3));
    spanning_style.grid_row = placement(line(1), line(2));
    spanning_style.justify_self = Some(JustifySelf::Start);
    let spanning = tree.push_leaf(
        spanning_style,
        Size::new(200.0, 10.0),
        Size::new(200.0, 10.0),
    );
    let mut marker_style = fixed_leaf_style(0.0, 1.0);
    marker_style.grid_column = placement(line(2), line(3));
    marker_style.grid_row = placement(line(1), line(2));
    marker_style.justify_self = Some(JustifySelf::Start);
    marker_style.align_self = Some(AlignSelf::Start);
    let marker = tree.push_leaf(marker_style, Size::ZERO, Size::ZERO);
    let root = tree.push_grid(
        grid_style(&[fr(1.0), fr(1.0)], &[px(10.0)]),
        vec![spanning, marker],
    );

    definite_layout(&mut tree, root, 100.0, 10.0);

    // Grid §12.5 step 4 distributes the 200px contribution across the two
    // flexible tracks. They overflow the 100px container at 100px each.
    assert_close(tree.layout(marker).location.x, 100.0);
}

#[test]
fn baseline_shims_expand_an_intrinsic_row_before_following_rows_are_positioned() {
    let mut tree = TestTree::default();
    let mut first_style = fixed_leaf_style(20.0, 20.0);
    first_style.grid_column = placement(line(1), line(2));
    first_style.grid_row = placement(line(1), line(2));
    let first = tree.push_leaf(first_style, Size::new(20.0, 20.0), Size::new(20.0, 20.0));
    tree.source_node_mut(first).first_baseline = Some(15.0);

    let mut second_style = fixed_leaf_style(20.0, 20.0);
    second_style.grid_column = placement(line(2), line(3));
    second_style.grid_row = placement(line(1), line(2));
    let second = tree.push_leaf(second_style, Size::new(20.0, 20.0), Size::new(20.0, 20.0));
    tree.source_node_mut(second).first_baseline = Some(5.0);

    let mut marker_style = fixed_leaf_style(10.0, 10.0);
    marker_style.grid_column = placement(line(1), line(2));
    marker_style.grid_row = placement(line(2), line(3));
    marker_style.align_self = Some(AlignSelf::Start);
    let marker = tree.push_leaf(marker_style, Size::new(10.0, 10.0), Size::new(10.0, 10.0));

    let mut style = grid_style(
        &[px(50.0), px(50.0)],
        &[TrackSizingFunction::AUTO, px(10.0)],
    );
    style.align_items = Some(AlignItems::Baseline);
    style.align_content = Some(AlignContent::Start);
    let root = tree.push_grid(style, vec![first, second, marker]);

    definite_layout(&mut tree, root, 100.0, 40.0);

    // The shared row needs 15px ascent + 15px descent. Applying baseline
    // offsets only after sizing leaves it at 20px and overlaps the next row.
    assert_close(tree.layout(second).location.y, 10.0);
    assert_close(tree.layout(marker).location.y, 30.0);
}

#[test]
fn auto_repeat_uses_the_smallest_count_that_fulfils_a_definite_minimum() {
    let mut tree = TestTree::default();
    let children = [
        fixed_leaf(&mut tree, 10.0, 10.0),
        fixed_leaf(&mut tree, 10.0, 10.0),
        fixed_leaf(&mut tree, 10.0, 10.0),
    ];
    let mut style = grid_style(&[], &[px(20.0)]);
    style.min_size.width = Dimension::Length(250.0);
    style.template_columns = vec![repeat(RepetitionCount::AutoFill, vec![px(100.0)])];
    style.justify_items = Some(JustifyItems::Start);
    style.align_items = Some(AlignItems::Start);
    let root = tree.push_grid(style, children.to_vec());

    let output = intrinsic_layout(&mut tree, root);

    // Grid §7.2.3.2 uses ceil-like behavior for a definite minimum: three
    // 100px repetitions are needed to fulfil 250px.
    assert_close(output.size.width, 300.0);
    assert_point(tree.layout(children[2]).location, Point::new(200.0, 0.0));
}

#[test]
fn overflowing_positional_content_alignment_preserves_negative_free_space() {
    let mut tree = TestTree::default();
    let mut first_style = fixed_leaf_style(10.0, 10.0);
    first_style.justify_self = Some(JustifySelf::Start);
    let first = tree.push_leaf(first_style, Size::new(10.0, 10.0), Size::new(10.0, 10.0));
    let mut second_style = fixed_leaf_style(10.0, 10.0);
    second_style.justify_self = Some(JustifySelf::Start);
    let second = tree.push_leaf(second_style, Size::new(10.0, 10.0), Size::new(10.0, 10.0));
    let mut style = grid_style(&[px(80.0), px(80.0)], &[px(20.0)]);
    style.justify_content = Some(JustifyContent::Center);
    let root = tree.push_grid(style, vec![first, second]);

    definite_layout(&mut tree, root, 100.0, 20.0);

    // Unqualified positional alignment is unsafe on overflow. Distributed
    // values have separate safe fallbacks, but center must remain centered.
    assert_close(tree.layout(first).location.x, -30.0);
    assert_close(tree.layout(second).location.x, 50.0);
}

#[test]
fn definite_preferred_size_limits_an_items_intrinsic_contribution() {
    let mut tree = TestTree::default();
    let mut child_style = fixed_leaf_style(50.0, 10.0);
    child_style.grid_column = placement(line(1), line(2));
    child_style.grid_row = placement(line(1), line(2));
    child_style.justify_self = Some(JustifySelf::Start);
    let child = tree.push_leaf(child_style, Size::new(200.0, 10.0), Size::new(200.0, 10.0));
    let mut marker_style = fixed_leaf_style(0.0, 1.0);
    marker_style.grid_column = placement(line(2), line(3));
    marker_style.grid_row = placement(line(1), line(2));
    marker_style.justify_self = Some(JustifySelf::Start);
    marker_style.align_self = Some(AlignSelf::Start);
    let marker = tree.push_leaf(marker_style, Size::ZERO, Size::ZERO);
    let root = tree.push_grid(
        grid_style(&[TrackSizingFunction::AUTO, px(0.0)], &[px(10.0)]),
        vec![child, marker],
    );

    let output = min_content_layout(&mut tree, root);

    // The 200px contents overflow the specified 50px box; they do not make
    // that box's min/max-content contribution 200px.
    assert_close(output.size.width, 50.0);
    assert_close(tree.layout(marker).location.x, 50.0);
}

#[test]
fn auto_repeat_clamps_its_counting_basis_with_minimum_precedence() {
    let mut tree = TestTree::default();
    let mut marker_style = fixed_leaf_style(1.0, 1.0);
    marker_style.position = Position::Absolute;
    marker_style.inset.left = LengthPercentageAuto::ZERO;
    marker_style.inset.top = LengthPercentageAuto::ZERO;
    marker_style.grid_column = placement(line(4), line(5));
    marker_style.grid_row = placement(line(1), line(2));
    marker_style.justify_self = Some(JustifySelf::Start);
    marker_style.align_self = Some(AlignSelf::Start);
    let marker = tree.push_leaf(marker_style, Size::new(1.0, 1.0), Size::new(1.0, 1.0));

    let mut style = grid_style(&[], &[px(10.0)]);
    style.min_size.width = Dimension::Length(200.0);
    style.max_size.width = Dimension::Length(100.0);
    style.template_columns = vec![repeat(RepetitionCount::AutoFill, vec![px(50.0)])];
    let root = tree.push_grid(style, vec![marker]);

    let output = intrinsic_layout(&mut tree, root);

    // CSS minimum sizes take precedence over conflicting maximum sizes. The
    // 200px used counting basis therefore creates four explicit tracks.
    assert_close(output.size.width, 200.0);
    assert_close(tree.layout(marker).location.x, 150.0);
}

#[test]
fn auto_repeat_resolves_percentage_gap_against_its_max_constraint() {
    let mut tree = TestTree::default();
    let mut marker_style = fixed_leaf_style(1.0, 1.0);
    marker_style.position = Position::Absolute;
    marker_style.inset.left = LengthPercentageAuto::ZERO;
    marker_style.inset.top = LengthPercentageAuto::ZERO;
    marker_style.grid_column = placement(line(4), GridPlacement::Auto);
    marker_style.grid_row = placement(line(1), line(2));
    marker_style.justify_self = Some(JustifySelf::Start);
    marker_style.align_self = Some(AlignSelf::Start);
    let marker = tree.push_leaf(marker_style, Size::new(1.0, 1.0), Size::new(1.0, 1.0));

    let mut style = grid_style(&[], &[px(10.0)]);
    style.max_size.width = Dimension::Length(200.0);
    style.template_columns = vec![repeat(RepetitionCount::AutoFill, vec![px(50.0)])];
    style.gap.width = LengthPercentage::percent(0.10);
    let root = tree.push_grid(style, vec![marker]);

    let output = intrinsic_layout(&mut tree, root);

    // The 20px gap resolved against max-width allows only three repetitions.
    // It contributes zero to intrinsic width, then resolves to 15px against
    // the resulting 150px content box, putting the final line at 180px.
    assert_close(output.size.width, 150.0);
    assert_close(tree.layout(marker).location.x, 180.0);
}

#[test]
fn spanning_scroll_item_uses_limited_min_content_under_intrinsic_constraint() {
    let mut tree = TestTree::default();
    let item_style = TestStyle {
        grid_column: placement(line(1), line(3)),
        grid_row: placement(line(1), line(2)),
        overflow: Point::new(Overflow::Scroll, Overflow::Visible),
        ..TestStyle::default()
    };
    let item = tree.push_leaf(item_style, Size::new(200.0, 10.0), Size::new(200.0, 10.0));
    let root = tree.push_grid(
        grid_style(
            &[TrackSizingFunction::AUTO, TrackSizingFunction::AUTO],
            &[px(10.0)],
        ),
        vec![item],
    );

    let output = min_content_layout(&mut tree, root);

    // The scroll container's automatic minimum is zero, but Grid §12.5 uses
    // its limited min-content contribution in this intrinsic sizing phase.
    assert_size(output.size, Size::new(200.0, 10.0));
}

#[test]
fn spanning_growth_only_expands_tracks_marked_infinitely_growable() {
    let mut tree = TestTree::default();
    let first_style = TestStyle {
        grid_column: placement(line(1), line(2)),
        grid_row: placement(line(1), line(2)),
        min_size: Size::new(Dimension::ZERO, Dimension::ZERO),
        justify_self: Some(JustifySelf::Start),
        ..TestStyle::default()
    };
    let first = tree.push_leaf(first_style, Size::new(10.0, 10.0), Size::new(10.0, 10.0));

    let spanning_style = TestStyle {
        grid_column: placement(line(1), line(3)),
        grid_row: placement(line(1), line(2)),
        min_size: Size::new(Dimension::ZERO, Dimension::ZERO),
        justify_self: Some(JustifySelf::Start),
        ..TestStyle::default()
    };
    let spanning = tree.push_leaf(
        spanning_style,
        Size::new(30.0, 10.0),
        Size::new(100.0, 10.0),
    );

    let mut marker_style = fixed_leaf_style(0.0, 1.0);
    marker_style.position = Position::Absolute;
    marker_style.inset.left = LengthPercentageAuto::ZERO;
    marker_style.inset.top = LengthPercentageAuto::ZERO;
    marker_style.grid_column = placement(line(2), line(3));
    marker_style.grid_row = placement(line(1), line(2));
    marker_style.justify_self = Some(JustifySelf::Start);
    marker_style.align_self = Some(AlignSelf::Start);
    let marker = tree.push_leaf(marker_style, Size::ZERO, Size::ZERO);

    let intrinsic = TrackSizingFunction::minmax(
        MinTrackSizingFunction::MinContent,
        MaxTrackSizingFunction::MaxContent,
    );
    let root = tree.push_grid(
        grid_style(&[intrinsic, intrinsic], &[px(10.0)]),
        vec![first, spanning, marker],
    );

    let output = intrinsic_layout(&mut tree, root);

    // This is the example from Grid §12.5: track one stays at 10px and the
    // track whose infinite limit became finite grows to 90px.
    assert_close(output.size.width, 100.0);
    assert_close(tree.layout(marker).location.x, 10.0);
}

#[test]
fn spanning_base_uses_non_affected_track_before_exceeding_growth_limit() {
    let mut tree = TestTree::default();
    let spanning_style = TestStyle {
        grid_column: placement(line(1), line(3)),
        grid_row: placement(line(1), line(2)),
        justify_self: Some(JustifySelf::Start),
        ..TestStyle::default()
    };
    let spanning = tree.push_leaf(
        spanning_style,
        Size::new(100.0, 10.0),
        Size::new(100.0, 10.0),
    );

    let mut marker_style = fixed_leaf_style(0.0, 1.0);
    marker_style.position = Position::Absolute;
    marker_style.inset.left = LengthPercentageAuto::ZERO;
    marker_style.inset.top = LengthPercentageAuto::ZERO;
    marker_style.grid_column = placement(line(2), line(3));
    marker_style.grid_row = placement(line(1), line(2));
    marker_style.justify_self = Some(JustifySelf::Start);
    marker_style.align_self = Some(AlignSelf::Start);
    let marker = tree.push_leaf(marker_style, Size::ZERO, Size::ZERO);

    let first = TrackSizingFunction::minmax(
        MinTrackSizingFunction::Auto,
        MaxTrackSizingFunction::Fixed(LengthPercentage::length(10.0)),
    );
    let second = TrackSizingFunction::minmax(
        MinTrackSizingFunction::Fixed(LengthPercentage::ZERO),
        MaxTrackSizingFunction::Fixed(LengthPercentage::length(100.0)),
    );
    let root = tree.push_grid(
        grid_style(&[first, second], &[px(10.0)]),
        vec![spanning, marker],
    );

    definite_layout(&mut tree, root, 100.0, 10.0);

    // §12.5.1 first fills the affected auto-min track to 10px, then puts the
    // remaining 90px into the non-affected track before violating that cap.
    assert_close(tree.layout(marker).location.x, 10.0);
}

#[test]
fn normal_item_alignment_preserves_a_preferred_aspect_ratio() {
    let mut tree = TestTree::default();
    let child_style = TestStyle {
        aspect_ratio: Some(2.0),
        ..TestStyle::default()
    };
    let child = tree.push_leaf(child_style, Size::new(20.0, 10.0), Size::new(20.0, 10.0));
    let root = tree.push_grid(grid_style(&[px(100.0)], &[px(100.0)]), vec![child]);

    definite_layout(&mut tree, root, 100.0, 100.0);

    // Grid's `normal` alignment uses block sizing instead of stretching both
    // axes when an item has a preferred aspect ratio.
    assert_size(tree.layout(child).size, Size::new(100.0, 50.0));
}

#[test]
fn absolute_auto_line_uses_padding_edge_of_overflowing_scrollable_area() {
    let mut tree = TestTree::default();
    let child_style = TestStyle {
        position: Position::Absolute,
        inset: Edges {
            left: LengthPercentageAuto::ZERO,
            right: LengthPercentageAuto::ZERO,
            top: LengthPercentageAuto::ZERO,
            bottom: LengthPercentageAuto::ZERO,
        },
        grid_column: placement(line(1), GridPlacement::Auto),
        grid_row: placement(line(1), line(2)),
        ..TestStyle::default()
    };
    let child = tree.push_leaf(child_style, Size::ZERO, Size::ZERO);
    let root = tree.push_grid(grid_style(&[px(200.0)], &[px(20.0)]), vec![child]);

    definite_layout(&mut tree, root, 100.0, 20.0);

    // Grid §10.1 uses the padding edge of the scrollable area for an auto
    // line, so overflowing tracks extend this containing block to 200px.
    assert_size(tree.layout(child).size, Size::new(200.0, 20.0));
}

#[test]
fn cross_axis_rerun_uses_effective_content_alignment_gaps() {
    let mut tree = TestTree::default();
    let child_style = TestStyle {
        aspect_ratio: Some(1.0),
        grid_row: placement(line(1), line(3)),
        min_size: Size::new(Dimension::ZERO, Dimension::ZERO),
        justify_self: Some(JustifySelf::Start),
        align_self: Some(AlignSelf::Stretch),
        ..TestStyle::default()
    };
    let child = tree.push_leaf(child_style, Size::ZERO, Size::ZERO);

    let mut style = grid_style(&[max_content_track()], &[px(20.0), px(20.0)]);
    style.align_content = Some(AlignContent::SpaceBetween);
    let root = tree.push_grid(style, vec![child]);
    let output = tree.compute_child_layout(
        root,
        LayoutInput::perform_layout(
            Size::new(None, Some(100.0)),
            Size::new(None, Some(100.0)),
            Size::new(AvailableSpace::MaxContent, AvailableSpace::Definite(100.0)),
        ),
    );

    // The effective 60px distributed row gap makes the spanning area 100px.
    // Grid §12.3 requires that aligned area size in the column feedback pass.
    assert_size(output.size, Size::new(100.0, 100.0));
    assert_size(tree.layout(child).size, Size::new(100.0, 100.0));
}
