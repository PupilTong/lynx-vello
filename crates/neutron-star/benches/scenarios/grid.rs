//! CSS Grid throughput benchmarks over a styling-engine-free, `Vec`-backed host.
//!
//! Fixture construction happens in divan's input generator, outside the timed
//! region. The measured closure therefore covers layout and the host-owned
//! cache protocol, but not DOM/style construction or fixture allocation.

use divan::counter::ItemsCount;
use neutron_star::cache::Cache;
use neutron_star::compute::{
    FnLeafMeasurer, LeafMetrics, compute_cached_layout, compute_grid_layout, compute_leaf_layout,
};
use neutron_star::prelude::*;
use neutron_star::style::{
    CalcHandle, Dimension, GridAutoFlow, GridLine, GridPlacement, GridTemplateComponent,
    LengthPercentage, MaxTrackSizingFunction, MinTrackSizingFunction, RepetitionCount,
    TrackSizingFunction,
};

#[derive(Debug, Clone, Copy)]
struct BoxStyle {
    size: Size<Dimension>,
}

impl Default for BoxStyle {
    fn default() -> Self {
        Self {
            size: Size::new(Dimension::Auto, Dimension::Auto),
        }
    }
}

impl CoreStyle for BoxStyle {
    #[inline]
    fn size(&self) -> Size<Dimension> {
        self.size
    }
}

#[derive(Debug, Clone, Copy)]
struct NoRepetition;

impl GridTemplateRepetition for NoRepetition {
    type Tracks<'a> = std::iter::Empty<TrackSizingFunction>;

    #[inline]
    fn count(&self) -> RepetitionCount {
        RepetitionCount::Count(1)
    }

    #[inline]
    fn tracks(&self) -> Self::Tracks<'_> {
        std::iter::empty()
    }
}

#[inline]
fn single_track(track: TrackSizingFunction) -> GridTemplateComponent<NoRepetition> {
    GridTemplateComponent::Single(track)
}

type TemplateTracks<'a> = std::iter::Map<
    std::iter::Copied<std::slice::Iter<'a, TrackSizingFunction>>,
    fn(TrackSizingFunction) -> GridTemplateComponent<NoRepetition>,
>;

#[derive(Debug, Clone)]
struct ContainerData {
    rows: Vec<TrackSizingFunction>,
    columns: Vec<TrackSizingFunction>,
    auto_rows: Vec<TrackSizingFunction>,
    auto_columns: Vec<TrackSizingFunction>,
    auto_flow: GridAutoFlow,
    gap: Size<LengthPercentage>,
}

impl Default for ContainerData {
    fn default() -> Self {
        Self {
            rows: Vec::new(),
            columns: Vec::new(),
            // Empty auto-track lists have the protocol-defined `auto`
            // behavior and avoid allocating for the common default.
            auto_rows: Vec::new(),
            auto_columns: Vec::new(),
            auto_flow: GridAutoFlow::Row,
            gap: Size::new(LengthPercentage::ZERO, LengthPercentage::ZERO),
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct ContainerStyleView<'a> {
    core: BoxStyle,
    grid: &'a ContainerData,
}

impl CoreStyle for ContainerStyleView<'_> {
    #[inline]
    fn size(&self) -> Size<Dimension> {
        self.core.size
    }
}

impl GridContainerStyle for ContainerStyleView<'_> {
    type Repetition<'a>
        = NoRepetition
    where
        Self: 'a;
    type TemplateTracks<'a>
        = TemplateTracks<'a>
    where
        Self: 'a;
    type AutoTracks<'a>
        = std::iter::Copied<std::slice::Iter<'a, TrackSizingFunction>>
    where
        Self: 'a;

    #[inline]
    fn grid_template_rows(&self) -> Self::TemplateTracks<'_> {
        self.grid.rows.iter().copied().map(single_track as _)
    }

    #[inline]
    fn grid_template_columns(&self) -> Self::TemplateTracks<'_> {
        self.grid.columns.iter().copied().map(single_track as _)
    }

    #[inline]
    fn grid_auto_rows(&self) -> Self::AutoTracks<'_> {
        self.grid.auto_rows.iter().copied()
    }

    #[inline]
    fn grid_auto_columns(&self) -> Self::AutoTracks<'_> {
        self.grid.auto_columns.iter().copied()
    }

    #[inline]
    fn grid_auto_flow(&self) -> GridAutoFlow {
        self.grid.auto_flow
    }

    #[inline]
    fn gap(&self) -> Size<LengthPercentage> {
        self.grid.gap
    }
}

#[derive(Debug, Clone, Copy)]
struct ItemData {
    row: Line<GridPlacement>,
    column: Line<GridPlacement>,
    order: i32,
}

impl Default for ItemData {
    fn default() -> Self {
        Self {
            row: Line::new(GridPlacement::Auto, GridPlacement::Auto),
            column: Line::new(GridPlacement::Auto, GridPlacement::Auto),
            order: 0,
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct ItemStyleView {
    core: BoxStyle,
    grid: ItemData,
}

impl CoreStyle for ItemStyleView {
    #[inline]
    fn size(&self) -> Size<Dimension> {
        self.core.size
    }
}

impl GridItemStyle for ItemStyleView {
    #[inline]
    fn grid_row(&self) -> Line<GridPlacement> {
        self.grid.row
    }

    #[inline]
    fn grid_column(&self) -> Line<GridPlacement> {
        self.grid.column
    }

    #[inline]
    fn order(&self) -> i32 {
        self.grid.order
    }
}

#[derive(Debug, Clone, Copy)]
struct IntrinsicSize {
    min: Size<f32>,
    max: Size<f32>,
}

#[derive(Debug, Clone, Copy)]
enum Display {
    Grid,
    Leaf,
}

#[derive(Debug)]
struct BenchSourceNode {
    display: Display,
    core_style: BoxStyle,
    container_index: usize,
    item_style: ItemData,
    children: Vec<NodeId>,
    intrinsic: IntrinsicSize,
}

#[derive(Debug)]
struct BenchSessionNode {
    cache: Cache,
    layout: Layout,
}

#[derive(Debug, Default)]
struct BenchSource {
    nodes: Vec<BenchSourceNode>,
    containers: Vec<ContainerData>,
}

impl BenchSource {
    #[inline]
    fn node(&self, node: NodeId) -> &BenchSourceNode {
        &self.nodes[usize::from(node)]
    }

    #[inline]
    fn node_mut(&mut self, node: NodeId) -> &mut BenchSourceNode {
        &mut self.nodes[usize::from(node)]
    }
}

#[derive(Debug, Default)]
struct BenchSession {
    nodes: Vec<BenchSessionNode>,
}

impl BenchSession {
    #[inline]
    fn node(&self, node: NodeId) -> &BenchSessionNode {
        &self.nodes[usize::from(node)]
    }

    #[inline]
    fn node_mut(&mut self, node: NodeId) -> &mut BenchSessionNode {
        &mut self.nodes[usize::from(node)]
    }
}

/// Builder and benchmark facade. Layout receives the two stores separately.
#[derive(Debug, Default)]
struct BenchTree {
    source: BenchSource,
    session: BenchSession,
}

impl BenchTree {
    fn push_leaf(
        &mut self,
        core_style: BoxStyle,
        item_style: ItemData,
        intrinsic: IntrinsicSize,
    ) -> NodeId {
        self.push(BenchSourceNode {
            display: Display::Leaf,
            core_style,
            container_index: usize::MAX,
            item_style,
            children: Vec::new(),
            intrinsic,
        })
    }

    fn push_grid(
        &mut self,
        container_style: ContainerData,
        item_style: ItemData,
        children: Vec<NodeId>,
    ) -> NodeId {
        let container_index = self.source.containers.len();
        self.source.containers.push(container_style);
        self.push(BenchSourceNode {
            display: Display::Grid,
            core_style: BoxStyle::default(),
            container_index,
            item_style,
            children,
            intrinsic: IntrinsicSize {
                min: Size::ZERO,
                max: Size::ZERO,
            },
        })
    }

    fn push(&mut self, node: BenchSourceNode) -> NodeId {
        debug_assert_eq!(self.source.nodes.len(), self.session.nodes.len());
        let id = NodeId::from(self.source.nodes.len());
        self.source.nodes.push(node);
        self.session.nodes.push(BenchSessionNode {
            cache: Cache::new(),
            layout: Layout::default(),
        });
        id
    }
}

impl TraverseTree for BenchSource {
    type ChildIter<'a> = std::iter::Copied<std::slice::Iter<'a, NodeId>>;

    #[inline]
    fn child_ids(&self, parent: NodeId) -> Self::ChildIter<'_> {
        self.node(parent).children.iter().copied()
    }

    #[inline]
    fn child_count(&self, parent: NodeId) -> usize {
        self.node(parent).children.len()
    }

    #[inline]
    fn child_id(&self, parent: NodeId, index: usize) -> NodeId {
        self.node(parent).children[index]
    }
}

impl LayoutSource for BenchSource {
    type CoreStyle<'a> = BoxStyle;

    #[inline]
    fn core_style(&self, node: NodeId) -> Self::CoreStyle<'_> {
        self.node(node).core_style
    }

    #[inline]
    fn resolve_calc(&self, _calc: CalcHandle, _basis: f32) -> f32 {
        unreachable!("benchmark styles do not contain calc() values")
    }
}

impl GridSource for BenchSource {
    type ContainerStyle<'a> = ContainerStyleView<'a>;
    type ItemStyle<'a> = ItemStyleView;

    #[inline]
    fn grid_container_style(&self, container: NodeId) -> Self::ContainerStyle<'_> {
        let node = self.node(container);
        ContainerStyleView {
            core: node.core_style,
            grid: &self.containers[node.container_index],
        }
    }

    #[inline]
    fn grid_item_style(&self, item: NodeId) -> Self::ItemStyle<'_> {
        let node = self.node(item);
        ItemStyleView {
            core: node.core_style,
            grid: node.item_style,
        }
    }
}

impl LayoutState for BenchSession {
    #[inline]
    fn set_unrounded_layout(&mut self, node: NodeId, layout: &Layout) {
        self.node_mut(node).layout = *layout;
    }

    #[inline]
    fn set_static_position(&mut self, _child: NodeId, _static_position: Point<f32>) {
        unreachable!("benchmark fixtures do not contain hoisted positioned nodes")
    }
}

impl CacheState for BenchSession {
    #[inline]
    fn cache_get(&self, node: NodeId, input: LayoutInput) -> Option<LayoutOutput> {
        self.node(node).cache.get(input)
    }

    #[inline]
    fn cache_store(&mut self, node: NodeId, input: LayoutInput, output: LayoutOutput) {
        self.node_mut(node).cache.store(input, output);
    }

    #[inline]
    fn cache_clear(&mut self, node: NodeId) {
        self.node_mut(node).cache.clear();
    }
}

impl LayoutSession<BenchSource> for BenchSession {
    fn compute_child_layout(
        &mut self,
        source: &BenchSource,
        child: NodeId,
        input: LayoutInput,
    ) -> LayoutOutput {
        let node = source.node(child);
        let display = node.display;
        let style = node.core_style;
        let intrinsic = node.intrinsic;
        compute_cached_layout(
            self,
            child,
            input,
            move |session, child, input| match display {
                Display::Grid => compute_grid_layout(source, session, child, input),
                Display::Leaf => {
                    let mut measurer = FnLeafMeasurer::new(move |measure_input| {
                        let measured = Size::new(
                            if measure_input.available_space.width == AvailableSpace::MinContent {
                                intrinsic.min.width
                            } else {
                                intrinsic.max.width
                            },
                            if measure_input.available_space.height == AvailableSpace::MinContent {
                                intrinsic.min.height
                            } else {
                                intrinsic.max.height
                            },
                        );
                        LeafMetrics::new(measure_input.known_dimensions.unwrap_or(measured))
                    });
                    compute_leaf_layout(
                        input,
                        &style,
                        |_calc, _basis| unreachable!("benchmark styles contain no calc() values"),
                        &mut measurer,
                    )
                }
            },
        )
    }
}

#[derive(Debug)]
struct Fixture {
    tree: BenchTree,
    root: NodeId,
    probe_a: NodeId,
    probe_b: NodeId,
    viewport: Size<f32>,
}

type GeometrySample = (LayoutOutput, Layout, Layout);

impl Fixture {
    #[inline]
    fn run(&mut self) -> GeometrySample {
        let available = Size::new(
            AvailableSpace::Definite(self.viewport.width),
            AvailableSpace::Definite(self.viewport.height),
        );
        let known = Size::new(Some(self.viewport.width), Some(self.viewport.height));
        let output = self.tree.session.compute_child_layout(
            &self.tree.source,
            self.root,
            LayoutInput::perform_layout(known, known, available),
        );
        (
            output,
            self.tree.session.node(self.probe_a).layout,
            self.tree.session.node(self.probe_b).layout,
        )
    }

    #[inline]
    fn clear_root_cache(&mut self) {
        self.tree.session.cache_clear(self.root);
    }
}

#[inline]
fn px(value: f32) -> TrackSizingFunction {
    TrackSizingFunction::fixed(LengthPercentage::length(value))
}

#[inline]
fn fr(value: f32) -> TrackSizingFunction {
    TrackSizingFunction::fr(value)
}

#[inline]
fn fixed_box(width: f32, height: f32) -> BoxStyle {
    BoxStyle {
        size: Size::new(Dimension::Length(width), Dimension::Length(height)),
    }
}

#[inline]
fn intrinsic(min_width: f32, max_width: f32, height: f32) -> IntrinsicSize {
    IntrinsicSize {
        min: Size::new(min_width, height),
        max: Size::new(max_width, height),
    }
}

#[inline]
fn small_u16(value: usize) -> u16 {
    u16::try_from(value).expect("benchmark fixture value fits in u16")
}

#[inline]
fn small_f32(value: usize) -> f32 {
    f32::from(small_u16(value))
}

#[inline]
fn positive_grid_line(value: usize) -> GridLine {
    GridLine::new(i16::try_from(value).expect("benchmark grid line fits in i16"))
}

fn sparse_auto_fixture(item_count: usize) -> Fixture {
    const COLUMNS: usize = 32;
    let mut tree = BenchTree::default();
    tree.source.nodes.reserve(item_count + 1);
    tree.session.nodes.reserve(item_count + 1);
    let mut children = Vec::with_capacity(item_count);
    for index in 0..item_count {
        let width = 6.0 + small_f32(index % 5);
        children.push(tree.push_leaf(
            fixed_box(width, 6.0),
            ItemData::default(),
            intrinsic(width, width, 6.0),
        ));
    }
    let probe_a = children[0];
    let probe_b = children[item_count - 1];
    let rows = item_count.div_ceil(COLUMNS);
    let root = tree.push_grid(
        ContainerData {
            columns: vec![px(16.0); COLUMNS],
            auto_rows: vec![px(12.0)],
            ..ContainerData::default()
        },
        ItemData::default(),
        children,
    );
    Fixture {
        tree,
        root,
        probe_a,
        probe_b,
        viewport: Size::new(small_f32(COLUMNS) * 16.0, small_f32(rows) * 12.0),
    }
}

fn dense_holes_fixture(item_count: usize) -> Fixture {
    const COLUMNS: usize = 16;
    let mut tree = BenchTree::default();
    tree.source.nodes.reserve(item_count + 1);
    tree.session.nodes.reserve(item_count + 1);
    let mut children = Vec::with_capacity(item_count);
    for index in 0..item_count {
        // Alternating wide items repeatedly strand short holes which later
        // one-track items can backfill in dense mode.
        let span = match index % 8 {
            0 | 3 => 7,
            1 | 5 => 5,
            _ => 1,
        };
        let item_style = ItemData {
            column: Line::new(GridPlacement::Auto, GridPlacement::Span(span)),
            ..ItemData::default()
        };
        children.push(tree.push_leaf(fixed_box(6.0, 6.0), item_style, intrinsic(6.0, 6.0, 6.0)));
    }
    let probe_a = children[item_count / 2];
    let probe_b = children[item_count - 1];
    let root = tree.push_grid(
        ContainerData {
            columns: vec![px(20.0); COLUMNS],
            auto_rows: vec![px(12.0)],
            auto_flow: GridAutoFlow::RowDense,
            ..ContainerData::default()
        },
        ItemData::default(),
        children,
    );
    Fixture {
        tree,
        root,
        probe_a,
        probe_b,
        viewport: Size::new(small_f32(COLUMNS) * 20.0, small_f32(item_count) * 3.0),
    }
}

fn fixed_fr_fixture() -> Fixture {
    const COLUMNS: usize = 24;
    const ITEMS: usize = 768;
    let mut tree = BenchTree::default();
    tree.source.nodes.reserve(ITEMS + 1);
    tree.session.nodes.reserve(ITEMS + 1);
    let mut children = Vec::with_capacity(ITEMS);
    for index in 0..ITEMS {
        let min_width = 4.0 + small_f32(index % 7);
        let max_width = min_width + 8.0 + small_f32(index % 11);
        children.push(tree.push_leaf(
            BoxStyle::default(),
            ItemData::default(),
            intrinsic(min_width, max_width, 9.0),
        ));
    }
    let probe_a = children[0];
    let probe_b = children[ITEMS - 1];
    let columns = (0..COLUMNS)
        .map(|index| {
            if index % 3 == 0 {
                px(24.0)
            } else {
                fr(small_f32(index % 4 + 1))
            }
        })
        .collect();
    let root = tree.push_grid(
        ContainerData {
            columns,
            auto_rows: vec![px(14.0)],
            gap: Size::new(LengthPercentage::length(2.0), LengthPercentage::length(1.0)),
            ..ContainerData::default()
        },
        ItemData::default(),
        children,
    );
    Fixture {
        tree,
        root,
        probe_a,
        probe_b,
        viewport: Size::new(1_200.0, 448.0),
    }
}

fn intrinsic_spans_fixture() -> Fixture {
    const COLUMNS: usize = 12;
    const ITEMS: usize = 256;
    let mut tree = BenchTree::default();
    tree.source.nodes.reserve(ITEMS + 1);
    tree.session.nodes.reserve(ITEMS + 1);
    let mut children = Vec::with_capacity(ITEMS);
    for index in 0..ITEMS {
        let span = 2 + small_u16(index % 4);
        let min_width = 14.0 + small_f32(index % 13);
        let max_width = min_width + 40.0 + small_f32(index % 29);
        children.push(tree.push_leaf(
            BoxStyle::default(),
            ItemData {
                column: Line::new(GridPlacement::Auto, GridPlacement::Span(span)),
                ..ItemData::default()
            },
            intrinsic(min_width, max_width, 10.0 + small_f32(index % 5)),
        ));
    }
    let probe_a = children[ITEMS / 2];
    let probe_b = children[ITEMS - 1];
    let intrinsic_track = TrackSizingFunction::minmax(
        MinTrackSizingFunction::MinContent,
        MaxTrackSizingFunction::MaxContent,
    );
    let root = tree.push_grid(
        ContainerData {
            columns: vec![intrinsic_track; COLUMNS],
            auto_rows: vec![TrackSizingFunction::AUTO],
            gap: Size::new(LengthPercentage::length(3.0), LengthPercentage::length(2.0)),
            ..ContainerData::default()
        },
        ItemData::default(),
        children,
    );
    Fixture {
        tree,
        root,
        probe_a,
        probe_b,
        viewport: Size::new(1_024.0, 768.0),
    }
}

/// Exercises the span-bucket sizing path with one item at every span length.
/// Keeping the number of items proportional to the track count makes changes
/// from a bucketed pass back to repeated whole-item scans visible as the
/// benchmark scales.
fn unique_intrinsic_spans_fixture(track_count: usize) -> Fixture {
    assert!(track_count >= 2);
    let mut tree = BenchTree::default();
    tree.source.nodes.reserve(track_count + 1);
    tree.session.nodes.reserve(track_count + 1);
    let mut children = Vec::with_capacity(track_count);
    for span in 1..=track_count {
        let min_width = 6.0 + small_f32(span % 17);
        let max_width = min_width + small_f32(span) * 0.75;
        children.push(tree.push_leaf(
            BoxStyle::default(),
            ItemData {
                column: Line::new(GridPlacement::Auto, GridPlacement::Span(small_u16(span))),
                ..ItemData::default()
            },
            intrinsic(min_width, max_width, 8.0),
        ));
    }
    let probe_a = children[track_count / 2];
    let probe_b = children[track_count - 1];
    let intrinsic_track = TrackSizingFunction::minmax(
        MinTrackSizingFunction::MinContent,
        MaxTrackSizingFunction::MaxContent,
    );
    let root = tree.push_grid(
        ContainerData {
            columns: vec![intrinsic_track; track_count],
            auto_rows: vec![px(10.0)],
            ..ContainerData::default()
        },
        ItemData::default(),
        children,
    );
    Fixture {
        tree,
        root,
        probe_a,
        probe_b,
        viewport: Size::new(small_f32(track_count) * 12.0, small_f32(track_count) * 10.0),
    }
}

/// Gives every flexible track a distinct base/flex threshold. At the chosen
/// width a substantial suffix must freeze, stressing the sorted-threshold
/// implementation of Grid §12.7.1 instead of the trivial all-flex case.
fn flex_freeze_threshold_fixture(track_count: usize) -> Fixture {
    assert!(track_count >= 2);
    let mut tree = BenchTree::default();
    tree.source.nodes.reserve(track_count + 1);
    tree.session.nodes.reserve(track_count + 1);
    let mut children = Vec::with_capacity(track_count);
    for index in 0..track_count {
        let threshold = 4.0 + small_f32(index);
        children.push(tree.push_leaf(
            BoxStyle::default(),
            ItemData {
                row: Line::new(
                    GridPlacement::Line(positive_grid_line(1)),
                    GridPlacement::Line(positive_grid_line(2)),
                ),
                column: Line::new(
                    GridPlacement::Line(positive_grid_line(index + 1)),
                    GridPlacement::Line(positive_grid_line(index + 2)),
                ),
                order: 0,
            },
            intrinsic(threshold, threshold, 8.0),
        ));
    }
    let probe_a = children[track_count / 2];
    let probe_b = children[track_count - 1];
    let root = tree.push_grid(
        ContainerData {
            rows: vec![px(12.0)],
            columns: vec![fr(1.0); track_count],
            ..ContainerData::default()
        },
        ItemData::default(),
        children,
    );
    let extent = small_f32(track_count);
    Fixture {
        tree,
        root,
        probe_a,
        probe_b,
        // This falls between the sum of all bases and a uniform allocation,
        // so the high-threshold tracks freeze while the rest redistribute.
        viewport: Size::new(extent * extent * 0.65, 12.0),
    }
}

fn nested_fixture() -> Fixture {
    const INNER_GRIDS: usize = 16;
    const LEAVES_PER_GRID: usize = 64;
    let mut tree = BenchTree::default();
    tree.source
        .nodes
        .reserve(INNER_GRIDS * (LEAVES_PER_GRID + 1) + 1);
    tree.session
        .nodes
        .reserve(INNER_GRIDS * (LEAVES_PER_GRID + 1) + 1);
    let mut inner_grids = Vec::with_capacity(INNER_GRIDS);
    let mut first_leaf = None;
    let mut last_leaf = None;
    for grid_index in 0..INNER_GRIDS {
        let mut leaves = Vec::with_capacity(LEAVES_PER_GRID);
        for leaf_index in 0..LEAVES_PER_GRID {
            let width = 4.0 + small_f32((grid_index + leaf_index) % 9);
            let leaf = tree.push_leaf(
                BoxStyle::default(),
                ItemData::default(),
                intrinsic(width, width + 12.0, 8.0),
            );
            first_leaf.get_or_insert(leaf);
            last_leaf = Some(leaf);
            leaves.push(leaf);
        }
        inner_grids.push(tree.push_grid(
            ContainerData {
                columns: vec![fr(1.0); 8],
                auto_rows: vec![px(18.0)],
                gap: Size::new(LengthPercentage::length(1.0), LengthPercentage::length(1.0)),
                ..ContainerData::default()
            },
            ItemData::default(),
            leaves,
        ));
    }
    let root = tree.push_grid(
        ContainerData {
            rows: vec![fr(1.0); 4],
            columns: vec![fr(1.0); 4],
            gap: Size::new(LengthPercentage::length(4.0), LengthPercentage::length(4.0)),
            ..ContainerData::default()
        },
        ItemData::default(),
        inner_grids,
    );
    Fixture {
        tree,
        root,
        probe_a: first_leaf.expect("nested fixture has leaves"),
        probe_b: last_leaf.expect("nested fixture has leaves"),
        viewport: Size::new(1_200.0, 800.0),
    }
}

#[derive(Debug)]
struct DirtyNestedFixture {
    fixture: Fixture,
    dirty_leaf: NodeId,
    dirty_ancestor: NodeId,
    wide: bool,
}

impl DirtyNestedFixture {
    fn new() -> Self {
        let mut fixture = nested_fixture();
        // Populate every cache before timing invalidation and incremental
        // relayout. The affected path is one leaf -> one inner Grid -> root.
        let _ = fixture.run();
        let dirty_leaf = fixture.probe_a;
        let dirty_ancestor = fixture
            .tree
            .source
            .nodes
            .iter()
            .position(|node| node.children.contains(&dirty_leaf))
            .map(NodeId::from)
            .expect("nested fixture leaf has a Grid parent");
        Self {
            fixture,
            dirty_leaf,
            dirty_ancestor,
            wide: false,
        }
    }

    #[inline]
    fn run(&mut self) -> GeometrySample {
        self.wide = !self.wide;
        let intrinsic = &mut self.fixture.tree.source.node_mut(self.dirty_leaf).intrinsic;
        intrinsic.max.width = if self.wide { 24.0 } else { 16.0 };
        self.fixture.tree.session.cache_clear(self.dirty_leaf);
        self.fixture.tree.session.cache_clear(self.dirty_ancestor);
        self.fixture.tree.session.cache_clear(self.fixture.root);
        self.fixture.run()
    }
}

const FIXED_TRACKS_BATCH: usize = 16;
const INTRINSIC_SPANS_BATCH: usize = 16;
const WARM_DESCENDANTS_BATCH: usize = 512;
const ROOT_CACHE_HIT_BATCH: usize = 131_072;
const DIRTY_PATH_BATCH: usize = 32;

const fn sparse_batch_size(item_count: usize) -> usize {
    if item_count <= 256 { 64 } else { 4 }
}

const fn dense_batch_size(item_count: usize) -> usize {
    if item_count <= 256 { 8 } else { 1 }
}

const fn unique_span_batch_size(track_count: usize) -> usize {
    match track_count {
        32 => 256,
        128 => 32,
        _ => 2,
    }
}

const fn flex_freeze_batch_size(track_count: usize) -> usize {
    match track_count {
        32 => 256,
        256 => 32,
        _ => 8,
    }
}

fn bench_cold<Make>(bencher: divan::Bencher<'_, '_>, batch_size: usize, make_fixture: Make)
where
    Make: Fn() -> Fixture + Copy,
{
    bencher
        .counter(ItemsCount::new(batch_size))
        .with_inputs(move || (0..batch_size).map(|_| make_fixture()).collect::<Vec<_>>())
        .bench_local_values(|mut fixtures| {
            for fixture in &mut fixtures {
                divan::black_box(fixture.run());
            }
            fixtures
        });
}

#[divan::bench(args = [256, 4_096])]
fn sparse_auto_placement_cold(bencher: divan::Bencher<'_, '_>, item_count: usize) {
    bench_cold(bencher, sparse_batch_size(item_count), || {
        sparse_auto_fixture(item_count)
    });
}

#[divan::bench(args = [256, 1_024])]
fn dense_hole_backfill_cold(bencher: divan::Bencher<'_, '_>, item_count: usize) {
    bench_cold(bencher, dense_batch_size(item_count), || {
        dense_holes_fixture(item_count)
    });
}

#[divan::bench]
fn fixed_and_fractional_tracks_cold(bencher: divan::Bencher<'_, '_>) {
    bench_cold(bencher, FIXED_TRACKS_BATCH, fixed_fr_fixture);
}

#[divan::bench]
fn intrinsic_spanning_items_cold(bencher: divan::Bencher<'_, '_>) {
    bench_cold(bencher, INTRINSIC_SPANS_BATCH, intrinsic_spans_fixture);
}

#[divan::bench(args = [32, 128, 512])]
fn unique_intrinsic_span_buckets_cold(bencher: divan::Bencher<'_, '_>, track_count: usize) {
    bench_cold(bencher, unique_span_batch_size(track_count), || {
        unique_intrinsic_spans_fixture(track_count)
    });
}

#[divan::bench(args = [32, 256, 1_024])]
fn flexible_track_freeze_thresholds_cold(bencher: divan::Bencher<'_, '_>, track_count: usize) {
    bench_cold(bencher, flex_freeze_batch_size(track_count), || {
        flex_freeze_threshold_fixture(track_count)
    });
}

#[divan::bench]
fn nested_grid_cold(bencher: divan::Bencher<'_, '_>) {
    bench_cold(bencher, 1, nested_fixture);
}

#[divan::bench]
fn nested_grid_warm_descendants(bencher: divan::Bencher<'_, '_>) {
    bencher
        .with_inputs(|| {
            let mut fixture = nested_fixture();
            let _ = fixture.run();
            // Keep descendant cache entries warm while forcing the root Grid
            // algorithm to run, matching a common incremental-relayout path.
            fixture.clear_root_cache();
            fixture
        })
        .counter(ItemsCount::new(WARM_DESCENDANTS_BATCH))
        .bench_local_refs(|fixture| {
            for _ in 0..WARM_DESCENDANTS_BATCH {
                divan::black_box(fixture.run());
                fixture.clear_root_cache();
            }
        });
}

#[divan::bench]
fn nested_grid_warm_root_cache_hit(bencher: divan::Bencher<'_, '_>) {
    bencher
        .with_inputs(|| {
            let mut fixture = nested_fixture();
            let _ = fixture.run();
            fixture
        })
        .counter(ItemsCount::new(ROOT_CACHE_HIT_BATCH))
        .bench_local_refs(|fixture| {
            for _ in 0..ROOT_CACHE_HIT_BATCH {
                divan::black_box(divan::black_box(&mut *fixture).run());
            }
        });
}

#[divan::bench]
fn nested_grid_dirty_leaf_and_ancestors(bencher: divan::Bencher<'_, '_>) {
    bencher
        .counter(ItemsCount::new(DIRTY_PATH_BATCH))
        .with_inputs(DirtyNestedFixture::new)
        .bench_local_refs(|fixture| {
            for _ in 0..DIRTY_PATH_BATCH {
                divan::black_box(fixture.run());
            }
        });
}
