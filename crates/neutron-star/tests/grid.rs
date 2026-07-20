//! Spec-focused CSS Grid integration tests over a plain `Vec`-backed host.
//!
//! There is deliberately no styling engine here: `TestStyle` is already a
//! computed-style view serving stylo computed values, track lists are
//! borrowed as real stylo `GridTemplateComponent` values (including
//! `repeat(...)` groups built as `TrackRepeat`/`TrackList` fixtures), and
//! display dispatch is static all the way into the generic Grid and leaf
//! entry points.

mod support;

use std::cell::Cell;
use std::fmt;

use neutron_star::compute::{
    FnLeafMeasurer, LeafMetrics, compute_absolute_layout, compute_cached_layout,
    compute_flexbox_layout, compute_grid_layout, compute_leaf_layout, hide_subtree,
};
use neutron_star::prelude::*;
use stylo::computed_values::{box_sizing, direction, flex_direction, flex_wrap};
use stylo::values::computed::length::NonNegativeLengthPercentageOrNormal;
use stylo::values::computed::{
    AspectRatio, Au, BorderSideWidth, ContentDistribution, Display, FlexBasis, GridAutoFlow,
    GridLine, GridTemplateComponent, ImplicitGridTracks, Inset, Integer, ItemPlacement,
    JustifyItems as ComputedJustifyItems, Length, LengthPercentage, Margin, MaxSize,
    NonNegativeLengthPercentage, NonNegativeNumber, Overflow, Percentage, PositionProperty, Ratio,
    SelfAlignment, Size as StyleSize, TrackBreadth, TrackList, TrackSize,
};
use stylo::values::generics::NonNegative;
use stylo::values::generics::grid::{
    Flex, ImplicitGridTracks as GenericImplicitGridTracks, RepeatCount, TrackListValue, TrackRepeat,
};
use stylo::values::generics::position::PreferredRatio;
use stylo::values::specified::align::{AlignFlags, JustifyItems as SpecifiedJustifyItems};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TestDisplay {
    Flex,
    Grid,
    Leaf,
}

#[derive(Debug, Clone)]
struct TestStyle {
    display: Display,
    position: PositionProperty,
    inset: Edges<Inset>,
    size: Size<StyleSize>,
    min_size: Size<StyleSize>,
    max_size: Size<MaxSize>,
    aspect_ratio: AspectRatio,
    margin: Edges<Margin>,
    padding: Edges<NonNegativeLengthPercentage>,
    border: Edges<BorderSideWidth>,
    overflow: Point<Overflow>,
    box_sizing: box_sizing::T,
    direction: direction::T,
    template_rows: GridTemplateComponent,
    template_columns: GridTemplateComponent,
    auto_rows: ImplicitGridTracks,
    auto_columns: ImplicitGridTracks,
    auto_flow: GridAutoFlow,
    gap: Size<NonNegativeLengthPercentageOrNormal>,
    align_content: ContentDistribution,
    justify_content: ContentDistribution,
    align_items: ItemPlacement,
    justify_items: ComputedJustifyItems,
    grid_row: Line<GridLine>,
    grid_column: Line<GridLine>,
    align_self: SelfAlignment,
    justify_self: SelfAlignment,
    flex_basis: FlexBasis,
    flex_grow: NonNegativeNumber,
    flex_shrink: NonNegativeNumber,
    order: i32,
}

impl Default for TestStyle {
    fn default() -> Self {
        Self {
            display: Display::Grid,
            position: PositionProperty::Relative,
            inset: Edges::uniform(Inset::auto()),
            size: Size::new(StyleSize::Auto, StyleSize::Auto),
            min_size: Size::new(StyleSize::Auto, StyleSize::Auto),
            max_size: Size::new(MaxSize::none(), MaxSize::none()),
            aspect_ratio: AspectRatio::auto(),
            margin: Edges::uniform(margin_px(0.0)),
            padding: Edges::uniform(nn_px(0.0)),
            border: Edges::uniform(BorderSideWidth(Au(0))),
            overflow: Point::new(Overflow::Visible, Overflow::Visible),
            box_sizing: box_sizing::T::ContentBox,
            direction: direction::T::Ltr,
            template_rows: GridTemplateComponent::None,
            template_columns: GridTemplateComponent::None,
            auto_rows: ImplicitGridTracks::default(),
            auto_columns: ImplicitGridTracks::default(),
            auto_flow: GridAutoFlow::ROW,
            gap: Size::new(
                NonNegativeLengthPercentageOrNormal::Normal,
                NonNegativeLengthPercentageOrNormal::Normal,
            ),
            align_content: ContentDistribution::normal(),
            justify_content: ContentDistribution::normal(),
            align_items: ItemPlacement::normal(),
            justify_items: ComputedJustifyItems {
                specified: SpecifiedJustifyItems::legacy(),
                computed: SpecifiedJustifyItems::normal(),
            },
            grid_row: Line::new(GridLine::auto(), GridLine::auto()),
            grid_column: Line::new(GridLine::auto(), GridLine::auto()),
            align_self: SelfAlignment::auto(),
            justify_self: SelfAlignment::auto(),
            flex_basis: FlexBasis::auto(),
            flex_grow: NonNegative(0.0),
            flex_shrink: NonNegative(1.0),
            order: 0,
        }
    }
}

impl CoreStyle for TestStyle {
    fn display(&self) -> Display {
        self.display
    }

    fn position(&self) -> PositionProperty {
        self.position
    }

    fn inset(&self) -> Edges<Inset> {
        self.inset.clone()
    }

    fn size(&self) -> Size<StyleSize> {
        self.size.clone()
    }

    fn min_size(&self) -> Size<StyleSize> {
        self.min_size.clone()
    }

    fn max_size(&self) -> Size<MaxSize> {
        self.max_size.clone()
    }

    fn aspect_ratio(&self) -> AspectRatio {
        self.aspect_ratio
    }

    fn margin(&self) -> Edges<Margin> {
        self.margin.clone()
    }

    fn padding(&self) -> Edges<NonNegativeLengthPercentage> {
        self.padding.clone()
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

    fn gap(&self) -> Size<NonNegativeLengthPercentageOrNormal> {
        self.gap.clone()
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

    fn justify_items(&self) -> ComputedJustifyItems {
        self.justify_items
    }
}

impl GridItemStyle for TestStyle {
    fn grid_row_start(&self) -> GridLine {
        self.grid_row.start.clone()
    }

    fn grid_row_end(&self) -> GridLine {
        self.grid_row.end.clone()
    }

    fn grid_column_start(&self) -> GridLine {
        self.grid_column.start.clone()
    }

    fn grid_column_end(&self) -> GridLine {
        self.grid_column.end.clone()
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

impl FlexContainerStyle for TestStyle {
    fn flex_direction(&self) -> flex_direction::T {
        flex_direction::T::Row
    }

    fn flex_wrap(&self) -> flex_wrap::T {
        flex_wrap::T::Nowrap
    }

    fn gap(&self) -> Size<NonNegativeLengthPercentageOrNormal> {
        self.gap.clone()
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
    fn flex_basis(&self) -> FlexBasis {
        self.flex_basis.clone()
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

/// Test-local node identity: a dense index into [`TestTree`]. Builders hand
/// these out during the mutation phase; layout and assertions resolve them
/// to borrowed [`TestRef`] handles.
type TestId = usize;

#[derive(Debug, Clone)]
struct TestSourceNode {
    display: TestDisplay,
    style: TestStyle,
    children: Vec<TestId>,
    min_content_size: Size<f32>,
    max_content_size: Size<f32>,
    first_baseline: Option<f32>,
}

/// Per-node mutable layout slots and instrumentation, written through
/// [`TestRef`] handles. Layout is single-threaded, so `Cell` interior
/// mutability is the whole synchronization story.
#[derive(Debug, Default)]
struct TestSessionNode {
    layout: Cell<Layout>,
    final_layout: Cell<Layout>,
    last_input: Cell<Option<LayoutInput>>,
    static_position: Cell<Option<Point<f32>>>,
    layout_writes: Cell<usize>,
    static_position_writes: Cell<usize>,
    measure_calls: Cell<usize>,
}

/// The one host tree: immutable node data plus interior-mutable session
/// slots and instrumentation. The session slots live in a parallel `Vec`
/// (not inline in `TestSourceNode`), keeping the immutable data as compact
/// as the pre-handle host's source storage.
#[derive(Debug, Default)]
struct TestTree {
    nodes: Vec<TestSourceNode>,
    session: Vec<TestSessionNode>,
    layout_writes: Cell<usize>,
    leaf_measure_calls: Cell<usize>,
}

impl TestTree {
    /// Resolves a builder-returned id to a borrowed node handle.
    fn node(&self, id: TestId) -> TestRef<'_> {
        TestRef {
            tree: self,
            index: id,
        }
    }

    fn push_leaf(
        &mut self,
        style: TestStyle,
        min_content_size: Size<f32>,
        max_content_size: Size<f32>,
    ) -> TestId {
        self.push(TestSourceNode {
            display: TestDisplay::Leaf,
            style,
            children: Vec::new(),
            min_content_size,
            max_content_size,
            first_baseline: None,
        })
    }

    fn push_grid(&mut self, style: TestStyle, children: Vec<TestId>) -> TestId {
        self.push(TestSourceNode {
            display: TestDisplay::Grid,
            style,
            children,
            min_content_size: Size::ZERO,
            max_content_size: Size::ZERO,
            first_baseline: None,
        })
    }

    fn push_flex(&mut self, style: TestStyle, children: Vec<TestId>) -> TestId {
        self.push(TestSourceNode {
            display: TestDisplay::Flex,
            style,
            children,
            min_content_size: Size::ZERO,
            max_content_size: Size::ZERO,
            first_baseline: None,
        })
    }

    fn push(&mut self, node: TestSourceNode) -> TestId {
        debug_assert_eq!(self.nodes.len(), self.session.len());
        let id = self.nodes.len();
        self.nodes.push(node);
        self.session.push(TestSessionNode::default());
        id
    }

    fn source_node_mut(&mut self, id: TestId) -> &mut TestSourceNode {
        &mut self.nodes[id]
    }

    /// The interior-mutable session slots of one node; tests mutate them
    /// through the `Cell` fields.
    fn session_node(&self, id: TestId) -> &TestSessionNode {
        &self.session[id]
    }

    fn layout(&self, id: TestId) -> Layout {
        self.session_node(id).layout.get()
    }

    /// Dispatches layout on `id` — the entry point tests use directly.
    fn compute_child_layout(&self, id: TestId, input: LayoutInput) -> LayoutOutput {
        self.node(id).compute_child_layout(input)
    }
}

/// The `Copy` node handle: a borrow of the tree plus a node index.
#[derive(Clone, Copy)]
struct TestRef<'t> {
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

struct TestChildren<'t> {
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
        self.slots().last_input.set(Some(input));
        let tree = self.tree;
        let node = self.source();
        let display = node.display;

        if node.style.display.is_none() {
            hide_subtree(self);
            return LayoutOutput::HIDDEN;
        }

        compute_cached_layout(self, input, |handle, input| match display {
            TestDisplay::Flex => compute_flexbox_layout(handle, input),
            TestDisplay::Grid => compute_grid_layout(handle, input),
            TestDisplay::Leaf => {
                let style = &node.style;
                let min_content_size = node.min_content_size;
                let max_content_size = node.max_content_size;
                let first_baseline = node.first_baseline;
                let slots = handle.slots();
                let mut measurer = FnLeafMeasurer::new(|measure_input| {
                    tree.leaf_measure_calls
                        .set(tree.leaf_measure_calls.get() + 1);
                    slots.measure_calls.set(slots.measure_calls.get() + 1);
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
                compute_leaf_layout(input, style, &mut measurer)
            }
        })
    }

    fn set_unrounded_layout(self, layout: &Layout) {
        self.tree
            .layout_writes
            .set(self.tree.layout_writes.get() + 1);
        let slots = self.slots();
        slots.layout_writes.set(slots.layout_writes.get() + 1);
        slots.layout.set(*layout);
    }

    fn unrounded_layout(self) -> Layout {
        self.slots().layout.get()
    }

    fn set_final_layout(self, layout: &Layout) {
        self.slots().final_layout.set(*layout);
    }

    fn set_static_position(self, static_position: Point<f32>) {
        let slots = self.slots();
        slots
            .static_position_writes
            .set(slots.static_position_writes.get() + 1);
        slots.static_position.set(Some(static_position));
    }

    // Caching deliberately disabled, matching the pre-handle grid host.
    fn cache_get(self, _input: LayoutInput) -> Option<LayoutOutput> {
        None
    }

    fn cache_store(self, _input: LayoutInput, _output: LayoutOutput) {}

    fn cache_clear(self) {}
}

fn lp(value: f32) -> LengthPercentage {
    LengthPercentage::new_length(Length::new(value))
}

fn lp_pct(fraction: f32) -> LengthPercentage {
    LengthPercentage::new_percent(Percentage(fraction))
}

fn nn_px(value: f32) -> NonNegativeLengthPercentage {
    NonNegative(lp(value))
}

fn gap_px(value: f32) -> NonNegativeLengthPercentageOrNormal {
    NonNegativeLengthPercentageOrNormal::LengthPercentage(NonNegative(lp(value)))
}

fn gap_pct(fraction: f32) -> NonNegativeLengthPercentageOrNormal {
    NonNegativeLengthPercentageOrNormal::LengthPercentage(NonNegative(lp_pct(fraction)))
}

fn size_px(value: f32) -> StyleSize {
    StyleSize::LengthPercentage(NonNegative(lp(value)))
}

fn size_pct(fraction: f32) -> StyleSize {
    StyleSize::LengthPercentage(NonNegative(lp_pct(fraction)))
}

fn max_px(value: f32) -> MaxSize {
    MaxSize::LengthPercentage(NonNegative(lp(value)))
}

fn margin_px(value: f32) -> Margin {
    Margin::LengthPercentage(lp(value))
}

fn inset_px(value: f32) -> Inset {
    Inset::LengthPercentage(lp(value))
}

fn border_px(value: i32) -> BorderSideWidth {
    BorderSideWidth(Au::from_px(value))
}

fn ratio(width: f32, height: f32) -> AspectRatio {
    AspectRatio {
        auto: false,
        ratio: PreferredRatio::Ratio(Ratio::new(width, height)),
    }
}

fn justify_items(flags: AlignFlags) -> ComputedJustifyItems {
    ComputedJustifyItems {
        specified: SpecifiedJustifyItems(ItemPlacement(flags)),
        computed: SpecifiedJustifyItems(ItemPlacement(flags)),
    }
}

fn fixed_breadth(value: f32) -> TrackBreadth {
    TrackBreadth::Breadth(lp(value))
}

fn px(value: f32) -> TrackSize {
    TrackSize::Breadth(fixed_breadth(value))
}

fn fr(value: f32) -> TrackSize {
    TrackSize::Breadth(TrackBreadth::Flex(Flex(value)))
}

fn percent(fraction: f32) -> TrackSize {
    TrackSize::Breadth(TrackBreadth::Breadth(lp_pct(fraction)))
}

fn auto_track() -> TrackSize {
    TrackSize::Breadth(TrackBreadth::Auto)
}

fn minmax(min: TrackBreadth, max: TrackBreadth) -> TrackSize {
    TrackSize::Minmax(min, max)
}

fn max_content_track() -> TrackSize {
    minmax(TrackBreadth::MaxContent, TrackBreadth::MaxContent)
}

fn fit_content_track(limit: f32) -> TrackSize {
    TrackSize::FitContent(fixed_breadth(limit))
}

fn repeat(
    count: RepeatCount<Integer>,
    sizes: Vec<TrackSize>,
) -> TrackListValue<LengthPercentage, Integer> {
    TrackListValue::TrackRepeat(TrackRepeat {
        count,
        // Real stylo track lists carry `values.len() + 1` line-name slots.
        line_names: vec![stylo::OwnedSlice::default(); sizes.len() + 1].into(),
        track_sizes: sizes.into(),
    })
}

fn track_list(values: Vec<TrackListValue<LengthPercentage, Integer>>) -> GridTemplateComponent {
    let auto_repeat_index = values
        .iter()
        .position(|value| {
            matches!(
                value,
                TrackListValue::TrackRepeat(repetition)
                    if matches!(repetition.count, RepeatCount::AutoFill | RepeatCount::AutoFit)
            )
        })
        .unwrap_or(usize::MAX);
    GridTemplateComponent::TrackList(Box::new(TrackList {
        auto_repeat_index,
        line_names: vec![stylo::OwnedSlice::default(); values.len() + 1].into(),
        values: values.into(),
    }))
}

fn tracks(sizes: &[TrackSize]) -> GridTemplateComponent {
    if sizes.is_empty() {
        return GridTemplateComponent::None;
    }
    track_list(
        sizes
            .iter()
            .cloned()
            .map(TrackListValue::TrackSize)
            .collect(),
    )
}

fn implicit(sizes: &[TrackSize]) -> ImplicitGridTracks {
    GenericImplicitGridTracks(sizes.to_vec().into())
}

fn grid_style(columns: &[TrackSize], rows: &[TrackSize]) -> TestStyle {
    TestStyle {
        template_columns: tracks(columns),
        template_rows: tracks(rows),
        ..TestStyle::default()
    }
}

fn fixed_leaf_style(width: f32, height: f32) -> TestStyle {
    TestStyle {
        size: Size::new(size_px(width), size_px(height)),
        ..TestStyle::default()
    }
}

fn fixed_leaf(tree: &mut TestTree, width: f32, height: f32) -> TestId {
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
) -> TestId {
    tree.push_leaf(TestStyle::default(), min_content_size, max_content_size)
}

fn line(number: i32) -> GridLine {
    let mut line = GridLine::auto();
    line.line_num = number;
    line
}

fn span(count: i32) -> GridLine {
    let mut line = GridLine::auto();
    line.is_span = true;
    line.line_num = count;
    line
}

fn placement(start: GridLine, end: GridLine) -> Line<GridLine> {
    Line::new(start, end)
}

fn definite_layout(tree: &TestTree, root: TestId, width: f32, height: f32) -> LayoutOutput {
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

fn intrinsic_layout(tree: &TestTree, root: TestId) -> LayoutOutput {
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
    style.gap = Size::new(gap_px(10.0), gap_px(5.0));
    style.justify_items = justify_items(AlignFlags::START);
    style.align_items = ItemPlacement(AlignFlags::START);
    let root = tree.push_grid(style, children.to_vec());

    let output = definite_layout(&tree, root, 210.0, 85.0);

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
    style.gap.width = gap_px(30.0);
    let root = tree.push_grid(style, vec![first, second]);

    definite_layout(&tree, root, 300.0, 20.0);

    assert_size(tree.layout(first).size, Size::new(90.0, 20.0));
    assert_point(tree.layout(second).location, Point::new(120.0, 0.0));
    assert_size(tree.layout(second).size, Size::new(180.0, 20.0));
}

#[test]
fn cyclic_percentage_track_resolves_after_intrinsic_container_sizing() {
    let mut tree = TestTree::default();
    let child_style = TestStyle {
        min_size: Size::new(size_px(0.0), size_px(0.0)),
        ..TestStyle::default()
    };
    let child = tree.push_leaf(child_style, Size::new(40.0, 10.0), Size::new(100.0, 10.0));
    let root = tree.push_grid(grid_style(&[percent(0.5)], &[px(20.0)]), vec![child]);

    let output = intrinsic_layout(&tree, root);

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
    style.gap.width = gap_pct(0.1);
    style.justify_items = justify_items(AlignFlags::START);
    style.align_items = ItemPlacement(AlignFlags::START);
    let root = tree.push_grid(style, vec![first, second]);

    let output = intrinsic_layout(&tree, root);

    // Percentage gaps contribute zero to the intrinsic width, then resolve
    // to 8px against the resulting 80px content box and may overflow it.
    assert_size(output.size, Size::new(80.0, 20.0));
    assert_point(tree.layout(second).location, Point::new(48.0, 0.0));
}

#[test]
fn minmax_and_fit_content_stop_at_their_growth_limits() {
    let mut minmax_tree = TestTree::default();
    let child = intrinsic_leaf(&mut minmax_tree, Size::ZERO, Size::ZERO);
    let bounded = minmax(fixed_breadth(40.0), fixed_breadth(80.0));
    let minmax_root = minmax_tree.push_grid(grid_style(&[bounded], &[px(20.0)]), vec![child]);

    definite_layout(&minmax_tree, minmax_root, 100.0, 20.0);
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
    marker_style.justify_self = SelfAlignment(AlignFlags::START);
    let marker = fit_tree.push_leaf(marker_style, Size::new(10.0, 10.0), Size::new(10.0, 10.0));
    let fit_root = fit_tree.push_grid(
        grid_style(&[fit_content_track(60.0), px(10.0)], &[px(20.0)]),
        vec![intrinsic, marker],
    );

    definite_layout(&fit_tree, fit_root, 100.0, 20.0);
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

    definite_layout(&tree, root, 200.0, 20.0);

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
        grid_column: placement(line(2), span(2)),
        grid_row: placement(line(1), span(2)),
        ..TestStyle::default()
    };
    let spanning = tree.push_leaf(spanning_style, Size::ZERO, Size::ZERO);

    let mut negative_style = fixed_leaf_style(10.0, 10.0);
    negative_style.grid_column = placement(line(-2), line(-1));
    negative_style.grid_row = placement(line(-2), line(-1));
    let negative = tree.push_leaf(negative_style, Size::new(10.0, 10.0), Size::new(10.0, 10.0));
    let mut style = grid_style(&[px(40.0), px(50.0), px(60.0)], &[px(30.0), px(40.0)]);
    style.gap = Size::new(gap_px(10.0), gap_px(5.0));
    let root = tree.push_grid(style, vec![spanning, negative]);

    definite_layout(&tree, root, 170.0, 75.0);

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

    definite_layout(&tree, root, 120.0, 20.0);

    assert_point(tree.layout(reversed).location, Point::new(0.0, 0.0));
    assert_size(tree.layout(reversed).size, Size::new(80.0, 20.0));
    assert_point(tree.layout(equal).location, Point::new(40.0, 0.0));
    assert_size(tree.layout(equal).size, Size::new(40.0, 20.0));
}

fn row_packing_layout(flow: GridAutoFlow) -> (Point<f32>, Point<f32>, Point<f32>) {
    let mut tree = TestTree::default();
    let mut wide = fixed_leaf_style(10.0, 10.0);
    wide.grid_column.end = span(2);
    let first = tree.push_leaf(wide.clone(), Size::ZERO, Size::new(10.0, 10.0));
    let second = tree.push_leaf(wide, Size::ZERO, Size::new(10.0, 10.0));
    let third = fixed_leaf(&mut tree, 10.0, 10.0);
    let mut style = grid_style(&[px(40.0), px(40.0), px(40.0)], &[px(30.0), px(30.0)]);
    style.auto_flow = flow;
    style.justify_items = justify_items(AlignFlags::START);
    style.align_items = ItemPlacement(AlignFlags::START);
    let root = tree.push_grid(style, vec![first, second, third]);
    definite_layout(&tree, root, 120.0, 60.0);
    (
        tree.layout(first).location,
        tree.layout(second).location,
        tree.layout(third).location,
    )
}

#[test]
fn row_dense_backfills_holes_that_sparse_flow_leaves_open() {
    let sparse = row_packing_layout(GridAutoFlow::ROW);
    let dense = row_packing_layout(GridAutoFlow::ROW | GridAutoFlow::DENSE);

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
    style.auto_flow = GridAutoFlow::COLUMN;
    style.justify_items = justify_items(AlignFlags::START);
    style.align_items = ItemPlacement(AlignFlags::START);
    let root = tree.push_grid(style, children.to_vec());

    definite_layout(&tree, root, 80.0, 60.0);

    assert_point(tree.layout(children[0]).location, Point::new(0.0, 0.0));
    assert_point(tree.layout(children[1]).location, Point::new(0.0, 30.0));
    assert_point(tree.layout(children[2]).location, Point::new(40.0, 0.0));
}

#[test]
fn column_dense_flow_backfills_a_hole_before_the_current_cursor() {
    let mut tree = TestTree::default();
    let mut tall = fixed_leaf_style(10.0, 10.0);
    tall.grid_row.end = span(2);
    let first = tree.push_leaf(tall.clone(), Size::ZERO, Size::new(10.0, 10.0));
    let second = tree.push_leaf(tall, Size::ZERO, Size::new(10.0, 10.0));
    let third = fixed_leaf(&mut tree, 10.0, 10.0);
    let mut style = grid_style(
        &[px(10.0), px(10.0), px(10.0)],
        &[px(10.0), px(10.0), px(10.0)],
    );
    style.auto_flow = GridAutoFlow::COLUMN | GridAutoFlow::DENSE;
    style.justify_items = justify_items(AlignFlags::START);
    style.align_items = ItemPlacement(AlignFlags::START);
    let root = tree.push_grid(style, vec![first, second, third]);

    definite_layout(&tree, root, 30.0, 30.0);

    assert_point(tree.layout(first).location, Point::new(0.0, 0.0));
    assert_point(tree.layout(second).location, Point::new(10.0, 0.0));
    assert_point(tree.layout(third).location, Point::new(0.0, 20.0));
}

#[test]
fn implicit_auto_tracks_cycle_after_the_explicit_grid() {
    let mut tree = TestTree::default();
    let mut children = Vec::new();
    for column in 2_i32..=4 {
        let mut child_style = fixed_leaf_style(5.0, 5.0);
        child_style.grid_column = placement(line(column), line(column + 1));
        child_style.grid_row = placement(line(1), line(2));
        children.push(tree.push_leaf(child_style, Size::new(5.0, 5.0), Size::new(5.0, 5.0)));
    }
    let mut style = grid_style(&[px(10.0)], &[px(20.0)]);
    style.auto_columns = implicit(&[px(30.0), px(50.0)]);
    style.justify_items = justify_items(AlignFlags::START);
    style.align_items = ItemPlacement(AlignFlags::START);
    let root = tree.push_grid(style, children.clone());

    definite_layout(&tree, root, 120.0, 20.0);

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
    style.auto_columns = implicit(&[px(20.0), px(30.0)]);
    style.justify_items = justify_items(AlignFlags::START);
    style.align_items = ItemPlacement(AlignFlags::START);
    let root = tree.push_grid(style, children.clone());

    definite_layout(&tree, root, 90.0, 20.0);

    // Leading tracks consume the auto-track pattern backwards: 30, 20, 30.
    assert_close(tree.layout(children[0]).location.x, 0.0);
    assert_close(tree.layout(children[1]).location.x, 30.0);
    assert_close(tree.layout(children[2]).location.x, 50.0);
    assert_close(tree.layout(children[3]).location.x, 80.0);
}

fn automatic_repeat_layout(count: RepeatCount<Integer>) -> (Layout, Layout) {
    let mut tree = TestTree::default();
    let first = intrinsic_leaf(&mut tree, Size::ZERO, Size::ZERO);
    let second = intrinsic_leaf(&mut tree, Size::ZERO, Size::ZERO);
    let repeated_track = minmax(fixed_breadth(40.0), TrackBreadth::Flex(Flex(1.0)));
    let mut style = grid_style(&[], &[px(20.0)]);
    style.template_columns = track_list(vec![repeat(count, vec![repeated_track])]);
    style.gap.width = gap_px(10.0);
    let root = tree.push_grid(style, vec![first, second]);
    definite_layout(&tree, root, 230.0, 20.0);
    (tree.layout(first), tree.layout(second))
}

#[test]
fn auto_fill_keeps_empty_tracks_while_auto_fit_collapses_them() {
    let fill = automatic_repeat_layout(RepeatCount::AutoFill);
    let fit = automatic_repeat_layout(RepeatCount::AutoFit);

    assert_size(fill.0.size, Size::new(50.0, 20.0));
    assert_point(fill.1.location, Point::new(60.0, 0.0));
    assert_size(fill.1.size, Size::new(50.0, 20.0));
    assert_size(fit.0.size, Size::new(110.0, 20.0));
    assert_point(fit.1.location, Point::new(120.0, 0.0));
    assert_size(fit.1.size, Size::new(110.0, 20.0));
}

#[test]
fn auto_fit_collapsed_track_gutters_coincide() {
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
    style.template_columns = track_list(vec![repeat(RepeatCount::AutoFit, vec![px(40.0)])]);
    style.gap.width = gap_px(10.0);
    style.justify_items = justify_items(AlignFlags::START);
    style.align_items = ItemPlacement(AlignFlags::START);
    let root = tree.push_grid(style, vec![first, third]);

    definite_layout(&tree, root, 190.0, 20.0);

    assert_close(tree.layout(first).location.x, 0.0);
    // Track 2 is empty and collapsed. Its two adjoining gutters coincide,
    // leaving exactly one 10px gutter between the surviving tracks.
    assert_close(tree.layout(third).location.x, 50.0);
}

#[test]
fn auto_fit_spanning_area_crosses_one_coincident_gutter() {
    let mut tree = TestTree::default();
    let mut first_style = fixed_leaf_style(10.0, 10.0);
    first_style.grid_column = placement(line(1), line(2));
    first_style.grid_row = placement(line(1), line(2));
    let first = tree.push_leaf(first_style, Size::new(10.0, 10.0), Size::new(10.0, 10.0));
    let mut third_style = fixed_leaf_style(10.0, 10.0);
    third_style.grid_column = placement(line(3), line(4));
    third_style.grid_row = placement(line(1), line(2));
    let third = tree.push_leaf(third_style, Size::new(10.0, 10.0), Size::new(10.0, 10.0));
    let spanning_style = TestStyle {
        position: PositionProperty::Absolute,
        inset: Edges::uniform(inset_px(0.0)),
        grid_column: placement(line(1), line(4)),
        grid_row: placement(line(1), line(2)),
        ..TestStyle::default()
    };
    let spanning = tree.push_leaf(spanning_style, Size::ZERO, Size::ZERO);
    let mut style = grid_style(&[], &[px(20.0)]);
    style.template_columns = track_list(vec![repeat(RepeatCount::AutoFit, vec![px(40.0)])]);
    style.gap.width = gap_px(10.0);
    style.justify_items = justify_items(AlignFlags::START);
    style.align_items = ItemPlacement(AlignFlags::START);
    let root = tree.push_grid(style, vec![first, third, spanning]);

    definite_layout(&tree, root, 190.0, 20.0);

    assert_close(tree.layout(third).location.x, 50.0);
    assert_point(tree.layout(spanning).location, Point::ZERO);
    assert_size(tree.layout(spanning).size, Size::new(90.0, 20.0));
}

#[test]
fn auto_fit_collapsed_gutters_overlap_distributed_alignment_space() {
    let mut tree = TestTree::default();
    let mut first_style = fixed_leaf_style(10.0, 10.0);
    first_style.grid_column = placement(line(1), line(2));
    let first = tree.push_leaf(first_style, Size::new(10.0, 10.0), Size::new(10.0, 10.0));
    let mut third_style = fixed_leaf_style(10.0, 10.0);
    third_style.grid_column = placement(line(3), line(4));
    let third = tree.push_leaf(third_style, Size::new(10.0, 10.0), Size::new(10.0, 10.0));
    let mut style = grid_style(&[], &[px(20.0)]);
    style.template_columns = track_list(vec![repeat(RepeatCount::AutoFit, vec![px(40.0)])]);
    style.gap.width = gap_px(10.0);
    style.justify_content = ContentDistribution::new(AlignFlags::SPACE_BETWEEN);
    style.justify_items = justify_items(AlignFlags::START);
    style.align_items = ItemPlacement(AlignFlags::START);
    let root = tree.push_grid(style, vec![first, third]);

    definite_layout(&tree, root, 190.0, 20.0);

    // The ordinary 10px gutter and all 100px distributed alignment space
    // overlap across the collapsed track instead of disappearing.
    assert_close(tree.layout(first).location.x, 0.0);
    assert_close(tree.layout(third).location.x, 150.0);
}

#[test]
fn max_content_track_uses_the_largest_single_track_contribution() {
    let mut tree = TestTree::default();
    let intrinsic_style = TestStyle {
        grid_column: placement(line(1), line(2)),
        justify_self: SelfAlignment(AlignFlags::START),
        align_self: SelfAlignment(AlignFlags::START),
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

    definite_layout(&tree, root, 90.0, 20.0);

    assert_close(tree.layout(marker).location.x, 70.0);
    assert_size(tree.layout(intrinsic).size, Size::new(70.0, 10.0));
    assert!((2..=6).contains(&tree.session_node(intrinsic).measure_calls.get()));
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
    marker_style.position = PositionProperty::Absolute;
    marker_style.inset.left = inset_px(0.0);
    marker_style.inset.top = inset_px(0.0);
    marker_style.grid_column = placement(line(2), line(3));
    let marker = tree.push_leaf(marker_style, Size::ZERO, Size::ZERO);
    let intrinsic_max = minmax(fixed_breadth(0.0), TrackBreadth::MaxContent);
    let root = tree.push_grid(
        grid_style(&[intrinsic_max, px(0.0)], &[px(10.0)]),
        vec![first, second, marker],
    );

    definite_layout(&tree, root, 100.0, 10.0);

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
    marker_style.position = PositionProperty::Absolute;
    marker_style.inset.left = inset_px(0.0);
    marker_style.inset.top = inset_px(0.0);
    marker_style.grid_column = placement(line(2), line(3));
    marker_style.justify_self = SelfAlignment(AlignFlags::START);
    marker_style.align_self = SelfAlignment(AlignFlags::START);
    let marker = tree.push_leaf(marker_style, Size::ZERO, Size::ZERO);
    let root = tree.push_grid(
        grid_style(&[max_content_track(), max_content_track()], &[px(20.0)]),
        vec![spanning, marker],
    );

    definite_layout(&tree, root, 100.0, 20.0);

    assert_size(tree.layout(spanning).size, Size::new(100.0, 20.0));
    assert_close(tree.layout(marker).location.x, 50.0);
    assert!((2..=8).contains(&tree.session_node(spanning).measure_calls.get()));
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
    style.justify_content = ContentDistribution::new(AlignFlags::SPACE_BETWEEN);
    style.align_content = ContentDistribution::new(AlignFlags::CENTER);
    style.justify_items = justify_items(AlignFlags::START);
    style.align_items = ItemPlacement(AlignFlags::START);
    let root = tree.push_grid(style, children.to_vec());

    definite_layout(&tree, root, 200.0, 100.0);

    assert_point(tree.layout(children[0]).location, Point::new(0.0, 30.0));
    assert_point(tree.layout(children[1]).location, Point::new(160.0, 30.0));
    assert_point(tree.layout(children[2]).location, Point::new(0.0, 50.0));
    assert_point(tree.layout(children[3]).location, Point::new(160.0, 50.0));
}

#[test]
fn self_alignment_positions_a_fixed_item_inside_its_area() {
    let mut tree = TestTree::default();
    let mut child_style = fixed_leaf_style(20.0, 10.0);
    child_style.justify_self = SelfAlignment(AlignFlags::END);
    child_style.align_self = SelfAlignment(AlignFlags::CENTER);
    let child = tree.push_leaf(child_style, Size::new(20.0, 10.0), Size::new(20.0, 10.0));
    let root = tree.push_grid(grid_style(&[px(100.0)], &[px(80.0)]), vec![child]);

    definite_layout(&tree, root, 100.0, 80.0);

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
    style.align_items = ItemPlacement(AlignFlags::BASELINE);
    style.justify_items = justify_items(AlignFlags::START);
    let root = tree.push_grid(style, vec![first, second]);

    let output = definite_layout(&tree, root, 100.0, 40.0);

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
    second_style.margin.top = Margin::Auto;
    let second = tree.push_leaf(second_style, Size::new(20.0, 10.0), Size::new(20.0, 10.0));
    tree.source_node_mut(second).first_baseline = Some(5.0);

    let mut style = grid_style(&[px(50.0), px(50.0)], &[px(40.0)]);
    style.align_items = ItemPlacement(AlignFlags::BASELINE);
    style.justify_items = justify_items(AlignFlags::START);
    let root = tree.push_grid(style, vec![first, second]);

    definite_layout(&tree, root, 100.0, 40.0);

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
    style.align_items = ItemPlacement(AlignFlags::START);
    style.justify_items = justify_items(AlignFlags::START);
    let root = tree.push_grid(style, vec![first, second]);

    let output = definite_layout(&tree, root, 20.0, 40.0);

    assert_eq!(output.first_baselines.y, Some(10.0));
    assert_close(tree.layout(second).location.y, 20.0);
}

#[test]
fn container_baseline_uses_grid_order_within_the_first_nonempty_row() {
    let mut tree = TestTree::default();
    let mut second_column_style = fixed_leaf_style(8.0, 10.0);
    second_column_style.grid_column = placement(line(2), line(3));
    second_column_style.grid_row = placement(line(1), line(2));
    let second_column = tree.push_leaf(
        second_column_style,
        Size::new(8.0, 10.0),
        Size::new(8.0, 10.0),
    );
    tree.source_node_mut(second_column).first_baseline = Some(12.0);

    let mut first_column_style = fixed_leaf_style(8.0, 10.0);
    first_column_style.grid_column = placement(line(1), line(2));
    first_column_style.grid_row = placement(line(1), line(2));
    let first_column = tree.push_leaf(
        first_column_style,
        Size::new(8.0, 10.0),
        Size::new(8.0, 10.0),
    );
    tree.source_node_mut(first_column).first_baseline = Some(5.0);

    let mut style = grid_style(&[px(20.0), px(20.0)], &[px(20.0)]);
    style.align_items = ItemPlacement(AlignFlags::START);
    style.justify_items = justify_items(AlignFlags::START);
    let root = tree.push_grid(style, vec![second_column, first_column]);

    let output = definite_layout(&tree, root, 40.0, 20.0);

    assert_eq!(output.first_baselines.y, Some(5.0));
    assert_point(tree.layout(first_column).location, Point::new(0.0, 0.0));
    assert_point(tree.layout(second_column).location, Point::new(20.0, 0.0));
}

#[test]
fn auto_sized_items_stretch_and_auto_margins_win_over_self_alignment() {
    let mut tree = TestTree::default();
    let stretch = intrinsic_leaf(&mut tree, Size::new(20.0, 10.0), Size::new(20.0, 10.0));
    let mut centered_style = fixed_leaf_style(20.0, 10.0);
    centered_style.grid_row = placement(line(2), line(3));
    centered_style.margin = Edges::uniform(Margin::Auto);
    centered_style.justify_self = SelfAlignment(AlignFlags::END);
    centered_style.align_self = SelfAlignment(AlignFlags::END);
    let centered = tree.push_leaf(centered_style, Size::new(20.0, 10.0), Size::new(20.0, 10.0));
    let root = tree.push_grid(
        grid_style(&[px(100.0)], &[px(40.0), px(40.0)]),
        vec![stretch, centered],
    );

    definite_layout(&tree, root, 100.0, 80.0);

    assert_size(tree.layout(stretch).size, Size::new(100.0, 40.0));
    assert_point(tree.layout(centered).location, Point::new(40.0, 55.0));
    assert_close(tree.layout(centered).margin.left, 40.0);
    assert_close(tree.layout(centered).margin.right, 40.0);
    assert_close(tree.layout(centered).margin.top, 15.0);
    assert_close(tree.layout(centered).margin.bottom, 15.0);
}

#[test]
fn a_single_inline_start_auto_margin_pushes_the_item_to_area_end() {
    let mut tree = TestTree::default();
    let mut child_style = fixed_leaf_style(20.0, 10.0);
    child_style.margin.left = Margin::Auto;
    child_style.justify_self = SelfAlignment(AlignFlags::START);
    let child = tree.push_leaf(child_style, Size::new(20.0, 10.0), Size::new(20.0, 10.0));
    let root = tree.push_grid(grid_style(&[px(100.0)], &[px(20.0)]), vec![child]);

    definite_layout(&tree, root, 100.0, 20.0);

    assert_point(tree.layout(child).location, Point::new(80.0, 0.0));
    assert_close(tree.layout(child).margin.left, 80.0);
    assert_close(tree.layout(child).margin.right, 0.0);
}

#[test]
fn overflowing_auto_margins_zero_out_then_self_alignment_applies() {
    let mut tree = TestTree::default();
    let mut child_style = fixed_leaf_style(80.0, 10.0);
    child_style.margin.left = Margin::Auto;
    child_style.margin.right = Margin::Auto;
    child_style.justify_self = SelfAlignment(AlignFlags::CENTER);
    child_style.align_self = SelfAlignment(AlignFlags::START);
    let child = tree.push_leaf(child_style, Size::new(80.0, 10.0), Size::new(80.0, 10.0));
    let root = tree.push_grid(grid_style(&[px(50.0)], &[px(20.0)]), vec![child]);

    definite_layout(&tree, root, 50.0, 20.0);

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
    style.direction = direction::T::Rtl;
    style.gap.width = gap_px(10.0);
    style.justify_items = justify_items(AlignFlags::START);
    style.align_items = ItemPlacement(AlignFlags::START);
    let root = tree.push_grid(style, children.to_vec());

    definite_layout(&tree, root, 140.0, 20.0);

    assert_close(tree.layout(children[0]).location.x, 130.0);
    assert_close(tree.layout(children[1]).location.x, 90.0);
    assert_close(tree.layout(children[2]).location.x, 40.0);
}

#[test]
fn rtl_container_uses_container_start_for_stretch_and_item_start_for_baseline() {
    let mut tree = TestTree::default();
    // The default justify-self is stretch, but a definite width prevents
    // stretching, so it falls back to the RTL container's right start edge.
    let default_stretch = fixed_leaf(&mut tree, 20.0, 10.0);
    let mut ltr_baseline_style = fixed_leaf_style(20.0, 10.0);
    ltr_baseline_style.grid_row = placement(line(2), line(3));
    ltr_baseline_style.justify_self = SelfAlignment(AlignFlags::BASELINE);
    let ltr_baseline = tree.push_leaf(
        ltr_baseline_style,
        Size::new(20.0, 10.0),
        Size::new(20.0, 10.0),
    );
    let mut rtl_baseline_style = fixed_leaf_style(20.0, 10.0);
    rtl_baseline_style.direction = direction::T::Rtl;
    rtl_baseline_style.grid_row = placement(line(3), line(4));
    rtl_baseline_style.justify_self = SelfAlignment(AlignFlags::BASELINE);
    let rtl_baseline = tree.push_leaf(
        rtl_baseline_style,
        Size::new(20.0, 10.0),
        Size::new(20.0, 10.0),
    );
    let mut style = grid_style(&[px(100.0)], &[px(20.0), px(20.0), px(20.0)]);
    style.direction = direction::T::Rtl;
    style.align_items = ItemPlacement(AlignFlags::START);
    let root = tree.push_grid(style, vec![default_stretch, ltr_baseline, rtl_baseline]);

    definite_layout(&tree, root, 100.0, 60.0);

    assert_point(tree.layout(default_stretch).location, Point::new(80.0, 0.0));
    // First-baseline falls back to safe self-start. The LTR item's start is
    // left even though its alignment container is RTL; the RTL item's is right.
    assert_point(tree.layout(ltr_baseline).location, Point::new(0.0, 20.0));
    assert_point(tree.layout(rtl_baseline).location, Point::new(80.0, 40.0));
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

    definite_layout(&tree, root, 120.0, 20.0);

    assert_close(tree.layout(earlier).location.x, 0.0);
    assert_close(tree.layout(first).location.x, 40.0);
    assert_close(tree.layout(third).location.x, 80.0);
    assert_eq!(tree.layout(earlier).order, 0);
    assert_eq!(tree.layout(first).order, 1);
    assert_eq!(tree.layout(third).order, 2);
}

#[test]
fn absolute_grid_children_use_order_zero_for_paint_order() {
    let mut tree = TestTree::default();
    let mut absolute_style = fixed_leaf_style(10.0, 10.0);
    absolute_style.position = PositionProperty::Absolute;
    absolute_style.order = 10;
    let absolute = tree.push_leaf(absolute_style, Size::ZERO, Size::new(10.0, 10.0));
    let in_flow = fixed_leaf(&mut tree, 10.0, 10.0);
    let root = tree.push_grid(
        grid_style(&[px(20.0)], &[px(10.0)]),
        vec![absolute, in_flow],
    );

    definite_layout(&tree, root, 20.0, 10.0);

    // An absolutely-positioned child is not a grid item, so its own
    // `order` value is ignored and it contributes the initial value zero to
    // the formatting parent's order-modified paint sequence.
    assert_eq!(tree.layout(absolute).order, 0);
    assert_eq!(tree.layout(in_flow).order, 1);
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
    tree.session_node(child).layout.set(sentinel);
    tree.session_node(root).layout.set(sentinel);

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
    assert_eq!(tree.layout_writes.get(), 0);
    assert_eq!(tree.layout(child), sentinel);
    assert_eq!(tree.layout(root), sentinel);
    assert!((1..=6).contains(&tree.session_node(child).measure_calls.get()));
}

#[test]
fn hidden_and_out_of_flow_children_do_not_occupy_grid_cells() {
    let mut tree = TestTree::default();
    let first = fixed_leaf(&mut tree, 10.0, 10.0);
    let mut hidden_style = fixed_leaf_style(1_000.0, 1_000.0);
    hidden_style.display = Display::None;
    let hidden = tree.push_leaf(hidden_style, Size::ZERO, Size::new(1_000.0, 1_000.0));
    let hidden_slots = tree.session_node(hidden);
    let mut hidden_sentinel = hidden_slots.layout.get();
    hidden_sentinel.size = Size::new(999.0, 999.0);
    hidden_slots.layout.set(hidden_sentinel);

    let mut absolute_style = fixed_leaf_style(20.0, 10.0);
    absolute_style.position = PositionProperty::Absolute;
    absolute_style.inset.left = inset_px(7.0);
    absolute_style.inset.top = inset_px(9.0);
    let absolute = tree.push_leaf(absolute_style, Size::new(20.0, 10.0), Size::new(20.0, 10.0));

    let mut hoisted_style = fixed_leaf_style(20.0, 10.0);
    hoisted_style.position = PositionProperty::Fixed;
    let hoisted = tree.push_leaf(hoisted_style, Size::new(20.0, 10.0), Size::new(20.0, 10.0));
    let second = fixed_leaf(&mut tree, 10.0, 10.0);
    let mut style = grid_style(&[px(50.0), px(50.0)], &[px(20.0)]);
    style.justify_items = justify_items(AlignFlags::START);
    style.align_items = ItemPlacement(AlignFlags::START);
    let root = tree.push_grid(style, vec![first, hidden, absolute, hoisted, second]);

    definite_layout(&tree, root, 100.0, 20.0);

    assert_close(tree.layout(first).location.x, 0.0);
    assert_close(tree.layout(second).location.x, 50.0);
    assert_eq!(tree.layout(hidden).size, Size::ZERO);
    assert_eq!(tree.session_node(hidden).measure_calls.get(), 0);
    assert_point(tree.layout(absolute).location, Point::new(7.0, 9.0));
    assert_eq!(tree.session_node(hoisted).layout_writes.get(), 0);
    assert_eq!(tree.session_node(hoisted).static_position_writes.get(), 1);
}

#[test]
fn direct_absolute_child_uses_its_definite_grid_area_as_containing_block() {
    let mut tree = TestTree::default();
    let child_style = TestStyle {
        position: PositionProperty::Absolute,
        inset: Edges {
            left: inset_px(5.0),
            right: inset_px(10.0),
            top: inset_px(2.0),
            bottom: inset_px(3.0),
        },
        grid_column: placement(line(2), line(3)),
        grid_row: placement(line(2), line(3)),
        ..TestStyle::default()
    };
    let child = tree.push_leaf(child_style, Size::ZERO, Size::ZERO);
    let mut style = grid_style(&[px(50.0), px(70.0)], &[px(30.0), px(40.0)]);
    style.gap = Size::new(gap_px(10.0), gap_px(5.0));
    let root = tree.push_grid(style, vec![child]);

    definite_layout(&tree, root, 130.0, 75.0);

    // The selected area starts at (60, 35) and is 70x40. Opposing insets
    // stretch the auto-sized absolute box within that area, not the grid root.
    assert_point(tree.layout(child).location, Point::new(65.0, 37.0));
    assert_size(tree.layout(child).size, Size::new(55.0, 35.0));
}

#[test]
fn rtl_grid_areas_keep_absolute_left_insets_physical() {
    let mut tree = TestTree::default();
    let mut child_style = fixed_leaf_style(5.0, 10.0);
    child_style.position = PositionProperty::Absolute;
    child_style.grid_column = placement(line(1), line(2));
    child_style.grid_row = placement(line(1), line(2));
    child_style.inset.left = inset_px(2.0);
    let child = tree.push_leaf(child_style, Size::new(5.0, 10.0), Size::new(5.0, 10.0));
    let mut style = grid_style(&[px(20.0), px(30.0)], &[px(10.0)]);
    style.direction = direction::T::Rtl;
    style.gap.width = gap_px(10.0);
    let root = tree.push_grid(style, vec![child]);

    definite_layout(&tree, root, 100.0, 10.0);

    // The first logical column occupies physical x=80..100. `left` remains
    // a physical inset into that area even though track order is RTL.
    assert_point(tree.layout(child).location, Point::new(82.0, 0.0));
    assert_size(tree.layout(child).size, Size::new(5.0, 10.0));
}

#[test]
fn absolute_auto_grid_lines_use_the_container_padding_edges() {
    let mut tree = TestTree::default();
    let child_style = TestStyle {
        position: PositionProperty::Absolute,
        inset: Edges::uniform(inset_px(0.0)),
        ..TestStyle::default()
    };
    let child = tree.push_leaf(child_style, Size::ZERO, Size::ZERO);
    let mut style = grid_style(&[], &[]);
    style.border = Edges::uniform(border_px(2));
    style.padding = Edges {
        left: nn_px(10.0),
        right: nn_px(20.0),
        top: nn_px(5.0),
        bottom: nn_px(15.0),
    };
    let root = tree.push_grid(style, vec![child]);

    definite_layout(&tree, root, 120.0, 80.0);

    // With both placement lines auto, Grid §10.1 uses the padding edges,
    // not the content edges, as the abspos containing block.
    assert_point(tree.layout(child).location, Point::new(2.0, 2.0));
    assert_size(tree.layout(child).size, Size::new(116.0, 76.0));
}

#[test]
fn absolute_static_fallback_uses_content_box_not_selected_grid_area() {
    let mut tree = TestTree::default();
    let mut child_style = fixed_leaf_style(20.0, 10.0);
    child_style.position = PositionProperty::Absolute;
    child_style.grid_column = placement(line(2), line(3));
    child_style.grid_row = placement(line(1), line(2));
    child_style.justify_self = SelfAlignment(AlignFlags::CENTER);
    child_style.align_self = SelfAlignment(AlignFlags::END);
    let child = tree.push_leaf(child_style, Size::new(20.0, 10.0), Size::new(20.0, 10.0));
    let root = tree.push_grid(grid_style(&[px(30.0), px(70.0)], &[px(50.0)]), vec![child]);

    definite_layout(&tree, root, 100.0, 50.0);

    // The selected area is x=30..100, but Grid §10.2 defines the static
    // position as if this were the sole item in the full content-edge area.
    assert_point(tree.layout(child).location, Point::new(40.0, 40.0));
}

#[test]
fn baseline_static_fallback_uses_self_start_and_safe_container_start() {
    let mut tree = TestTree::default();
    let mut ltr_style = fixed_leaf_style(120.0, 10.0);
    ltr_style.position = PositionProperty::Fixed;
    ltr_style.justify_self = SelfAlignment(AlignFlags::BASELINE);
    let ltr = tree.push_leaf(ltr_style, Size::new(120.0, 10.0), Size::new(120.0, 10.0));
    let mut rtl_style = fixed_leaf_style(20.0, 10.0);
    rtl_style.position = PositionProperty::Fixed;
    rtl_style.justify_self = SelfAlignment(AlignFlags::BASELINE);
    rtl_style.direction = direction::T::Rtl;
    let rtl = tree.push_leaf(rtl_style, Size::new(20.0, 10.0), Size::new(20.0, 10.0));
    let mut style = grid_style(&[], &[]);
    style.direction = direction::T::Rtl;
    let root = tree.push_grid(style, vec![ltr, rtl]);

    definite_layout(&tree, root, 100.0, 50.0);

    // Positive free space uses the RTL item's own inline start. The wider
    // LTR item overflows, so `safe self-start` instead falls back to the RTL
    // container's start edge and keeps the item's right edge at 100px.
    assert_eq!(
        tree.session_node(ltr).static_position.get(),
        Some(Point::new(-20.0, 0.0))
    );
    assert_eq!(
        tree.session_node(rtl).static_position.get(),
        Some(Point::new(80.0, 0.0))
    );
    let static_position = tree.session_node(rtl).static_position.get().unwrap();
    let positioned =
        compute_absolute_layout(tree.node(rtl), Size::new(100.0, 50.0), static_position);
    assert_point(positioned.location, Point::new(80.0, 0.0));
}

#[test]
fn hoisted_absolute_records_grid_aware_static_position_for_positioned_pass() {
    let mut tree = TestTree::default();
    let mut child_style = fixed_leaf_style(20.0, 10.0);
    child_style.position = PositionProperty::Fixed;
    child_style.justify_self = SelfAlignment(AlignFlags::CENTER);
    child_style.align_self = SelfAlignment(AlignFlags::END);
    child_style.margin.left = margin_px(5.0);
    child_style.margin.top = margin_px(3.0);
    let child = tree.push_leaf(child_style, Size::new(20.0, 10.0), Size::new(20.0, 10.0));
    let root = tree.push_grid(grid_style(&[], &[]), vec![child]);

    definite_layout(&tree, root, 100.0, 50.0);

    assert_eq!(tree.session_node(child).layout_writes.get(), 0);
    assert_eq!(
        tree.session_node(child).static_position.get(),
        Some(Point::new(37.5, 37.0))
    );

    let static_position = tree.session_node(child).static_position.get().unwrap();
    let positioned =
        compute_absolute_layout(tree.node(child), Size::new(100.0, 50.0), static_position);
    assert_point(positioned.location, Point::new(42.5, 40.0));
    assert_size(positioned.size, Size::new(20.0, 10.0));
}

#[test]
fn hoisted_static_position_ignores_placement_and_measures_auto_content() {
    let mut tree = TestTree::default();
    let child_style = TestStyle {
        position: PositionProperty::Fixed,
        grid_column: placement(line(2), line(3)),
        grid_row: placement(line(1), line(2)),
        justify_self: SelfAlignment(AlignFlags::CENTER),
        align_self: SelfAlignment(AlignFlags::END),
        ..TestStyle::default()
    };
    let child = tree.push_leaf(child_style, Size::new(20.0, 10.0), Size::new(20.0, 10.0));
    let root = tree.push_grid(grid_style(&[px(30.0), px(70.0)], &[px(50.0)]), vec![child]);

    definite_layout(&tree, root, 100.0, 50.0);

    assert!(tree.session_node(child).measure_calls.get() > 0);
    assert_eq!(tree.session_node(child).layout_writes.get(), 0);
    assert_eq!(
        tree.session_node(child).static_position.get(),
        Some(Point::new(40.0, 40.0))
    );
}

#[test]
fn nested_grid_uses_its_outer_area_for_fractional_tracks() {
    let mut tree = TestTree::default();
    let first = intrinsic_leaf(&mut tree, Size::ZERO, Size::ZERO);
    let second = intrinsic_leaf(&mut tree, Size::ZERO, Size::ZERO);
    let inner = tree.push_grid(
        grid_style(&[fr(1.0), fr(1.0)], &[px(20.0)]),
        vec![first, second],
    );
    let root = tree.push_grid(grid_style(&[px(120.0)], &[px(40.0)]), vec![inner]);

    definite_layout(&tree, root, 120.0, 40.0);

    assert_size(tree.layout(inner).size, Size::new(120.0, 40.0));
    assert_size(tree.layout(first).size, Size::new(60.0, 20.0));
    assert_point(tree.layout(second).location, Point::new(60.0, 0.0));
}

#[test]
fn a_flex_item_uses_its_grid_area_for_space_distribution() {
    let mut tree = TestTree::default();
    let first = fixed_leaf(&mut tree, 20.0, 10.0);
    let second = fixed_leaf(&mut tree, 20.0, 10.0);
    let inner = tree.push_flex(
        TestStyle {
            align_items: ItemPlacement(AlignFlags::START),
            justify_content: ContentDistribution::new(AlignFlags::SPACE_BETWEEN),
            ..TestStyle::default()
        },
        vec![first, second],
    );
    let root = tree.push_grid(grid_style(&[px(120.0)], &[px(40.0)]), vec![inner]);

    definite_layout(&tree, root, 120.0, 40.0);

    assert_size(tree.layout(inner).size, Size::new(120.0, 40.0));
    assert_point(tree.layout(first).location, Point::ZERO);
    assert_point(tree.layout(second).location, Point::new(100.0, 0.0));
}

#[test]
fn flex_known_but_indefinite_grid_size_does_not_seed_initial_auto_repeat() {
    let mut tree = TestTree::default();
    let first = fixed_leaf(&mut tree, 20.0, 10.0);
    let second = fixed_leaf(&mut tree, 20.0, 10.0);

    let mut grid = grid_style(&[], &[px(10.0)]);
    grid.template_columns = track_list(vec![repeat(RepeatCount::AutoFill, vec![percent(0.5)])]);
    grid.size.height = size_px(20.0);
    grid.flex_basis = FlexBasis::Size(size_pct(0.5));
    grid.flex_shrink = NonNegative(0.0);
    grid.align_items = ItemPlacement(AlignFlags::START);
    grid.justify_items = justify_items(AlignFlags::START);
    let inner = tree.push_grid(grid, vec![first, second]);

    let root = tree.push_flex(
        TestStyle {
            size: Size::new(StyleSize::Auto, size_px(20.0)),
            align_items: ItemPlacement(AlignFlags::START),
            ..TestStyle::default()
        },
        vec![inner],
    );

    let output = intrinsic_layout(&tree, root);

    let grid_input = tree.session_node(inner).last_input.get().unwrap();
    assert_close(grid_input.known_dimensions.width.unwrap(), 20.0);
    assert!(!grid_input.definite_dimensions.width);
    assert_close(output.size.width, 20.0);
    assert_size(tree.layout(inner).size, Size::new(20.0, 20.0));
    // The unresolved percentage flex basis gives Grid numeric used geometry,
    // but not an initial percentage basis. auto-fill therefore materializes
    // once; its 50% track resolves during the bounded final sizing rerun and
    // the second item auto-places into an implicit row rather than column 2.
    assert_point(tree.layout(first).location, Point::ZERO);
    assert_point(tree.layout(second).location, Point::new(0.0, 10.0));
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

    definite_layout(&tree, root, 240.0, 20.0);

    assert!(tree.leaf_measure_calls.get() >= ITEM_COUNT);
    assert!(tree.leaf_measure_calls.get() <= ITEM_COUNT * MAX_PROBES_PER_ITEM);
    for child in children {
        assert!((1..=MAX_PROBES_PER_ITEM).contains(&tree.session_node(child).measure_calls.get()));
    }
}

fn min_content_layout(tree: &TestTree, root: TestId) -> LayoutOutput {
    tree.compute_child_layout(
        root,
        LayoutInput::perform_layout(Size::NONE, Size::NONE, Size::MIN_CONTENT),
    )
}

#[test]
fn min_content_constraint_uses_zero_flex_fraction() {
    let mut tree = TestTree::default();
    let item = intrinsic_leaf(&mut tree, Size::new(20.0, 10.0), Size::new(80.0, 10.0));
    let root = tree.push_grid(grid_style(&[fr(1.0)], &[px(10.0)]), vec![item]);

    let output = min_content_layout(&tree, root);

    // Grid §12.7 fixes the flex fraction at zero under a min-content
    // constraint. The track keeps its 20px intrinsic base instead of taking
    // the indefinite/max-content branch and growing to 80px.
    assert_size(output.size, Size::new(20.0, 10.0));
    assert_size(tree.layout(item).size, Size::new(20.0, 10.0));
}

#[test]
fn automatic_minimum_is_clamped_by_a_fixed_max_track() {
    let mut tree = TestTree::default();
    let item = intrinsic_leaf(&mut tree, Size::new(200.0, 10.0), Size::new(200.0, 10.0));
    let bounded = minmax(TrackBreadth::Auto, fixed_breadth(50.0));
    let root = tree.push_grid(grid_style(&[bounded], &[px(10.0)]), vec![item]);

    let output = min_content_layout(&tree, root);

    // Grid §6.6 clamps the content-based automatic minimum to the grid
    // area's fixed maximum even though the item's min-content width is 200px.
    assert_size(output.size, Size::new(50.0, 10.0));
    assert_size(tree.layout(item).size, Size::new(50.0, 10.0));
}

#[test]
fn spanning_fixed_maximum_limit_includes_the_interior_gap() {
    let mut tree = TestTree::default();
    let item_style = TestStyle {
        grid_column: placement(line(1), line(3)),
        grid_row: placement(line(1), line(2)),
        ..TestStyle::default()
    };
    let item = tree.push_leaf(item_style, Size::new(200.0, 10.0), Size::new(200.0, 10.0));
    let bounded = minmax(TrackBreadth::Auto, fixed_breadth(50.0));
    let mut style = grid_style(&[bounded.clone(), bounded], &[px(10.0)]);
    style.gap.width = gap_px(10.0);
    let root = tree.push_grid(style, vec![item]);

    let output = min_content_layout(&tree, root);

    // Both §6.6's automatic minimum and §12.5's limited min-content
    // contribution use 50px + 10px gutter + 50px as the maximum area.
    assert_size(output.size, Size::new(110.0, 10.0));
    assert_size(tree.layout(item).size, Size::new(110.0, 10.0));
}

#[test]
fn max_content_spanning_contribution_is_limited_by_fixed_max_tracks() {
    let mut tree = TestTree::default();
    let item_style = TestStyle {
        grid_column: placement(line(1), line(3)),
        grid_row: placement(line(1), line(2)),
        // Disable the automatic minimum so this isolates §12.5's limited
        // max-content contribution rather than §6.6's automatic-min clamp.
        min_size: Size::new(size_px(0.0), size_px(0.0)),
        ..TestStyle::default()
    };
    let item = tree.push_leaf(item_style, Size::new(200.0, 10.0), Size::new(300.0, 10.0));
    let bounded = minmax(TrackBreadth::Auto, fixed_breadth(50.0));
    let mut style = grid_style(&[bounded.clone(), bounded], &[px(10.0)]);
    style.gap.width = gap_px(10.0);
    let root = tree.push_grid(style, vec![item]);

    let output = intrinsic_layout(&tree, root);

    // `intrinsic_layout` supplies a max-content constraint. The 300px
    // contribution is capped at 50px + 10px gutter + 50px.
    assert_size(output.size, Size::new(110.0, 10.0));
    assert_size(tree.layout(item).size, Size::new(110.0, 10.0));
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
        grid_style(&[auto_track(), auto_track()], &[px(10.0)]),
        vec![item],
    );

    let output = min_content_layout(&tree, root);

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
    spanning_style.justify_self = SelfAlignment(AlignFlags::START);
    let spanning = tree.push_leaf(
        spanning_style,
        Size::new(200.0, 10.0),
        Size::new(200.0, 10.0),
    );
    let mut marker_style = fixed_leaf_style(0.0, 1.0);
    marker_style.grid_column = placement(line(2), line(3));
    marker_style.grid_row = placement(line(1), line(2));
    marker_style.justify_self = SelfAlignment(AlignFlags::START);
    marker_style.align_self = SelfAlignment(AlignFlags::START);
    let marker = tree.push_leaf(marker_style, Size::ZERO, Size::ZERO);
    let root = tree.push_grid(
        grid_style(&[fr(1.0), fr(1.0)], &[px(10.0)]),
        vec![spanning, marker],
    );

    definite_layout(&tree, root, 100.0, 10.0);

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
    marker_style.align_self = SelfAlignment(AlignFlags::START);
    let marker = tree.push_leaf(marker_style, Size::new(10.0, 10.0), Size::new(10.0, 10.0));

    let mut style = grid_style(&[px(50.0), px(50.0)], &[auto_track(), px(10.0)]);
    style.align_items = ItemPlacement(AlignFlags::BASELINE);
    style.align_content = ContentDistribution::new(AlignFlags::START);
    let root = tree.push_grid(style, vec![first, second, marker]);

    definite_layout(&tree, root, 100.0, 40.0);

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
    style.min_size.width = size_px(250.0);
    style.template_columns = track_list(vec![repeat(RepeatCount::AutoFill, vec![px(100.0)])]);
    style.justify_items = justify_items(AlignFlags::START);
    style.align_items = ItemPlacement(AlignFlags::START);
    let root = tree.push_grid(style, children.to_vec());

    let output = intrinsic_layout(&tree, root);

    // Grid §7.2.3.2 uses ceil-like behavior for a definite minimum: three
    // 100px repetitions are needed to fulfil 250px.
    assert_close(output.size.width, 300.0);
    assert_point(tree.layout(children[2]).location, Point::new(200.0, 0.0));
}

#[test]
fn overflowing_positional_content_alignment_preserves_negative_free_space() {
    let mut tree = TestTree::default();
    let mut first_style = fixed_leaf_style(10.0, 10.0);
    first_style.justify_self = SelfAlignment(AlignFlags::START);
    let first = tree.push_leaf(first_style, Size::new(10.0, 10.0), Size::new(10.0, 10.0));
    let mut second_style = fixed_leaf_style(10.0, 10.0);
    second_style.justify_self = SelfAlignment(AlignFlags::START);
    let second = tree.push_leaf(second_style, Size::new(10.0, 10.0), Size::new(10.0, 10.0));
    let mut style = grid_style(&[px(80.0), px(80.0)], &[px(20.0)]);
    style.justify_content = ContentDistribution::new(AlignFlags::CENTER);
    let root = tree.push_grid(style, vec![first, second]);

    definite_layout(&tree, root, 100.0, 20.0);

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
    child_style.justify_self = SelfAlignment(AlignFlags::START);
    let child = tree.push_leaf(child_style, Size::new(200.0, 10.0), Size::new(200.0, 10.0));
    let mut marker_style = fixed_leaf_style(0.0, 1.0);
    marker_style.grid_column = placement(line(2), line(3));
    marker_style.grid_row = placement(line(1), line(2));
    marker_style.justify_self = SelfAlignment(AlignFlags::START);
    marker_style.align_self = SelfAlignment(AlignFlags::START);
    let marker = tree.push_leaf(marker_style, Size::ZERO, Size::ZERO);
    let root = tree.push_grid(
        grid_style(&[auto_track(), px(0.0)], &[px(10.0)]),
        vec![child, marker],
    );

    let output = min_content_layout(&tree, root);

    // The 200px contents overflow the specified 50px box; they do not make
    // that box's min/max-content contribution 200px.
    assert_close(output.size.width, 50.0);
    assert_close(tree.layout(marker).location.x, 50.0);
}

#[test]
fn auto_repeat_clamps_its_counting_basis_with_minimum_precedence() {
    let mut tree = TestTree::default();
    let mut marker_style = fixed_leaf_style(1.0, 1.0);
    marker_style.position = PositionProperty::Absolute;
    marker_style.inset.left = inset_px(0.0);
    marker_style.inset.top = inset_px(0.0);
    marker_style.grid_column = placement(line(4), line(5));
    marker_style.grid_row = placement(line(1), line(2));
    marker_style.justify_self = SelfAlignment(AlignFlags::START);
    marker_style.align_self = SelfAlignment(AlignFlags::START);
    let marker = tree.push_leaf(marker_style, Size::new(1.0, 1.0), Size::new(1.0, 1.0));

    let mut style = grid_style(&[], &[px(10.0)]);
    style.min_size.width = size_px(200.0);
    style.max_size.width = max_px(100.0);
    style.template_columns = track_list(vec![repeat(RepeatCount::AutoFill, vec![px(50.0)])]);
    let root = tree.push_grid(style, vec![marker]);

    let output = intrinsic_layout(&tree, root);

    // CSS minimum sizes take precedence over conflicting maximum sizes. The
    // 200px used counting basis therefore creates four explicit tracks.
    assert_close(output.size.width, 200.0);
    assert_close(tree.layout(marker).location.x, 150.0);
}

#[test]
fn auto_repeat_resolves_percentage_gap_against_its_max_constraint() {
    let mut tree = TestTree::default();
    let mut marker_style = fixed_leaf_style(1.0, 1.0);
    marker_style.position = PositionProperty::Absolute;
    marker_style.inset.left = inset_px(0.0);
    marker_style.inset.top = inset_px(0.0);
    marker_style.grid_column = placement(line(4), GridLine::auto());
    marker_style.grid_row = placement(line(1), line(2));
    marker_style.justify_self = SelfAlignment(AlignFlags::START);
    marker_style.align_self = SelfAlignment(AlignFlags::START);
    let marker = tree.push_leaf(marker_style, Size::new(1.0, 1.0), Size::new(1.0, 1.0));

    let mut style = grid_style(&[], &[px(10.0)]);
    style.max_size.width = max_px(200.0);
    style.template_columns = track_list(vec![repeat(RepeatCount::AutoFill, vec![px(50.0)])]);
    style.gap.width = gap_pct(0.10);
    let root = tree.push_grid(style, vec![marker]);

    let output = intrinsic_layout(&tree, root);

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
        // `overflow: hidden` is the Lynx scroll-container value (the lynx
        // grammar has no `scroll`/`clip`; scrollbars are overlay-only).
        overflow: Point::new(Overflow::Hidden, Overflow::Visible),
        ..TestStyle::default()
    };
    let item = tree.push_leaf(item_style, Size::new(200.0, 10.0), Size::new(200.0, 10.0));
    let root = tree.push_grid(
        grid_style(&[auto_track(), auto_track()], &[px(10.0)]),
        vec![item],
    );

    let output = min_content_layout(&tree, root);

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
        min_size: Size::new(size_px(0.0), size_px(0.0)),
        justify_self: SelfAlignment(AlignFlags::START),
        ..TestStyle::default()
    };
    let first = tree.push_leaf(first_style, Size::new(10.0, 10.0), Size::new(10.0, 10.0));

    let spanning_style = TestStyle {
        grid_column: placement(line(1), line(3)),
        grid_row: placement(line(1), line(2)),
        min_size: Size::new(size_px(0.0), size_px(0.0)),
        justify_self: SelfAlignment(AlignFlags::START),
        ..TestStyle::default()
    };
    let spanning = tree.push_leaf(
        spanning_style,
        Size::new(30.0, 10.0),
        Size::new(100.0, 10.0),
    );

    let mut marker_style = fixed_leaf_style(0.0, 1.0);
    marker_style.position = PositionProperty::Absolute;
    marker_style.inset.left = inset_px(0.0);
    marker_style.inset.top = inset_px(0.0);
    marker_style.grid_column = placement(line(2), line(3));
    marker_style.grid_row = placement(line(1), line(2));
    marker_style.justify_self = SelfAlignment(AlignFlags::START);
    marker_style.align_self = SelfAlignment(AlignFlags::START);
    let marker = tree.push_leaf(marker_style, Size::ZERO, Size::ZERO);

    let intrinsic = minmax(TrackBreadth::MinContent, TrackBreadth::MaxContent);
    let root = tree.push_grid(
        grid_style(&[intrinsic.clone(), intrinsic], &[px(10.0)]),
        vec![first, spanning, marker],
    );

    let output = intrinsic_layout(&tree, root);

    // This is the example from Grid §12.5: track one stays at 10px and the
    // track whose infinite limit became finite grows to 90px.
    assert_close(output.size.width, 100.0);
    assert_close(tree.layout(marker).location.x, 10.0);
}

#[test]
fn single_track_intrinsic_base_floors_its_growth_limit_before_spanning_growth() {
    let mut tree = TestTree::default();
    let first_style = TestStyle {
        grid_column: placement(line(1), line(2)),
        grid_row: placement(line(1), line(2)),
        ..TestStyle::default()
    };
    let first = tree.push_leaf(first_style, Size::new(100.0, 10.0), Size::new(100.0, 10.0));
    let spanning_style = TestStyle {
        grid_column: placement(line(1), line(3)),
        grid_row: placement(line(1), line(2)),
        min_size: Size::new(size_px(0.0), size_px(0.0)),
        ..TestStyle::default()
    };
    let spanning = tree.push_leaf(spanning_style, Size::new(0.0, 10.0), Size::new(150.0, 10.0));
    let first_track = minmax(TrackBreadth::MinContent, fixed_breadth(50.0));
    let second_track = minmax(fixed_breadth(0.0), TrackBreadth::MaxContent);
    let root = tree.push_grid(
        grid_style(&[first_track, second_track], &[px(10.0)]),
        vec![first, spanning],
    );

    let output = intrinsic_layout(&tree, root);

    // Track one has a 100px intrinsic base despite its fixed 50px maximum.
    // Its growth limit must be floored to that base before the spanning
    // item's 150px max-content contribution gives the second track 50px.
    assert_size(output.size, Size::new(150.0, 10.0));
}

#[test]
fn spanning_base_uses_non_affected_track_before_exceeding_growth_limit() {
    let mut tree = TestTree::default();
    let spanning_style = TestStyle {
        grid_column: placement(line(1), line(3)),
        grid_row: placement(line(1), line(2)),
        justify_self: SelfAlignment(AlignFlags::START),
        ..TestStyle::default()
    };
    let spanning = tree.push_leaf(
        spanning_style,
        Size::new(100.0, 10.0),
        Size::new(100.0, 10.0),
    );

    let mut marker_style = fixed_leaf_style(0.0, 1.0);
    marker_style.position = PositionProperty::Absolute;
    marker_style.inset.left = inset_px(0.0);
    marker_style.inset.top = inset_px(0.0);
    marker_style.grid_column = placement(line(2), line(3));
    marker_style.grid_row = placement(line(1), line(2));
    marker_style.justify_self = SelfAlignment(AlignFlags::START);
    marker_style.align_self = SelfAlignment(AlignFlags::START);
    let marker = tree.push_leaf(marker_style, Size::ZERO, Size::ZERO);

    let first = minmax(TrackBreadth::Auto, fixed_breadth(10.0));
    let second = minmax(fixed_breadth(0.0), fixed_breadth(100.0));
    let root = tree.push_grid(
        grid_style(&[first, second], &[px(10.0)]),
        vec![spanning, marker],
    );

    definite_layout(&tree, root, 100.0, 10.0);

    // §12.5.1 first fills the affected auto-min track to 10px, then puts the
    // remaining 90px into the non-affected track before violating that cap.
    assert_close(tree.layout(marker).location.x, 10.0);
}

#[test]
fn normal_item_alignment_preserves_a_preferred_aspect_ratio() {
    let mut tree = TestTree::default();
    let child_style = TestStyle {
        aspect_ratio: ratio(2.0, 1.0),
        ..TestStyle::default()
    };
    let child = tree.push_leaf(child_style, Size::new(20.0, 10.0), Size::new(20.0, 10.0));
    let root = tree.push_grid(grid_style(&[px(100.0)], &[px(100.0)]), vec![child]);

    definite_layout(&tree, root, 100.0, 100.0);

    // Grid's `normal` alignment uses block sizing instead of stretching both
    // axes when an item has a preferred aspect ratio.
    assert_size(tree.layout(child).size, Size::new(100.0, 50.0));
}

#[test]
fn absolute_auto_line_uses_padding_edge_of_overflowing_scrollable_area() {
    let mut tree = TestTree::default();
    let child_style = TestStyle {
        position: PositionProperty::Absolute,
        inset: Edges {
            left: inset_px(0.0),
            right: inset_px(0.0),
            top: inset_px(0.0),
            bottom: inset_px(0.0),
        },
        grid_column: placement(line(1), GridLine::auto()),
        grid_row: placement(line(1), line(2)),
        ..TestStyle::default()
    };
    let child = tree.push_leaf(child_style, Size::ZERO, Size::ZERO);
    let root = tree.push_grid(grid_style(&[px(200.0)], &[px(20.0)]), vec![child]);

    definite_layout(&tree, root, 100.0, 20.0);

    // Grid §10.1 uses the padding edge of the scrollable area for an auto
    // line, so overflowing tracks extend this containing block to 200px.
    assert_size(tree.layout(child).size, Size::new(200.0, 20.0));
}

#[test]
fn cross_axis_rerun_uses_effective_content_alignment_gaps() {
    let mut tree = TestTree::default();
    let child_style = TestStyle {
        aspect_ratio: ratio(1.0, 1.0),
        grid_row: placement(line(1), line(3)),
        min_size: Size::new(size_px(0.0), size_px(0.0)),
        justify_self: SelfAlignment(AlignFlags::START),
        align_self: SelfAlignment(AlignFlags::STRETCH),
        ..TestStyle::default()
    };
    let child = tree.push_leaf(child_style, Size::ZERO, Size::ZERO);

    let mut style = grid_style(&[max_content_track()], &[px(20.0), px(20.0)]);
    style.align_content = ContentDistribution::new(AlignFlags::SPACE_BETWEEN);
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

mod sizing {
    use super::*;

    #[test]
    fn an_auto_row_uses_a_child_preferred_aspect_ratio() {
        let mut tree = support::TestTree::default();
        let child = tree.push_leaf(
            support::TestStyle {
                size: Size::new(size_px(80.0), StyleSize::Auto),
                aspect_ratio: ratio(2.0, 1.0),
                ..support::TestStyle::default()
            },
            Size::ZERO,
            None,
        );
        let root = tree.push_grid(
            support::TestStyle {
                size: Size::new(size_px(80.0), StyleSize::Auto),
                template_columns: tracks(&[px(80.0)]),
                ..support::TestStyle::default()
            },
            vec![child],
        );

        let output = support::perform_layout(
            &tree,
            root,
            Size::NONE,
            Size::new(AvailableSpace::MaxContent, AvailableSpace::MaxContent),
        );

        support::assert_size(output.size, Size::new(80.0, 40.0));
        support::assert_size(tree.layout(child).size, Size::new(80.0, 40.0));
        support::assert_point(tree.layout(child).location, Point::ZERO);
    }
}

mod visibility {
    use super::*;

    // `visibility: collapse` does not exist in the lynx grammar, so the
    // former collapsed fixture is re-expressed as a second hidden item: a
    // hidden grid item generates a box and must keep its auto-placement
    // cell and geometry (painting is the host's concern).
    #[test]
    fn hidden_grid_items_keep_their_auto_placement_cells() {
        let mut tree = support::TestTree::default();
        let mut hidden_style = support::fixed_leaf_style(50.0, 20.0);
        hidden_style.visibility = stylo::computed_values::visibility::T::Hidden;
        let hidden = tree.push_leaf(hidden_style, Size::new(50.0, 20.0), None);
        let mut second_hidden_style = support::fixed_leaf_style(50.0, 20.0);
        second_hidden_style.visibility = stylo::computed_values::visibility::T::Hidden;
        let second_hidden = tree.push_leaf(second_hidden_style, Size::new(50.0, 20.0), None);
        let visible = support::fixed_leaf(&mut tree, 50.0, 20.0);
        let root = tree.push_grid(
            support::TestStyle {
                template_columns: tracks(&[px(50.0), px(50.0), px(50.0)]),
                template_rows: tracks(&[px(20.0)]),
                ..support::TestStyle::default()
            },
            vec![hidden, second_hidden, visible],
        );

        support::definite_layout(&tree, root, 150.0, 20.0);

        for (name, node, expected_x) in [
            ("hidden", hidden, 0.0),
            ("second hidden", second_hidden, 50.0),
            ("visible", visible, 100.0),
        ] {
            let layout = tree.layout(node);
            assert_eq!(layout.size, Size::new(50.0, 20.0), "{name} item size");
            assert_eq!(layout.location, Point::new(expected_x, 0.0), "{name} cell");
        }
    }
}

// ---------------------------------------------------------------------------
// Cross-size feedback, keyword sizing, and defensive-placement behavior.
// ---------------------------------------------------------------------------

/// Grid §12.1: an item whose inline contribution depends on its block size
/// (here through `aspect-ratio` + `align-self: stretch`) gets exactly one
/// columns→rows rerun once row sizes are known.
#[test]
fn cross_size_dependent_ratio_item_forces_one_column_feedback_rerun() {
    let mut tree = TestTree::default();
    let fixed = fixed_leaf(&mut tree, 40.0, 80.0);
    let square_style = TestStyle {
        aspect_ratio: ratio(1.0, 1.0),
        align_self: SelfAlignment(AlignFlags::STRETCH),
        ..TestStyle::default()
    };
    let square = tree.push_leaf(square_style, Size::new(10.0, 10.0), Size::new(10.0, 10.0));
    let style = grid_style(&[auto_track(), auto_track()], &[]);
    let root = tree.push_grid(style, vec![fixed, square]);

    let output = intrinsic_layout(&tree, root);

    // Before rows are sized the ratio item's min/max-content width is its
    // 10px content. The 80px row makes it 80px wide, so the second column
    // must be re-sized to 80 rather than staying at 10.
    assert_size(output.size, Size::new(120.0, 80.0));
    assert_point(tree.layout(square).location, Point::new(40.0, 0.0));
    assert_size(tree.layout(square).size, Size::new(80.0, 80.0));
    assert_size(tree.layout(fixed).size, Size::new(40.0, 80.0));
}

/// Grid §11.6: the container baseline comes from the first non-empty row.
/// A non-baseline item there synthesizes from its bottom border edge and
/// wins over a real baseline-sharing group in a later row.
#[test]
fn container_baseline_prefers_first_row_synthesis_over_later_baseline_group() {
    let mut tree = TestTree::default();
    let top = fixed_leaf(&mut tree, 20.0, 10.0);
    let mut bottom_style = fixed_leaf_style(20.0, 10.0);
    bottom_style.align_self = SelfAlignment(AlignFlags::BASELINE);
    bottom_style.grid_row = placement(line(2), line(3));
    let bottom = tree.push_leaf(bottom_style, Size::new(20.0, 10.0), Size::new(20.0, 10.0));
    tree.source_node_mut(bottom).first_baseline = Some(6.0);
    let mut style = grid_style(&[px(50.0)], &[px(30.0), px(30.0)]);
    style.align_items = ItemPlacement(AlignFlags::START);
    style.justify_items = justify_items(AlignFlags::START);
    let root = tree.push_grid(style, vec![top, bottom]);

    let output = definite_layout(&tree, root, 50.0, 60.0);

    // Row 1's only item exposes no baseline, so its bottom edge (y=10)
    // becomes the container baseline; row 2's group (y=30+6) is ignored.
    assert_eq!(output.first_baselines.y, Some(10.0));
    assert_close(tree.layout(bottom).location.y, 30.0);
}

/// A text-like leaf for keyword-sizing tests: min-content 30, max-content
/// 90, and it wraps into a definite available width like real text.
fn wrapping_leaf(input: neutron_star::compute::LeafMeasureInput) -> LeafMetrics {
    let width = input
        .known_dimensions
        .width
        .unwrap_or(match input.available_space.width {
            AvailableSpace::MinContent => 30.0,
            AvailableSpace::MaxContent => 90.0,
            AvailableSpace::Definite(limit) => limit.clamp(30.0, 90.0),
        });
    let height = input.known_dimensions.height.unwrap_or(10.0);
    LeafMetrics::new(Size::new(width, height))
}

/// The intrinsic sizing keywords resolve against content on grid items:
/// `min-content`/`max-content`/`fit-content()` size to their contributions,
/// while the bare keywords `fit-content`/`stretch` behave as `auto` and
/// stretch to the track.
#[test]
fn intrinsic_keyword_preferred_sizes_resolve_against_content() {
    let mut tree = support::TestTree::default();
    let widths = [
        (StyleSize::MinContent, 30.0),
        (StyleSize::MaxContent, 90.0),
        (StyleSize::FitContentFunction(NonNegative(lp(50.0))), 50.0),
        (StyleSize::FitContent, 100.0),
        (StyleSize::Stretch, 100.0),
    ];
    let mut items = Vec::new();
    for (width, _) in &widths {
        let style = support::TestStyle {
            size: Size::new(width.clone(), StyleSize::Auto),
            ..support::TestStyle::default()
        };
        items.push(tree.push_measured_leaf(style, wrapping_leaf));
    }
    let root = tree.push_grid(
        support::TestStyle {
            template_columns: support::track_list(vec![support::track_px(100.0)]),
            ..support::TestStyle::default()
        },
        items.clone(),
    );

    support::definite_layout(&tree, root, 100.0, 50.0);

    for (item, (width, expected)) in items.iter().zip(&widths) {
        assert_close(tree.layout(*item).size.width, *expected);
        let _ = width;
    }
}

/// Intrinsic keywords on `max-width` clamp a stretched grid item, while the
/// bare `fit-content`/`stretch` keywords behave as `none` on `max-width`
/// and as `auto` on `min-width`.
#[test]
fn intrinsic_keyword_minimum_and_maximum_sizes_clamp_grid_items() {
    let mut tree = support::TestTree::default();
    let cases: Vec<(support::TestStyle, f32)> = vec![
        // The default stretch fills the 100px track.
        (support::TestStyle::default(), 100.0),
        // max-width:min-content clamps the stretch down to 30.
        (
            support::TestStyle {
                max_size: Size::new(MaxSize::MinContent, MaxSize::none()),
                ..support::TestStyle::default()
            },
            30.0,
        ),
        // max-width:max-content clamps it to 90.
        (
            support::TestStyle {
                max_size: Size::new(MaxSize::MaxContent, MaxSize::none()),
                ..support::TestStyle::default()
            },
            90.0,
        ),
        // max-width:60px clamps between the contributions.
        (
            support::TestStyle {
                max_size: Size::new(max_px(60.0), MaxSize::none()),
                ..support::TestStyle::default()
            },
            60.0,
        ),
        // The bare keywords behave as `none`.
        (
            support::TestStyle {
                max_size: Size::new(MaxSize::FitContent, MaxSize::none()),
                ..support::TestStyle::default()
            },
            100.0,
        ),
        (
            support::TestStyle {
                max_size: Size::new(MaxSize::Stretch, MaxSize::none()),
                ..support::TestStyle::default()
            },
            100.0,
        ),
        // min-width bare keywords behave as `auto` (no forced floor).
        (
            support::TestStyle {
                min_size: Size::new(StyleSize::FitContent, StyleSize::Auto),
                ..support::TestStyle::default()
            },
            100.0,
        ),
        (
            support::TestStyle {
                min_size: Size::new(StyleSize::Stretch, StyleSize::Auto),
                ..support::TestStyle::default()
            },
            100.0,
        ),
    ];
    let mut items = Vec::new();
    for (style, _) in &cases {
        items.push(tree.push_measured_leaf(style.clone(), wrapping_leaf));
    }
    let root = tree.push_grid(
        support::TestStyle {
            template_columns: support::track_list(vec![support::track_px(100.0)]),
            ..support::TestStyle::default()
        },
        items.clone(),
    );

    support::definite_layout(&tree, root, 100.0, 100.0);

    for (item, (_, expected)) in items.iter().zip(&cases) {
        assert_close(tree.layout(*item).size.width, *expected);
    }
}

/// `justify-content: end` packs tracks against the end edge, and the
/// physical `left`/`right` keywords keep their physical meaning under both
/// directions for content and item alignment.
#[test]
fn physical_alignment_keywords_stay_physical_across_directions() {
    // (justify_content, justify_items, direction, expected item x)
    let track_x =
        |content_flags: AlignFlags, item_flags: AlignFlags, text_direction: direction::T| -> f32 {
            let mut tree = TestTree::default();
            let item = fixed_leaf(&mut tree, 30.0, 10.0);
            let mut style = grid_style(&[px(30.0)], &[px(10.0)]);
            style.justify_content = ContentDistribution::new(content_flags);
            style.justify_items = justify_items(item_flags);
            style.direction = text_direction;
            let root = tree.push_grid(style, vec![item]);
            definite_layout(&tree, root, 100.0, 10.0);
            tree.layout(item).location.x
        };

    // Content distribution: end packs the lone track at the far edge.
    assert_close(
        track_x(AlignFlags::END, AlignFlags::START, direction::T::Ltr),
        70.0,
    );
    // `left` puts the track at the physical left under both directions.
    assert_close(
        track_x(AlignFlags::LEFT, AlignFlags::START, direction::T::Ltr),
        0.0,
    );
    assert_close(
        track_x(AlignFlags::LEFT, AlignFlags::START, direction::T::Rtl),
        0.0,
    );
    // `right` puts the track at the physical right under both directions.
    assert_close(
        track_x(AlignFlags::RIGHT, AlignFlags::START, direction::T::Ltr),
        70.0,
    );
    assert_close(
        track_x(AlignFlags::RIGHT, AlignFlags::START, direction::T::Rtl),
        70.0,
    );

    // Item self-alignment inside a wide track: physical keywords again.
    let item_x = |item_flags: AlignFlags, text_direction: direction::T| -> f32 {
        let mut tree = TestTree::default();
        let item = fixed_leaf(&mut tree, 30.0, 10.0);
        let mut style = grid_style(&[px(100.0)], &[px(10.0)]);
        style.justify_items = justify_items(item_flags);
        style.direction = text_direction;
        let root = tree.push_grid(style, vec![item]);
        definite_layout(&tree, root, 100.0, 10.0);
        tree.layout(item).location.x
    };
    assert_close(item_x(AlignFlags::LEFT, direction::T::Ltr), 0.0);
    assert_close(item_x(AlignFlags::LEFT, direction::T::Rtl), 0.0);
    assert_close(item_x(AlignFlags::RIGHT, direction::T::Ltr), 70.0);
    assert_close(item_x(AlignFlags::RIGHT, direction::T::Rtl), 70.0);
}

/// Absolutely positioned grid children with intrinsic preferred widths
/// measure at those sizes for their static-position fallback, and the bare
/// keywords behave as `auto`.
#[test]
fn absolute_children_resolve_intrinsic_preferred_widths() {
    let mut tree = support::TestTree::default();
    let widths = [
        (StyleSize::MinContent, 30.0),
        (StyleSize::MaxContent, 90.0),
        (StyleSize::FitContentFunction(NonNegative(lp(60.0))), 60.0),
        // The treated-as-auto keywords measure against the containing block.
        (StyleSize::FitContent, 90.0),
        (StyleSize::Stretch, 90.0),
        (StyleSize::WebkitFillAvailable, 90.0),
    ];
    let mut items = Vec::new();
    for (width, _) in &widths {
        let style = support::TestStyle {
            position: PositionProperty::Absolute,
            size: Size::new(width.clone(), support::size_px(10.0)),
            ..support::TestStyle::default()
        };
        items.push(tree.push_measured_leaf(style, wrapping_leaf));
    }
    let root = tree.push_grid(
        support::TestStyle {
            template_columns: support::track_list(vec![support::track_px(100.0)]),
            template_rows: support::track_list(vec![support::track_px(40.0)]),
            ..support::TestStyle::default()
        },
        items.clone(),
    );

    support::definite_layout(&tree, root, 100.0, 40.0);

    for (item, (width, expected)) in items.iter().zip(&widths) {
        assert_close(tree.layout(*item).size.width, *expected);
        let _ = width;
    }
}

/// An absolutely positioned child with fully-auto insets and horizontal
/// auto margins centers in the containing block via the margin share.
#[test]
fn absolute_child_auto_margins_share_free_space_in_static_position() {
    let mut tree = TestTree::default();
    let mut child_style = fixed_leaf_style(40.0, 10.0);
    child_style.position = PositionProperty::Absolute;
    child_style.margin.left = Margin::Auto;
    child_style.margin.right = Margin::Auto;
    let child = tree.push_leaf(child_style, Size::new(40.0, 10.0), Size::new(40.0, 10.0));
    let root = tree.push_grid(grid_style(&[px(100.0)], &[px(50.0)]), vec![child]);

    definite_layout(&tree, root, 100.0, 50.0);

    assert_close(tree.layout(child).location.x, 30.0);
    assert_size(tree.layout(child).size, Size::new(40.0, 10.0));
}

/// Grid §8.3 defensive placements on absolutely positioned children: the
/// invalid line number 0 behaves as `auto`, and span/span placements leave
/// both edges attached to the padding box.
#[test]
fn absolute_defensive_placements_fall_back_to_padding_edges() {
    let mut tree = TestTree::default();
    // Line 0 on the start side: only the end line binds.
    let mut zero_start = fixed_leaf_style(10.0, 10.0);
    zero_start.position = PositionProperty::Absolute;
    zero_start.grid_column = placement(line(0), line(2));
    zero_start.inset.left = inset_px(0.0);
    let zero_start = tree.push_leaf(zero_start, Size::new(10.0, 10.0), Size::new(10.0, 10.0));

    // Line 0 on the end side of an otherwise-auto axis: both edges auto,
    // so opposing insets stretch across the whole padding box.
    let zero_end_style = TestStyle {
        position: PositionProperty::Absolute,
        inset: Edges::uniform(inset_px(0.0)),
        grid_column: placement(GridLine::auto(), line(0)),
        ..TestStyle::default()
    };
    let zero_end = tree.push_leaf(zero_end_style, Size::ZERO, Size::ZERO);

    // span/span is indefinite in both directions: full padding box.
    let span_span_style = TestStyle {
        position: PositionProperty::Absolute,
        inset: Edges::uniform(inset_px(0.0)),
        grid_column: placement(span(1), span(1)),
        ..TestStyle::default()
    };
    let span_span = tree.push_leaf(span_span_style, Size::ZERO, Size::ZERO);

    // span/line binds both edges through §8.3 conflict handling.
    let span_line_style = TestStyle {
        position: PositionProperty::Absolute,
        inset: Edges::uniform(inset_px(0.0)),
        grid_column: placement(span(1), line(2)),
        ..TestStyle::default()
    };
    let span_line = tree.push_leaf(span_line_style, Size::ZERO, Size::ZERO);

    let style = grid_style(&[px(60.0), px(40.0)], &[px(20.0)]);
    let root = tree.push_grid(style, vec![zero_start, zero_end, span_span, span_line]);

    definite_layout(&tree, root, 100.0, 20.0);

    // Start fell back to the padding edge; the end line (x=60) held.
    assert_close(tree.layout(zero_start).location.x, 0.0);
    // Both-auto axes stretch across the whole 100x20 padding box.
    assert_size(tree.layout(zero_end).size, Size::new(100.0, 20.0));
    assert_size(tree.layout(span_span).size, Size::new(100.0, 20.0));
    // The span start resolved against line 2: containing block is track 1.
    assert_close(tree.layout(span_line).location.x, 0.0);
    assert_size(tree.layout(span_line).size, Size::new(60.0, 20.0));
}

/// `position: static` grid items ignore their insets while `relative` items
/// are nudged; an end-side auto margin absorbs free space without moving
/// the item.
#[test]
fn static_items_ignore_insets_and_end_auto_margins_absorb_space() {
    let mut tree = TestTree::default();
    let mut static_style = fixed_leaf_style(20.0, 10.0);
    static_style.position = PositionProperty::Static;
    static_style.inset.left = inset_px(15.0);
    static_style.inset.top = inset_px(5.0);
    let static_item = tree.push_leaf(static_style, Size::new(20.0, 10.0), Size::new(20.0, 10.0));

    let mut relative_style = fixed_leaf_style(20.0, 10.0);
    relative_style.inset.left = inset_px(15.0);
    relative_style.inset.top = inset_px(5.0);
    let relative_item =
        tree.push_leaf(relative_style, Size::new(20.0, 10.0), Size::new(20.0, 10.0));

    let mut margin_style = fixed_leaf_style(20.0, 10.0);
    margin_style.margin.right = Margin::Auto;
    margin_style.margin.bottom = Margin::Auto;
    let margin_item = tree.push_leaf(margin_style, Size::new(20.0, 10.0), Size::new(20.0, 10.0));

    let mut style = grid_style(&[px(100.0)], &[px(20.0), px(20.0), px(30.0)]);
    style.justify_items = justify_items(AlignFlags::START);
    style.align_items = ItemPlacement(AlignFlags::START);
    let root = tree.push_grid(style, vec![static_item, relative_item, margin_item]);

    definite_layout(&tree, root, 100.0, 70.0);

    assert_point(tree.layout(static_item).location, Point::new(0.0, 0.0));
    assert_point(tree.layout(relative_item).location, Point::new(15.0, 25.0));
    // The auto end margins soak up the 80x20 free space; the item stays at
    // the start of its 100x30 area.
    assert_point(tree.layout(margin_item).location, Point::new(0.0, 40.0));
    let margin = tree.layout(margin_item).margin;
    assert_close(margin.right, 80.0);
    assert_close(margin.bottom, 20.0);
}

/// `repeat(auto-fill, ...)` derives its count from the definite side of
/// each repeated track and runs exactly once when no side is definite or
/// nothing fits.
#[test]
fn auto_fill_counts_use_definite_breadths_or_run_once() {
    // Returns the columns of the first three auto-placed 10x10 items.
    let columns_of = |template: TrackSize, inner: f32, gap: f32| -> Vec<f32> {
        let mut tree = TestTree::default();
        let items = [
            fixed_leaf(&mut tree, 10.0, 10.0),
            fixed_leaf(&mut tree, 10.0, 10.0),
            fixed_leaf(&mut tree, 10.0, 10.0),
        ];
        let mut style = grid_style(&[], &[]);
        style.template_columns = track_list(vec![repeat(RepeatCount::AutoFill, vec![template])]);
        if gap > 0.0 {
            style.gap.width = gap_px(gap);
        }
        style.justify_items = justify_items(AlignFlags::START);
        style.align_items = ItemPlacement(AlignFlags::START);
        let root = tree.push_grid(style, items.to_vec());
        definite_layout(&tree, root, inner, 40.0);
        items
            .iter()
            .map(|item| tree.layout(*item).location.x)
            .collect()
    };

    // minmax(auto, 40px): the 40px maximum counts. 100px fits two
    // repetitions once the 10px gutter is charged: item 3 wraps.
    let positions = columns_of(minmax(TrackBreadth::Auto, fixed_breadth(40.0)), 100.0, 10.0);
    assert_eq!(positions, vec![0.0, 50.0, 0.0]);

    // minmax(min-content, 40px): the fixed maximum still counts.
    let positions = columns_of(
        minmax(TrackBreadth::MinContent, fixed_breadth(40.0)),
        90.0,
        0.0,
    );
    assert_eq!(positions, vec![0.0, 40.0, 0.0]);

    // minmax(20px, min-content): only the 20px minimum is definite.
    let positions = columns_of(
        minmax(fixed_breadth(20.0), TrackBreadth::MinContent),
        50.0,
        0.0,
    );
    assert_eq!(positions, vec![0.0, 20.0, 0.0]);

    // minmax(30px, max-content): likewise counted by the 30px minimum.
    let positions = columns_of(
        minmax(fixed_breadth(30.0), TrackBreadth::MaxContent),
        70.0,
        0.0,
    );
    assert_eq!(positions, vec![0.0, 30.0, 0.0]);

    // minmax(max-content, 40px): the intrinsic minimum has no counting
    // breadth, so only the 40px maximum counts.
    let positions = columns_of(
        minmax(TrackBreadth::MaxContent, fixed_breadth(40.0)),
        90.0,
        0.0,
    );
    assert_eq!(positions, vec![0.0, 40.0, 0.0]);

    // A fully intrinsic track has no counting breadth: one repetition, so
    // every item lands at x=0 in its own row.
    let positions = columns_of(auto_track(), 300.0, 0.0);
    assert_eq!(positions, vec![0.0, 0.0, 0.0]);

    // A repetition wider than the container still occurs once.
    let positions = columns_of(px(200.0), 100.0, 0.0);
    assert_eq!(positions, vec![0.0, 0.0, 0.0]);

    // A zero-sized repetition is counted with the 1px anti-DoS floor, so
    // expansion terminates; every zero-width column starts at x=0.
    let positions = columns_of(px(0.0), 100.0, 0.0);
    assert_eq!(positions, vec![0.0, 0.0, 0.0]);
}

/// Template tracks size an itemless grid: fixed tracks contribute their
/// length and intrinsic tracks contribute nothing.
#[test]
fn empty_template_tracks_size_an_itemless_grid() {
    let mut tree = TestTree::default();
    let style = grid_style(&[auto_track(), px(50.0)], &[auto_track()]);
    let root = tree.push_grid(style, Vec::new());

    let output = intrinsic_layout(&tree, root);

    assert_size(output.size, Size::new(50.0, 0.0));
}

/// Two items with the same multi-track span form one distribution group;
/// the larger minimum drives both intrinsic columns.
#[test]
fn equal_span_groups_distribute_min_contributions_together() {
    let mut tree = TestTree::default();
    let narrow_style = TestStyle {
        grid_column: placement(line(1), line(3)),
        ..TestStyle::default()
    };
    let narrow = tree.push_leaf(narrow_style, Size::new(60.0, 10.0), Size::new(60.0, 10.0));
    let wide_style = TestStyle {
        grid_column: placement(line(1), line(3)),
        ..TestStyle::default()
    };
    let wide = tree.push_leaf(wide_style, Size::new(80.0, 10.0), Size::new(80.0, 10.0));
    let mut style = grid_style(&[auto_track(), auto_track()], &[]);
    style.justify_content = ContentDistribution::new(AlignFlags::START);
    let root = tree.push_grid(style, vec![narrow, wide]);

    definite_layout(&tree, root, 200.0, 20.0);

    assert_size(tree.layout(narrow).size, Size::new(80.0, 10.0));
    assert_size(tree.layout(wide).size, Size::new(80.0, 10.0));
}

/// Intrinsic min-/max-content measurement of the container sizes auto and
/// `minmax(min-content, 1fr)` columns from item contributions.
#[test]
fn container_intrinsic_measures_size_auto_and_flexible_tracks() {
    let measure = |track: TrackSize, available: AvailableSpace| -> f32 {
        let mut tree = TestTree::default();
        let item = intrinsic_leaf(&mut tree, Size::new(30.0, 10.0), Size::new(90.0, 10.0));
        let style = grid_style(&[track], &[]);
        let root = tree.push_grid(style, vec![item]);
        let output = tree.compute_child_layout(
            root,
            LayoutInput::compute_size(
                Size::NONE,
                Size::NONE,
                Size::new(available, available),
                RequestedAxis::Both,
            ),
        );
        output.size.width
    };

    assert_close(measure(auto_track(), AvailableSpace::MinContent), 30.0);
    assert_close(measure(auto_track(), AvailableSpace::MaxContent), 90.0);
    assert_close(
        measure(
            minmax(TrackBreadth::MinContent, TrackBreadth::Flex(Flex(1.0))),
            AvailableSpace::MinContent,
        ),
        30.0,
    );
    assert_close(
        measure(
            minmax(TrackBreadth::MinContent, TrackBreadth::Flex(Flex(1.0))),
            AvailableSpace::MaxContent,
        ),
        90.0,
    );
}

/// `minmax(20px, min-content)` and hostile fixed repetition counts: the
/// min-content maximum caps growth at the contribution, and huge repeat
/// counts clamp to the UA track limit instead of allocating unbounded
/// track lists.
#[test]
fn min_content_maximums_and_hostile_repeat_counts_stay_bounded() {
    let mut tree = TestTree::default();
    let item = intrinsic_leaf(&mut tree, Size::new(30.0, 10.0), Size::new(90.0, 10.0));
    let mut style = grid_style(
        &[minmax(fixed_breadth(20.0), TrackBreadth::MinContent)],
        &[],
    );
    style.justify_content = ContentDistribution::new(AlignFlags::START);
    let root = tree.push_grid(style, vec![item]);
    definite_layout(&tree, root, 200.0, 20.0);
    // Base 20 grows to the 30px min-content growth limit, no further.
    assert_close(tree.layout(item).size.width, 30.0);

    // 40,000 requested 1px tracks clamp to the 10,000-track UA limit.
    let mut tree = TestTree::default();
    let mut probe_style = fixed_leaf_style(1.0, 10.0);
    probe_style.grid_column = placement(line(-1), line(-2));
    let probe = tree.push_leaf(probe_style, Size::new(1.0, 10.0), Size::new(1.0, 10.0));
    let mut style = grid_style(&[], &[px(10.0)]);
    style.template_columns = track_list(vec![repeat(RepeatCount::Number(40_000), vec![px(1.0)])]);
    let root = tree.push_grid(style, vec![probe]);
    let output = intrinsic_layout(&tree, root);
    assert_size(output.size, Size::new(10_000.0, 10.0));
    // The probe sits in the final (10,000th) track.
    assert_close(tree.layout(probe).location.x, 9_999.0);
}

/// Intrinsic keyword heights on grid items request the matching vertical
/// measurement constraint in the final positioning pass.
#[test]
fn intrinsic_keyword_heights_resolve_against_content() {
    // A text-like leaf on the block axis: min-content 12, max-content 48,
    // and a definite available height is honored like a wrap constraint.
    fn column_leaf(input: neutron_star::compute::LeafMeasureInput) -> LeafMetrics {
        let height = input
            .known_dimensions
            .height
            .unwrap_or(match input.available_space.height {
                AvailableSpace::MinContent => 12.0,
                AvailableSpace::MaxContent => 48.0,
                AvailableSpace::Definite(limit) => limit.clamp(12.0, 48.0),
            });
        let width = input.known_dimensions.width.unwrap_or(40.0);
        LeafMetrics::new(Size::new(width, height))
    }

    let mut tree = support::TestTree::default();
    let heights = [
        (StyleSize::MinContent, 12.0),
        (StyleSize::MaxContent, 48.0),
        (StyleSize::FitContentFunction(NonNegative(lp(20.0))), 20.0),
    ];
    let mut items = Vec::new();
    for (height, _) in &heights {
        let style = support::TestStyle {
            size: Size::new(support::size_px(40.0), height.clone()),
            ..support::TestStyle::default()
        };
        items.push(tree.push_measured_leaf(style, column_leaf));
    }
    let root = tree.push_grid(
        support::TestStyle {
            template_columns: support::track_list(vec![support::track_px(50.0)]),
            ..support::TestStyle::default()
        },
        items.clone(),
    );

    support::definite_layout(&tree, root, 50.0, 200.0);

    for (item, (height, expected)) in items.iter().zip(&heights) {
        assert_close(tree.layout(*item).size.height, *expected);
        let _ = height;
    }
}

/// A grid container whose own preferred width is an intrinsic keyword sizes
/// its tracks under that constraint even inside definite available space.
#[test]
fn container_intrinsic_keyword_widths_override_available_space() {
    let width_of = |width: StyleSize| -> f32 {
        let mut tree = TestTree::default();
        let item = intrinsic_leaf(&mut tree, Size::new(30.0, 10.0), Size::new(90.0, 10.0));
        let mut style = grid_style(&[auto_track()], &[]);
        style.size = Size::new(width, StyleSize::Auto);
        let root = tree.push_grid(style, vec![item]);
        let output = tree.compute_child_layout(
            root,
            LayoutInput::perform_layout(
                Size::NONE,
                Size::new(Some(200.0), Some(50.0)),
                Size::new(
                    AvailableSpace::Definite(200.0),
                    AvailableSpace::Definite(50.0),
                ),
            ),
        );
        output.size.width
    };

    assert_close(width_of(StyleSize::MinContent), 30.0);
    assert_close(width_of(StyleSize::MaxContent), 90.0);

    // The block axis takes the same override.
    let height_of = |height: StyleSize| -> f32 {
        let mut tree = TestTree::default();
        let item = intrinsic_leaf(&mut tree, Size::new(30.0, 10.0), Size::new(90.0, 40.0));
        let mut style = grid_style(&[px(90.0)], &[auto_track()]);
        style.size = Size::new(StyleSize::Auto, height);
        let root = tree.push_grid(style, vec![item]);
        let output = tree.compute_child_layout(
            root,
            LayoutInput::perform_layout(
                Size::NONE,
                Size::new(Some(200.0), Some(200.0)),
                Size::new(
                    AvailableSpace::Definite(200.0),
                    AvailableSpace::Definite(200.0),
                ),
            ),
        );
        output.size.height
    };
    assert_close(height_of(StyleSize::MinContent), 10.0);
    assert_close(height_of(StyleSize::MaxContent), 40.0);
}

/// Auto-placement details: an `auto / <line>` placement binds the end edge,
/// fully definite items exit placement early under column flow, and
/// definite-column items steer the sparse and dense cursors.
#[test]
fn placement_binds_end_lines_and_flows_around_definite_items() {
    // auto / 3 occupies the track just before line 3.
    let mut tree = TestTree::default();
    let mut style = fixed_leaf_style(10.0, 10.0);
    style.grid_column = placement(GridLine::auto(), line(3));
    let item = tree.push_leaf(style, Size::new(10.0, 10.0), Size::new(10.0, 10.0));
    let mut grid = grid_style(&[px(20.0), px(20.0), px(20.0)], &[px(10.0)]);
    grid.justify_items = justify_items(AlignFlags::START);
    let root = tree.push_grid(grid, vec![item]);
    definite_layout(&tree, root, 60.0, 10.0);
    assert_close(tree.layout(item).location.x, 20.0);

    // Fully definite placements under column flow keep their areas.
    let mut tree = TestTree::default();
    let mut style = fixed_leaf_style(10.0, 10.0);
    style.grid_column = placement(line(2), line(3));
    style.grid_row = placement(line(1), line(2));
    let definite = tree.push_leaf(style, Size::new(10.0, 10.0), Size::new(10.0, 10.0));
    let mut grid = grid_style(&[px(20.0), px(20.0)], &[px(10.0), px(10.0)]);
    grid.auto_flow = GridAutoFlow::COLUMN;
    grid.justify_items = justify_items(AlignFlags::START);
    grid.align_items = ItemPlacement(AlignFlags::START);
    let root = tree.push_grid(grid, vec![definite]);
    definite_layout(&tree, root, 40.0, 20.0);
    assert_point(tree.layout(definite).location, Point::new(20.0, 0.0));

    // Sparse row flow: a definite-column item behind the cursor wraps to
    // the next row instead of backtracking.
    let sparse = |dense: bool| -> (Point<f32>, Point<f32>, Point<f32>) {
        let mut tree = TestTree::default();
        let first = fixed_leaf(&mut tree, 10.0, 10.0);
        let mut second_style = fixed_leaf_style(10.0, 10.0);
        second_style.grid_column = placement(line(2), line(3));
        let second = tree.push_leaf(second_style, Size::new(10.0, 10.0), Size::new(10.0, 10.0));
        let mut third_style = fixed_leaf_style(10.0, 10.0);
        third_style.grid_column = placement(line(1), line(2));
        let third = tree.push_leaf(third_style, Size::new(10.0, 10.0), Size::new(10.0, 10.0));
        let mut grid = grid_style(&[px(20.0), px(20.0)], &[px(10.0), px(10.0), px(10.0)]);
        grid.auto_flow = if dense {
            GridAutoFlow::ROW | GridAutoFlow::DENSE
        } else {
            GridAutoFlow::ROW
        };
        grid.justify_items = justify_items(AlignFlags::START);
        grid.align_items = ItemPlacement(AlignFlags::START);
        let root = tree.push_grid(grid, vec![first, second, third]);
        definite_layout(&tree, root, 40.0, 30.0);
        (
            tree.layout(first).location,
            tree.layout(second).location,
            tree.layout(third).location,
        )
    };

    let (first, second, third) = sparse(false);
    assert_point(first, Point::new(0.0, 0.0));
    assert_point(second, Point::new(20.0, 0.0));
    // Sparse: the cursor is past column 1, so the item wraps to row 2.
    assert_point(third, Point::new(0.0, 10.0));

    let (first, second, third) = sparse(true);
    assert_point(first, Point::new(0.0, 0.0));
    assert_point(second, Point::new(20.0, 0.0));
    // Dense: packing restarts from the row start; both cells of row 1 are
    // taken, so the item still lands in row 2 after probing row 1.
    assert_point(third, Point::new(0.0, 10.0));
}

/// Trailing template components after the UA track limit are dropped
/// rather than materialized.
#[test]
fn template_components_after_the_track_limit_are_dropped() {
    let mut tree = TestTree::default();
    let mut probe_style = fixed_leaf_style(1.0, 10.0);
    probe_style.grid_column = placement(line(-1), line(-2));
    let probe = tree.push_leaf(probe_style, Size::new(1.0, 10.0), Size::new(1.0, 10.0));
    let mut style = grid_style(&[], &[px(10.0)]);
    style.template_columns = track_list(vec![
        repeat(RepeatCount::Number(10_000), vec![px(1.0)]),
        TrackListValue::TrackSize(px(50.0)),
    ]);
    let root = tree.push_grid(style, vec![probe]);

    let output = intrinsic_layout(&tree, root);

    // The 50px track fell past the 10,000-track limit: total width is the
    // repetition alone and the last line is still the 1px track.
    assert_size(output.size, Size::new(10_000.0, 10.0));
    assert_close(tree.layout(probe).location.x, 9_999.0);
}
