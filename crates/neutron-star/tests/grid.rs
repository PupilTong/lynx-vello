//! Spec-focused CSS Grid integration tests over a plain `Vec`-backed host.

mod support;

use neutron_star::compute::compute_absolute_layout;
use neutron_star::prelude::*;
use stylo::computed_values::direction;
use stylo::values::computed::{
    AspectRatio, ContentDistribution, Display, FlexBasis, GridAutoFlow, GridLine,
    GridTemplateComponent, Integer, ItemPlacement, JustifyItems as ComputedJustifyItems,
    LengthPercentage, Margin, MaxSize, Overflow, PositionProperty, Ratio, SelfAlignment,
    Size as StyleSize, TrackBreadth, TrackSize,
};
use stylo::values::generics::NonNegative;
use stylo::values::generics::grid::{Flex, RepeatCount, TrackListValue};
use stylo::values::generics::position::PreferredRatio;
use stylo::values::specified::align::{AlignFlags, JustifyItems as SpecifiedJustifyItems};
use support::{
    TestId, TestMeasure, TestStyle, assert_close, assert_point, assert_size, border_px,
    breadth_px as fixed_breadth, definite_layout, gap_pct, gap_px, grid_line as line,
    grid_span as span, inset_px, justify_items, margin_px, max_px, npx as nn_px, px as lp,
    size_pct, size_px, snapshot_layout, track_auto as auto_track, track_fr as fr,
    track_max_content as max_content_track, track_minmax as minmax, track_pct as percent,
    track_px as px, track_repeat as repeat,
};

#[derive(Debug, Default)]
struct TestTree(support::TestTree);

impl core::ops::Deref for TestTree {
    type Target = support::TestTree;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl core::ops::DerefMut for TestTree {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl TestTree {
    fn push_leaf(
        &mut self,
        style: TestStyle,
        min_content_size: Size<f32>,
        max_content_size: Size<f32>,
    ) -> TestId {
        self.0
            .push_intrinsic_leaf(style, min_content_size, max_content_size)
    }

    fn set_first_baseline(&mut self, id: TestId, value: f32) {
        let TestMeasure::Intrinsic { first_baseline, .. } = &mut self.0.source_node_mut(id).measure
        else {
            panic!("baseline fixture must use intrinsic measurement");
        };
        *first_baseline = Some(value);
    }

    fn measure_call_count(&self, id: TestId) -> usize {
        self.0.measure_inputs(id).len()
    }

    fn install_layout_sentinel(&self, id: TestId) -> Layout {
        let mut sentinel = Layout::default();
        sentinel.location = Point::new(1_234.0, 5_678.0);
        sentinel.size = Size::new(9_876.0, 5_432.0);
        self.0
            .set_layout_for_testing(id, snapshot_layout(&sentinel));
        sentinel
    }
}

fn grid_default() -> TestStyle {
    TestStyle {
        display: Display::Grid,
        justify_items: ComputedJustifyItems {
            specified: SpecifiedJustifyItems::legacy(),
            computed: SpecifiedJustifyItems::normal(),
        },
        ..support::TestStyle::default()
    }
}

fn ratio(width: f32, height: f32) -> AspectRatio {
    AspectRatio {
        auto: false,
        ratio: PreferredRatio::Ratio(Ratio::new(width, height)),
    }
}

fn fit_content_track(limit: f32) -> TrackSize {
    TrackSize::FitContent(fixed_breadth(limit))
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
    support::template_values(values, auto_repeat_index)
}

fn tracks(sizes: &[TrackSize]) -> GridTemplateComponent {
    if sizes.is_empty() {
        return support::template_none();
    }
    support::track_list(sizes.to_vec())
}

fn implicit(sizes: &[TrackSize]) -> stylo::values::computed::ImplicitGridTracks {
    support::implicit_tracks(sizes.to_vec())
}

fn grid_style(columns: &[TrackSize], rows: &[TrackSize]) -> TestStyle {
    TestStyle {
        template_columns: tracks(columns),
        template_rows: tracks(rows),
        ..grid_default()
    }
}

fn fixed_leaf_style(width: f32, height: f32) -> TestStyle {
    TestStyle {
        size: Size::new(size_px(width), size_px(height)),
        ..grid_default()
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
    tree.push_leaf(grid_default(), min_content_size, max_content_size)
}

fn placement(start: GridLine, end: GridLine) -> Line<GridLine> {
    Line::new(start, end)
}

fn intrinsic_layout(tree: &TestTree, root: TestId) -> LayoutOutput {
    tree.compute_layout(
        root,
        LayoutInput::commit(Size::NONE, Size::NONE, Size::MAX_CONTENT),
    )
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
        ..grid_default()
    };
    let child = tree.push_leaf(child_style, Size::new(40.0, 10.0), Size::new(100.0, 10.0));
    let root = tree.push_grid(grid_style(&[percent(0.5)], &[px(20.0)]), vec![child]);

    let output = intrinsic_layout(&tree, root);

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
        ..grid_default()
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
        ..grid_default()
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
        ..grid_default()
    };
    let reversed = tree.push_leaf(reversed_style, Size::ZERO, Size::ZERO);
    let equal_style = TestStyle {
        grid_column: placement(line(2), line(2)),
        grid_row: placement(line(1), line(2)),
        ..grid_default()
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
        ..grid_default()
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
        ..grid_default()
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
    assert!((2..=6).contains(&tree.measure_call_count(intrinsic)));
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
        ..grid_default()
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
    assert!((2..=8).contains(&tree.measure_call_count(spanning)));
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
    tree.set_first_baseline(first, 15.0);
    let second = fixed_leaf(&mut tree, 20.0, 30.0);
    tree.set_first_baseline(second, 10.0);
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
    tree.set_first_baseline(first, 15.0);

    let mut second_style = fixed_leaf_style(20.0, 10.0);
    second_style.margin.top = Margin::Auto;
    let second = tree.push_leaf(second_style, Size::new(20.0, 10.0), Size::new(20.0, 10.0));
    tree.set_first_baseline(second, 5.0);

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
    tree.set_first_baseline(second, 5.0);
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
    tree.set_first_baseline(second_column, 12.0);

    let mut first_column_style = fixed_leaf_style(8.0, 10.0);
    first_column_style.grid_column = placement(line(1), line(2));
    first_column_style.grid_row = placement(line(1), line(2));
    let first_column = tree.push_leaf(
        first_column_style,
        Size::new(8.0, 10.0),
        Size::new(8.0, 10.0),
    );
    tree.set_first_baseline(first_column, 5.0);

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
    tree.set_layout_for_testing(child, snapshot_layout(&sentinel));
    tree.set_layout_for_testing(root, snapshot_layout(&sentinel));

    let output = tree.compute_layout(
        root,
        LayoutInput::measure(
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
    assert!((1..=6).contains(&tree.measure_call_count(child)));
}

#[test]
fn hidden_and_out_of_flow_children_do_not_occupy_grid_cells() {
    let mut tree = TestTree::default();
    let first = fixed_leaf(&mut tree, 10.0, 10.0);
    let mut hidden_style = fixed_leaf_style(1_000.0, 1_000.0);
    hidden_style.display = Display::None;
    let hidden = tree.push_leaf(hidden_style, Size::ZERO, Size::new(1_000.0, 1_000.0));
    let mut hidden_sentinel = tree.layout(hidden);
    hidden_sentinel.size = Size::new(999.0, 999.0);
    tree.set_layout_for_testing(hidden, hidden_sentinel);

    let mut absolute_style = fixed_leaf_style(20.0, 10.0);
    absolute_style.position = PositionProperty::Absolute;
    absolute_style.inset.left = inset_px(7.0);
    absolute_style.inset.top = inset_px(9.0);
    let absolute = tree.push_leaf(absolute_style, Size::new(20.0, 10.0), Size::new(20.0, 10.0));

    let mut hoisted_style = fixed_leaf_style(20.0, 10.0);
    hoisted_style.position = PositionProperty::Fixed;
    let hoisted = tree.push_leaf(hoisted_style, Size::new(20.0, 10.0), Size::new(20.0, 10.0));
    let hoisted_sentinel = tree.install_layout_sentinel(hoisted);
    let second = fixed_leaf(&mut tree, 10.0, 10.0);
    let mut style = grid_style(&[px(50.0), px(50.0)], &[px(20.0)]);
    style.justify_items = justify_items(AlignFlags::START);
    style.align_items = ItemPlacement(AlignFlags::START);
    let root = tree.push_grid(style, vec![first, hidden, absolute, hoisted, second]);

    definite_layout(&tree, root, 100.0, 20.0);

    assert_close(tree.layout(first).location.x, 0.0);
    assert_close(tree.layout(second).location.x, 50.0);
    assert_eq!(tree.layout(hidden).size, Size::ZERO);
    assert_eq!(tree.measure_call_count(hidden), 0);
    assert_point(tree.layout(absolute).location, Point::new(7.0, 9.0));
    assert_eq!(tree.layout(hoisted), hoisted_sentinel);
    assert_eq!(tree.static_position_writes.get(), 1);
    assert!(tree.static_position(hoisted).is_some());
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
        ..grid_default()
    };
    let child = tree.push_leaf(child_style, Size::ZERO, Size::ZERO);
    let mut style = grid_style(&[px(50.0), px(70.0)], &[px(30.0), px(40.0)]);
    style.gap = Size::new(gap_px(10.0), gap_px(5.0));
    let root = tree.push_grid(style, vec![child]);

    definite_layout(&tree, root, 130.0, 75.0);

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

    assert_point(tree.layout(child).location, Point::new(82.0, 0.0));
    assert_size(tree.layout(child).size, Size::new(5.0, 10.0));
}

#[test]
fn absolute_auto_grid_lines_use_the_container_padding_edges() {
    let mut tree = TestTree::default();
    let child_style = TestStyle {
        position: PositionProperty::Absolute,
        inset: Edges::uniform(inset_px(0.0)),
        ..grid_default()
    };
    let child = tree.push_leaf(child_style, Size::ZERO, Size::ZERO);
    let mut style = grid_style(&[], &[]);
    style.border = Edges::uniform(border_px(2.0));
    style.padding = Edges {
        left: nn_px(10.0),
        right: nn_px(20.0),
        top: nn_px(5.0),
        bottom: nn_px(15.0),
    };
    let root = tree.push_grid(style, vec![child]);

    definite_layout(&tree, root, 120.0, 80.0);

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

    assert_eq!(
        tree.session_node(ltr).static_position.get(),
        Some(Point::new(-20.0, 0.0))
    );
    assert_eq!(
        tree.session_node(rtl).static_position.get(),
        Some(Point::new(80.0, 0.0))
    );
    let static_position = tree.session_node(rtl).static_position.get().unwrap();
    let positioned = tree.with_layout_state(true, |tree, state| {
        compute_absolute_layout(
            tree,
            state,
            tree.node(rtl),
            Size::new(100.0, 50.0),
            static_position,
        )
    });
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
    let sentinel = tree.install_layout_sentinel(child);
    let root = tree.push_grid(grid_style(&[], &[]), vec![child]);

    definite_layout(&tree, root, 100.0, 50.0);

    assert_eq!(tree.layout(child), sentinel);
    assert_eq!(
        tree.session_node(child).static_position.get(),
        Some(Point::new(37.5, 37.0))
    );

    let static_position = tree.session_node(child).static_position.get().unwrap();
    let positioned = tree.with_layout_state(true, |tree, state| {
        compute_absolute_layout(
            tree,
            state,
            tree.node(child),
            Size::new(100.0, 50.0),
            static_position,
        )
    });
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
        ..grid_default()
    };
    let child = tree.push_leaf(child_style, Size::new(20.0, 10.0), Size::new(20.0, 10.0));
    let sentinel = tree.install_layout_sentinel(child);
    let root = tree.push_grid(grid_style(&[px(30.0), px(70.0)], &[px(50.0)]), vec![child]);

    definite_layout(&tree, root, 100.0, 50.0);

    assert!(tree.measure_call_count(child) > 0);
    assert_eq!(tree.layout(child), sentinel);
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
            ..grid_default()
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
    tree.enable_cache();
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
            ..grid_default()
        },
        vec![inner],
    );

    let output = intrinsic_layout(&tree, root);

    let grid_input = tree.committed_input(inner).unwrap();
    assert_close(grid_input.known_dimensions.width.unwrap(), 20.0);
    assert!(!grid_input.definite_dimensions.width);
    assert_close(output.size.width, 20.0);
    assert_size(tree.layout(inner).size, Size::new(20.0, 20.0));
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
        assert!((1..=MAX_PROBES_PER_ITEM).contains(&tree.measure_call_count(child)));
    }
}

fn min_content_layout(tree: &TestTree, root: TestId) -> LayoutOutput {
    tree.compute_layout(
        root,
        LayoutInput::commit(Size::NONE, Size::NONE, Size::MIN_CONTENT),
    )
}

#[test]
fn min_content_constraint_uses_zero_flex_fraction() {
    let mut tree = TestTree::default();
    let item = intrinsic_leaf(&mut tree, Size::new(20.0, 10.0), Size::new(80.0, 10.0));
    let root = tree.push_grid(grid_style(&[fr(1.0)], &[px(10.0)]), vec![item]);

    let output = min_content_layout(&tree, root);

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

    assert_size(output.size, Size::new(50.0, 10.0));
    assert_size(tree.layout(item).size, Size::new(50.0, 10.0));
}

#[test]
fn spanning_fixed_maximum_limit_includes_the_interior_gap() {
    let mut tree = TestTree::default();
    let item_style = TestStyle {
        grid_column: placement(line(1), line(3)),
        grid_row: placement(line(1), line(2)),
        ..grid_default()
    };
    let item = tree.push_leaf(item_style, Size::new(200.0, 10.0), Size::new(200.0, 10.0));
    let bounded = minmax(TrackBreadth::Auto, fixed_breadth(50.0));
    let mut style = grid_style(&[bounded.clone(), bounded], &[px(10.0)]);
    style.gap.width = gap_px(10.0);
    let root = tree.push_grid(style, vec![item]);

    let output = min_content_layout(&tree, root);

    assert_size(output.size, Size::new(110.0, 10.0));
    assert_size(tree.layout(item).size, Size::new(110.0, 10.0));
}

#[test]
fn max_content_spanning_contribution_is_limited_by_fixed_max_tracks() {
    let mut tree = TestTree::default();
    let item_style = TestStyle {
        grid_column: placement(line(1), line(3)),
        grid_row: placement(line(1), line(2)),
        min_size: Size::new(size_px(0.0), size_px(0.0)),
        ..grid_default()
    };
    let item = tree.push_leaf(item_style, Size::new(200.0, 10.0), Size::new(300.0, 10.0));
    let bounded = minmax(TrackBreadth::Auto, fixed_breadth(50.0));
    let mut style = grid_style(&[bounded.clone(), bounded], &[px(10.0)]);
    style.gap.width = gap_px(10.0);
    let root = tree.push_grid(style, vec![item]);

    let output = intrinsic_layout(&tree, root);

    assert_size(output.size, Size::new(110.0, 10.0));
    assert_size(tree.layout(item).size, Size::new(110.0, 10.0));
}

#[test]
fn multitrack_auto_minimum_contributes_to_intrinsic_track_sizes() {
    let mut tree = TestTree::default();
    let item_style = TestStyle {
        grid_column: placement(line(1), line(3)),
        grid_row: placement(line(1), line(2)),
        ..grid_default()
    };
    let item = tree.push_leaf(item_style, Size::new(200.0, 10.0), Size::new(200.0, 10.0));
    let root = tree.push_grid(
        grid_style(&[auto_track(), auto_track()], &[px(10.0)]),
        vec![item],
    );

    let output = min_content_layout(&tree, root);

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

    assert_close(tree.layout(marker).location.x, 100.0);
}

#[test]
fn baseline_shims_expand_an_intrinsic_row_before_following_rows_are_positioned() {
    let mut tree = TestTree::default();
    let mut first_style = fixed_leaf_style(20.0, 20.0);
    first_style.grid_column = placement(line(1), line(2));
    first_style.grid_row = placement(line(1), line(2));
    let first = tree.push_leaf(first_style, Size::new(20.0, 20.0), Size::new(20.0, 20.0));
    tree.set_first_baseline(first, 15.0);

    let mut second_style = fixed_leaf_style(20.0, 20.0);
    second_style.grid_column = placement(line(2), line(3));
    second_style.grid_row = placement(line(1), line(2));
    let second = tree.push_leaf(second_style, Size::new(20.0, 20.0), Size::new(20.0, 20.0));
    tree.set_first_baseline(second, 5.0);

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

    assert_close(output.size.width, 150.0);
    assert_close(tree.layout(marker).location.x, 180.0);
}

#[test]
fn spanning_scroll_item_uses_limited_min_content_under_intrinsic_constraint() {
    let mut tree = TestTree::default();
    let item_style = TestStyle {
        grid_column: placement(line(1), line(3)),
        grid_row: placement(line(1), line(2)),
        overflow: Point::new(Overflow::Hidden, Overflow::Visible),
        ..grid_default()
    };
    let item = tree.push_leaf(item_style, Size::new(200.0, 10.0), Size::new(200.0, 10.0));
    let root = tree.push_grid(
        grid_style(&[auto_track(), auto_track()], &[px(10.0)]),
        vec![item],
    );

    let output = min_content_layout(&tree, root);

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
        ..grid_default()
    };
    let first = tree.push_leaf(first_style, Size::new(10.0, 10.0), Size::new(10.0, 10.0));

    let spanning_style = TestStyle {
        grid_column: placement(line(1), line(3)),
        grid_row: placement(line(1), line(2)),
        min_size: Size::new(size_px(0.0), size_px(0.0)),
        justify_self: SelfAlignment(AlignFlags::START),
        ..grid_default()
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

    assert_close(output.size.width, 100.0);
    assert_close(tree.layout(marker).location.x, 10.0);
}

#[test]
fn single_track_intrinsic_base_floors_its_growth_limit_before_spanning_growth() {
    let mut tree = TestTree::default();
    let first_style = TestStyle {
        grid_column: placement(line(1), line(2)),
        grid_row: placement(line(1), line(2)),
        ..grid_default()
    };
    let first = tree.push_leaf(first_style, Size::new(100.0, 10.0), Size::new(100.0, 10.0));
    let spanning_style = TestStyle {
        grid_column: placement(line(1), line(3)),
        grid_row: placement(line(1), line(2)),
        min_size: Size::new(size_px(0.0), size_px(0.0)),
        ..grid_default()
    };
    let spanning = tree.push_leaf(spanning_style, Size::new(0.0, 10.0), Size::new(150.0, 10.0));
    let first_track = minmax(TrackBreadth::MinContent, fixed_breadth(50.0));
    let second_track = minmax(fixed_breadth(0.0), TrackBreadth::MaxContent);
    let root = tree.push_grid(
        grid_style(&[first_track, second_track], &[px(10.0)]),
        vec![first, spanning],
    );

    let output = intrinsic_layout(&tree, root);

    assert_size(output.size, Size::new(150.0, 10.0));
}

#[test]
fn spanning_base_uses_non_affected_track_before_exceeding_growth_limit() {
    let mut tree = TestTree::default();
    let spanning_style = TestStyle {
        grid_column: placement(line(1), line(3)),
        grid_row: placement(line(1), line(2)),
        justify_self: SelfAlignment(AlignFlags::START),
        ..grid_default()
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

    assert_close(tree.layout(marker).location.x, 10.0);
}

#[test]
fn normal_item_alignment_preserves_a_preferred_aspect_ratio() {
    let mut tree = TestTree::default();
    let child_style = TestStyle {
        aspect_ratio: ratio(2.0, 1.0),
        ..grid_default()
    };
    let child = tree.push_leaf(child_style, Size::new(20.0, 10.0), Size::new(20.0, 10.0));
    let root = tree.push_grid(grid_style(&[px(100.0)], &[px(100.0)]), vec![child]);

    definite_layout(&tree, root, 100.0, 100.0);

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
        ..grid_default()
    };
    let child = tree.push_leaf(child_style, Size::ZERO, Size::ZERO);
    let root = tree.push_grid(grid_style(&[px(200.0)], &[px(20.0)]), vec![child]);

    definite_layout(&tree, root, 100.0, 20.0);

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
        ..grid_default()
    };
    let child = tree.push_leaf(child_style, Size::ZERO, Size::ZERO);

    let mut style = grid_style(&[max_content_track()], &[px(20.0), px(20.0)]);
    style.align_content = ContentDistribution::new(AlignFlags::SPACE_BETWEEN);
    let root = tree.push_grid(style, vec![child]);
    let output = tree.compute_layout(
        root,
        LayoutInput::commit(
            Size::new(None, Some(100.0)),
            Size::new(None, Some(100.0)),
            Size::new(AvailableSpace::MaxContent, AvailableSpace::Definite(100.0)),
        ),
    );

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

#[test]
fn cross_size_dependent_ratio_item_forces_one_column_feedback_rerun() {
    let mut tree = TestTree::default();
    let fixed = fixed_leaf(&mut tree, 40.0, 80.0);
    let square_style = TestStyle {
        aspect_ratio: ratio(1.0, 1.0),
        align_self: SelfAlignment(AlignFlags::STRETCH),
        ..grid_default()
    };
    let square = tree.push_leaf(square_style, Size::new(10.0, 10.0), Size::new(10.0, 10.0));
    let style = grid_style(&[auto_track(), auto_track()], &[]);
    let root = tree.push_grid(style, vec![fixed, square]);

    let output = intrinsic_layout(&tree, root);

    assert_size(output.size, Size::new(120.0, 80.0));
    assert_point(tree.layout(square).location, Point::new(40.0, 0.0));
    assert_size(tree.layout(square).size, Size::new(80.0, 80.0));
    assert_size(tree.layout(fixed).size, Size::new(40.0, 80.0));
}

#[test]
fn container_baseline_prefers_first_row_synthesis_over_later_baseline_group() {
    let mut tree = TestTree::default();
    let top = fixed_leaf(&mut tree, 20.0, 10.0);
    let mut bottom_style = fixed_leaf_style(20.0, 10.0);
    bottom_style.align_self = SelfAlignment(AlignFlags::BASELINE);
    bottom_style.grid_row = placement(line(2), line(3));
    let bottom = tree.push_leaf(bottom_style, Size::new(20.0, 10.0), Size::new(20.0, 10.0));
    tree.set_first_baseline(bottom, 6.0);
    let mut style = grid_style(&[px(50.0)], &[px(30.0), px(30.0)]);
    style.align_items = ItemPlacement(AlignFlags::START);
    style.justify_items = justify_items(AlignFlags::START);
    let root = tree.push_grid(style, vec![top, bottom]);

    let output = definite_layout(&tree, root, 50.0, 60.0);

    assert_eq!(output.first_baselines.y, Some(10.0));
    assert_close(tree.layout(bottom).location.y, 30.0);
}

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

#[test]
fn intrinsic_keyword_minimum_and_maximum_sizes_clamp_grid_items() {
    let mut tree = support::TestTree::default();
    let cases: Vec<(support::TestStyle, f32)> = vec![
        (support::TestStyle::default(), 100.0),
        (
            support::TestStyle {
                max_size: Size::new(MaxSize::MinContent, MaxSize::none()),
                ..support::TestStyle::default()
            },
            30.0,
        ),
        (
            support::TestStyle {
                max_size: Size::new(MaxSize::MaxContent, MaxSize::none()),
                ..support::TestStyle::default()
            },
            90.0,
        ),
        (
            support::TestStyle {
                max_size: Size::new(max_px(60.0), MaxSize::none()),
                ..support::TestStyle::default()
            },
            60.0,
        ),
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

#[test]
fn physical_alignment_keywords_stay_physical_across_directions() {
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

    assert_close(
        track_x(AlignFlags::END, AlignFlags::START, direction::T::Ltr),
        70.0,
    );
    assert_close(
        track_x(AlignFlags::LEFT, AlignFlags::START, direction::T::Ltr),
        0.0,
    );
    assert_close(
        track_x(AlignFlags::LEFT, AlignFlags::START, direction::T::Rtl),
        0.0,
    );
    assert_close(
        track_x(AlignFlags::RIGHT, AlignFlags::START, direction::T::Ltr),
        70.0,
    );
    assert_close(
        track_x(AlignFlags::RIGHT, AlignFlags::START, direction::T::Rtl),
        70.0,
    );

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

#[test]
fn absolute_children_resolve_intrinsic_preferred_widths() {
    let mut tree = support::TestTree::default();
    let widths = [
        (StyleSize::MinContent, 30.0),
        (StyleSize::MaxContent, 90.0),
        (StyleSize::FitContentFunction(NonNegative(lp(60.0))), 60.0),
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

#[test]
fn absolute_defensive_placements_fall_back_to_padding_edges() {
    let mut tree = TestTree::default();
    let mut zero_start = fixed_leaf_style(10.0, 10.0);
    zero_start.position = PositionProperty::Absolute;
    zero_start.grid_column = placement(line(0), line(2));
    zero_start.inset.left = inset_px(0.0);
    let zero_start = tree.push_leaf(zero_start, Size::new(10.0, 10.0), Size::new(10.0, 10.0));

    let zero_end_style = TestStyle {
        position: PositionProperty::Absolute,
        inset: Edges::uniform(inset_px(0.0)),
        grid_column: placement(GridLine::auto(), line(0)),
        ..grid_default()
    };
    let zero_end = tree.push_leaf(zero_end_style, Size::ZERO, Size::ZERO);

    let span_span_style = TestStyle {
        position: PositionProperty::Absolute,
        inset: Edges::uniform(inset_px(0.0)),
        grid_column: placement(span(1), span(1)),
        ..grid_default()
    };
    let span_span = tree.push_leaf(span_span_style, Size::ZERO, Size::ZERO);

    let span_line_style = TestStyle {
        position: PositionProperty::Absolute,
        inset: Edges::uniform(inset_px(0.0)),
        grid_column: placement(span(1), line(2)),
        ..grid_default()
    };
    let span_line = tree.push_leaf(span_line_style, Size::ZERO, Size::ZERO);

    let style = grid_style(&[px(60.0), px(40.0)], &[px(20.0)]);
    let root = tree.push_grid(style, vec![zero_start, zero_end, span_span, span_line]);

    definite_layout(&tree, root, 100.0, 20.0);

    assert_close(tree.layout(zero_start).location.x, 0.0);
    assert_size(tree.layout(zero_end).size, Size::new(100.0, 20.0));
    assert_size(tree.layout(span_span).size, Size::new(100.0, 20.0));
    assert_close(tree.layout(span_line).location.x, 0.0);
    assert_size(tree.layout(span_line).size, Size::new(60.0, 20.0));
}

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
    assert_point(tree.layout(margin_item).location, Point::new(0.0, 40.0));
    let margin = tree.layout(margin_item).margin;
    assert_close(margin.right, 80.0);
    assert_close(margin.bottom, 20.0);
}

#[test]
fn auto_fill_counts_use_definite_breadths_or_run_once() {
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

    let positions = columns_of(minmax(TrackBreadth::Auto, fixed_breadth(40.0)), 100.0, 10.0);
    assert_eq!(positions, vec![0.0, 50.0, 0.0]);

    let positions = columns_of(
        minmax(TrackBreadth::MinContent, fixed_breadth(40.0)),
        90.0,
        0.0,
    );
    assert_eq!(positions, vec![0.0, 40.0, 0.0]);

    let positions = columns_of(
        minmax(fixed_breadth(20.0), TrackBreadth::MinContent),
        50.0,
        0.0,
    );
    assert_eq!(positions, vec![0.0, 20.0, 0.0]);

    let positions = columns_of(
        minmax(fixed_breadth(30.0), TrackBreadth::MaxContent),
        70.0,
        0.0,
    );
    assert_eq!(positions, vec![0.0, 30.0, 0.0]);

    let positions = columns_of(
        minmax(TrackBreadth::MaxContent, fixed_breadth(40.0)),
        90.0,
        0.0,
    );
    assert_eq!(positions, vec![0.0, 40.0, 0.0]);

    let positions = columns_of(auto_track(), 300.0, 0.0);
    assert_eq!(positions, vec![0.0, 0.0, 0.0]);

    let positions = columns_of(px(200.0), 100.0, 0.0);
    assert_eq!(positions, vec![0.0, 0.0, 0.0]);

    let positions = columns_of(px(0.0), 100.0, 0.0);
    assert_eq!(positions, vec![0.0, 0.0, 0.0]);
}

#[test]
fn empty_template_tracks_size_an_itemless_grid() {
    let mut tree = TestTree::default();
    let style = grid_style(&[auto_track(), px(50.0)], &[auto_track()]);
    let root = tree.push_grid(style, Vec::new());

    let output = intrinsic_layout(&tree, root);

    assert_size(output.size, Size::new(50.0, 0.0));
}

#[test]
fn equal_span_groups_distribute_min_contributions_together() {
    let mut tree = TestTree::default();
    let narrow_style = TestStyle {
        grid_column: placement(line(1), line(3)),
        ..grid_default()
    };
    let narrow = tree.push_leaf(narrow_style, Size::new(60.0, 10.0), Size::new(60.0, 10.0));
    let wide_style = TestStyle {
        grid_column: placement(line(1), line(3)),
        ..grid_default()
    };
    let wide = tree.push_leaf(wide_style, Size::new(80.0, 10.0), Size::new(80.0, 10.0));
    let mut style = grid_style(&[auto_track(), auto_track()], &[]);
    style.justify_content = ContentDistribution::new(AlignFlags::START);
    let root = tree.push_grid(style, vec![narrow, wide]);

    definite_layout(&tree, root, 200.0, 20.0);

    assert_size(tree.layout(narrow).size, Size::new(80.0, 10.0));
    assert_size(tree.layout(wide).size, Size::new(80.0, 10.0));
}

#[test]
fn container_intrinsic_measures_size_auto_and_flexible_tracks() {
    let measure = |track: TrackSize, available: AvailableSpace| -> f32 {
        let mut tree = TestTree::default();
        let item = intrinsic_leaf(&mut tree, Size::new(30.0, 10.0), Size::new(90.0, 10.0));
        let style = grid_style(&[track], &[]);
        let root = tree.push_grid(style, vec![item]);
        let output = tree.compute_layout(
            root,
            LayoutInput::measure(
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
    assert_close(tree.layout(item).size.width, 30.0);

    let mut tree = TestTree::default();
    let mut probe_style = fixed_leaf_style(1.0, 10.0);
    probe_style.grid_column = placement(line(-1), line(-2));
    let probe = tree.push_leaf(probe_style, Size::new(1.0, 10.0), Size::new(1.0, 10.0));
    let mut style = grid_style(&[], &[px(10.0)]);
    style.template_columns = track_list(vec![repeat(RepeatCount::Number(40_000), vec![px(1.0)])]);
    let root = tree.push_grid(style, vec![probe]);
    let output = intrinsic_layout(&tree, root);
    assert_size(output.size, Size::new(10_000.0, 10.0));
    assert_close(tree.layout(probe).location.x, 9_999.0);
}

#[test]
fn intrinsic_keyword_heights_resolve_against_content() {
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

#[test]
fn container_intrinsic_keyword_widths_override_available_space() {
    let width_of = |width: StyleSize| -> f32 {
        let mut tree = TestTree::default();
        let item = intrinsic_leaf(&mut tree, Size::new(30.0, 10.0), Size::new(90.0, 10.0));
        let mut style = grid_style(&[auto_track()], &[]);
        style.size = Size::new(width, StyleSize::Auto);
        let root = tree.push_grid(style, vec![item]);
        let output = tree.compute_layout(
            root,
            LayoutInput::commit(
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

    let height_of = |height: StyleSize| -> f32 {
        let mut tree = TestTree::default();
        let item = intrinsic_leaf(&mut tree, Size::new(30.0, 10.0), Size::new(90.0, 40.0));
        let mut style = grid_style(&[px(90.0)], &[auto_track()]);
        style.size = Size::new(StyleSize::Auto, height);
        let root = tree.push_grid(style, vec![item]);
        let output = tree.compute_layout(
            root,
            LayoutInput::commit(
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

#[test]
fn placement_binds_end_lines_and_flows_around_definite_items() {
    let mut tree = TestTree::default();
    let mut style = fixed_leaf_style(10.0, 10.0);
    style.grid_column = placement(GridLine::auto(), line(3));
    let item = tree.push_leaf(style, Size::new(10.0, 10.0), Size::new(10.0, 10.0));
    let mut grid = grid_style(&[px(20.0), px(20.0), px(20.0)], &[px(10.0)]);
    grid.justify_items = justify_items(AlignFlags::START);
    let root = tree.push_grid(grid, vec![item]);
    definite_layout(&tree, root, 60.0, 10.0);
    assert_close(tree.layout(item).location.x, 20.0);

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
    assert_point(third, Point::new(0.0, 10.0));

    let (first, second, third) = sparse(true);
    assert_point(first, Point::new(0.0, 0.0));
    assert_point(second, Point::new(20.0, 0.0));
    assert_point(third, Point::new(0.0, 10.0));
}

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

    assert_size(output.size, Size::new(10_000.0, 10.0));
    assert_close(tree.layout(probe).location.x, 9_999.0);
}
