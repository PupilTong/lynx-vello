//! CSS Grid throughput benchmarks over a styling-engine-free, `Vec`-backed host.
//!
//! Fixture construction happens in divan's input generator, outside the timed
//! region. The measured closure therefore covers layout and the host-owned
//! cache protocol, but not DOM/style construction or fixture allocation.

use std::cell::{Cell, RefCell};

use divan::counter::ItemsCount;
use neutron_star::cache::Cache;
use neutron_star::compute::{
    FnLeafMeasurer, LeafMetrics, compute_cached_layout, compute_grid_layout, compute_leaf_layout,
};
use neutron_star::prelude::*;
use stylo::values::computed::length::NonNegativeLengthPercentageOrNormal;
use stylo::values::computed::{
    Display, GridAutoFlow, GridLine, GridTemplateComponent, ImplicitGridTracks, Length,
    LengthPercentage, Size as StyleSize, TrackList, TrackSize,
};
use stylo::values::generics::NonNegative;
use stylo::values::generics::grid::{
    Flex, ImplicitGridTracks as GenericImplicitGridTracks, TrackBreadth, TrackListValue,
};

#[derive(Debug, Clone)]
struct BoxStyle {
    size: Size<StyleSize>,
}

impl Default for BoxStyle {
    fn default() -> Self {
        Self {
            size: Size::new(StyleSize::Auto, StyleSize::Auto),
        }
    }
}

impl CoreStyle for BoxStyle {
    #[inline]
    fn display(&self) -> Display {
        Display::Grid
    }

    #[inline]
    fn size(&self) -> Size<&StyleSize> {
        self.size.as_ref()
    }
}

#[derive(Debug, Clone)]
struct ContainerData {
    rows: GridTemplateComponent,
    columns: GridTemplateComponent,
    auto_rows: ImplicitGridTracks,
    auto_columns: ImplicitGridTracks,
    auto_flow: GridAutoFlow,
    gap: Size<NonNegativeLengthPercentageOrNormal>,
}

impl Default for ContainerData {
    fn default() -> Self {
        Self {
            rows: GridTemplateComponent::None,
            columns: GridTemplateComponent::None,
            // Empty auto-track lists have the protocol-defined `auto`
            // behavior and avoid allocating for the common default.
            auto_rows: ImplicitGridTracks::default(),
            auto_columns: ImplicitGridTracks::default(),
            auto_flow: GridAutoFlow::ROW,
            gap: Size::new(
                NonNegativeLengthPercentageOrNormal::Normal,
                NonNegativeLengthPercentageOrNormal::Normal,
            ),
        }
    }
}

#[derive(Debug, Clone)]
struct ItemData {
    row_start: GridLine,
    row_end: GridLine,
    column_start: GridLine,
    column_end: GridLine,
    order: i32,
}

impl Default for ItemData {
    fn default() -> Self {
        Self {
            row_start: GridLine::auto(),
            row_end: GridLine::auto(),
            column_start: GridLine::auto(),
            column_end: GridLine::auto(),
            order: 0,
        }
    }
}

/// The merged style view: one `Copy` handle serves the core, container, and
/// item roles of the protocol. Container data stays behind an `Option` that
/// is `None` for leaves, so materializing the view on every `style()` call
/// never touches the container side table for non-containers; container
/// accessors fetch through the reference lazily.
#[derive(Debug, Clone, Copy)]
struct GridStyleView<'t> {
    core: &'t BoxStyle,
    container: Option<&'t ContainerData>,
    item: &'t ItemData,
}

impl<'t> GridStyleView<'t> {
    #[inline]
    fn container(&self) -> &'t ContainerData {
        self.container
            .expect("container style accessors are only called on grid containers")
    }
}

impl CoreStyle for GridStyleView<'_> {
    #[inline]
    fn display(&self) -> Display {
        Display::Grid
    }

    #[inline]
    fn size(&self) -> Size<&StyleSize> {
        self.core.size.as_ref()
    }
}

impl GridContainerStyle for GridStyleView<'_> {
    #[inline]
    fn grid_template_rows(&self) -> &GridTemplateComponent {
        &self.container().rows
    }

    #[inline]
    fn grid_template_columns(&self) -> &GridTemplateComponent {
        &self.container().columns
    }

    #[inline]
    fn grid_auto_rows(&self) -> &ImplicitGridTracks {
        &self.container().auto_rows
    }

    #[inline]
    fn grid_auto_columns(&self) -> &ImplicitGridTracks {
        &self.container().auto_columns
    }

    #[inline]
    fn grid_auto_flow(&self) -> GridAutoFlow {
        self.container().auto_flow
    }

    #[inline]
    fn gap(&self) -> Size<&NonNegativeLengthPercentageOrNormal> {
        self.container().gap.as_ref()
    }
}

impl GridItemStyle for GridStyleView<'_> {
    #[inline]
    fn grid_row_start(&self) -> &GridLine {
        &self.item.row_start
    }

    #[inline]
    fn grid_row_end(&self) -> &GridLine {
        &self.item.row_end
    }

    #[inline]
    fn grid_column_start(&self) -> &GridLine {
        &self.item.column_start
    }

    #[inline]
    fn grid_column_end(&self) -> &GridLine {
        &self.item.column_end
    }

    #[inline]
    fn order(&self) -> i32 {
        self.item.order
    }
}

#[derive(Debug, Clone, Copy)]
struct IntrinsicSize {
    min: Size<f32>,
    max: Size<f32>,
}

#[derive(Debug, Clone, Copy)]
enum BenchDisplay {
    Grid,
    Leaf,
}

#[derive(Debug)]
struct BenchSourceNode {
    display: BenchDisplay,
    core_style: BoxStyle,
    container_index: usize,
    item_style: ItemData,
    children: Vec<usize>,
    intrinsic: IntrinsicSize,
}

/// Per-node mutable layout slots, written through [`BenchRef`] handles.
/// Layout is single-threaded, so `Cell`/`RefCell` interior mutability is the
/// whole synchronization story.
#[derive(Debug, Default)]
struct BenchSessionNode {
    cache: RefCell<Cache>,
    layout: Cell<Layout>,
}

/// The one host tree: source-shaped immutable node data plus a parallel
/// `Vec` of interior-mutable session slots, keeping memory layout comparable
/// with the pre-handle two-store host.
#[derive(Debug, Default)]
struct BenchTree {
    nodes: Vec<BenchSourceNode>,
    containers: Vec<ContainerData>,
    session: Vec<BenchSessionNode>,
}

impl BenchTree {
    fn push_leaf(
        &mut self,
        core_style: BoxStyle,
        item_style: ItemData,
        intrinsic: IntrinsicSize,
    ) -> usize {
        self.push(BenchSourceNode {
            display: BenchDisplay::Leaf,
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
        children: Vec<usize>,
    ) -> usize {
        let container_index = self.containers.len();
        self.containers.push(container_style);
        self.push(BenchSourceNode {
            display: BenchDisplay::Grid,
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

    fn push(&mut self, node: BenchSourceNode) -> usize {
        debug_assert_eq!(self.nodes.len(), self.session.len());
        let id = self.nodes.len();
        self.nodes.push(node);
        self.session.push(BenchSessionNode::default());
        id
    }

    /// Resolves a builder-returned index to a borrowed node handle.
    #[inline]
    fn node(&self, index: usize) -> BenchRef<'_> {
        BenchRef { tree: self, index }
    }

    #[inline]
    fn layout(&self, index: usize) -> Layout {
        self.session[index].layout.get()
    }
}

/// The `Copy` node handle: a borrow of the tree plus a node index.
#[derive(Clone, Copy)]
struct BenchRef<'t> {
    tree: &'t BenchTree,
    index: usize,
}

impl std::fmt::Debug for BenchRef<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_tuple("BenchRef")
            .field(&self.index)
            .finish()
    }
}

impl<'t> BenchRef<'t> {
    #[inline]
    fn source(self) -> &'t BenchSourceNode {
        &self.tree.nodes[self.index]
    }

    #[inline]
    fn slots(self) -> &'t BenchSessionNode {
        &self.tree.session[self.index]
    }
}

struct BenchChildren<'t> {
    tree: &'t BenchTree,
    ids: std::slice::Iter<'t, usize>,
}

impl<'t> Iterator for BenchChildren<'t> {
    type Item = BenchRef<'t>;

    fn next(&mut self) -> Option<BenchRef<'t>> {
        let index = *self.ids.next()?;
        Some(BenchRef {
            tree: self.tree,
            index,
        })
    }
}

impl<'t> LayoutNode for BenchRef<'t> {
    type Style = GridStyleView<'t>;
    type ChildIter = BenchChildren<'t>;

    #[inline]
    fn children(self) -> BenchChildren<'t> {
        BenchChildren {
            tree: self.tree,
            ids: self.source().children.iter(),
        }
    }

    #[inline]
    fn child_count(self) -> usize {
        self.source().children.len()
    }

    #[inline]
    fn style(self) -> GridStyleView<'t> {
        let node = self.source();
        GridStyleView {
            core: &node.core_style,
            container: (node.container_index != usize::MAX)
                .then(|| &self.tree.containers[node.container_index]),
            item: &node.item_style,
        }
    }

    fn compute_child_layout(self, input: LayoutInput) -> LayoutOutput {
        let node = self.source();
        let display = node.display;
        let style = &node.core_style;
        let intrinsic = node.intrinsic;
        compute_cached_layout(self, input, move |handle, input| match display {
            BenchDisplay::Grid => compute_grid_layout(handle, input),
            BenchDisplay::Leaf => {
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
                compute_leaf_layout(input, style, &mut measurer)
            }
        })
    }

    #[inline]
    fn set_unrounded_layout(self, layout: &Layout) {
        self.slots().layout.set(*layout);
    }

    #[inline]
    fn unrounded_layout(self) -> Layout {
        self.slots().layout.get()
    }

    fn set_final_layout(self, _layout: &Layout) {
        unreachable!("grid benchmarks do not run the rounding pass")
    }

    fn set_static_position(self, _static_position: Point<f32>) {
        unreachable!("benchmark fixtures do not contain hoisted positioned nodes")
    }

    #[inline]
    fn cache_get(self, input: LayoutInput) -> Option<LayoutOutput> {
        self.slots().cache.borrow().get(input)
    }

    #[inline]
    fn cache_store(self, input: LayoutInput, output: LayoutOutput) {
        self.slots().cache.borrow_mut().store(input, output);
    }

    #[inline]
    fn cache_clear(self) {
        self.slots().cache.borrow_mut().clear();
    }
}

#[derive(Debug)]
struct Fixture {
    tree: BenchTree,
    root: usize,
    probe_a: usize,
    probe_b: usize,
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
        let output = self
            .tree
            .node(self.root)
            .compute_child_layout(LayoutInput::perform_layout(known, known, available));
        (
            output,
            self.tree.layout(self.probe_a),
            self.tree.layout(self.probe_b),
        )
    }

    #[inline]
    fn clear_root_cache(&mut self) {
        self.tree.node(self.root).cache_clear();
    }
}

#[inline]
fn px(value: f32) -> TrackSize {
    TrackSize::Breadth(TrackBreadth::Breadth(LengthPercentage::new_length(
        Length::new(value),
    )))
}

#[inline]
fn fr(value: f32) -> TrackSize {
    TrackSize::Breadth(TrackBreadth::Flex(Flex(value)))
}

/// A `minmax(min-content, max-content)` track.
#[inline]
fn intrinsic_track() -> TrackSize {
    TrackSize::Minmax(TrackBreadth::MinContent, TrackBreadth::MaxContent)
}

/// An `auto` track (i.e. `minmax(auto, auto)`).
#[inline]
fn auto_track() -> TrackSize {
    TrackSize::Breadth(TrackBreadth::Auto)
}

/// Builds an explicit template from a plain track-size list, respecting the
/// `line_names.len() == values.len() + 1` invariant stylo's parser upholds.
fn template(tracks: Vec<TrackSize>) -> GridTemplateComponent {
    let values: Vec<_> = tracks.into_iter().map(TrackListValue::TrackSize).collect();
    let line_names = vec![stylo::OwnedSlice::default(); values.len() + 1];
    GridTemplateComponent::TrackList(Box::new(TrackList {
        auto_repeat_index: usize::MAX,
        values: values.into(),
        line_names: line_names.into(),
    }))
}

fn implicit(tracks: Vec<TrackSize>) -> ImplicitGridTracks {
    GenericImplicitGridTracks(tracks.into())
}

#[inline]
fn fixed_box(width: f32, height: f32) -> BoxStyle {
    BoxStyle {
        size: Size::new(
            StyleSize::LengthPercentage(NonNegative(LengthPercentage::new_length(Length::new(
                width,
            )))),
            StyleSize::LengthPercentage(NonNegative(LengthPercentage::new_length(Length::new(
                height,
            )))),
        ),
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

/// A numeric (non-span) grid line.
#[inline]
fn line(value: usize) -> GridLine {
    let mut line = GridLine::auto();
    line.line_num = i32::try_from(value).expect("benchmark grid line fits in i32");
    line
}

/// A `span <n>` grid line.
#[inline]
fn span(value: usize) -> GridLine {
    let mut line = GridLine::auto();
    line.is_span = true;
    line.line_num = i32::try_from(value).expect("benchmark span fits in i32");
    line
}

fn sparse_auto_fixture(item_count: usize) -> Fixture {
    const COLUMNS: usize = 32;
    let mut tree = BenchTree::default();
    tree.nodes.reserve(item_count + 1);
    tree.session.reserve(item_count + 1);
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
            columns: template(vec![px(16.0); COLUMNS]),
            auto_rows: implicit(vec![px(12.0)]),
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
    tree.nodes.reserve(item_count + 1);
    tree.session.reserve(item_count + 1);
    let mut children = Vec::with_capacity(item_count);
    for index in 0..item_count {
        // Alternating wide items repeatedly strand short holes which later
        // one-track items can backfill in dense mode.
        let span_length = match index % 8 {
            0 | 3 => 7,
            1 | 5 => 5,
            _ => 1,
        };
        let item_style = ItemData {
            column_end: span(span_length),
            ..ItemData::default()
        };
        children.push(tree.push_leaf(fixed_box(6.0, 6.0), item_style, intrinsic(6.0, 6.0, 6.0)));
    }
    let probe_a = children[item_count / 2];
    let probe_b = children[item_count - 1];
    let root = tree.push_grid(
        ContainerData {
            columns: template(vec![px(20.0); COLUMNS]),
            auto_rows: implicit(vec![px(12.0)]),
            auto_flow: GridAutoFlow::ROW | GridAutoFlow::DENSE,
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
    tree.nodes.reserve(ITEMS + 1);
    tree.session.reserve(ITEMS + 1);
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
            columns: template(columns),
            auto_rows: implicit(vec![px(14.0)]),
            gap: Size::new(gap_px(2.0), gap_px(1.0)),
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

#[inline]
fn gap_px(value: f32) -> NonNegativeLengthPercentageOrNormal {
    NonNegativeLengthPercentageOrNormal::LengthPercentage(NonNegative(
        LengthPercentage::new_length(Length::new(value)),
    ))
}

fn intrinsic_spans_fixture() -> Fixture {
    const COLUMNS: usize = 12;
    const ITEMS: usize = 256;
    let mut tree = BenchTree::default();
    tree.nodes.reserve(ITEMS + 1);
    tree.session.reserve(ITEMS + 1);
    let mut children = Vec::with_capacity(ITEMS);
    for index in 0..ITEMS {
        let span_length = 2 + index % 4;
        let min_width = 14.0 + small_f32(index % 13);
        let max_width = min_width + 40.0 + small_f32(index % 29);
        children.push(tree.push_leaf(
            BoxStyle::default(),
            ItemData {
                column_end: span(span_length),
                ..ItemData::default()
            },
            intrinsic(min_width, max_width, 10.0 + small_f32(index % 5)),
        ));
    }
    let probe_a = children[ITEMS / 2];
    let probe_b = children[ITEMS - 1];
    let root = tree.push_grid(
        ContainerData {
            columns: template(vec![intrinsic_track(); COLUMNS]),
            auto_rows: implicit(vec![auto_track()]),
            gap: Size::new(gap_px(3.0), gap_px(2.0)),
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
    tree.nodes.reserve(track_count + 1);
    tree.session.reserve(track_count + 1);
    let mut children = Vec::with_capacity(track_count);
    for span_length in 1..=track_count {
        let min_width = 6.0 + small_f32(span_length % 17);
        let max_width = min_width + small_f32(span_length) * 0.75;
        children.push(tree.push_leaf(
            BoxStyle::default(),
            ItemData {
                column_end: span(span_length),
                ..ItemData::default()
            },
            intrinsic(min_width, max_width, 8.0),
        ));
    }
    let probe_a = children[track_count / 2];
    let probe_b = children[track_count - 1];
    let root = tree.push_grid(
        ContainerData {
            columns: template(vec![intrinsic_track(); track_count]),
            auto_rows: implicit(vec![px(10.0)]),
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
    tree.nodes.reserve(track_count + 1);
    tree.session.reserve(track_count + 1);
    let mut children = Vec::with_capacity(track_count);
    for index in 0..track_count {
        let threshold = 4.0 + small_f32(index);
        children.push(tree.push_leaf(
            BoxStyle::default(),
            ItemData {
                row_start: line(1),
                row_end: line(2),
                column_start: line(index + 1),
                column_end: line(index + 2),
                order: 0,
            },
            intrinsic(threshold, threshold, 8.0),
        ));
    }
    let probe_a = children[track_count / 2];
    let probe_b = children[track_count - 1];
    let root = tree.push_grid(
        ContainerData {
            rows: template(vec![px(12.0)]),
            columns: template(vec![fr(1.0); track_count]),
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
    tree.nodes.reserve(INNER_GRIDS * (LEAVES_PER_GRID + 1) + 1);
    tree.session
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
                columns: template(vec![fr(1.0); 8]),
                auto_rows: implicit(vec![px(18.0)]),
                gap: Size::new(gap_px(1.0), gap_px(1.0)),
                ..ContainerData::default()
            },
            ItemData::default(),
            leaves,
        ));
    }
    let root = tree.push_grid(
        ContainerData {
            rows: template(vec![fr(1.0); 4]),
            columns: template(vec![fr(1.0); 4]),
            gap: Size::new(gap_px(4.0), gap_px(4.0)),
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
    dirty_leaf: usize,
    dirty_ancestor: usize,
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
            .nodes
            .iter()
            .position(|node| node.children.contains(&dirty_leaf))
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
        self.fixture.tree.nodes[self.dirty_leaf].intrinsic.max.width =
            if self.wide { 24.0 } else { 16.0 };
        self.fixture.tree.node(self.dirty_leaf).cache_clear();
        self.fixture.tree.node(self.dirty_ancestor).cache_clear();
        self.fixture.tree.node(self.fixture.root).cache_clear();
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
