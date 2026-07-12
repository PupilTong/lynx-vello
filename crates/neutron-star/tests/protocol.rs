//! Protocol conformance: a minimal but *complete* host implementing every
//! neutron-star trait, proving the protocol is implementable over plain
//! storage with zero `dyn`, zero allocation at the boundary, and zero
//! engine-side state.
//!
//! Runtime tests exercise traversal, style/value plumbing, and the shared L1
//! machinery. Flexbox behavior has its own `tests/flexbox.rs` suite; Grid's
//! algorithm remains pending while this host proves its protocol is
//! implementable.

use neutron_star::cache::Cache;
use neutron_star::compute::{
    FnLeafMeasurer, LeafMeasureInput, LeafMeasurement, LeafMeasurer, LeafMetrics,
    compute_absolute_layout, compute_cached_layout, compute_leaf_layout, compute_root_layout,
    hide_subtree, round_layout,
};
use neutron_star::prelude::*;
use neutron_star::style::{
    BoxGenerationMode, CalcHandle, Dimension, GridPlacement, GridTemplateComponent,
    LengthPercentage, LengthPercentageAuto, Position, RelativeCenter, RelativeContainerStyle,
    RelativeItemStyle, RelativeReference, RepetitionCount, TrackSizingFunction, Visibility,
};

/// The host's own display vocabulary — deliberately *not* an engine type
/// (dispatch belongs to the host; see the `compute` module docs).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
enum MockDisplay {
    #[default]
    Leaf,
    Hidden,
}

#[derive(Debug, Clone)]
struct MockRepetition {
    count: RepetitionCount,
    tracks: Vec<TrackSizingFunction>,
}

#[derive(Debug, Clone)]
enum MockTemplateComponent {
    Single(TrackSizingFunction),
    Repeat(MockRepetition),
}

impl neutron_star::style::GridTemplateRepetition for MockRepetition {
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

#[derive(Debug, Clone, Default)]
struct MockStyle {
    display: MockDisplay,
    auto_horizontal_margin: bool,
    size: Size<Dimension>,
    padding: Edges<LengthPercentage>,
    flex_grow: f32,
    grid_column: Line<GridPlacement>,
    template_columns: Vec<MockTemplateComponent>,
}

impl CoreStyle for MockStyle {
    fn box_generation_mode(&self) -> BoxGenerationMode {
        if self.display == MockDisplay::Hidden {
            BoxGenerationMode::None
        } else {
            BoxGenerationMode::Normal
        }
    }

    fn size(&self) -> Size<Dimension> {
        self.size
    }

    fn padding(&self) -> Edges<LengthPercentage> {
        self.padding
    }

    fn margin(&self) -> Edges<LengthPercentageAuto> {
        if self.auto_horizontal_margin {
            Edges {
                left: LengthPercentageAuto::Auto,
                right: LengthPercentageAuto::Auto,
                top: LengthPercentageAuto::ZERO,
                bottom: LengthPercentageAuto::ZERO,
            }
        } else {
            Edges::uniform(LengthPercentageAuto::ZERO)
        }
    }
}

impl FlexContainerStyle for MockStyle {}

impl FlexItemStyle for MockStyle {
    fn flex_grow(&self) -> f32 {
        self.flex_grow
    }
}

impl RelativeContainerStyle for MockStyle {}
impl RelativeItemStyle for MockStyle {}

fn to_component(component: &MockTemplateComponent) -> GridTemplateComponent<&MockRepetition> {
    match component {
        MockTemplateComponent::Single(track) => GridTemplateComponent::Single(*track),
        MockTemplateComponent::Repeat(repetition) => GridTemplateComponent::Repeat(repetition),
    }
}

const NO_COMPONENTS: &[MockTemplateComponent] = &[];
const NO_TRACKS: &[TrackSizingFunction] = &[];

impl GridContainerStyle for MockStyle {
    type Repetition<'a>
        = &'a MockRepetition
    where
        Self: 'a;
    type TemplateTracks<'a>
        = std::iter::Map<
        std::slice::Iter<'a, MockTemplateComponent>,
        fn(&'a MockTemplateComponent) -> GridTemplateComponent<&'a MockRepetition>,
    >
    where
        Self: 'a;
    type AutoTracks<'a>
        = std::iter::Copied<std::slice::Iter<'a, TrackSizingFunction>>
    where
        Self: 'a;

    fn grid_template_rows(&self) -> Self::TemplateTracks<'_> {
        NO_COMPONENTS.iter().map(to_component as _)
    }

    fn grid_template_columns(&self) -> Self::TemplateTracks<'_> {
        self.template_columns.iter().map(to_component as _)
    }

    fn grid_auto_rows(&self) -> Self::AutoTracks<'_> {
        NO_TRACKS.iter().copied()
    }

    fn grid_auto_columns(&self) -> Self::AutoTracks<'_> {
        NO_TRACKS.iter().copied()
    }
}

impl GridItemStyle for MockStyle {
    fn grid_column(&self) -> Line<GridPlacement> {
        self.grid_column
    }
}

#[derive(Debug, Default)]
struct MockSourceNode {
    style: MockStyle,
    children: Vec<NodeId>,
}

#[derive(Debug, Default)]
struct MockSource {
    nodes: Vec<MockSourceNode>,
}

impl MockSource {
    fn push(&mut self, style: MockStyle, children: Vec<NodeId>) -> NodeId {
        self.nodes.push(MockSourceNode { style, children });
        NodeId::from(self.nodes.len() - 1)
    }

    fn node(&self, id: NodeId) -> &MockSourceNode {
        &self.nodes[usize::from(id)]
    }
}

#[derive(Debug, Default)]
struct MockStateNode {
    unrounded: Layout,
    finalized: Layout,
    /// Static position recorded for `Position::AbsoluteHoisted` children.
    static_position: Point<f32>,
    cache: Cache,
}

#[derive(Debug, Default)]
struct MockSession {
    nodes: Vec<MockStateNode>,
    invalidated: Vec<NodeId>,
}

impl MockSession {
    fn push(&mut self) -> NodeId {
        self.nodes.push(MockStateNode::default());
        NodeId::from(self.nodes.len() - 1)
    }

    fn node(&self, id: NodeId) -> &MockStateNode {
        &self.nodes[usize::from(id)]
    }

    fn node_mut(&mut self, id: NodeId) -> &mut MockStateNode {
        &mut self.nodes[usize::from(id)]
    }
}

/// Construction-only facade. Layout consumes its fields as independent
/// immutable-source and mutable-session objects.
#[derive(Debug, Default)]
struct MockHost {
    source: MockSource,
    session: MockSession,
}

impl MockHost {
    fn push(&mut self, style: MockStyle, children: Vec<NodeId>) -> NodeId {
        let source_id = self.source.push(style, children);
        let session_id = self.session.push();
        assert_eq!(source_id, session_id);
        source_id
    }
}

impl TraverseTree for MockSource {
    type ChildIter<'a> = std::iter::Copied<std::slice::Iter<'a, NodeId>>;

    fn child_ids(&self, parent: NodeId) -> Self::ChildIter<'_> {
        self.node(parent).children.iter().copied()
    }

    fn child_count(&self, parent: NodeId) -> usize {
        self.node(parent).children.len()
    }

    fn child_id(&self, parent: NodeId, index: usize) -> NodeId {
        self.node(parent).children[index]
    }
}

impl LayoutSource for MockSource {
    type CoreStyle<'a> = &'a MockStyle;

    fn core_style(&self, node: NodeId) -> Self::CoreStyle<'_> {
        &self.node(node).style
    }

    fn resolve_calc(&self, _calc: CalcHandle, _basis: f32) -> f32 {
        unreachable!("mock styles never carry calc()")
    }
}

impl FlexSource for MockSource {
    type ContainerStyle<'a> = &'a MockStyle;
    type ItemStyle<'a> = &'a MockStyle;

    fn flex_container_style(&self, container: NodeId) -> Self::ContainerStyle<'_> {
        &self.node(container).style
    }

    fn flex_item_style(&self, item: NodeId) -> Self::ItemStyle<'_> {
        &self.node(item).style
    }
}

impl GridSource for MockSource {
    type ContainerStyle<'a> = &'a MockStyle;
    type ItemStyle<'a> = &'a MockStyle;

    fn grid_container_style(&self, container: NodeId) -> Self::ContainerStyle<'_> {
        &self.node(container).style
    }

    fn grid_item_style(&self, item: NodeId) -> Self::ItemStyle<'_> {
        &self.node(item).style
    }
}

impl RelativeSource for MockSource {
    type ContainerStyle<'a> = &'a MockStyle;
    type ItemStyle<'a> = &'a MockStyle;

    fn relative_container_style(&self, container: NodeId) -> Self::ContainerStyle<'_> {
        &self.node(container).style
    }

    fn relative_item_style(&self, item: NodeId) -> Self::ItemStyle<'_> {
        &self.node(item).style
    }
}

impl LayoutState for MockSession {
    fn set_unrounded_layout(&mut self, node: NodeId, layout: &Layout) {
        self.node_mut(node).unrounded = *layout;
    }

    fn set_static_position(&mut self, child: NodeId, static_position: Point<f32>) {
        self.node_mut(child).static_position = static_position;
    }
}

impl CacheState for MockSession {
    fn cache_get(&self, node: NodeId, input: LayoutInput) -> Option<LayoutOutput> {
        self.node(node).cache.get(input)
    }

    fn cache_store(&mut self, node: NodeId, input: LayoutInput, layout_output: LayoutOutput) {
        self.node_mut(node).cache.store(input, layout_output);
    }

    fn cache_clear(&mut self, node: NodeId) {
        self.node_mut(node).cache.clear();
        self.invalidated.push(node);
    }
}

impl LayoutSession<MockSource> for MockSession {
    fn compute_child_layout(
        &mut self,
        source: &MockSource,
        child: NodeId,
        input: LayoutInput,
    ) -> LayoutOutput {
        let source_node = source.node(child);
        if source_node.style.box_generation_mode() == BoxGenerationMode::None {
            hide_subtree(source, self, child);
            return LayoutOutput::HIDDEN;
        }

        compute_cached_layout(
            self,
            child,
            input,
            |_session, _child, input| match source_node.style.display {
                MockDisplay::Hidden => unreachable!("handled before the cache boundary"),
                MockDisplay::Leaf => {
                    LayoutOutput::new(input.known_dimensions.unwrap_or(Size::ZERO), Size::ZERO)
                }
            },
        )
    }
}

impl RoundState for MockSession {
    fn unrounded_layout(&self, node: NodeId) -> Layout {
        self.node(node).unrounded
    }

    fn set_final_layout(&mut self, node: NodeId, layout: &Layout) {
        self.node_mut(node).finalized = *layout;
    }
}

fn leaf_tree() -> (MockHost, NodeId) {
    let mut host = MockHost::default();
    let a = host.push(MockStyle::default(), vec![]);
    let b = host.push(MockStyle::default(), vec![]);
    let root = host.push(MockStyle::default(), vec![a, b]);
    (host, root)
}

#[test]
fn traversal_over_host_storage() {
    let (host, root) = leaf_tree();
    assert_eq!(host.source.child_count(root), 2);
    assert_eq!(host.source.child_ids(root).count(), 2);
    assert_eq!(
        host.source.child_id(root, 1),
        host.source.child_ids(root).nth(1).unwrap()
    );
}

#[test]
fn style_views_serve_css_initial_defaults() {
    let style = MockStyle::default();
    // Through the blanket `&S` view, as the engine will consume it.
    let view: <MockSource as LayoutSource>::CoreStyle<'_> = &style;
    assert_eq!(view.position(), Position::Relative);
    assert_eq!(view.visibility(), Visibility::Visible);
    assert!(view.size().width.is_auto());
    assert_eq!(FlexItemStyle::order(&view), 0);
    assert!(!RelativeContainerStyle::relative_layout_once(&view));
    assert_eq!(
        RelativeItemStyle::relative_id(&view),
        RelativeReference::NONE
    );
    assert_eq!(
        RelativeItemStyle::relative_center(&view),
        RelativeCenter::None
    );
    assert_eq!(
        GridItemStyle::grid_column(&view),
        Line::new(GridPlacement::Auto, GridPlacement::Auto)
    );
}

#[test]
fn grid_template_gats_iterate_without_allocation() {
    let style = MockStyle {
        template_columns: vec![
            MockTemplateComponent::Single(TrackSizingFunction::fixed(LengthPercentage::length(
                100.0,
            ))),
            MockTemplateComponent::Repeat(MockRepetition {
                count: RepetitionCount::AutoFill,
                tracks: vec![TrackSizingFunction::fr(1.0), TrackSizingFunction::AUTO],
            }),
        ],
        ..MockStyle::default()
    };

    let mut components = GridContainerStyle::grid_template_columns(&style);
    match components.next() {
        Some(GridTemplateComponent::Single(track)) => {
            assert_eq!(
                track,
                TrackSizingFunction::fixed(LengthPercentage::length(100.0))
            );
        }
        other => panic!("expected single track, got {other:?}"),
    }
    match components.next() {
        Some(GridTemplateComponent::Repeat(repetition)) => {
            assert_eq!(repetition.count(), RepetitionCount::AutoFill);
            // ExactSizeIterator + Clone: both required by the protocol for
            // repeat expansion.
            let tracks = repetition.tracks();
            assert_eq!(tracks.len(), 2);
            assert_eq!(tracks.clone().count(), 2);
        }
        other => panic!("expected repetition, got {other:?}"),
    }
    assert!(components.next().is_none());
    assert!(
        GridContainerStyle::grid_template_rows(&style)
            .next()
            .is_none()
    );
}

#[test]
fn grid_track_view_remains_live_across_recursive_session_layout() {
    let mut host = MockHost::default();
    let child = host.push(MockStyle::default(), vec![]);
    let first_track = TrackSizingFunction::fixed(LengthPercentage::length(10.0));
    let second_track = TrackSizingFunction::fr(1.0);
    let root = host.push(
        MockStyle {
            template_columns: vec![
                MockTemplateComponent::Single(first_track),
                MockTemplateComponent::Single(second_track),
            ],
            ..MockStyle::default()
        },
        vec![child],
    );

    let style = host.source.grid_container_style(root);
    let mut tracks = style.grid_template_columns();
    match tracks.next() {
        Some(GridTemplateComponent::Single(track)) => assert_eq!(track, first_track),
        other => panic!("expected first single track, got {other:?}"),
    }

    // The iterator borrows only the immutable source. Recursive layout may
    // mutate the separate session without invalidating the Grid style view.
    let output = host.session.compute_child_layout(
        &host.source,
        child,
        LayoutInput::perform_layout(
            Size::new(Some(25.0), Some(10.0)),
            Size::NONE,
            Size::MAX_CONTENT,
        ),
    );
    assert_eq!(output.size, Size::new(25.0, 10.0));
    match tracks.next() {
        Some(GridTemplateComponent::Single(track)) => assert_eq!(track, second_track),
        other => panic!("expected second single track, got {other:?}"),
    }
}

#[test]
fn leaf_dispatch_round_trips_layout_io() {
    let (mut host, root) = leaf_tree();
    let child = host.source.child_id(root, 0);
    let input =
        LayoutInput::perform_layout(Size::new(Some(40.0), None), Size::NONE, Size::MAX_CONTENT);
    let output = host
        .session
        .compute_child_layout(&host.source, child, input);
    assert_eq!(output.size, Size::new(40.0, 0.0));

    // `Layout` is #[non_exhaustive]: construct via default + field writes.
    let mut layout = Layout::with_order(0);
    layout.size = output.size;
    host.session.set_unrounded_layout(child, &layout);
    assert_eq!(
        host.session.unrounded_layout(child).size,
        Size::new(40.0, 0.0)
    );
}

#[test]
fn static_position_round_trips_through_the_tree() {
    // For `Position::AbsoluteHoisted` children the formatting parent records
    // a static position instead of a layout; the host stores it for the
    // positioned pass.
    let (mut host, root) = leaf_tree();
    let child = host.source.child_id(root, 1);
    host.session
        .set_static_position(child, Point::new(12.5, 7.0));
    assert_eq!(
        host.session.node(child).static_position,
        Point::new(12.5, 7.0)
    );
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
    let (mut host, root) = leaf_tree();
    compute_root_layout(
        &host.source,
        &mut host.session,
        root,
        Size::new(
            AvailableSpace::Definite(100.0),
            AvailableSpace::Definite(80.0),
        ),
    );
    assert_eq!(host.session.unrounded_layout(root).location, Point::ZERO);
    assert_eq!(host.session.unrounded_layout(root).size, Size::ZERO);
}

#[test]
#[allow(clippy::float_cmp)] // Exact halves of an integer available size.
fn compute_root_layout_resolves_horizontal_auto_margins() {
    let mut host = MockHost::default();
    let root = host.push(
        MockStyle {
            auto_horizontal_margin: true,
            ..MockStyle::default()
        },
        vec![],
    );
    compute_root_layout(
        &host.source,
        &mut host.session,
        root,
        Size::new(
            AvailableSpace::Definite(100.0),
            AvailableSpace::Definite(20.0),
        ),
    );
    assert_eq!(host.session.unrounded_layout(root).margin.left, 50.0);
    assert_eq!(host.session.unrounded_layout(root).margin.right, 50.0);
    assert_eq!(host.session.unrounded_layout(root).location.x, 50.0);
}

#[test]
fn compute_root_layout_preserves_a_hidden_zero_box() {
    let mut host = MockHost::default();
    let root = host.push(
        MockStyle {
            display: MockDisplay::Hidden,
            auto_horizontal_margin: true,
            ..MockStyle::default()
        },
        vec![],
    );
    compute_root_layout(
        &host.source,
        &mut host.session,
        root,
        Size::new(
            AvailableSpace::Definite(100.0),
            AvailableSpace::Definite(20.0),
        ),
    );
    assert_eq!(host.session.unrounded_layout(root), Layout::default());
}

#[test]
fn explicit_hidden_cleanup_clears_stale_geometry() {
    let (mut host, root) = leaf_tree();
    let hidden = host.push(
        MockStyle {
            display: MockDisplay::Hidden,
            ..MockStyle::default()
        },
        vec![root],
    );
    host.session.node_mut(hidden).unrounded.size = Size::new(50.0, 20.0);
    host.session.node_mut(root).unrounded.size = Size::new(40.0, 10.0);
    hide_subtree(&host.source, &mut host.session, hidden);
    assert_eq!(host.session.unrounded_layout(hidden), Layout::default());
    assert_eq!(host.session.unrounded_layout(root), Layout::default());
    assert!(host.session.invalidated.contains(&hidden));
    assert!(host.session.invalidated.contains(&root));
}

#[test]
fn compute_leaf_layout_uses_the_host_measurement() {
    let style = MockStyle {
        size: Size::new(Dimension::Length(50.0), Dimension::Auto),
        padding: Edges::uniform(LengthPercentage::length(5.0)),
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
        compute_leaf_layout(
            input,
            &style,
            |_, _| unreachable!("mock styles never carry calc()"),
            &mut measurer,
        )
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

    let output = compute_leaf_layout(
        input,
        &MockStyle::default(),
        |_, _| unreachable!("mock styles never carry calc()"),
        &mut measurer,
    );

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
    use std::cell::Cell;

    let (mut host, root) = leaf_tree();
    let calls = Cell::new(0);
    let output = compute_cached_layout(
        &mut host.session,
        root,
        LayoutInput::default(),
        |session, node, input| {
            calls.set(calls.get() + 1);
            session.compute_child_layout(&host.source, node, input)
        },
    );
    assert_eq!(calls.get(), 1);
    assert_eq!(output, LayoutOutput::HIDDEN);
}

#[test]
fn compute_absolute_layout_uses_the_static_position() {
    let (mut host, root) = leaf_tree();
    let hoisted = host.source.child_id(root, 0);
    let layout = compute_absolute_layout(
        &host.source,
        &mut host.session,
        hoisted,
        Size::new(800.0, 600.0),
        Point::new(12.5, 7.0),
    );
    assert_eq!(layout.location, Point::new(12.5, 7.0));
    assert_eq!(layout.size, Size::ZERO);
}

#[test]
fn round_layout_snaps_on_the_device_pixel_grid() {
    let (mut host, root) = leaf_tree();
    let child = host.source.child_id(root, 0);
    let mut root_layout = Layout::default();
    root_layout.location = Point::new(0.24, 0.24);
    root_layout.size = Size::new(10.26, 10.26);
    host.session.node_mut(root).unrounded = root_layout;
    let mut child_layout = Layout::default();
    child_layout.location = Point::new(0.26, 0.26);
    child_layout.size = Size::new(4.74, 4.74);
    host.session.node_mut(child).unrounded = child_layout;

    round_layout(&host.source, &mut host.session, root, 2.0);
    assert_eq!(host.session.node(root).finalized.location, Point::ZERO);
    assert_eq!(
        host.session.node(root).finalized.size,
        Size::new(10.5, 10.5)
    );
    assert_eq!(
        host.session.node(child).finalized.location,
        Point::new(0.5, 0.5)
    );
    assert_eq!(host.session.node(child).finalized.size, Size::new(4.5, 4.5));
}

#[test]
fn round_layout_uses_css_positive_infinity_tie_breaking() {
    let (mut host, root) = leaf_tree();
    let mut root_layout = Layout::default();
    // At DPR 2 these become -1.5 and +1.5 device pixels. CSS nearest-
    // integer rounding chooses the upper integer in both cases: -1 and +2.
    root_layout.location = Point::new(-0.75, 0.75);
    host.session.node_mut(root).unrounded = root_layout;

    round_layout(&host.source, &mut host.session, root, 2.0);

    assert_eq!(
        host.session.node(root).finalized.location,
        Point::new(-0.5, 1.0)
    );
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
