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
    compute_absolute_layout, compute_cached_layout, compute_leaf_layout, compute_root_layout,
    hide_subtree, round_layout,
};
use neutron_star::prelude::*;
use neutron_star::style::{
    BoxGenerationMode, CalcHandle, Dimension, GridPlacement, GridTemplateComponent,
    LengthPercentage, LengthPercentageAuto, Position, RepetitionCount, TrackSizingFunction,
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
struct MockNode {
    style: MockStyle,
    children: Vec<NodeId>,
    unrounded: Layout,
    finalized: Layout,
    /// Static position recorded for `Position::AbsoluteHoisted` children.
    static_position: Point<f32>,
}

#[derive(Debug, Default)]
struct MockTree {
    nodes: Vec<MockNode>,
    invalidated: Vec<NodeId>,
}

impl MockTree {
    fn push(&mut self, style: MockStyle, children: Vec<NodeId>) -> NodeId {
        self.nodes.push(MockNode {
            style,
            children,
            ..MockNode::default()
        });
        NodeId::from(self.nodes.len() - 1)
    }

    fn node(&self, id: NodeId) -> &MockNode {
        &self.nodes[usize::from(id)]
    }

    fn node_mut(&mut self, id: NodeId) -> &mut MockNode {
        &mut self.nodes[usize::from(id)]
    }
}

impl TraverseTree for MockTree {
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

impl LayoutTree for MockTree {
    type CoreStyle<'a> = &'a MockStyle;

    fn core_style(&self, node: NodeId) -> Self::CoreStyle<'_> {
        &self.node(node).style
    }

    fn resolve_calc(&self, _calc: CalcHandle, _basis: f32) -> f32 {
        unreachable!("mock styles never carry calc()")
    }

    fn set_unrounded_layout(&mut self, node: NodeId, layout: &Layout) {
        self.node_mut(node).unrounded = *layout;
    }

    fn set_static_position(&mut self, child: NodeId, static_position: Point<f32>) {
        self.node_mut(child).static_position = static_position;
    }

    fn invalidate_layout_cache(&mut self, node: NodeId) {
        self.invalidated.push(node);
    }

    // A reduced dispatch shape for this protocol-only mock. Real hosts wrap
    // generated boxes in compute_cached_layout and add flex/grid/custom
    // modes; hidden cleanup must happen before that cache boundary.
    fn compute_child_layout(&mut self, child: NodeId, input: LayoutInput) -> LayoutOutput {
        if self.node(child).style.box_generation_mode() == BoxGenerationMode::None {
            hide_subtree(self, child);
            return LayoutOutput::HIDDEN;
        }
        match self.node(child).style.display {
            MockDisplay::Hidden => unreachable!("handled before the cache boundary"),
            MockDisplay::Leaf => {
                LayoutOutput::new(input.known_dimensions.unwrap_or(Size::ZERO), Size::ZERO)
            }
        }
    }
}

impl FlexTree for MockTree {
    type ContainerStyle<'a> = &'a MockStyle;
    type ItemStyle<'a> = &'a MockStyle;

    fn flex_container_style(&self, container: NodeId) -> Self::ContainerStyle<'_> {
        &self.node(container).style
    }

    fn flex_item_style(&self, item: NodeId) -> Self::ItemStyle<'_> {
        &self.node(item).style
    }
}

impl GridTree for MockTree {
    type ContainerStyle<'a> = &'a MockStyle;
    type ItemStyle<'a> = &'a MockStyle;

    fn grid_container_style(&self, container: NodeId) -> Self::ContainerStyle<'_> {
        &self.node(container).style
    }

    fn grid_item_style(&self, item: NodeId) -> Self::ItemStyle<'_> {
        &self.node(item).style
    }
}

/// A trivially-conformant always-miss cache (the protocol mock intentionally
/// keeps cache behavior observable through the standalone [`Cache`] tests).
/// Keyed on the complete `LayoutInput` per the `CacheTree` contract.
impl CacheTree for MockTree {
    fn cache_get(&self, _node: NodeId, _input: LayoutInput) -> Option<LayoutOutput> {
        None
    }

    fn cache_store(&mut self, _node: NodeId, _input: LayoutInput, _layout_output: LayoutOutput) {}

    fn cache_clear(&mut self, _node: NodeId) {}
}

impl RoundTree for MockTree {
    fn unrounded_layout(&self, node: NodeId) -> Layout {
        self.node(node).unrounded
    }

    fn set_final_layout(&mut self, node: NodeId, layout: &Layout) {
        self.node_mut(node).finalized = *layout;
    }
}

fn leaf_tree() -> (MockTree, NodeId) {
    let mut tree = MockTree::default();
    let a = tree.push(MockStyle::default(), vec![]);
    let b = tree.push(MockStyle::default(), vec![]);
    let root = tree.push(MockStyle::default(), vec![a, b]);
    (tree, root)
}

#[test]
fn traversal_over_host_storage() {
    let (tree, root) = leaf_tree();
    assert_eq!(tree.child_count(root), 2);
    assert_eq!(tree.child_ids(root).count(), 2);
    assert_eq!(tree.child_id(root, 1), tree.child_ids(root).nth(1).unwrap());
}

#[test]
fn style_views_serve_css_initial_defaults() {
    let style = MockStyle::default();
    // Through the blanket `&S` view, as the engine will consume it.
    let view: <MockTree as LayoutTree>::CoreStyle<'_> = &style;
    assert_eq!(view.position(), Position::Relative);
    assert!(view.size().width.is_auto());
    assert_eq!(FlexItemStyle::order(&view), 0);
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
fn leaf_dispatch_round_trips_layout_io() {
    let (mut tree, root) = leaf_tree();
    let child = tree.child_id(root, 0);
    let input =
        LayoutInput::perform_layout(Size::new(Some(40.0), None), Size::NONE, Size::MAX_CONTENT);
    let output = tree.compute_child_layout(child, input);
    assert_eq!(output.size, Size::new(40.0, 0.0));

    // `Layout` is #[non_exhaustive]: construct via default + field writes.
    let mut layout = Layout::with_order(0);
    layout.size = output.size;
    tree.set_unrounded_layout(child, &layout);
    assert_eq!(tree.unrounded_layout(child).size, Size::new(40.0, 0.0));
}

#[test]
fn static_position_round_trips_through_the_tree() {
    // For `Position::AbsoluteHoisted` children the formatting parent records
    // a static position instead of a layout; the host stores it for the
    // positioned pass.
    let (mut tree, root) = leaf_tree();
    let child = tree.child_id(root, 1);
    tree.set_static_position(child, Point::new(12.5, 7.0));
    assert_eq!(tree.node(child).static_position, Point::new(12.5, 7.0));
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
    let (mut tree, root) = leaf_tree();
    compute_root_layout(
        &mut tree,
        root,
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
        &mut tree,
        root,
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
        &mut tree,
        root,
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
    tree.node_mut(hidden).unrounded.size = Size::new(50.0, 20.0);
    tree.node_mut(root).unrounded.size = Size::new(40.0, 10.0);
    hide_subtree(&mut tree, hidden);
    assert_eq!(tree.unrounded_layout(hidden), Layout::default());
    assert_eq!(tree.unrounded_layout(root), Layout::default());
    assert!(tree.invalidated.contains(&hidden));
    assert!(tree.invalidated.contains(&root));
}

#[test]
fn compute_leaf_layout_uses_the_host_measurement() {
    let output = compute_leaf_layout(
        LayoutInput::default(),
        &MockStyle::default(),
        |_, _| unreachable!("mock styles never carry calc()"),
        |_known, _available| Size::new(31.0, 17.0),
    );
    assert_eq!(output.size, Size::new(31.0, 17.0));
    assert_eq!(output.content_size, Size::new(31.0, 17.0));
}

#[test]
fn compute_cached_layout_runs_an_uncached_dispatch() {
    use std::cell::Cell;

    let (mut tree, root) = leaf_tree();
    let calls = Cell::new(0);
    let output = compute_cached_layout(
        &mut tree,
        root,
        LayoutInput::default(),
        |tree, node, input| {
            calls.set(calls.get() + 1);
            tree.compute_child_layout(node, input)
        },
    );
    assert_eq!(calls.get(), 1);
    assert_eq!(output, LayoutOutput::HIDDEN);
}

#[test]
fn compute_absolute_layout_uses_the_static_position() {
    let (mut tree, root) = leaf_tree();
    let hoisted = tree.child_id(root, 0);
    let layout = compute_absolute_layout(
        &mut tree,
        hoisted,
        Size::new(800.0, 600.0),
        Point::new(12.5, 7.0),
    );
    assert_eq!(layout.location, Point::new(12.5, 7.0));
    assert_eq!(layout.size, Size::ZERO);
}

#[test]
fn round_layout_snaps_on_the_device_pixel_grid() {
    let (mut tree, root) = leaf_tree();
    let child = tree.child_id(root, 0);
    let mut root_layout = Layout::default();
    root_layout.location = Point::new(0.24, 0.24);
    root_layout.size = Size::new(10.26, 10.26);
    tree.node_mut(root).unrounded = root_layout;
    let mut child_layout = Layout::default();
    child_layout.location = Point::new(0.26, 0.26);
    child_layout.size = Size::new(4.74, 4.74);
    tree.node_mut(child).unrounded = child_layout;

    round_layout(&mut tree, root, 2.0);
    assert_eq!(tree.node(root).finalized.location, Point::ZERO);
    assert_eq!(tree.node(root).finalized.size, Size::new(10.5, 10.5));
    assert_eq!(tree.node(child).finalized.location, Point::new(0.5, 0.5));
    assert_eq!(tree.node(child).finalized.size, Size::new(4.5, 4.5));
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
}
