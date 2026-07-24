//! Protocol conformance: a minimal but *complete* host implementing the
//! neutron-star tree protocol, proving [`LayoutTree`] is implementable over
//! plain storage with zero `dyn`, zero allocation at the boundary, and zero
//! engine-side state.

use std::cell::Cell;
use std::fmt;

use neutron_star::cache::Cache;
use neutron_star::compute::{
    compute_absolute_layout, compute_boundary_relayout, compute_cached_layout,
    compute_flexbox_layout, compute_leaf_layout, compute_root_layout,
    compute_skipped_contents_layout, hide_subtree, round_layout,
};
use neutron_star::invalidate::{invalidate_for_relayout, is_relayout_boundary};
use neutron_star::prelude::*;
use style_traits::values::specified::AllowedNumericType;
use stylo::Zero;
use stylo::computed_values::{relative_center, relative_layout_once, visibility};
use stylo::values::computed::length_percentage::{CalcNode, ComputedLeaf};
use stylo::values::computed::{
    Contain, ContainIntrinsicSize, Display, GridLine, GridTemplateComponent, ImplicitGridTracks,
    Length, LengthPercentage, Margin, NonNegativeLengthPercentage, NonNegativeNumber, Percentage,
    PositionProperty, Size as StyleSize,
};
use stylo::values::generics::NonNegative;
use stylo::values::generics::grid::{
    ImplicitGridTracks as GenericImplicitGridTracks, RepeatCount, TrackBreadth, TrackList,
    TrackListValue, TrackRepeat, TrackSize,
};

fn px(value: f32) -> LengthPercentage {
    LengthPercentage::new_length(Length::new(value))
}

fn npx(value: f32) -> NonNegativeLengthPercentage {
    NonNegative(px(value))
}

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

fn fixed_track(value: f32) -> TrackSize<LengthPercentage> {
    TrackSize::Breadth(TrackBreadth::Breadth(px(value)))
}

fn fr_track(value: f32) -> TrackSize<LengthPercentage> {
    TrackSize::Breadth(TrackBreadth::Flex(stylo::values::generics::grid::Flex(
        value,
    )))
}

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

#[derive(Debug, Clone, Copy, Default, PartialEq)]
enum MockDisplay {
    #[default]
    Leaf,
    Flex,
    Hidden,
}

#[derive(Debug, Clone)]
struct MockStyle {
    display: MockDisplay,
    margin: Edges<Margin>,
    size: Size<StyleSize>,
    padding: Edges<NonNegativeLengthPercentage>,
    flex_grow: NonNegativeNumber,
    grid_column: Line<GridLine>,
    template_columns: GridTemplateComponent,
    implicit_tracks: ImplicitGridTracks,
    containment: Contain,
    contain_intrinsic_width: ContainIntrinsicSize,
    contain_intrinsic_height: ContainIntrinsicSize,
    skips_contents: bool,
}

fn auto_horizontal_margin() -> Edges<Margin> {
    Edges {
        left: Margin::Auto,
        right: Margin::Auto,
        top: Margin::zero(),
        bottom: Margin::zero(),
    }
}

impl Default for MockStyle {
    fn default() -> Self {
        Self {
            display: MockDisplay::Leaf,
            margin: Edges::uniform(Margin::zero()),
            size: Size::new(StyleSize::auto(), StyleSize::auto()),
            padding: Edges::uniform(NonNegative(LengthPercentage::zero())),
            flex_grow: NonNegativeNumber::from(0.0),
            grid_column: Line::new(GridLine::auto(), GridLine::auto()),
            template_columns: GridTemplateComponent::None,
            implicit_tracks: GenericImplicitGridTracks(stylo::OwnedSlice::default()),
            containment: Contain::empty(),
            contain_intrinsic_width: ContainIntrinsicSize::None,
            contain_intrinsic_height: ContainIntrinsicSize::None,
            skips_contents: false,
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

    fn size(&self) -> Size<&StyleSize> {
        self.size.as_ref()
    }

    fn padding(&self) -> Edges<&NonNegativeLengthPercentage> {
        self.padding.as_ref()
    }

    fn margin(&self) -> Edges<&Margin> {
        self.margin.as_ref()
    }

    fn containment(&self) -> Contain {
        self.containment
    }

    fn contain_intrinsic_width(&self) -> ContainIntrinsicSize {
        self.contain_intrinsic_width.clone()
    }

    fn contain_intrinsic_height(&self) -> ContainIntrinsicSize {
        self.contain_intrinsic_height.clone()
    }

    fn skips_contents(&self) -> bool {
        self.skips_contents
    }

    fn flex_grow(&self) -> NonNegativeNumber {
        self.flex_grow
    }

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

    fn grid_column_start(&self) -> &GridLine {
        &self.grid_column.start
    }

    fn grid_column_end(&self) -> &GridLine {
        &self.grid_column.end
    }
}

/// Immutable per-node data: style and topology, fixed for the layout epoch.
#[derive(Debug, Default)]
struct MockSourceNode {
    style: MockStyle,
    children: Vec<usize>,
}

#[derive(Debug, Default)]
struct MockState {
    slots: Vec<LayoutSlot>,
    invalidated: Vec<usize>,
}

/// Immutable source tree. A separate [`MockState`] owns all mutable layout
/// slots for an epoch.
#[derive(Debug, Default)]
struct MockTree {
    nodes: Vec<MockSourceNode>,
}

impl MockTree {
    fn push(&mut self, style: MockStyle, children: Vec<usize>) -> usize {
        let id = self.nodes.len();
        self.nodes.push(MockSourceNode { style, children });
        id
    }

    fn node(&self, id: usize) -> MockRef {
        debug_assert!(id < self.nodes.len());
        MockRef { index: id }
    }

    fn new_state(&self) -> MockState {
        MockState {
            slots: (0..self.nodes.len())
                .map(|_| LayoutSlot::default())
                .collect(),
            invalidated: Vec::new(),
        }
    }

    fn compute_layout(&self, state: &mut MockState, id: usize, input: LayoutInput) -> LayoutOutput {
        LayoutTree::compute_layout(self, state, self.node(id), input)
    }

    fn unrounded_layout<'state>(&self, state: &'state MockState, id: usize) -> &'state Layout {
        debug_assert!(id < self.nodes.len());
        state.slots[id].unrounded()
    }

    fn final_layout<'state>(&self, state: &'state MockState, id: usize) -> &'state Layout {
        debug_assert!(id < self.nodes.len());
        state.slots[id].rounded()
    }
}

/// The `Copy` node id.
#[derive(Clone, Copy)]
struct MockRef {
    index: usize,
}

impl fmt::Debug for MockRef {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_tuple("MockRef").field(&self.index).finish()
    }
}

struct MockChildren<'t> {
    ids: std::slice::Iter<'t, usize>,
}

impl Iterator for MockChildren<'_> {
    type Item = MockRef;

    fn next(&mut self) -> Option<MockRef> {
        let index = *self.ids.next()?;
        Some(MockRef { index })
    }
}

impl LayoutTree for MockTree {
    type NodeId = MockRef;
    type State = MockState;
    type Style<'tree> = &'tree MockStyle;
    type ChildIter<'tree> = MockChildren<'tree>;

    fn children(&self, node: MockRef) -> MockChildren<'_> {
        MockChildren {
            ids: self.nodes[node.index].children.iter(),
        }
    }

    fn style(&self, node: MockRef) -> &MockStyle {
        &self.nodes[node.index].style
    }

    fn layout<'state>(&self, state: &'state MockState, node: MockRef) -> &'state LayoutSlot {
        &state.slots[node.index]
    }

    fn layout_mut<'state>(
        &self,
        state: &'state mut MockState,
        node: MockRef,
    ) -> &'state mut LayoutSlot {
        &mut state.slots[node.index]
    }

    fn compute_layout(
        &self,
        state: &mut MockState,
        node: MockRef,
        input: LayoutInput,
    ) -> LayoutOutput {
        let style = self.style(node);
        if style.display().is_none() {
            hide_subtree(self, state, node);
            return LayoutOutput::HIDDEN;
        }

        if style.skips_contents() {
            return compute_skipped_contents_layout(self, state, node, input);
        }

        let display = style.display;
        compute_cached_layout(
            self,
            state,
            node,
            input,
            |tree, state, node, input| match display {
                MockDisplay::Hidden => unreachable!("handled before the cache boundary"),
                MockDisplay::Flex => compute_flexbox_layout(tree, state, node, input),
                MockDisplay::Leaf => {
                    LayoutOutput::new(input.known_dimensions.unwrap_or(Size::ZERO), Size::ZERO)
                }
            },
        )
    }

    fn clear_layout_cache(&self, state: &mut MockState, node: MockRef) {
        state.slots[node.index].clear_layout_cache();
        state.invalidated.push(node.index);
    }
}

fn leaf_tree() -> (MockTree, MockState, usize) {
    let mut tree = MockTree::default();
    let a = tree.push(MockStyle::default(), vec![]);
    let b = tree.push(MockStyle::default(), vec![]);
    let root = tree.push(MockStyle::default(), vec![a, b]);
    let state = tree.new_state();
    (tree, state, root)
}

#[test]
fn traversal_over_host_storage() {
    let (tree, _state, root) = leaf_tree();
    let root_handle = tree.node(root);
    assert_eq!(tree.child_count(root_handle), 2);
    assert_eq!(tree.children(root_handle).count(), 2);
    let ids: Vec<usize> = tree
        .children(root_handle)
        .map(|child| child.index)
        .collect();
    assert_eq!(ids, tree.nodes[root].children);
}

#[test]
fn style_views_serve_initial_defaults() {
    let style = MockStyle::default();
    let view: <MockTree as LayoutTree>::Style<'_> = &style;
    assert_eq!(view.position(), PositionProperty::Static);
    assert_eq!(view.visibility(), visibility::T::Visible);
    assert!(matches!(view.size().width, StyleSize::Auto));
    assert_eq!(view.order(), 0);
    assert_eq!(view.relative_layout_once(), relative_layout_once::T::True);
    assert_eq!(view.relative_id(), -1);
    assert_eq!(view.relative_center(), relative_center::T::None);
    assert_eq!(view.grid_column_start(), &GridLine::auto());
    assert_eq!(view.grid_column_end(), &GridLine::auto());
    assert_eq!(view.grid_row_start(), &GridLine::auto());
    assert_eq!(view.grid_row_end(), &GridLine::auto());
}

#[test]
fn grid_template_borrow_serves_stylo_track_lists() {
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

    let template = style.grid_template_columns();
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
            assert_eq!(repetition.track_sizes.len(), 2);
        }
        other @ TrackListValue::TrackSize(_) => panic!("expected a repetition, got {other:?}"),
    }
    assert!(matches!(
        style.grid_template_rows(),
        GridTemplateComponent::None
    ));
    assert!(style.grid_auto_rows().0.is_empty());
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

    let mut state = tree.new_state();
    let style = tree.style(tree.node(root));
    let GridTemplateComponent::TrackList(tracks) = style.grid_template_columns() else {
        panic!("expected a track list");
    };
    match &tracks.values[0] {
        TrackListValue::TrackSize(track) => assert_eq!(*track, fixed_track(10.0)),
        other @ TrackListValue::TrackRepeat(_) => {
            panic!("expected first single track, got {other:?}")
        }
    }

    let output = tree.compute_layout(
        &mut state,
        child,
        LayoutInput::commit(
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
    let (tree, mut state, root) = leaf_tree();
    let child = tree.children(tree.node(root)).next().unwrap();
    let input = LayoutInput::commit(Size::new(Some(40.0), None), Size::NONE, Size::MAX_CONTENT);
    let output = LayoutTree::compute_layout(&tree, &mut state, child, input);
    assert_eq!(output.size, Size::new(40.0, 0.0));

    let mut layout = Layout::with_order(0);
    layout.size = output.size;
    tree.layout_mut(&mut state, child).set_unrounded(layout);
    assert_eq!(
        tree.layout(&state, child).unrounded().size,
        Size::new(40.0, 0.0)
    );
}

#[test]
fn static_position_round_trips_through_the_tree() {
    let (tree, mut state, root) = leaf_tree();
    let child = tree.children(tree.node(root)).nth(1).unwrap();
    tree.layout_mut(&mut state, child)
        .set_static_position(Point::new(12.5, 7.0));
    assert_eq!(
        tree.layout(&state, child).static_position(),
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
    let (tree, mut state, root) = leaf_tree();
    compute_root_layout(
        &tree,
        &mut state,
        tree.node(root),
        Size::new(
            AvailableSpace::Definite(100.0),
            AvailableSpace::Definite(80.0),
        ),
    );
    assert_eq!(tree.unrounded_layout(&state, root).location, Point::ZERO);
    assert_eq!(tree.unrounded_layout(&state, root).size, Size::ZERO);
}

#[test]
#[allow(clippy::float_cmp)]
fn compute_root_layout_resolves_horizontal_auto_margins() {
    let mut tree = MockTree::default();
    let root = tree.push(
        MockStyle {
            margin: auto_horizontal_margin(),
            ..MockStyle::default()
        },
        vec![],
    );
    let mut state = tree.new_state();
    compute_root_layout(
        &tree,
        &mut state,
        tree.node(root),
        Size::new(
            AvailableSpace::Definite(100.0),
            AvailableSpace::Definite(20.0),
        ),
    );
    assert_eq!(tree.unrounded_layout(&state, root).margin.left, 50.0);
    assert_eq!(tree.unrounded_layout(&state, root).margin.right, 50.0);
    assert_eq!(tree.unrounded_layout(&state, root).location.x, 50.0);
}

#[test]
fn compute_root_layout_preserves_a_hidden_zero_box() {
    let mut tree = MockTree::default();
    let root = tree.push(
        MockStyle {
            display: MockDisplay::Hidden,
            margin: auto_horizontal_margin(),
            ..MockStyle::default()
        },
        vec![],
    );
    let mut state = tree.new_state();
    compute_root_layout(
        &tree,
        &mut state,
        tree.node(root),
        Size::new(
            AvailableSpace::Definite(100.0),
            AvailableSpace::Definite(20.0),
        ),
    );
    assert_eq!(tree.unrounded_layout(&state, root), &Layout::default());
}

#[test]
fn explicit_hidden_cleanup_clears_stale_geometry() {
    let (mut tree, _state, root) = leaf_tree();
    let hidden = tree.push(
        MockStyle {
            display: MockDisplay::Hidden,
            ..MockStyle::default()
        },
        vec![root],
    );
    let mut state = tree.new_state();
    let mut hidden_layout = Layout::default();
    hidden_layout.size = Size::new(50.0, 20.0);
    state.slots[hidden].set_unrounded(hidden_layout);
    let mut root_layout = Layout::default();
    root_layout.size = Size::new(40.0, 10.0);
    state.slots[root].set_unrounded(root_layout);

    hide_subtree(&tree, &mut state, tree.node(hidden));
    assert_eq!(tree.unrounded_layout(&state, hidden), &Layout::default());
    assert_eq!(tree.unrounded_layout(&state, root), &Layout::default());
    assert!(state.invalidated.contains(&hidden));
    assert!(state.invalidated.contains(&root));
}

#[test]
fn compute_leaf_layout_uses_internal_natural_size() {
    let style = MockStyle {
        size: Size::new(StyleSize::LengthPercentage(npx(50.0)), StyleSize::auto()),
        padding: Edges::uniform(npx(5.0)),
        ..MockStyle::default()
    };
    let input = LayoutInput::commit(
        Size::NONE,
        Size::new(Some(100.0), Some(100.0)),
        Size::new(
            AvailableSpace::Definite(100.0),
            AvailableSpace::Definite(100.0),
        ),
    );
    let output = compute_leaf_layout(
        input,
        &style,
        NaturalSize::new(Size::new(None, Some(17.0)), None),
    );

    assert_eq!(output.size, Size::new(60.0, 27.0));
    assert_eq!(output.content_size, Size::new(60.0, 27.0));
    assert_eq!(output.first_baselines, Point::NONE);
}

#[test]
fn calc_padding_resolves_through_stylo_style_values() {
    let style = MockStyle {
        size: Size::new(StyleSize::LengthPercentage(npx(50.0)), StyleSize::auto()),
        padding: Edges::uniform(calc_lp(10.0, 0.05)),
        ..MockStyle::default()
    };
    let input = LayoutInput::commit(
        Size::NONE,
        Size::new(Some(200.0), Some(100.0)),
        Size::new(
            AvailableSpace::Definite(200.0),
            AvailableSpace::Definite(100.0),
        ),
    );
    let output = compute_leaf_layout(
        input,
        &style,
        NaturalSize::new(Size::new(None, Some(17.0)), None),
    );

    assert_eq!(output.size, Size::new(90.0, 57.0));
}

#[test]
fn compute_cached_layout_runs_an_uncached_dispatch() {
    let (tree, mut state, root) = leaf_tree();
    let calls = Cell::new(0);
    let output = compute_cached_layout(
        &tree,
        &mut state,
        tree.node(root),
        LayoutInput::default(),
        |tree, state, node, input| {
            calls.set(calls.get() + 1);
            LayoutTree::compute_layout(tree, state, node, input)
        },
    );
    assert_eq!(calls.get(), 1);
    assert_eq!(output, LayoutOutput::HIDDEN);
}

#[test]
fn compute_absolute_layout_uses_the_static_position() {
    let (tree, mut state, root) = leaf_tree();
    let hoisted = tree.children(tree.node(root)).next().unwrap();
    let layout = compute_absolute_layout(
        &tree,
        &mut state,
        hoisted,
        Size::new(800.0, 600.0),
        Point::new(12.5, 7.0),
    );
    assert_eq!(layout.location, Point::new(12.5, 7.0));
    assert_eq!(layout.size, Size::ZERO);
}

#[test]
fn round_layout_snaps_on_the_device_pixel_grid() {
    let (tree, mut state, root) = leaf_tree();
    let child = tree.nodes[root].children[0];
    let mut root_layout = Layout::default();
    root_layout.location = Point::new(0.24, 0.24);
    root_layout.size = Size::new(10.26, 10.26);
    state.slots[root].set_unrounded(root_layout);
    let mut child_layout = Layout::default();
    child_layout.location = Point::new(0.26, 0.26);
    child_layout.size = Size::new(4.74, 4.74);
    state.slots[child].set_unrounded(child_layout);

    round_layout(&tree, &mut state, tree.node(root), 2.0);
    assert_eq!(tree.final_layout(&state, root).location, Point::ZERO);
    assert_eq!(tree.final_layout(&state, root).size, Size::new(10.5, 10.5));
    assert_eq!(
        tree.final_layout(&state, child).location,
        Point::new(0.5, 0.5)
    );
    assert_eq!(tree.final_layout(&state, child).size, Size::new(4.5, 4.5));
}

#[test]
fn round_layout_uses_css_positive_infinity_tie_breaking() {
    let (tree, mut state, root) = leaf_tree();
    let mut root_layout = Layout::default();
    root_layout.location = Point::new(-0.75, 0.75);
    state.slots[root].set_unrounded(root_layout);

    round_layout(&tree, &mut state, tree.node(root), 2.0);

    assert_eq!(
        tree.final_layout(&state, root).location,
        Point::new(-0.5, 1.0)
    );
}

#[test]
fn embeddable_cache_round_trips_a_complete_key() {
    let mut cache = Cache::new();
    let input = LayoutInput::measure(
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

#[test]
fn default_child_count_counts_the_children_iterator() {
    let (tree, _state, root) = leaf_tree();
    assert_eq!(tree.child_count(tree.node(root)), 2);
    assert_eq!(tree.child_count(tree.node(tree.nodes[root].children[0])), 0);
}

fn size_px(value: f32) -> StyleSize {
    StyleSize::LengthPercentage(npx(value))
}

fn contain_len(value: f32) -> ContainIntrinsicSize {
    ContainIntrinsicSize::Length(NonNegative(Length::new(value)))
}

fn contain_auto_len(value: f32) -> ContainIntrinsicSize {
    ContainIntrinsicSize::AutoLength(NonNegative(Length::new(value)))
}

fn contained_style(containment: Contain) -> MockStyle {
    MockStyle {
        containment,
        ..MockStyle::default()
    }
}

#[test]
fn relayout_boundary_requires_both_layout_and_size() {
    assert!(is_relayout_boundary(&contained_style(Contain::STRICT)));
    assert!(is_relayout_boundary(&contained_style(
        Contain::SIZE | Contain::LAYOUT
    )));
    assert!(!is_relayout_boundary(&contained_style(Contain::LAYOUT)));
    assert!(!is_relayout_boundary(&contained_style(Contain::SIZE)));
    assert!(!is_relayout_boundary(&contained_style(Contain::CONTENT)));
    assert!(!is_relayout_boundary(&contained_style(Contain::empty())));
    assert!(!is_relayout_boundary(&contained_style(
        Contain::INLINE_SIZE | Contain::LAYOUT
    )));
}

#[test]
fn invalidate_for_relayout_stops_at_the_nearest_boundary() {
    let mut tree = MockTree::default();
    let leaf = tree.push(MockStyle::default(), vec![]);
    let boundary = tree.push(contained_style(Contain::STRICT), vec![leaf]);
    let root = tree.push(MockStyle::default(), vec![boundary]);
    let mut state = tree.new_state();

    for node in [leaf, boundary, root] {
        state.slots[node].store_cached_layout(
            LayoutInput::default(),
            LayoutOutput::new(Size::new(1.0, 1.0), Size::ZERO),
        );
    }

    let re_root = invalidate_for_relayout(
        &tree,
        &mut state,
        tree.node(leaf),
        [tree.node(boundary), tree.node(root)].into_iter(),
    );

    assert_eq!(re_root.index, boundary);
    assert_eq!(state.invalidated, vec![leaf, boundary]);
    assert!(state.slots[leaf].layout_cache_is_empty());
    assert!(state.slots[boundary].layout_cache_is_empty());
    assert!(!state.slots[root].layout_cache_is_empty());
}

#[test]
fn invalidate_for_relayout_walks_to_root_without_a_boundary() {
    let mut tree = MockTree::default();
    let leaf = tree.push(MockStyle::default(), vec![]);
    let mid = tree.push(contained_style(Contain::CONTENT), vec![leaf]);
    let root = tree.push(MockStyle::default(), vec![mid]);
    let mut state = tree.new_state();

    let re_root = invalidate_for_relayout(
        &tree,
        &mut state,
        tree.node(leaf),
        [tree.node(mid), tree.node(root)].into_iter(),
    );

    assert_eq!(re_root.index, root);
    assert_eq!(state.invalidated, vec![leaf, mid, root]);
}

#[test]
fn invalidate_for_relayout_returns_the_node_when_it_is_the_root() {
    let mut tree = MockTree::default();
    let lone = tree.push(MockStyle::default(), vec![]);
    let mut state = tree.new_state();

    let re_root = invalidate_for_relayout(&tree, &mut state, tree.node(lone), std::iter::empty());

    assert_eq!(re_root.index, lone);
    assert_eq!(state.invalidated, vec![lone]);
}

fn stretched_boundary_tree() -> (MockTree, MockState, usize, usize, usize) {
    let mut tree = MockTree::default();
    let interior = tree.push(
        MockStyle {
            size: Size::new(size_px(40.0), StyleSize::auto()),
            ..MockStyle::default()
        },
        vec![],
    );
    let boundary = tree.push(
        MockStyle {
            display: MockDisplay::Flex,
            size: Size::new(size_px(50.0), StyleSize::auto()),
            containment: Contain::STRICT,
            contain_intrinsic_width: contain_len(50.0),
            contain_intrinsic_height: contain_len(30.0),
            ..MockStyle::default()
        },
        vec![interior],
    );
    let parent = tree.push(
        MockStyle {
            display: MockDisplay::Flex,
            size: Size::new(size_px(300.0), size_px(200.0)),
            ..MockStyle::default()
        },
        vec![boundary],
    );
    let mut state = tree.new_state();

    tree.compute_layout(
        &mut state,
        parent,
        LayoutInput::commit(Size::NONE, Size::NONE, Size::MAX_CONTENT),
    );
    (tree, state, parent, boundary, interior)
}

#[test]
fn compute_boundary_relayout_preserves_the_parent_imposed_size_and_updates_the_interior() {
    let (mut tree, mut state, parent, boundary, interior) = stretched_boundary_tree();

    let boundary_location = tree.unrounded_layout(&state, boundary).location;
    let boundary_size = tree.unrounded_layout(&state, boundary).size;
    assert_eq!(boundary_size, Size::new(50.0, 200.0));
    assert_eq!(
        tree.unrounded_layout(&state, interior).size,
        Size::new(40.0, 200.0)
    );

    let committed = state.slots[boundary]
        .committed_input()
        .expect("the boundary has a committed layout");

    tree.nodes[interior].style.size.width = size_px(20.0);

    let re_root = invalidate_for_relayout(
        &tree,
        &mut state,
        tree.node(interior),
        [tree.node(boundary), tree.node(parent)].into_iter(),
    );
    assert_eq!(re_root.index, boundary);

    let output = compute_boundary_relayout(&tree, &mut state, tree.node(boundary), committed);

    assert_eq!(output.size, Size::new(50.0, 200.0));
    let after = tree.unrounded_layout(&state, boundary);
    assert_eq!(after.location, boundary_location);
    assert_eq!(after.size, boundary_size);
    assert_eq!(
        tree.unrounded_layout(&state, interior).size,
        Size::new(20.0, 200.0)
    );
}

#[test]
fn compute_root_layout_would_resize_a_stretched_boundary_regression_contrast() {
    let (tree, mut state, _parent, boundary, _interior) = stretched_boundary_tree();
    let stretched = tree.unrounded_layout(&state, boundary).size;
    assert_eq!(stretched, Size::new(50.0, 200.0));

    compute_root_layout(
        &tree,
        &mut state,
        tree.node(boundary),
        Size::new(
            AvailableSpace::Definite(300.0),
            AvailableSpace::Definite(200.0),
        ),
    );

    let self_determined = tree.unrounded_layout(&state, boundary).size;
    assert_eq!(self_determined, Size::new(50.0, 30.0));
    assert_ne!(self_determined, stretched);
}

#[test]
fn skipped_contents_dispatch_sizes_from_intrinsic_and_hides_descendants() {
    let mut tree = MockTree::default();
    let child = tree.push(MockStyle::default(), vec![]);
    let skipped = tree.push(
        MockStyle {
            skips_contents: true,
            containment: Contain::STRICT,
            contain_intrinsic_width: contain_len(40.0),
            contain_intrinsic_height: contain_auto_len(24.0),
            ..MockStyle::default()
        },
        vec![child],
    );
    let mut state = tree.new_state();
    let mut child_layout = Layout::default();
    child_layout.size = Size::new(99.0, 99.0);
    state.slots[child].set_unrounded(child_layout);

    let output = tree.compute_layout(
        &mut state,
        skipped,
        LayoutInput::commit(
            Size::NONE,
            Size::new(Some(200.0), Some(200.0)),
            Size::new(
                AvailableSpace::Definite(200.0),
                AvailableSpace::Definite(200.0),
            ),
        ),
    );

    assert_eq!(output.size, Size::new(40.0, 24.0));
    assert_eq!(output.first_baselines, Point::NONE);
    assert_eq!(tree.unrounded_layout(&state, child), &Layout::default());
    assert!(state.invalidated.contains(&child));
}
