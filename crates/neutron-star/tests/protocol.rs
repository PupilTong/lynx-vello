//! Protocol conformance: a minimal but *complete* host implementing every
//! neutron-star trait, proving the protocol is implementable over plain
//! storage with zero `dyn`, zero allocation at the boundary, and zero
//! engine-side state.
//!
//! Runtime tests only exercise implemented surface (traversal, style views,
//! value plumbing); the machinery entry points are asserted to be *callable*
//! and currently-stubbed via `#[should_panic]`. The flex/grid algorithm
//! entry points don't exist yet (L1/L2) — only their contracts
//! (`FlexTree`/`GridTree` and the style traits) do, and this host implements
//! all of them.

use neutron_star::cache::Cache;
use neutron_star::compute::{
    compute_cached_layout, compute_hidden_layout, compute_leaf_layout, compute_root_layout,
    round_layout,
};
use neutron_star::prelude::*;
use neutron_star::style::{
    CalcHandle, Dimension, GridPlacement, GridTemplateComponent, LengthPercentage, Position,
    RepetitionCount, TrackSizingFunction,
};

/// The host's own display vocabulary — deliberately *not* an engine type
/// (dispatch belongs to the host; see the `compute` module docs). Flex/grid
/// arms join in L1/L2 when their algorithm entry points exist.
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
    size: Size<Dimension>,
    flex_grow: f32,
    grid_column: Line<GridPlacement>,
    template_columns: Vec<MockTemplateComponent>,
}

impl CoreStyle for MockStyle {
    fn size(&self) -> Size<Dimension> {
        self.size
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
}

#[derive(Debug, Default)]
struct MockTree {
    nodes: Vec<MockNode>,
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

    // The canonical dispatch shape (compute_cached_layout is omitted only
    // because it is still a stub; real hosts wrap the match in it). The
    // flexbox/grid arms join in L1/L2.
    fn compute_child_layout(&mut self, child: NodeId, input: LayoutInput) -> LayoutOutput {
        match self.node(child).style.display {
            MockDisplay::Hidden => compute_hidden_layout(self, child),
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

/// A trivially-conformant always-miss cache (the embeddable
/// [`Cache`]'s matching policy is an L1 stub, so the mock supplies its own).
impl CacheTree for MockTree {
    fn cache_get(
        &self,
        _node: NodeId,
        _known_dimensions: Size<Option<f32>>,
        _available_space: Size<AvailableSpace>,
        _run_mode: RunMode,
    ) -> Option<LayoutOutput> {
        None
    }

    fn cache_store(
        &mut self,
        _node: NodeId,
        _known_dimensions: Size<Option<f32>>,
        _available_space: Size<AvailableSpace>,
        _run_mode: RunMode,
        _layout_output: LayoutOutput,
    ) {
    }

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
fn embeddable_cache_lifecycle() {
    let mut cache = Cache::new();
    assert!(cache.is_empty());
    cache.clear();
    assert!(cache.is_empty());
    assert_eq!(cache, Cache::default());
}

// ---------------------------------------------------------------------------
// Stub-callability: every algorithm entry point monomorphizes against the
// mock host and is reachable; bodies are L1/L2 work.
// ---------------------------------------------------------------------------

#[test]
#[should_panic(expected = "not yet implemented")]
fn stub_compute_root_layout() {
    let (mut tree, root) = leaf_tree();
    compute_root_layout(&mut tree, root, Size::MAX_CONTENT);
}

#[test]
#[should_panic(expected = "not yet implemented")]
fn stub_compute_hidden_layout() {
    let (mut tree, _) = leaf_tree();
    let hidden = tree.push(
        MockStyle {
            display: MockDisplay::Hidden,
            ..MockStyle::default()
        },
        vec![],
    );
    compute_hidden_layout(&mut tree, hidden);
}

#[test]
#[should_panic(expected = "not yet implemented")]
fn stub_compute_leaf_layout() {
    compute_leaf_layout(
        LayoutInput::default(),
        &MockStyle::default(),
        |_, _| unreachable!("mock styles never carry calc()"),
        |_known, _available| Size::ZERO,
    );
}

#[test]
#[should_panic(expected = "not yet implemented")]
fn stub_compute_cached_layout() {
    let (mut tree, root) = leaf_tree();
    compute_cached_layout(
        &mut tree,
        root,
        LayoutInput::default(),
        LayoutTree::compute_child_layout,
    );
}

#[test]
#[should_panic(expected = "not yet implemented")]
fn stub_round_layout() {
    let (mut tree, root) = leaf_tree();
    round_layout(&mut tree, root);
}

#[test]
#[should_panic(expected = "not yet implemented")]
fn stub_cache_get() {
    let cache = Cache::new();
    let _ = cache.get(Size::NONE, Size::MAX_CONTENT, RunMode::ComputeSize);
}
