//! Protocol conformance: a minimal but *complete* host implementing the
//! neutron-star node protocol, proving [`LayoutNode`] is implementable over
//! plain storage with zero `dyn`, zero allocation at the boundary, and zero
//! engine-side state.

use std::cell::{Cell, RefCell};
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

/// Per-node layout slots, written through [`MockRef`] handles. Layout is
/// single-threaded, so `Cell`/`RefCell` interior mutability is the whole
/// synchronization story — the protocol has no `&mut`.
#[derive(Debug, Default)]
struct MockSessionNode {
    unrounded: RefCell<Layout>,
    finalized: RefCell<Layout>,
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

    fn node(&self, id: usize) -> MockRef<'_> {
        MockRef {
            tree: self,
            index: id,
        }
    }

    fn compute_layout(&self, id: usize, input: LayoutInput) -> LayoutOutput {
        self.node(id).compute_layout(input)
    }

    fn session_node(&self, id: usize) -> &MockSessionNode {
        &self.session[id]
    }

    fn unrounded_layout(&self, id: usize) -> Layout {
        self.session_node(id).unrounded.borrow().clone()
    }

    fn final_layout(&self, id: usize) -> Layout {
        self.session_node(id).finalized.borrow().clone()
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

    fn compute_layout(self, input: LayoutInput) -> LayoutOutput {
        let style = self.style();
        if style.display().is_none() {
            hide_subtree(self);
            return LayoutOutput::HIDDEN;
        }

        if style.skips_contents() {
            return compute_skipped_contents_layout(self, input);
        }

        compute_cached_layout(self, input, |handle, input| match handle.style().display {
            MockDisplay::Hidden => unreachable!("handled before the cache boundary"),
            MockDisplay::Flex => compute_flexbox_layout(handle, input),
            MockDisplay::Leaf => {
                LayoutOutput::new(input.known_dimensions.unwrap_or(Size::ZERO), Size::ZERO)
            }
        })
    }

    fn set_unrounded_layout(self, layout: Layout) {
        *self.slots().unrounded.borrow_mut() = layout;
    }

    fn with_unrounded_layout<R>(self, read: impl FnOnce(&Layout) -> R) -> R {
        let layout = self.slots().unrounded.borrow();
        read(&layout)
    }

    fn set_rounded_layout(self, layout: Layout) {
        *self.slots().finalized.borrow_mut() = layout;
    }

    fn set_static_position(self, static_position: Point<f32>) {
        self.slots().static_position.set(static_position);
    }

    fn cached_layout(self, input: LayoutInput) -> Option<LayoutOutput> {
        self.slots().cache.borrow().get(input)
    }

    fn store_cached_layout(self, input: LayoutInput, output: LayoutOutput) {
        self.slots().cache.borrow_mut().store(input, output);
    }

    fn clear_layout_cache(self) {
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
    let ids: Vec<usize> = root_handle.children().map(|child| child.index).collect();
    assert_eq!(ids, tree.nodes[root].children);
}

#[test]
fn style_views_serve_initial_defaults() {
    let style = MockStyle::default();
    let view: <MockRef<'_> as LayoutNode>::Style = &style;
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

    let output = tree.compute_layout(
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
    let (tree, root) = leaf_tree();
    let child = tree.node(root).children().next().unwrap();
    let input = LayoutInput::commit(Size::new(Some(40.0), None), Size::NONE, Size::MAX_CONTENT);
    let output = child.compute_layout(input);
    assert_eq!(output.size, Size::new(40.0, 0.0));

    let mut layout = Layout::with_order(0);
    layout.size = output.size;
    child.set_unrounded_layout(layout);
    assert_eq!(
        child.with_unrounded_layout(|layout| layout.size),
        Size::new(40.0, 0.0)
    );
}

#[test]
fn static_position_round_trips_through_the_tree() {
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
            margin: auto_horizontal_margin(),
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
    let mut stale = tree.session_node(hidden).unrounded.borrow().clone();
    stale.size = Size::new(50.0, 20.0);
    *tree.session_node(hidden).unrounded.borrow_mut() = stale;
    let mut stale = tree.session_node(root).unrounded.borrow().clone();
    stale.size = Size::new(40.0, 10.0);
    *tree.session_node(root).unrounded.borrow_mut() = stale;

    hide_subtree(tree.node(hidden));
    assert_eq!(tree.unrounded_layout(hidden), Layout::default());
    assert_eq!(tree.unrounded_layout(root), Layout::default());
    assert!(tree.invalidated.borrow().contains(&hidden));
    assert!(tree.invalidated.borrow().contains(&root));
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
    let (tree, root) = leaf_tree();
    let calls = Cell::new(0);
    let output = compute_cached_layout(tree.node(root), LayoutInput::default(), |node, input| {
        calls.set(calls.get() + 1);
        node.compute_layout(input)
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
    *tree.session_node(root).unrounded.borrow_mut() = root_layout;
    let mut child_layout = Layout::default();
    child_layout.location = Point::new(0.26, 0.26);
    child_layout.size = Size::new(4.74, 4.74);
    *tree.session_node(child).unrounded.borrow_mut() = child_layout;

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
    root_layout.location = Point::new(-0.75, 0.75);
    *tree.session_node(root).unrounded.borrow_mut() = root_layout;

    round_layout(tree.node(root), 2.0);

    assert_eq!(tree.final_layout(root).location, Point::new(-0.5, 1.0));
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

/// A minimal host that does not override `child_count`: the protocol
/// default must count through the `children()` iterator.
#[derive(Clone, Copy)]
struct CountingRef<'t> {
    tree: &'t MockTree,
    index: usize,
}

impl fmt::Debug for CountingRef<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("CountingRef")
            .field(&self.index)
            .finish()
    }
}

struct CountingChildren<'t> {
    tree: &'t MockTree,
    ids: std::slice::Iter<'t, usize>,
}

impl<'t> Iterator for CountingChildren<'t> {
    type Item = CountingRef<'t>;

    fn next(&mut self) -> Option<CountingRef<'t>> {
        let index = *self.ids.next()?;
        Some(CountingRef {
            tree: self.tree,
            index,
        })
    }
}

impl<'t> LayoutNode for CountingRef<'t> {
    type Style = &'t MockStyle;
    type ChildIter = CountingChildren<'t>;

    fn children(self) -> CountingChildren<'t> {
        CountingChildren {
            tree: self.tree,
            ids: self.tree.nodes[self.index].children.iter(),
        }
    }

    fn style(self) -> &'t MockStyle {
        &self.tree.nodes[self.index].style
    }

    fn compute_layout(self, input: LayoutInput) -> LayoutOutput {
        LayoutOutput::new(input.known_dimensions.unwrap_or(Size::ZERO), Size::ZERO)
    }

    fn set_unrounded_layout(self, layout: Layout) {
        *self.tree.session[self.index].unrounded.borrow_mut() = layout;
    }

    fn with_unrounded_layout<R>(self, read: impl FnOnce(&Layout) -> R) -> R {
        let layout = self.tree.session[self.index].unrounded.borrow();
        read(&layout)
    }

    fn set_rounded_layout(self, layout: Layout) {
        *self.tree.session[self.index].finalized.borrow_mut() = layout;
    }

    fn set_static_position(self, static_position: Point<f32>) {
        self.tree.session[self.index]
            .static_position
            .set(static_position);
    }

    fn cached_layout(self, _input: LayoutInput) -> Option<LayoutOutput> {
        None
    }

    fn store_cached_layout(self, _input: LayoutInput, _output: LayoutOutput) {}

    fn clear_layout_cache(self) {}
}

#[test]
fn default_child_count_counts_the_children_iterator() {
    let (tree, root) = leaf_tree();
    let handle = CountingRef {
        tree: &tree,
        index: root,
    };
    assert_eq!(handle.child_count(), 2);
    assert_eq!(
        CountingRef {
            tree: &tree,
            index: tree.nodes[root].children[0],
        }
        .child_count(),
        0
    );
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

    for node in [leaf, boundary, root] {
        tree.session_node(node).cache.borrow_mut().store(
            LayoutInput::default(),
            LayoutOutput::new(Size::new(1.0, 1.0), Size::ZERO),
        );
    }

    let re_root = invalidate_for_relayout(
        tree.node(leaf),
        [tree.node(boundary), tree.node(root)].into_iter(),
    );

    assert_eq!(re_root.index, boundary);
    assert_eq!(*tree.invalidated.borrow(), vec![leaf, boundary]);
    assert!(tree.session_node(leaf).cache.borrow().is_empty());
    assert!(tree.session_node(boundary).cache.borrow().is_empty());
    assert!(!tree.session_node(root).cache.borrow().is_empty());
}

#[test]
fn invalidate_for_relayout_walks_to_root_without_a_boundary() {
    let mut tree = MockTree::default();
    let leaf = tree.push(MockStyle::default(), vec![]);
    let mid = tree.push(contained_style(Contain::CONTENT), vec![leaf]);
    let root = tree.push(MockStyle::default(), vec![mid]);

    let re_root = invalidate_for_relayout(
        tree.node(leaf),
        [tree.node(mid), tree.node(root)].into_iter(),
    );

    assert_eq!(re_root.index, root);
    assert_eq!(*tree.invalidated.borrow(), vec![leaf, mid, root]);
}

#[test]
fn invalidate_for_relayout_returns_the_node_when_it_is_the_root() {
    let mut tree = MockTree::default();
    let lone = tree.push(MockStyle::default(), vec![]);

    let re_root = invalidate_for_relayout(tree.node(lone), std::iter::empty());

    assert_eq!(re_root.index, lone);
    assert_eq!(*tree.invalidated.borrow(), vec![lone]);
}

fn stretched_boundary_tree() -> (MockTree, usize, usize, usize) {
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

    tree.compute_layout(
        parent,
        LayoutInput::commit(Size::NONE, Size::NONE, Size::MAX_CONTENT),
    );
    (tree, parent, boundary, interior)
}

#[test]
fn compute_boundary_relayout_preserves_the_parent_imposed_size_and_updates_the_interior() {
    let (mut tree, parent, boundary, interior) = stretched_boundary_tree();

    let boundary_layout = tree.unrounded_layout(boundary);
    assert_eq!(boundary_layout.size, Size::new(50.0, 200.0));
    assert_eq!(tree.unrounded_layout(interior).size, Size::new(40.0, 200.0));

    let committed = tree
        .session_node(boundary)
        .cache
        .borrow()
        .committed_input()
        .expect("the boundary has a committed layout");

    tree.nodes[interior].style.size.width = size_px(20.0);

    let re_root = invalidate_for_relayout(
        tree.node(interior),
        [tree.node(boundary), tree.node(parent)].into_iter(),
    );
    assert_eq!(re_root.index, boundary);

    let output = compute_boundary_relayout(tree.node(boundary), committed);

    assert_eq!(output.size, Size::new(50.0, 200.0));
    let after = tree.unrounded_layout(boundary);
    assert_eq!(after.location, boundary_layout.location);
    assert_eq!(after.size, boundary_layout.size);
    assert_eq!(tree.unrounded_layout(interior).size, Size::new(20.0, 200.0));
}

#[test]
fn compute_root_layout_would_resize_a_stretched_boundary_regression_contrast() {
    let (tree, _parent, boundary, _interior) = stretched_boundary_tree();
    let stretched = tree.unrounded_layout(boundary).size;
    assert_eq!(stretched, Size::new(50.0, 200.0));

    compute_root_layout(
        tree.node(boundary),
        Size::new(
            AvailableSpace::Definite(300.0),
            AvailableSpace::Definite(200.0),
        ),
    );

    let self_determined = tree.unrounded_layout(boundary).size;
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
    let mut stale = tree.session_node(child).unrounded.borrow().clone();
    stale.size = Size::new(99.0, 99.0);
    *tree.session_node(child).unrounded.borrow_mut() = stale;

    let output = tree.compute_layout(
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
    assert_eq!(tree.unrounded_layout(child), Layout::default());
    assert!(tree.invalidated.borrow().contains(&child));
}
