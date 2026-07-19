//! Protocol conformance: a minimal but *complete* host implementing the
//! neutron-star node protocol, proving [`LayoutNode`] is implementable over
//! plain storage with zero `dyn`, zero allocation at the boundary, and zero
//! engine-side state.
//!
//! The style traits speak stylo computed values directly: the host stores
//! stylo types and hands them out per accessor — no engine-side style
//! vocabulary, no calc callback (stylo `LengthPercentage` self-resolves).
//!
//! Runtime tests exercise traversal, style/value plumbing, and shared
//! machinery. Algorithm behavior lives in `tests/flexbox.rs`, `tests/grid.rs`,
//! `tests/linear.rs`, and `tests/relative.rs`; this host proves their complete
//! protocol surface is implementable.

use std::cell::{Cell, RefCell};
use std::fmt;

use neutron_star::cache::Cache;
use neutron_star::compute::{
    compute_absolute_layout, compute_cached_layout, compute_leaf_layout, compute_root_layout,
    hide_subtree, round_layout,
};
use neutron_star::prelude::*;
use style_traits::values::specified::AllowedNumericType;
use stylo::Zero;
use stylo::computed_values::{relative_center, relative_layout_once, visibility};
use stylo::values::computed::length_percentage::{CalcNode, ComputedLeaf};
use stylo::values::computed::{
    Display, GridLine, GridTemplateComponent, ImplicitGridTracks, Length, LengthPercentage, Margin,
    NonNegativeLengthPercentage, NonNegativeNumber, Percentage, PositionProperty,
    Size as StyleSize,
};
use stylo::values::generics::NonNegative;
use stylo::values::generics::grid::{
    ImplicitGridTracks as GenericImplicitGridTracks, RepeatCount, TrackBreadth, TrackList,
    TrackListValue, TrackRepeat, TrackSize,
};

/// `<length>` in CSS pixels.
fn px(value: f32) -> LengthPercentage {
    LengthPercentage::new_length(Length::new(value))
}

/// Non-negative `<length>` (padding).
fn npx(value: f32) -> NonNegativeLengthPercentage {
    NonNegative(px(value))
}

/// `calc(<length> + <percentage>)`; `percentage` is a fraction (`0.05` = 5%).
fn calc_lp(length: f32, percentage: f32) -> NonNegativeLengthPercentage {
    NonNegative(LengthPercentage::new_calc(
        CalcNode::Sum(
            vec![
                CalcNode::Leaf(ComputedLeaf::Length(Length::new(length))),
                CalcNode::Leaf(ComputedLeaf::Percentage(Percentage(percentage))),
            ]
            .into(),
        ),
        AllowedNumericType::NonNegative,
    ))
}

/// A fixed-breadth track sizing function.
fn fixed_track(value: f32) -> TrackSize<LengthPercentage> {
    TrackSize::Breadth(TrackBreadth::Breadth(px(value)))
}

/// A `<flex>` (`fr`) track sizing function.
fn fr_track(value: f32) -> TrackSize<LengthPercentage> {
    TrackSize::Breadth(TrackBreadth::Flex(stylo::values::generics::grid::Flex(
        value,
    )))
}

/// A template track list from plain tracks (no repetitions; empty line
/// names honoring the `line_names.len() == values.len() + 1` invariant).
fn track_list(tracks: Vec<TrackSize<LengthPercentage>>) -> GridTemplateComponent {
    let values: Vec<TrackListValue<LengthPercentage, i32>> =
        tracks.into_iter().map(TrackListValue::TrackSize).collect();
    let count = values.len();
    GridTemplateComponent::TrackList(Box::new(TrackList {
        auto_repeat_index: usize::MAX,
        values: values.into(),
        line_names: vec![stylo::OwnedSlice::default(); count + 1].into(),
    }))
}

/// The host's own display vocabulary — deliberately *not* an engine type
/// (dispatch belongs to the host; see the `compute` module docs). The engine
/// consumes only [`CoreStyle::display`]'s `is_none` projection.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
enum MockDisplay {
    #[default]
    Leaf,
    Hidden,
}

#[derive(Debug, Clone)]
struct MockStyle {
    display: MockDisplay,
    auto_horizontal_margin: bool,
    size: Size<StyleSize>,
    padding: Edges<NonNegativeLengthPercentage>,
    flex_grow: NonNegativeNumber,
    grid_column: Line<GridLine>,
    template_columns: GridTemplateComponent,
    implicit_tracks: ImplicitGridTracks,
}

impl Default for MockStyle {
    fn default() -> Self {
        Self {
            display: MockDisplay::Leaf,
            auto_horizontal_margin: false,
            size: Size::new(StyleSize::auto(), StyleSize::auto()),
            padding: Edges::uniform(NonNegative(LengthPercentage::zero())),
            flex_grow: NonNegativeNumber::from(0.0),
            grid_column: Line::new(GridLine::auto(), GridLine::auto()),
            template_columns: GridTemplateComponent::None,
            implicit_tracks: GenericImplicitGridTracks(stylo::OwnedSlice::default()),
        }
    }
}

impl CoreStyle for MockStyle {
    fn display(&self) -> Display {
        if self.display == MockDisplay::Hidden {
            Display::None
        } else {
            Display::Flex
        }
    }

    fn size(&self) -> Size<StyleSize> {
        self.size.clone()
    }

    fn padding(&self) -> Edges<NonNegativeLengthPercentage> {
        self.padding.clone()
    }

    fn margin(&self) -> Edges<Margin> {
        if self.auto_horizontal_margin {
            Edges {
                left: Margin::Auto,
                right: Margin::Auto,
                top: Margin::zero(),
                bottom: Margin::zero(),
            }
        } else {
            Edges::uniform(Margin::zero())
        }
    }
}

impl FlexContainerStyle for MockStyle {}

impl FlexItemStyle for MockStyle {
    fn flex_grow(&self) -> NonNegativeNumber {
        self.flex_grow
    }
}

impl RelativeContainerStyle for MockStyle {}
impl RelativeItemStyle for MockStyle {}
impl LinearContainerStyle for MockStyle {}
impl LinearItemStyle for MockStyle {}

impl GridContainerStyle for MockStyle {
    fn grid_template_rows(&self) -> &GridTemplateComponent {
        const NONE: &GridTemplateComponent = &GridTemplateComponent::None;
        NONE
    }

    fn grid_template_columns(&self) -> &GridTemplateComponent {
        &self.template_columns
    }

    fn grid_auto_rows(&self) -> &ImplicitGridTracks {
        &self.implicit_tracks
    }

    fn grid_auto_columns(&self) -> &ImplicitGridTracks {
        &self.implicit_tracks
    }
}

impl GridItemStyle for MockStyle {
    fn grid_column_start(&self) -> GridLine {
        self.grid_column.start.clone()
    }

    fn grid_column_end(&self) -> GridLine {
        self.grid_column.end.clone()
    }
}

/// Immutable per-node data: style and topology, fixed for the layout epoch.
#[derive(Debug, Default)]
struct MockSourceNode {
    style: MockStyle,
    children: Vec<usize>,
}

/// Per-node layout slots, written through [`MockRef`] handles. Layout is
/// single-threaded, so `Cell`/`RefCell` interior mutability is the whole
/// synchronization story — the protocol has no `&mut`.
#[derive(Debug, Default)]
struct MockSessionNode {
    unrounded: Cell<Layout>,
    finalized: Cell<Layout>,
    /// Static position recorded for `PositionProperty::Fixed` children.
    static_position: Cell<Point<f32>>,
    cache: RefCell<Cache>,
}

/// The one host tree: immutable node data plus parallel interior-mutable
/// session slots. Builders mutate it (`&mut self`); layout only ever sees
/// `&MockTree` through [`MockRef`] handles and writes through the slots.
#[derive(Debug, Default)]
struct MockTree {
    nodes: Vec<MockSourceNode>,
    session: Vec<MockSessionNode>,
    /// Instrumentation: every node whose cache the engine cleared, in order.
    invalidated: RefCell<Vec<usize>>,
}

impl MockTree {
    fn push(&mut self, style: MockStyle, children: Vec<usize>) -> usize {
        debug_assert_eq!(self.nodes.len(), self.session.len());
        let id = self.nodes.len();
        self.nodes.push(MockSourceNode { style, children });
        self.session.push(MockSessionNode::default());
        id
    }

    /// Resolves a builder-returned id to a borrowed node handle.
    fn node(&self, id: usize) -> MockRef<'_> {
        MockRef {
            tree: self,
            index: id,
        }
    }

    /// Dispatches layout on `id` — the entry point tests use directly.
    fn compute_child_layout(&self, id: usize, input: LayoutInput) -> LayoutOutput {
        self.node(id).compute_child_layout(input)
    }

    /// The interior-mutable session slots of one node.
    fn session_node(&self, id: usize) -> &MockSessionNode {
        &self.session[id]
    }

    fn unrounded_layout(&self, id: usize) -> Layout {
        self.session_node(id).unrounded.get()
    }

    fn final_layout(&self, id: usize) -> Layout {
        self.session_node(id).finalized.get()
    }
}

/// The `Copy` node handle: a borrow of the tree plus a node index.
#[derive(Clone, Copy)]
struct MockRef<'t> {
    tree: &'t MockTree,
    index: usize,
}

impl fmt::Debug for MockRef<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_tuple("MockRef").field(&self.index).finish()
    }
}

impl<'t> MockRef<'t> {
    fn source(self) -> &'t MockSourceNode {
        &self.tree.nodes[self.index]
    }

    fn slots(self) -> &'t MockSessionNode {
        &self.tree.session[self.index]
    }
}

struct MockChildren<'t> {
    tree: &'t MockTree,
    ids: std::slice::Iter<'t, usize>,
}

impl<'t> Iterator for MockChildren<'t> {
    type Item = MockRef<'t>;

    fn next(&mut self) -> Option<MockRef<'t>> {
        let index = *self.ids.next()?;
        Some(MockRef {
            tree: self.tree,
            index,
        })
    }
}

impl<'t> LayoutNode for MockRef<'t> {
    type Style = &'t MockStyle;
    type ChildIter = MockChildren<'t>;

    fn children(self) -> MockChildren<'t> {
        MockChildren {
            tree: self.tree,
            ids: self.source().children.iter(),
        }
    }

    fn child_count(self) -> usize {
        self.source().children.len()
    }

    fn style(self) -> &'t MockStyle {
        &self.source().style
    }

    fn compute_child_layout(self, input: LayoutInput) -> LayoutOutput {
        let style = self.style();
        if style.display().is_none() {
            hide_subtree(self);
            return LayoutOutput::HIDDEN;
        }

        compute_cached_layout(self, input, |handle, input| match handle.style().display {
            MockDisplay::Hidden => unreachable!("handled before the cache boundary"),
            MockDisplay::Leaf => {
                LayoutOutput::new(input.known_dimensions.unwrap_or(Size::ZERO), Size::ZERO)
            }
        })
    }

    fn set_unrounded_layout(self, layout: &Layout) {
        self.slots().unrounded.set(*layout);
    }

    fn unrounded_layout(self) -> Layout {
        self.slots().unrounded.get()
    }

    fn set_final_layout(self, layout: &Layout) {
        self.slots().finalized.set(*layout);
    }

    fn set_static_position(self, static_position: Point<f32>) {
        self.slots().static_position.set(static_position);
    }

    fn cache_get(self, input: LayoutInput) -> Option<LayoutOutput> {
        self.slots().cache.borrow().get(input)
    }

    fn cache_store(self, input: LayoutInput, output: LayoutOutput) {
        self.slots().cache.borrow_mut().store(input, output);
    }

    fn cache_clear(self) {
        self.slots().cache.borrow_mut().clear();
        self.tree.invalidated.borrow_mut().push(self.index);
    }
}

fn leaf_tree() -> (MockTree, usize) {
    let mut tree = MockTree::default();
    let a = tree.push(MockStyle::default(), vec![]);
    let b = tree.push(MockStyle::default(), vec![]);
    let root = tree.push(MockStyle::default(), vec![a, b]);
    (tree, root)
}

#[test]
fn traversal_over_host_storage() {
    let (tree, root) = leaf_tree();
    let root_handle = tree.node(root);
    assert_eq!(root_handle.child_count(), 2);
    assert_eq!(root_handle.children().count(), 2);
    // The iterator hands out handles in document order, straight from host
    // storage.
    let ids: Vec<usize> = root_handle.children().map(|child| child.index).collect();
    assert_eq!(ids, tree.nodes[root].children);
}

#[test]
fn style_views_serve_initial_defaults() {
    let style = MockStyle::default();
    // Through the handle's borrowed view type, as the engine will consume it.
    let view: <MockRef<'_> as LayoutNode>::Style = &style;
    // The CSS initial value; Lynx hosts compute their default `relative`
    // (which means CSS `static`) in their own style system.
    assert_eq!(view.position(), PositionProperty::Static);
    assert_eq!(view.visibility(), visibility::T::Visible);
    assert!(matches!(view.size().width, StyleSize::Auto));
    assert_eq!(FlexItemStyle::order(&view), 0);
    // Lynx's `relative-layout-once` initial value is `true` (the fork's
    // initial, adopted by the protocol default).
    assert_eq!(
        RelativeContainerStyle::relative_layout_once(&view),
        relative_layout_once::T::True
    );
    // `-1` is the reserved "no reference" relative-layout sentinel.
    assert_eq!(RelativeItemStyle::relative_id(&view), -1);
    assert_eq!(
        RelativeItemStyle::relative_center(&view),
        relative_center::T::None
    );
    assert_eq!(GridItemStyle::grid_column_start(&view), GridLine::auto());
    assert_eq!(GridItemStyle::grid_column_end(&view), GridLine::auto());
    assert_eq!(GridItemStyle::grid_row_start(&view), GridLine::auto());
    assert_eq!(GridItemStyle::grid_row_end(&view), GridLine::auto());
}

#[test]
fn grid_template_borrow_serves_stylo_track_lists() {
    // [100px, repeat(auto-fill, [1fr, auto])] straight from host storage:
    // a single track plus an auto-repeat group, exactly as stylo computes it.
    let repeat = TrackRepeat {
        count: RepeatCount::AutoFill,
        line_names: vec![stylo::OwnedSlice::default(); 3].into(),
        track_sizes: vec![fr_track(1.0), TrackSize::default()].into(),
    };
    let style = MockStyle {
        template_columns: GridTemplateComponent::TrackList(Box::new(TrackList {
            auto_repeat_index: 1,
            values: vec![
                TrackListValue::TrackSize(fixed_track(100.0)),
                TrackListValue::TrackRepeat(repeat),
            ]
            .into(),
            line_names: vec![stylo::OwnedSlice::default(); 3].into(),
        })),
        ..MockStyle::default()
    };

    let template = GridContainerStyle::grid_template_columns(&style);
    let GridTemplateComponent::TrackList(list) = template else {
        panic!("expected a track list, got {template:?}");
    };
    assert!(list.has_auto_repeat());
    assert_eq!(list.values.len(), 2);
    match &list.values[0] {
        TrackListValue::TrackSize(track) => assert_eq!(*track, fixed_track(100.0)),
        other @ TrackListValue::TrackRepeat(_) => panic!("expected a single track, got {other:?}"),
    }
    match &list.values[1] {
        TrackListValue::TrackRepeat(repetition) => {
            assert_eq!(repetition.count, RepeatCount::AutoFill);
            // Auto-fill/auto-fit need the per-repetition track count up
            // front to solve the repetition count.
            assert_eq!(repetition.track_sizes.len(), 2);
        }
        other @ TrackListValue::TrackSize(_) => panic!("expected a repetition, got {other:?}"),
    }
    // An axis without explicit tracks serves `None`.
    assert!(matches!(
        GridContainerStyle::grid_template_rows(&style),
        GridTemplateComponent::None
    ));
    // Empty implicit track lists mean `auto` (the engine synthesizes).
    assert!(GridContainerStyle::grid_auto_rows(&style).0.is_empty());
}

#[test]
fn grid_track_view_remains_live_across_recursive_session_layout() {
    let mut tree = MockTree::default();
    let child = tree.push(MockStyle::default(), vec![]);
    let root = tree.push(
        MockStyle {
            template_columns: track_list(vec![fixed_track(10.0), fr_track(1.0)]),
            ..MockStyle::default()
        },
        vec![child],
    );

    let style = tree.node(root).style();
    let GridTemplateComponent::TrackList(tracks) = style.grid_template_columns() else {
        panic!("expected a track list");
    };
    match &tracks.values[0] {
        TrackListValue::TrackSize(track) => assert_eq!(*track, fixed_track(10.0)),
        other @ TrackListValue::TrackRepeat(_) => {
            panic!("expected first single track, got {other:?}")
        }
    }

    // The style view borrows the tree's immutable side through the handle.
    // Recursive layout writes only through host-owned interior-mutable
    // per-node slots — the protocol has no `&mut` anywhere — so the borrow
    // checker never sees a conflict and the borrowed stylo track list stays
    // live across the recursion.
    let output = tree.compute_child_layout(
        child,
        LayoutInput::perform_layout(
            Size::new(Some(25.0), Some(10.0)),
            Size::NONE,
            Size::MAX_CONTENT,
        ),
    );
    assert_eq!(output.size, Size::new(25.0, 10.0));
    match &tracks.values[1] {
        TrackListValue::TrackSize(track) => assert_eq!(*track, fr_track(1.0)),
        other @ TrackListValue::TrackRepeat(_) => {
            panic!("expected second single track, got {other:?}")
        }
    }
}

#[test]
fn leaf_dispatch_round_trips_layout_io() {
    let (tree, root) = leaf_tree();
    let child = tree.node(root).children().next().unwrap();
    let input =
        LayoutInput::perform_layout(Size::new(Some(40.0), None), Size::NONE, Size::MAX_CONTENT);
    let output = child.compute_child_layout(input);
    assert_eq!(output.size, Size::new(40.0, 0.0));

    // `Layout` is #[non_exhaustive]: construct via default + field writes.
    let mut layout = Layout::with_order(0);
    layout.size = output.size;
    child.set_unrounded_layout(&layout);
    assert_eq!(child.unrounded_layout().size, Size::new(40.0, 0.0));
}

#[test]
fn static_position_round_trips_through_the_tree() {
    // For `PositionProperty::Fixed` children the formatting parent records
    // a static position instead of a layout; the host stores it for the
    // positioned pass.
    let (tree, root) = leaf_tree();
    let child = tree.node(root).children().nth(1).unwrap();
    child.set_static_position(Point::new(12.5, 7.0));
    assert_eq!(child.slots().static_position.get(), Point::new(12.5, 7.0));
}

#[test]
fn embeddable_cache_lifecycle() {
    let mut cache = Cache::new();
    assert!(cache.is_empty());
    cache.clear();
    assert!(cache.is_empty());
    assert_eq!(cache, Cache::default());
}

#[test]
fn compute_root_layout_stores_the_root_box() {
    let (tree, root) = leaf_tree();
    compute_root_layout(
        tree.node(root),
        Size::new(
            AvailableSpace::Definite(100.0),
            AvailableSpace::Definite(80.0),
        ),
    );
    assert_eq!(tree.unrounded_layout(root).location, Point::ZERO);
    assert_eq!(tree.unrounded_layout(root).size, Size::ZERO);
}

#[test]
#[allow(clippy::float_cmp)] // Exact halves of an integer available size.
fn compute_root_layout_resolves_horizontal_auto_margins() {
    let mut tree = MockTree::default();
    let root = tree.push(
        MockStyle {
            auto_horizontal_margin: true,
            ..MockStyle::default()
        },
        vec![],
    );
    compute_root_layout(
        tree.node(root),
        Size::new(
            AvailableSpace::Definite(100.0),
            AvailableSpace::Definite(20.0),
        ),
    );
    assert_eq!(tree.unrounded_layout(root).margin.left, 50.0);
    assert_eq!(tree.unrounded_layout(root).margin.right, 50.0);
    assert_eq!(tree.unrounded_layout(root).location.x, 50.0);
}

#[test]
fn compute_root_layout_preserves_a_hidden_zero_box() {
    let mut tree = MockTree::default();
    let root = tree.push(
        MockStyle {
            display: MockDisplay::Hidden,
            auto_horizontal_margin: true,
            ..MockStyle::default()
        },
        vec![],
    );
    compute_root_layout(
        tree.node(root),
        Size::new(
            AvailableSpace::Definite(100.0),
            AvailableSpace::Definite(20.0),
        ),
    );
    assert_eq!(tree.unrounded_layout(root), Layout::default());
}

#[test]
fn explicit_hidden_cleanup_clears_stale_geometry() {
    let (mut tree, root) = leaf_tree();
    let hidden = tree.push(
        MockStyle {
            display: MockDisplay::Hidden,
            ..MockStyle::default()
        },
        vec![root],
    );
    // Seed stale geometry through the interior-mutable slots (get-modify-set:
    // the slots are `Cell`s).
    let mut stale = tree.session_node(hidden).unrounded.get();
    stale.size = Size::new(50.0, 20.0);
    tree.session_node(hidden).unrounded.set(stale);
    let mut stale = tree.session_node(root).unrounded.get();
    stale.size = Size::new(40.0, 10.0);
    tree.session_node(root).unrounded.set(stale);

    hide_subtree(tree.node(hidden));
    assert_eq!(tree.unrounded_layout(hidden), Layout::default());
    assert_eq!(tree.unrounded_layout(root), Layout::default());
    assert!(tree.invalidated.borrow().contains(&hidden));
    assert!(tree.invalidated.borrow().contains(&root));
}

#[test]
fn compute_leaf_layout_uses_the_host_measurement() {
    let style = MockStyle {
        size: Size::new(StyleSize::LengthPercentage(npx(50.0)), StyleSize::auto()),
        padding: Edges::uniform(npx(5.0)),
        ..MockStyle::default()
    };
    let input = LayoutInput::perform_layout(
        Size::NONE,
        Size::new(Some(100.0), Some(100.0)),
        Size::new(
            AvailableSpace::Definite(100.0),
            AvailableSpace::Definite(100.0),
        ),
    );
    let mut seen = None;
    let output = {
        let mut measurer = FnLeafMeasurer::new(|request: LeafMeasureInput| {
            seen = Some(request);
            LeafMetrics::new(Size::new(
                request.known_dimensions.width.unwrap_or(31.0),
                17.0,
            ))
            .with_first_baselines(Point::new(None, Some(12.0)))
        });
        compute_leaf_layout(input, &style, &mut measurer)
    };

    assert_eq!(
        seen,
        Some(LeafMeasureInput::new(
            Size::new(Some(50.0), None),
            Size::new(
                AvailableSpace::Definite(50.0),
                AvailableSpace::Definite(90.0),
            ),
            LayoutGoal::Commit,
        ))
    );
    assert_eq!(output.size, Size::new(60.0, 27.0));
    assert_eq!(output.content_size, Size::new(60.0, 27.0));
    assert_eq!(output.first_baselines.y, Some(17.0));
}

#[test]
fn calc_padding_resolves_through_stylo_style_values() {
    // Replaces the deleted `CalcHandle`/`resolve_calc` plumbing: a stylo
    // `calc()` mixing length and percentage self-resolves inside the engine
    // against the layout-time percentage basis (the parent width, here 200
    // CSS px, for padding on every edge): calc(10px + 5%) = 20px per side.
    let style = MockStyle {
        size: Size::new(StyleSize::LengthPercentage(npx(50.0)), StyleSize::auto()),
        padding: Edges::uniform(calc_lp(10.0, 0.05)),
        ..MockStyle::default()
    };
    let input = LayoutInput::perform_layout(
        Size::NONE,
        Size::new(Some(200.0), Some(100.0)),
        Size::new(
            AvailableSpace::Definite(200.0),
            AvailableSpace::Definite(100.0),
        ),
    );
    let output = {
        let mut measurer = FnLeafMeasurer::new(|request: LeafMeasureInput| {
            LeafMetrics::new(Size::new(
                request.known_dimensions.width.unwrap_or(31.0),
                17.0,
            ))
        });
        compute_leaf_layout(input, &style, &mut measurer)
    };

    // 50px content box + 20px calc padding per side.
    assert_eq!(output.size, Size::new(90.0, 57.0));
}

struct RetainedLeafArtifact {
    metrics: LeafMetrics,
    paint_data: Vec<u8>,
}

struct BorrowedLeafMeasurement<'a>(&'a RetainedLeafArtifact);

impl LeafMeasurement for BorrowedLeafMeasurement<'_> {
    fn size(&self) -> Size<f32> {
        self.0.metrics.size
    }

    fn first_baselines(&self) -> Point<Option<f32>> {
        self.0.metrics.first_baselines
    }
}

struct BorrowingLeafMeasurer {
    artifact: RetainedLeafArtifact,
    last_input: Option<LeafMeasureInput>,
}

impl LeafMeasurer for BorrowingLeafMeasurer {
    type Measurement<'a>
        = BorrowedLeafMeasurement<'a>
    where
        Self: 'a;

    fn measure(&mut self, input: LeafMeasureInput) -> Self::Measurement<'_> {
        self.last_input = Some(input);
        BorrowedLeafMeasurement(&self.artifact)
    }
}

#[test]
fn compute_leaf_layout_accepts_a_non_clone_borrowed_measurement() {
    let input = LayoutInput::compute_size(
        Size::NONE,
        Size::NONE,
        Size::MAX_CONTENT,
        RequestedAxis::Both,
    );
    let mut measurer = BorrowingLeafMeasurer {
        artifact: RetainedLeafArtifact {
            metrics: LeafMetrics::new(Size::new(31.0, 17.0))
                .with_first_baselines(Point::new(None, Some(11.0))),
            paint_data: vec![1, 2, 3],
        },
        last_input: None,
    };

    let output = compute_leaf_layout(input, &MockStyle::default(), &mut measurer);

    assert_eq!(output.size, Size::new(31.0, 17.0));
    assert_eq!(output.first_baselines.y, Some(11.0));
    assert_eq!(
        measurer.last_input,
        Some(LeafMeasureInput::new(
            Size::NONE,
            Size::MAX_CONTENT,
            LayoutGoal::Measure(RequestedAxis::Both),
        ))
    );
    assert_eq!(measurer.artifact.paint_data, [1, 2, 3]);
}

#[test]
fn compute_cached_layout_runs_an_uncached_dispatch() {
    let (tree, root) = leaf_tree();
    let calls = Cell::new(0);
    let output = compute_cached_layout(tree.node(root), LayoutInput::default(), |node, input| {
        calls.set(calls.get() + 1);
        node.compute_child_layout(input)
    });
    assert_eq!(calls.get(), 1);
    assert_eq!(output, LayoutOutput::HIDDEN);
}

#[test]
fn compute_absolute_layout_uses_the_static_position() {
    let (tree, root) = leaf_tree();
    let hoisted = tree.node(root).children().next().unwrap();
    let layout = compute_absolute_layout(hoisted, Size::new(800.0, 600.0), Point::new(12.5, 7.0));
    assert_eq!(layout.location, Point::new(12.5, 7.0));
    assert_eq!(layout.size, Size::ZERO);
}

#[test]
fn round_layout_snaps_on_the_device_pixel_grid() {
    let (tree, root) = leaf_tree();
    let child = tree.nodes[root].children[0];
    let mut root_layout = Layout::default();
    root_layout.location = Point::new(0.24, 0.24);
    root_layout.size = Size::new(10.26, 10.26);
    tree.session_node(root).unrounded.set(root_layout);
    let mut child_layout = Layout::default();
    child_layout.location = Point::new(0.26, 0.26);
    child_layout.size = Size::new(4.74, 4.74);
    tree.session_node(child).unrounded.set(child_layout);

    round_layout(tree.node(root), 2.0);
    assert_eq!(tree.final_layout(root).location, Point::ZERO);
    assert_eq!(tree.final_layout(root).size, Size::new(10.5, 10.5));
    assert_eq!(tree.final_layout(child).location, Point::new(0.5, 0.5));
    assert_eq!(tree.final_layout(child).size, Size::new(4.5, 4.5));
}

#[test]
fn round_layout_uses_css_positive_infinity_tie_breaking() {
    let (tree, root) = leaf_tree();
    let mut root_layout = Layout::default();
    // At DPR 2 these become -1.5 and +1.5 device pixels. CSS nearest-
    // integer rounding chooses the upper integer in both cases: -1 and +2.
    root_layout.location = Point::new(-0.75, 0.75);
    tree.session_node(root).unrounded.set(root_layout);

    round_layout(tree.node(root), 2.0);

    assert_eq!(tree.final_layout(root).location, Point::new(-0.5, 1.0));
}

#[test]
fn embeddable_cache_round_trips_a_complete_key() {
    let mut cache = Cache::new();
    let input = LayoutInput::compute_size(
        Size::new(Some(20.0), None),
        Size::new(Some(100.0), Some(80.0)),
        Size::new(AvailableSpace::Definite(20.0), AvailableSpace::MaxContent),
        RequestedAxis::Horizontal,
    );
    let output = LayoutOutput::new(Size::new(20.0, 12.0), Size::new(20.0, 12.0));
    cache.store(input, output);
    assert_eq!(cache.get(input), Some(output));

    let mut different_axis = input;
    different_axis.goal = LayoutGoal::Measure(RequestedAxis::Both);
    assert_eq!(cache.get(different_axis), None);

    assert_eq!(input.definite_dimensions, Size::new(true, false));
    let mut same_geometry_but_indefinite = input;
    same_geometry_but_indefinite.definite_dimensions.width = false;
    assert_eq!(cache.get(same_geometry_but_indefinite), None);
}
