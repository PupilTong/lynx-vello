//! CSS Grid Layout Level 3 grid-lanes integration tests over a plain
//! `Vec`-backed host.

mod support;

use hughie::prelude::*;
use stylo::computed_values::direction;
use stylo::values::computed::{Contain, Display, Overflow, PositionProperty};
use stylo::values::specified::align::AlignFlags;
use support::{
    TestId, TestRepeatCount, TestStyle, TestTrack, assert_close, assert_point, assert_size,
    content, gap_px, grid_line as line, grid_span as span, implicit_tracks, inset_px,
    justify_items, margin_auto, margin_px, max_px, self_align, size_auto, size_px, snapshot_layout,
    template_none, template_values, tolerance_infinite, tolerance_normal, tolerance_pct,
    tolerance_px, track_fr as fr, track_list, track_max_content as max_content,
    track_min_content as min_content, track_pct, track_px as px, track_repeat,
};

type TestTree = support::TestTree;

fn lanes_style(columns: &[TestTrack], rows: &[TestTrack]) -> TestStyle {
    TestStyle {
        display: Display::GridLanes,
        template_columns: if columns.is_empty() {
            template_none()
        } else {
            track_list(columns.to_vec())
        },
        template_rows: if rows.is_empty() {
            template_none()
        } else {
            track_list(rows.to_vec())
        },
        justify_items: justify_items(AlignFlags::NORMAL),
        // Every trace below is written against an exact shortest-lane rule;
        // the `normal` (1em) tie threshold gets its own test.
        flow_tolerance: tolerance_px(0.0),
        ..TestStyle::default()
    }
}

fn leaf_style(width: f32, height: f32) -> TestStyle {
    TestStyle {
        size: Size::new(size_px(width), size_px(height)),
        ..TestStyle::default()
    }
}

fn fixed(tree: &mut TestTree, width: f32, height: f32) -> TestId {
    tree.push_leaf(leaf_style(width, height), Size::new(width, height), None)
}

fn styled(tree: &mut TestTree, style: TestStyle, width: f32, height: f32) -> TestId {
    tree.push_leaf(style, Size::new(width, height), None)
}

fn auto_layout(tree: &TestTree, root: TestId) -> LayoutOutput {
    support::perform_layout(tree, root, Size::NONE, Size::MAX_CONTENT)
}

fn sized_layout(
    tree: &TestTree,
    root: TestId,
    width: Option<f32>,
    height: Option<f32>,
) -> LayoutOutput {
    let available = |value: Option<f32>| value.map_or(AvailableSpace::MaxContent, Into::into);
    support::perform_layout(
        tree,
        root,
        Size::new(width, height),
        Size::new(available(width), available(height)),
    )
}

fn locations(tree: &TestTree, ids: &[TestId]) -> Vec<Point<f32>> {
    ids.iter().map(|&id| tree.layout(id).location).collect()
}

#[test]
fn orientation_follows_the_templates_that_name_tracks() {
    // §2.3: the block axis carries the tracks exactly when `grid-template-rows`
    // alone names any; every other combination lays lanes out as columns.
    let cases: [(Vec<TestTrack>, Vec<TestTrack>, Size<f32>); 4] = [
        (Vec::new(), Vec::new(), Size::new(20.0, 40.0)),
        (vec![px(30.0)], Vec::new(), Size::new(30.0, 40.0)),
        (Vec::new(), vec![px(30.0)], Size::new(40.0, 30.0)),
        (vec![px(30.0)], vec![px(40.0)], Size::new(30.0, 40.0)),
    ];
    for (columns, rows, expected) in cases {
        let mut tree = TestTree::default();
        let first = fixed(&mut tree, 20.0, 20.0);
        let second = fixed(&mut tree, 20.0, 20.0);
        let root = tree.push_grid_lanes(lanes_style(&columns, &rows), vec![first, second]);

        assert_size(auto_layout(&tree, root).size, expected);
    }
}

#[test]
fn a_container_without_items_or_tracks_is_its_own_box() {
    let mut tree = TestTree::default();
    let root = tree.push_grid_lanes(
        TestStyle {
            padding: Edges::uniform(support::npx(4.0)),
            ..lanes_style(&[], &[])
        },
        Vec::new(),
    );

    let output = auto_layout(&tree, root);

    assert_size(output.size, Size::new(8.0, 8.0));
    assert_size(output.content_size, Size::new(8.0, 8.0));
    assert_eq!(output.first_baselines.y, None);
}

#[test]
fn items_land_in_the_shortest_lane_and_stack_from_its_end() {
    let mut tree = TestTree::default();
    let heights = [30.0, 20.0, 10.0, 25.0];
    let children = heights
        .iter()
        .map(|&height| fixed(&mut tree, 10.0, height))
        .collect::<Vec<_>>();
    let root = tree.push_grid_lanes(
        lanes_style(&[px(50.0), px(50.0), px(50.0)], &[]),
        children.clone(),
    );

    let output = auto_layout(&tree, root);

    assert_size(output.size, Size::new(150.0, 35.0));
    assert_eq!(
        locations(&tree, &children),
        vec![
            Point::new(0.0, 0.0),
            Point::new(50.0, 0.0),
            Point::new(100.0, 0.0),
            // The third lane is the shortest, and nothing sits at or after the
            // cursor, so the first possible lane wins.
            Point::new(100.0, 10.0),
        ]
    );
}

#[test]
fn flow_tolerance_decides_which_lanes_count_as_equally_short() {
    // The same six items, laid out under four tie thresholds.
    let cases = [
        (
            tolerance_px(0.0),
            16.0,
            vec![0.0, 0.0, 0.0, 10.0, 20.0, 30.0],
        ),
        (
            tolerance_normal(),
            16.0,
            vec![0.0, 0.0, 0.0, 20.0, 10.0, 30.0],
        ),
        (
            tolerance_normal(),
            40.0,
            vec![0.0, 0.0, 0.0, 30.0, 20.0, 10.0],
        ),
    ];
    let lanes = [
        vec![0.0, 50.0, 100.0, 100.0, 50.0, 100.0],
        vec![0.0, 50.0, 100.0, 50.0, 100.0, 0.0],
        vec![0.0, 50.0, 100.0, 0.0, 50.0, 100.0],
    ];
    for ((tolerance, font_size, tops), lefts) in cases.into_iter().zip(lanes) {
        let mut tree = TestTree::default();
        let children = [30.0, 20.0, 10.0, 20.0, 10.0, 5.0]
            .iter()
            .map(|&height| fixed(&mut tree, 10.0, height))
            .collect::<Vec<_>>();
        let root = tree.push_grid_lanes(
            TestStyle {
                flow_tolerance: tolerance,
                font_size,
                ..lanes_style(&[px(50.0), px(50.0), px(50.0)], &[])
            },
            children.clone(),
        );

        auto_layout(&tree, root);

        assert_eq!(
            locations(&tree, &children),
            lefts
                .into_iter()
                .zip(tops)
                .map(|(left, top)| Point::new(left, top))
                .collect::<Vec<_>>()
        );
    }
}

#[test]
fn a_percentage_tolerance_resolves_against_the_used_grid_axis_size() {
    for (tolerance, expected_last) in [
        // 20% of the two 50px tracks the auto-width container resolved to.
        (tolerance_pct(0.2), Point::new(0.0, 30.0)),
        (tolerance_px(0.0), Point::new(50.0, 15.0)),
    ] {
        let mut tree = TestTree::default();
        let children = [30.0, 15.0, 5.0]
            .iter()
            .map(|&height| fixed(&mut tree, 10.0, height))
            .collect::<Vec<_>>();
        let root = tree.push_grid_lanes(
            TestStyle {
                flow_tolerance: tolerance,
                ..lanes_style(&[px(50.0), px(50.0)], &[])
            },
            children.clone(),
        );

        auto_layout(&tree, root);

        assert_point(tree.layout(children[2]).location, expected_last);
    }
}

#[test]
fn an_infinite_tolerance_distributes_items_strictly_in_order() {
    for (tolerance, expected_last) in [
        (tolerance_infinite(), Point::new(0.0, 1000.0)),
        (tolerance_px(40.0), Point::new(50.0, 1.0)),
    ] {
        let mut tree = TestTree::default();
        let children = [1000.0, 1.0, 1.0, 1.0]
            .iter()
            .map(|&height| fixed(&mut tree, 10.0, height))
            .collect::<Vec<_>>();
        let root = tree.push_grid_lanes(
            TestStyle {
                flow_tolerance: tolerance,
                ..lanes_style(&[px(50.0), px(50.0), px(50.0)], &[])
            },
            children.clone(),
        );

        auto_layout(&tree, root);

        // Once the cursor has passed the last lane it wraps to the first
        // possible one, which `infinite` makes the tallest lane of all.
        assert_point(tree.layout(children[3]).location, expected_last);
    }
}

#[test]
fn an_auto_placed_span_takes_consecutive_lanes_and_moves_the_cursor_past_them() {
    let mut tree = TestTree::default();
    let wide = styled(
        &mut tree,
        TestStyle {
            size: Size::new(size_auto(), size_px(30.0)),
            grid_column: Line::new(span(2), support::grid_auto_placement()),
            ..TestStyle::default()
        },
        10.0,
        30.0,
    );
    let narrow = fixed(&mut tree, 10.0, 10.0);
    let root = tree.push_grid_lanes(
        lanes_style(&[px(50.0), px(50.0), px(50.0)], &[]),
        vec![wide, narrow],
    );

    let output = auto_layout(&tree, root);

    assert_size(output.size, Size::new(150.0, 30.0));
    assert_point(tree.layout(wide).location, Point::new(0.0, 0.0));
    assert_size(tree.layout(wide).size, Size::new(100.0, 30.0));
    assert_point(tree.layout(narrow).location, Point::new(100.0, 0.0));
}

#[test]
fn a_span_wider_than_the_explicit_grid_materialises_implicit_tracks() {
    let mut tree = TestTree::default();
    let wide = styled(
        &mut tree,
        TestStyle {
            size: Size::new(size_auto(), size_px(20.0)),
            grid_column: Line::new(span(3), support::grid_auto_placement()),
            ..TestStyle::default()
        },
        10.0,
        20.0,
    );
    let root = tree.push_grid_lanes(
        TestStyle {
            auto_columns: implicit_tracks(vec![px(30.0)]),
            ..lanes_style(&[px(50.0), px(50.0)], &[])
        },
        vec![wide],
    );

    let output = auto_layout(&tree, root);

    assert_size(output.size, Size::new(130.0, 20.0));
    assert_size(tree.layout(wide).size, Size::new(130.0, 20.0));
}

#[test]
fn a_definite_position_is_honoured_without_moving_the_cursor() {
    let mut tree = TestTree::default();
    let first = fixed(&mut tree, 10.0, 20.0);
    let second = fixed(&mut tree, 10.0, 10.0);
    let full = styled(
        &mut tree,
        TestStyle {
            size: Size::new(size_auto(), size_px(5.0)),
            grid_column: Line::new(line(1), line(-1)),
            ..TestStyle::default()
        },
        10.0,
        5.0,
    );
    let last = fixed(&mut tree, 10.0, 5.0);
    let root = tree.push_grid_lanes(
        lanes_style(&[px(50.0), px(50.0), px(50.0)], &[]),
        vec![first, second, full, last],
    );

    let output = auto_layout(&tree, root);

    assert_size(output.size, Size::new(150.0, 30.0));
    assert_point(tree.layout(full).location, Point::new(0.0, 20.0));
    assert_size(tree.layout(full).size, Size::new(150.0, 5.0));
    // The cursor still points past the second item, so the last one takes the
    // third lane rather than wrapping to the first.
    assert_point(tree.layout(last).location, Point::new(100.0, 25.0));
}

#[test]
fn a_negative_definite_line_materialises_tracks_before_the_explicit_grid() {
    let mut tree = TestTree::default();
    let before = styled(
        &mut tree,
        TestStyle {
            size: Size::new(size_auto(), size_px(30.0)),
            grid_column: Line::new(line(-5), line(-4)),
            ..TestStyle::default()
        },
        10.0,
        30.0,
    );
    let flowing = fixed(&mut tree, 10.0, 10.0);
    let root = tree.push_grid_lanes(
        TestStyle {
            auto_columns: implicit_tracks(vec![px(20.0)]),
            ..lanes_style(&[px(50.0), px(50.0), px(50.0)], &[])
        },
        vec![before, flowing],
    );

    let output = auto_layout(&tree, root);

    assert_size(output.size, Size::new(170.0, 30.0));
    assert_point(tree.layout(before).location, Point::new(0.0, 0.0));
    assert_size(tree.layout(before).size, Size::new(20.0, 30.0));
    assert_point(tree.layout(flowing).location, Point::new(20.0, 0.0));
}

#[test]
fn gutters_separate_tracks_and_items_without_a_trailing_one() {
    let mut tree = TestTree::default();
    let children = (0..3)
        .map(|_| fixed(&mut tree, 10.0, 20.0))
        .collect::<Vec<_>>();
    let root = tree.push_grid_lanes(
        TestStyle {
            gap: Size::new(gap_px(10.0), gap_px(6.0)),
            flow_tolerance: tolerance_normal(),
            ..lanes_style(&[px(50.0), px(50.0)], &[])
        },
        children.clone(),
    );

    let output = auto_layout(&tree, root);

    // 50 + 10 + 50 across, and 20 + 6 + 20 down with no gutter after the last
    // item in a lane.
    assert_size(output.size, Size::new(110.0, 46.0));
    assert_eq!(
        locations(&tree, &children),
        vec![
            Point::new(0.0, 0.0),
            Point::new(60.0, 0.0),
            Point::new(0.0, 26.0),
        ]
    );
}

#[test]
fn order_modified_document_order_drives_placement_and_paint_order() {
    let mut tree = TestTree::default();
    let later = styled(
        &mut tree,
        TestStyle {
            order: 1,
            ..leaf_style(10.0, 30.0)
        },
        10.0,
        30.0,
    );
    let earlier = styled(
        &mut tree,
        TestStyle {
            order: 0,
            ..leaf_style(10.0, 10.0)
        },
        10.0,
        10.0,
    );
    let root = tree.push_grid_lanes(
        lanes_style(&[px(50.0), px(50.0)], &[]),
        vec![later, earlier],
    );

    auto_layout(&tree, root);

    assert_point(tree.layout(earlier).location, Point::new(0.0, 0.0));
    assert_point(tree.layout(later).location, Point::new(50.0, 0.0));
    assert_eq!(tree.layout(earlier).order, 0);
    assert_eq!(tree.layout(later).order, 1);
}

#[test]
fn hidden_and_out_of_flow_children_take_no_lane() {
    let mut tree = TestTree::default();
    let first = fixed(&mut tree, 10.0, 20.0);
    let hidden = styled(
        &mut tree,
        TestStyle {
            display: Display::None,
            ..leaf_style(10.0, 90.0)
        },
        10.0,
        90.0,
    );
    let positioned = styled(
        &mut tree,
        TestStyle {
            position: PositionProperty::Absolute,
            inset: Edges::uniform(inset_px(0.0)),
            size: Size::new(size_auto(), size_auto()),
            ..TestStyle::default()
        },
        10.0,
        90.0,
    );
    let second = fixed(&mut tree, 10.0, 10.0);
    let root = tree.push_grid_lanes(
        lanes_style(&[px(50.0), px(50.0)], &[]),
        vec![first, hidden, positioned, second],
    );

    let output = auto_layout(&tree, root);

    assert_size(output.size, Size::new(100.0, 20.0));
    assert_point(tree.layout(first).location, Point::new(0.0, 0.0));
    assert_point(tree.layout(second).location, Point::new(50.0, 0.0));
    assert_size(tree.layout(hidden).size, Size::ZERO);
    assert_size(tree.layout(positioned).size, Size::new(100.0, 20.0));
}

#[test]
fn absolute_children_resolve_against_grid_lines_and_the_stacking_range() {
    let mut tree = TestTree::default();
    let first = fixed(&mut tree, 10.0, 30.0);
    let second = fixed(&mut tree, 10.0, 20.0);
    let in_range = styled(
        &mut tree,
        TestStyle {
            position: PositionProperty::Absolute,
            inset: Edges::uniform(inset_px(0.0)),
            size: Size::new(size_auto(), size_auto()),
            grid_column: Line::new(line(2), line(3)),
            grid_row: Line::new(line(1), line(2)),
            ..TestStyle::default()
        },
        10.0,
        10.0,
    );
    let padding_box = styled(
        &mut tree,
        TestStyle {
            position: PositionProperty::Absolute,
            inset: Edges::uniform(inset_px(0.0)),
            size: Size::new(size_auto(), size_auto()),
            ..TestStyle::default()
        },
        10.0,
        10.0,
    );
    let root = tree.push_grid_lanes(
        lanes_style(&[px(50.0), px(50.0)], &[]),
        vec![first, second, in_range, padding_box],
    );

    sized_layout(&tree, root, None, Some(50.0));

    // §8: stacking lines 1 and 2 are the range's own edges.
    assert_point(tree.layout(in_range).location, Point::new(50.0, 0.0));
    assert_size(tree.layout(in_range).size, Size::new(50.0, 30.0));
    // An auto stacking placement still resolves against the padding edge.
    assert_point(tree.layout(padding_box).location, Point::ZERO);
    assert_size(tree.layout(padding_box).size, Size::new(100.0, 50.0));
}

#[test]
fn row_lanes_count_negative_absolute_lines_from_each_axis_own_end() {
    let mut tree = TestTree::default();
    let first = fixed(&mut tree, 30.0, 20.0);
    let second = fixed(&mut tree, 20.0, 20.0);
    let positioned = styled(
        &mut tree,
        TestStyle {
            position: PositionProperty::Absolute,
            inset: Edges::uniform(inset_px(0.0)),
            size: Size::new(size_auto(), size_auto()),
            grid_row: Line::new(line(-2), line(-1)),
            grid_column: Line::new(line(-2), line(-1)),
            ..TestStyle::default()
        },
        10.0,
        10.0,
    );
    let root = tree.push_grid_lanes(
        lanes_style(&[], &[px(40.0), px(40.0)]),
        vec![first, second, positioned],
    );

    auto_layout(&tree, root);

    // The grid axis counts back over both explicit rows; the stacking axis has
    // only its own two lines, so -2 / -1 is the whole range.
    assert_point(tree.layout(positioned).location, Point::new(0.0, 40.0));
    assert_size(tree.layout(positioned).size, Size::new(30.0, 40.0));
}

#[test]
fn an_auto_placed_item_grows_every_flexible_lane() {
    let mut tree = TestTree::default();
    let wide = tree.push_intrinsic_leaf(
        TestStyle {
            size: Size::new(size_auto(), size_px(20.0)),
            ..TestStyle::default()
        },
        Size::new(150.0, 20.0),
        Size::new(150.0, 20.0),
    );
    let marker = styled(
        &mut tree,
        TestStyle {
            grid_column: Line::new(line(3), line(4)),
            ..leaf_style(10.0, 10.0)
        },
        10.0,
        10.0,
    );
    let root = tree.push_grid_lanes(
        lanes_style(&[fr(1.0), fr(1.0), fr(1.0)], &[]),
        vec![wide, marker],
    );

    let output = sized_layout(&tree, root, Some(300.0), None);

    // The auto-placed item contributes at every start position, so all three
    // lanes carry its 150px automatic minimum.
    assert_size(tree.layout(wide).size, Size::new(150.0, 20.0));
    assert_close(tree.layout(marker).location.x, 300.0);
    assert_close(output.content_size.width, 310.0);
}

/// `repeat(n, minmax(0, 1fr))` — the template the Lynx `<list
/// list-type="waterfall">` component lowers onto. The fixed `0` minimum is what
/// makes it differ from `1fr` (= `minmax(auto, 1fr)`): it suppresses the
/// automatic minimum size, so nothing an item contributes can grow a lane.
fn zero_minimum_flexible_lanes() -> Vec<TestTrack> {
    vec![support::track_minmax(support::breadth_px(0.0), support::breadth_fr(1.0)); 2]
}

fn wide_item(tree: &mut TestTree) -> TestId {
    tree.push_intrinsic_leaf(
        TestStyle {
            size: Size::new(size_auto(), size_px(20.0)),
            ..TestStyle::default()
        },
        Size::new(150.0, 20.0),
        Size::new(150.0, 20.0),
    )
}

#[test]
fn zero_minimum_flexible_lanes_keep_equal_shares_under_a_definite_grid_axis() {
    let mut tree = TestTree::default();
    let wide = wide_item(&mut tree);
    let marker = styled(
        &mut tree,
        TestStyle {
            grid_column: Line::new(line(2), line(3)),
            ..leaf_style(10.0, 10.0)
        },
        10.0,
        10.0,
    );
    let root = tree.push_grid_lanes(
        lanes_style(&zero_minimum_flexible_lanes(), &[]),
        vec![wide, marker],
    );

    sized_layout(&tree, root, Some(200.0), None);

    // Both lanes stay at their `1fr` share even though the item's min-content
    // width overflows one.
    assert_close(tree.layout(marker).location.x, 100.0);
    assert_size(tree.layout(wide).size, Size::new(100.0, 20.0));
    // One call for the container plus one per item in the placement pass:
    // track sizing issued none of its own, because every contribution a
    // fixed-minimum flexible track collects resolves without a measurement.
    assert_eq!(tree.child_layout_calls.get(), 3);
}

#[test]
fn zero_minimum_flexible_lanes_still_grow_under_an_indefinite_grid_axis() {
    let mut tree = TestTree::default();
    let wide = wide_item(&mut tree);
    let marker = styled(
        &mut tree,
        TestStyle {
            grid_column: Line::new(line(2), line(3)),
            ..leaf_style(10.0, 10.0)
        },
        10.0,
        10.0,
    );
    let root = tree.push_grid_lanes(
        lanes_style(&zero_minimum_flexible_lanes(), &[]),
        vec![wide, marker],
    );

    let output = auto_layout(&tree, root);

    // Under a max-content constraint §12.7 sizes each `fr` from the items that
    // could land in it, so the auto-placed item grows both lanes.
    assert_size(output.size, Size::new(300.0, 20.0));
    assert_close(tree.layout(marker).location.x, 150.0);
}

#[test]
fn a_scroll_container_item_contributes_no_automatic_minimum() {
    let mut tree = TestTree::default();
    let wide = tree.push_intrinsic_leaf(
        TestStyle {
            size: Size::new(size_auto(), size_px(20.0)),
            overflow: Point::new(Overflow::Hidden, Overflow::Hidden),
            ..TestStyle::default()
        },
        Size::new(150.0, 20.0),
        Size::new(150.0, 20.0),
    );
    let marker = styled(
        &mut tree,
        TestStyle {
            grid_column: Line::new(line(3), line(4)),
            ..leaf_style(10.0, 10.0)
        },
        10.0,
        10.0,
    );
    let root = tree.push_grid_lanes(
        lanes_style(&[fr(1.0), fr(1.0), fr(1.0)], &[]),
        vec![wide, marker],
    );

    sized_layout(&tree, root, Some(300.0), None);

    assert_close(tree.layout(marker).location.x, 200.0);
}

/// A leaf with a fixed content area: whichever dimension is left open comes out
/// at 3000 divided by the other, and an unconstrained axis falls back to 100.
fn area_leaf(input: hughie::compute::LeafMeasureInput) -> LeafMetrics {
    let bounded = |known: Option<f32>, available: AvailableSpace| {
        known.or(match available {
            AvailableSpace::Definite(limit) => Some(limit),
            AvailableSpace::MinContent | AvailableSpace::MaxContent => None,
        })
    };
    let width = bounded(input.known_dimensions.width, input.available_space.width);
    let height = bounded(input.known_dimensions.height, input.available_space.height);
    let size = match (width, height) {
        (Some(width), Some(height)) => Size::new(width, height),
        (Some(width), None) => Size::new(width, 3000.0 / width),
        (None, Some(height)) => Size::new(3000.0 / height, height),
        (None, None) => Size::new(100.0, 30.0),
    };
    LeafMetrics::new(size)
}

#[test]
fn stacking_axis_intrinsic_keywords_measure_across_the_lane() {
    let mut tree = TestTree::default();
    let item = tree.push_measured_leaf(
        TestStyle {
            size: Size::new(size_auto(), size_px(10.0)),
            min_size: Size::new(size_auto(), support::size_min_content()),
            ..TestStyle::default()
        },
        area_leaf,
    );
    let root = tree.push_grid_lanes(lanes_style(&[px(50.0)], &[]), vec![item]);

    let output = auto_layout(&tree, root);

    // The block-axis `min-content` is measured at the lane's 50px width, not
    // at the width an unconstrained measurement would have picked.
    assert_size(output.size, Size::new(50.0, 60.0));
    assert_size(tree.layout(item).size, Size::new(50.0, 60.0));

    let mut tree = TestTree::default();
    let item = tree.push_measured_leaf(
        TestStyle {
            size: Size::new(size_px(10.0), size_auto()),
            min_size: Size::new(support::size_min_content(), size_auto()),
            ..TestStyle::default()
        },
        area_leaf,
    );
    let root = tree.push_grid_lanes(lanes_style(&[], &[px(50.0)]), vec![item]);

    let output = auto_layout(&tree, root);

    // Row lanes stack along the inline axis, so the same rule measures the
    // inline `min-content` at the row's 50px height.
    assert_size(output.size, Size::new(60.0, 50.0));
    assert_size(tree.layout(item).size, Size::new(60.0, 50.0));
}

#[test]
fn intrinsic_tracks_take_the_contribution_of_every_reachable_item() {
    let mut tree = TestTree::default();
    let item = tree.push_intrinsic_leaf(
        TestStyle {
            size: Size::new(size_auto(), size_px(10.0)),
            ..TestStyle::default()
        },
        Size::new(30.0, 10.0),
        Size::new(60.0, 10.0),
    );
    let root = tree.push_grid_lanes(
        lanes_style(&[min_content(), max_content(), px(50.0)], &[]),
        vec![item],
    );

    let output = auto_layout(&tree, root);

    assert_size(output.size, Size::new(140.0, 10.0));
    assert_size(tree.layout(item).size, Size::new(30.0, 10.0));
}

#[test]
fn percentage_tracks_resolve_against_the_container_size_they_produce() {
    let mut tree = TestTree::default();
    let flowing = fixed(&mut tree, 10.0, 20.0);
    let marker = styled(
        &mut tree,
        TestStyle {
            grid_column: Line::new(line(2), line(3)),
            ..leaf_style(10.0, 10.0)
        },
        10.0,
        10.0,
    );
    let root = tree.push_grid_lanes(
        TestStyle {
            min_size: Size::new(size_px(200.0), size_auto()),
            ..lanes_style(&[track_pct(0.25), track_pct(0.25)], &[])
        },
        vec![flowing, marker],
    );

    let output = auto_layout(&tree, root);

    assert_size(output.size, Size::new(200.0, 20.0));
    // The second run sizes each track at 25% of the 200px the first produced.
    assert_point(tree.layout(marker).location, Point::new(50.0, 0.0));
}

#[test]
fn auto_fill_repeats_across_the_definite_grid_axis() {
    let mut tree = TestTree::default();
    let marker = styled(
        &mut tree,
        TestStyle {
            grid_column: Line::new(line(4), line(5)),
            ..leaf_style(10.0, 10.0)
        },
        10.0,
        10.0,
    );
    let root = tree.push_grid_lanes(
        TestStyle {
            template_columns: template_values(
                vec![track_repeat(TestRepeatCount::AutoFill, vec![px(50.0)])],
                0,
            ),
            ..lanes_style(&[], &[])
        },
        vec![marker],
    );

    sized_layout(&tree, root, Some(200.0), None);

    assert_close(tree.layout(marker).location.x, 150.0);
}

#[test]
fn auto_fit_keeps_one_track_per_auto_placed_span_and_collapses_the_rest() {
    let mut tree = TestTree::default();
    let first = fixed(&mut tree, 10.0, 20.0);
    let second = fixed(&mut tree, 10.0, 10.0);
    let marker = styled(
        &mut tree,
        TestStyle {
            grid_column: Line::new(line(4), line(5)),
            ..leaf_style(10.0, 5.0)
        },
        10.0,
        5.0,
    );
    let root = tree.push_grid_lanes(
        TestStyle {
            template_columns: template_values(
                vec![track_repeat(TestRepeatCount::AutoFit, vec![px(50.0)])],
                0,
            ),
            ..lanes_style(&[], &[])
        },
        vec![first, second, marker],
    );

    sized_layout(&tree, root, Some(200.0), None);

    // Four `auto-fit` tracks: the fourth is occupied by the definite item, the
    // first two are kept for the two auto-placed spans, and the third
    // collapses onto the fourth.
    assert_eq!(
        locations(&tree, &[first, second, marker]),
        vec![
            Point::new(0.0, 0.0),
            Point::new(50.0, 0.0),
            Point::new(100.0, 0.0),
        ]
    );
}

#[test]
fn container_min_and_max_clamp_both_axes() {
    let mut tree = TestTree::default();
    let first = fixed(&mut tree, 10.0, 30.0);
    let second = fixed(&mut tree, 10.0, 20.0);
    let root = tree.push_grid_lanes(
        TestStyle {
            max_size: Size::new(max_px(80.0), support::max_none()),
            min_size: Size::new(size_auto(), size_px(50.0)),
            ..lanes_style(&[px(50.0), px(50.0)], &[])
        },
        vec![first, second],
    );

    let output = auto_layout(&tree, root);

    assert_size(output.size, Size::new(80.0, 50.0));
}

#[test]
fn size_containment_replaces_both_axes_and_still_lays_children_out() {
    let mut tree = TestTree::default();
    let first = fixed(&mut tree, 10.0, 30.0);
    let second = fixed(&mut tree, 10.0, 20.0);
    let root = tree.push_grid_lanes(
        TestStyle {
            containment: Contain::SIZE,
            contain_intrinsic_width: support::contain_intrinsic_px(70.0),
            contain_intrinsic_height: support::contain_intrinsic_px(25.0),
            ..lanes_style(&[px(50.0), px(50.0)], &[])
        },
        vec![first, second],
    );

    let output = auto_layout(&tree, root);

    assert_size(output.size, Size::new(70.0, 25.0));
    assert_size(output.content_size, Size::new(70.0, 30.0));
    assert_point(tree.layout(second).location, Point::new(50.0, 0.0));
}

#[test]
fn a_measure_goal_reports_the_same_box_without_writing_children() {
    let mut tree = TestTree::default();
    let first = fixed(&mut tree, 10.0, 30.0);
    let second = fixed(&mut tree, 10.0, 20.0);
    let root = tree.push_grid_lanes(lanes_style(&[px(50.0), px(50.0)], &[]), vec![first, second]);

    let mut sentinel = Layout::default();
    sentinel.location = Point::new(123.0, 456.0);
    sentinel.size = Size::new(7.0, 8.0);
    tree.set_layout_for_testing(first, snapshot_layout(&sentinel));
    tree.set_layout_for_testing(second, snapshot_layout(&sentinel));

    let output = support::measure_layout(&tree, root, Size::NONE, Size::MAX_CONTENT);

    assert_size(output.size, Size::new(100.0, 30.0));
    assert_eq!(tree.layout_writes.get(), 0);
    assert_eq!(tree.layout(first), sentinel);
    assert_eq!(tree.layout(second), sentinel);
}

#[test]
fn an_overflowing_stacking_range_is_reported_as_scrollable_content() {
    let mut tree = TestTree::default();
    let first = fixed(&mut tree, 10.0, 30.0);
    let second = fixed(&mut tree, 10.0, 20.0);
    let root = tree.push_grid_lanes(lanes_style(&[px(50.0)], &[]), vec![first, second]);

    let output = sized_layout(&tree, root, None, Some(20.0));

    assert_size(output.size, Size::new(50.0, 20.0));
    assert_size(output.content_size, Size::new(50.0, 50.0));
    assert_point(tree.layout(second).location, Point::new(0.0, 30.0));
}

#[test]
fn content_distribution_positions_the_tracks_and_the_stacking_range() {
    let cases = [
        (
            AlignFlags::CENTER,
            AlignFlags::CENTER,
            Point::new(50.0, 35.0),
            Point::new(100.0, 35.0),
        ),
        (
            AlignFlags::SPACE_BETWEEN,
            AlignFlags::END,
            Point::new(0.0, 70.0),
            Point::new(150.0, 70.0),
        ),
        (
            AlignFlags::START,
            AlignFlags::SPACE_BETWEEN,
            Point::new(0.0, 0.0),
            Point::new(50.0, 0.0),
        ),
    ];
    for (justify, align, expected_first, expected_second) in cases {
        let mut tree = TestTree::default();
        let first = fixed(&mut tree, 10.0, 30.0);
        let second = fixed(&mut tree, 10.0, 20.0);
        let root = tree.push_grid_lanes(
            TestStyle {
                justify_content: content(justify),
                align_content: content(align),
                ..lanes_style(&[px(50.0), px(50.0)], &[])
            },
            vec![first, second],
        );

        sized_layout(&tree, root, Some(200.0), Some(100.0));

        assert_point(tree.layout(first).location, expected_first);
        assert_point(tree.layout(second).location, expected_second);
    }
}

#[test]
fn self_alignment_places_the_item_inside_its_track() {
    let mut tree = TestTree::default();
    let aligned = [
        AlignFlags::START,
        AlignFlags::CENTER,
        AlignFlags::END,
        AlignFlags::STRETCH,
    ]
    .iter()
    .map(|&flags| {
        let stretches = flags == AlignFlags::STRETCH;
        styled(
            &mut tree,
            TestStyle {
                size: Size::new(
                    if stretches {
                        size_auto()
                    } else {
                        size_px(20.0)
                    },
                    size_px(10.0),
                ),
                justify_self: self_align(flags),
                ..TestStyle::default()
            },
            20.0,
            10.0,
        )
    })
    .collect::<Vec<_>>();
    let root = tree.push_grid_lanes(lanes_style(&[px(100.0)], &[]), aligned.clone());

    auto_layout(&tree, root);

    assert_eq!(
        aligned
            .iter()
            .map(|&id| tree.layout(id).location.x)
            .collect::<Vec<_>>(),
        vec![0.0, 40.0, 80.0, 0.0]
    );
    assert_close(tree.layout(aligned[3]).size.width, 100.0);
}

#[test]
fn auto_margins_absorb_the_grid_axis_free_space() {
    let mut tree = TestTree::default();
    let end = styled(
        &mut tree,
        TestStyle {
            margin: Edges {
                left: margin_auto(),
                ..Edges::uniform(margin_px(0.0))
            },
            ..leaf_style(20.0, 10.0)
        },
        20.0,
        10.0,
    );
    let centre = styled(
        &mut tree,
        TestStyle {
            margin: Edges {
                left: margin_auto(),
                right: margin_auto(),
                ..Edges::uniform(margin_px(0.0))
            },
            ..leaf_style(20.0, 10.0)
        },
        20.0,
        10.0,
    );
    let root = tree.push_grid_lanes(lanes_style(&[px(100.0)], &[]), vec![end, centre]);

    auto_layout(&tree, root);

    assert_point(tree.layout(end).location, Point::new(80.0, 0.0));
    assert_point(tree.layout(centre).location, Point::new(40.0, 10.0));
}

#[test]
fn a_percentage_stacking_gutter_resolves_in_two_passes_and_commits_once() {
    let mut tree = TestTree::default();
    let first = fixed(&mut tree, 10.0, 20.0);
    let second = fixed(&mut tree, 10.0, 20.0);
    let root = tree.push_grid_lanes(
        TestStyle {
            gap: Size::new(support::gap_normal(), support::gap_pct(0.5)),
            ..lanes_style(&[px(50.0)], &[])
        },
        vec![first, second],
    );

    let output = auto_layout(&tree, root);

    // The gapless pass reports 40, against which 50% resolves to 20, and the
    // real pass then reports 20 + 20 + 20.
    assert_size(output.size, Size::new(50.0, 60.0));
    assert_point(tree.layout(second).location, Point::new(0.0, 40.0));
    assert_eq!(tree.layout_writes.get(), 2);
}

#[test]
fn margin_boxes_tile_the_lane_and_negative_margins_floor_at_zero() {
    let mut tree = TestTree::default();
    let spaced = styled(
        &mut tree,
        TestStyle {
            margin: Edges::uniform(margin_px(5.0)),
            ..leaf_style(20.0, 20.0)
        },
        20.0,
        20.0,
    );
    let after = fixed(&mut tree, 20.0, 20.0);
    let root = tree.push_grid_lanes(lanes_style(&[px(50.0)], &[]), vec![spaced, after]);

    let output = auto_layout(&tree, root);

    assert_size(output.size, Size::new(50.0, 50.0));
    assert_point(tree.layout(spaced).location, Point::new(5.0, 5.0));
    assert_point(tree.layout(after).location, Point::new(0.0, 30.0));

    let mut tree = TestTree::default();
    let pulled = styled(
        &mut tree,
        TestStyle {
            margin: Edges {
                top: margin_px(-15.0),
                bottom: margin_px(-15.0),
                ..Edges::uniform(margin_px(0.0))
            },
            ..leaf_style(20.0, 20.0)
        },
        20.0,
        20.0,
    );
    let after = fixed(&mut tree, 20.0, 20.0);
    let root = tree.push_grid_lanes(lanes_style(&[px(50.0)], &[]), vec![pulled, after]);

    let output = auto_layout(&tree, root);

    // A negative margin box never pulls the running position backwards.
    assert_size(output.size, Size::new(50.0, 20.0));
    assert_point(tree.layout(pulled).location, Point::new(0.0, -15.0));
    assert_point(tree.layout(after).location, Point::new(0.0, 0.0));
}

#[test]
fn row_lanes_stack_along_the_inline_axis_from_the_inline_start_edge() {
    for (direction, expected) in [
        (
            direction::T::Ltr,
            vec![Point::new(0.0, 0.0), Point::new(0.0, 40.0)],
        ),
        (
            direction::T::Rtl,
            vec![Point::new(0.0, 0.0), Point::new(10.0, 40.0)],
        ),
    ] {
        let mut tree = TestTree::default();
        let first = fixed(&mut tree, 30.0, 20.0);
        let second = fixed(&mut tree, 20.0, 20.0);
        let root = tree.push_grid_lanes(
            TestStyle {
                direction,
                ..lanes_style(&[], &[px(40.0), px(40.0)])
            },
            vec![first, second],
        );

        let output = auto_layout(&tree, root);

        assert_size(output.size, Size::new(30.0, 80.0));
        assert_eq!(locations(&tree, &[first, second]), expected);
    }
}

#[test]
fn rtl_mirrors_the_grid_axis_tracks() {
    let mut tree = TestTree::default();
    let first = styled(
        &mut tree,
        TestStyle {
            size: Size::new(size_auto(), size_px(20.0)),
            ..TestStyle::default()
        },
        10.0,
        20.0,
    );
    let second = styled(
        &mut tree,
        TestStyle {
            size: Size::new(size_auto(), size_px(10.0)),
            ..TestStyle::default()
        },
        10.0,
        10.0,
    );
    let root = tree.push_grid_lanes(
        TestStyle {
            direction: direction::T::Rtl,
            ..lanes_style(&[px(50.0), px(50.0)], &[])
        },
        vec![first, second],
    );

    sized_layout(&tree, root, Some(100.0), None);

    assert_point(tree.layout(first).location, Point::new(50.0, 0.0));
    assert_point(tree.layout(second).location, Point::new(0.0, 0.0));
}

#[test]
fn the_first_baseline_comes_from_the_items_that_open_each_track() {
    let mut tree = TestTree::default();
    let first = tree.push_leaf(leaf_style(10.0, 30.0), Size::new(10.0, 30.0), Some(5.0));
    let second = tree.push_leaf(leaf_style(10.0, 20.0), Size::new(10.0, 20.0), Some(8.0));
    let third = tree.push_leaf(leaf_style(10.0, 10.0), Size::new(10.0, 10.0), Some(2.0));
    let root = tree.push_grid_lanes(
        lanes_style(&[px(50.0), px(50.0)], &[]),
        vec![first, second, third],
    );

    // The third item reopens no track, so its baseline never competes.
    assert_eq!(auto_layout(&tree, root).first_baselines.y, Some(5.0));

    let mut tree = TestTree::default();
    let first = fixed(&mut tree, 10.0, 30.0);
    let second = tree.push_leaf(leaf_style(10.0, 20.0), Size::new(10.0, 20.0), Some(8.0));
    let root = tree.push_grid_lanes(lanes_style(&[px(50.0), px(50.0)], &[]), vec![first, second]);

    // An item without a baseline contributes none rather than its bottom edge.
    assert_eq!(auto_layout(&tree, root).first_baselines.y, Some(8.0));

    let mut tree = TestTree::default();
    let first = tree.push_leaf(leaf_style(30.0, 20.0), Size::new(30.0, 20.0), Some(15.0));
    let second = tree.push_leaf(leaf_style(20.0, 20.0), Size::new(20.0, 20.0), Some(8.0));
    let root = tree.push_grid_lanes(lanes_style(&[], &[px(40.0), px(40.0)]), vec![first, second]);

    // Row lanes read the first item of the first non-empty track instead.
    assert_eq!(auto_layout(&tree, root).first_baselines.y, Some(15.0));
}

#[test]
fn row_lanes_size_intrinsic_tracks_and_materialise_implicit_ones() {
    let mut tree = TestTree::default();
    let flowing = tree.push_intrinsic_leaf(
        TestStyle::default(),
        Size::new(10.0, 12.0),
        Size::new(10.0, 12.0),
    );
    let placed = styled(
        &mut tree,
        TestStyle {
            grid_row: Line::new(line(3), line(4)),
            ..leaf_style(10.0, 10.0)
        },
        10.0,
        10.0,
    );
    let root = tree.push_grid_lanes(
        TestStyle {
            auto_rows: implicit_tracks(vec![px(20.0)]),
            ..lanes_style(&[], &[min_content(), px(30.0)])
        },
        vec![flowing, placed],
    );

    let output = auto_layout(&tree, root);

    // 12 (the min-content row) + 30 (explicit) + 20 (implicit for row 3).
    assert_size(output.size, Size::new(10.0, 62.0));
    assert_size(tree.layout(flowing).size, Size::new(10.0, 12.0));
    assert_point(tree.layout(placed).location, Point::new(0.0, 42.0));
}

#[test]
fn a_measure_goal_still_applies_order_modified_document_order() {
    let mut tree = TestTree::default();
    let first = fixed(&mut tree, 10.0, 10.0);
    let second = fixed(&mut tree, 10.0, 30.0);
    let hoisted = styled(
        &mut tree,
        TestStyle {
            order: -1,
            ..leaf_style(10.0, 5.0)
        },
        10.0,
        5.0,
    );
    let root = tree.push_grid_lanes(
        lanes_style(&[px(50.0), px(50.0)], &[]),
        vec![first, second, hoisted],
    );

    // Laid out in document order the range would be 30; the hoisted item runs
    // first, which pushes the tall item onto the lane it opened.
    assert_size(
        support::measure_layout(&tree, root, Size::NONE, Size::MAX_CONTENT).size,
        Size::new(100.0, 35.0),
    );
    assert_size(auto_layout(&tree, root).size, Size::new(100.0, 35.0));
}

#[test]
fn baseline_self_alignment_falls_back_to_start_in_the_grid_axis() {
    let mut tree = TestTree::default();
    let item = styled(
        &mut tree,
        TestStyle {
            direction: direction::T::Ltr,
            align_self: self_align(AlignFlags::BASELINE),
            justify_self: self_align(AlignFlags::BASELINE),
            ..leaf_style(20.0, 10.0)
        },
        20.0,
        10.0,
    );
    let root = tree.push_grid_lanes(
        TestStyle {
            direction: direction::T::Rtl,
            ..lanes_style(&[px(100.0)], &[])
        },
        vec![item],
    );

    sized_layout(&tree, root, Some(100.0), None);

    // `baseline` would have aligned to the item's own inline start (the left
    // edge here); its `start` fallback takes the container's instead.
    assert_point(tree.layout(item).location, Point::new(80.0, 0.0));
}

#[test]
fn layout_containment_suppresses_the_container_baseline() {
    let mut tree = TestTree::default();
    let item = tree.push_leaf(leaf_style(10.0, 30.0), Size::new(10.0, 30.0), Some(5.0));
    let root = tree.push_grid_lanes(
        TestStyle {
            containment: Contain::LAYOUT,
            ..lanes_style(&[px(50.0)], &[])
        },
        vec![item],
    );

    let output = auto_layout(&tree, root);

    assert_eq!(output.first_baselines.y, None);
    assert_size(output.content_size, Size::new(50.0, 30.0));
}

#[test]
fn committed_items_claim_grid_axis_independence_only_over_fixed_tracks() {
    // An auto-placed item reaches every lane, so it only inherits a still area
    // when every live track is fixed; a definite one only needs its own span.
    let cases = [
        (vec![px(50.0)], Vec::new(), false, Size::new(true, false)),
        (vec![px(50.0)], Vec::new(), true, Size::new(true, false)),
        (
            vec![px(50.0), fr(1.0)],
            Vec::new(),
            false,
            Size::new(false, false),
        ),
        (
            vec![px(50.0), fr(1.0)],
            Vec::new(),
            true,
            Size::new(true, false),
        ),
        (vec![fr(1.0)], Vec::new(), true, Size::new(false, false)),
        // Row lanes swap which axis the tracks can vouch for.
        (Vec::new(), vec![px(50.0)], false, Size::new(false, true)),
    ];
    for (index, (columns, rows, placed, expected)) in cases.into_iter().enumerate() {
        let mut tree = TestTree::default();
        tree.enable_cache();
        let mut style = leaf_style(20.0, 20.0);
        if placed {
            style.grid_column = Line::new(line(1), line(2));
            style.grid_row = Line::new(line(1), line(2));
        }
        let child = styled(&mut tree, style, 20.0, 20.0);
        let root = tree.push_grid_lanes(lanes_style(&columns, &rows), vec![child]);

        tree.compute_layout(
            root,
            LayoutInput::commit(
                Size::new(Some(50.0), Some(50.0)),
                Size::new(Some(50.0), Some(50.0)),
                Size::new(
                    AvailableSpace::Definite(50.0),
                    AvailableSpace::Definite(50.0),
                ),
                Size::new(true, true),
            ),
        );

        assert_eq!(
            tree.committed_input(child).unwrap().goal,
            LayoutGoal::Commit {
                content_independent: expected
            },
            "case {index}",
        );
    }
}

// ---------------------------------------------------------------------------
// Items with an intrinsic aspect ratio
//
// css-grid-3 §6.2 routes grid-axis alignment straight through regular Grid, so
// css-grid-1 §6.2 decides whether a lanes item fills its lane and css-sizing-4
// §5 decides what that lane size transfers into the stacking axis. The leaves
// below are replaced: they carry a natural size and answer a known dimension
// through their own ratio, the way `NaturalSize::measure` does for a bitmap.
// ---------------------------------------------------------------------------

/// A replaced 40x20 leaf, ratio 2:1.
fn wide_bitmap(input: LeafMeasureInput) -> LeafMetrics {
    let known = input.known_dimensions;
    LeafMetrics::new(match (known.width, known.height) {
        (Some(width), Some(height)) => Size::new(width, height),
        (Some(width), None) => Size::new(width, width / 2.0),
        (None, Some(height)) => Size::new(height * 2.0, height),
        (None, None) => Size::new(40.0, 20.0),
    })
}

/// The same leaf standing up: 20x40, ratio 1:2.
fn tall_bitmap(input: LeafMeasureInput) -> LeafMetrics {
    let known = input.known_dimensions;
    LeafMetrics::new(match (known.width, known.height) {
        (Some(width), Some(height)) => Size::new(width, height),
        (Some(width), None) => Size::new(width, width * 2.0),
        (None, Some(height)) => Size::new(height / 2.0, height),
        (None, None) => Size::new(20.0, 40.0),
    })
}

fn wide_ratio_item(tree: &mut TestTree, style: TestStyle) -> TestId {
    tree.push_measured_leaf(
        TestStyle {
            natural_size: Size::new(Some(40.0), Some(20.0)),
            ..style
        },
        wide_bitmap,
    )
}

fn tall_ratio_item(tree: &mut TestTree, style: TestStyle) -> TestId {
    tree.push_measured_leaf(
        TestStyle {
            natural_size: Size::new(Some(20.0), Some(40.0)),
            ..style
        },
        tall_bitmap,
    )
}

fn stretched() -> TestStyle {
    TestStyle {
        justify_self: self_align(AlignFlags::STRETCH),
        ..TestStyle::default()
    }
}

/// The waterfall case. Three 100px lanes, four replaced items told to fill
/// their lane: each is 100 wide, each takes 50 of stacking extent from
/// css-sizing-4 §5, and §4.4 then places the fourth at the end of the shortest
/// lane — which after three equal items is the first. The stacking range is
/// the 100 that lane reaches.
#[test]
fn stretched_ratio_items_take_their_stacking_size_from_their_lane() {
    let mut tree = TestTree::default();
    let items = (0..4)
        .map(|_| wide_ratio_item(&mut tree, stretched()))
        .collect::<Vec<_>>();
    let root = tree.push_grid_lanes(
        lanes_style(&[px(100.0), px(100.0), px(100.0)], &[]),
        items.clone(),
    );

    let output = sized_layout(&tree, root, Some(300.0), None);

    assert_size(output.size, Size::new(300.0, 100.0));
    for &item in &items {
        assert_size(tree.layout(item).size, Size::new(100.0, 50.0));
    }
    assert_eq!(
        locations(&tree, &items),
        vec![
            Point::new(0.0, 0.0),
            Point::new(100.0, 0.0),
            Point::new(200.0, 0.0),
            Point::new(0.0, 50.0),
        ]
    );
}

/// css-grid-1 §6.2 through css-grid-3 §6.2: `normal` does not stretch a
/// replaced item, in lanes either. The same four items keep their natural
/// 40x20, so lane 0 stacks two of them and the range is 40 — the lane is still
/// 100 wide, the item simply does not fill it.
#[test]
fn normal_leaves_a_replaced_lanes_item_at_its_natural_size() {
    let mut tree = TestTree::default();
    let items = (0..4)
        .map(|_| wide_ratio_item(&mut tree, TestStyle::default()))
        .collect::<Vec<_>>();
    let root = tree.push_grid_lanes(
        lanes_style(&[px(100.0), px(100.0), px(100.0)], &[]),
        items.clone(),
    );

    let output = sized_layout(&tree, root, Some(300.0), None);

    assert_size(output.size, Size::new(300.0, 40.0));
    for &item in &items {
        assert_size(tree.layout(item).size, Size::new(40.0, 20.0));
    }
    assert_point(tree.layout(items[3]).location, Point::new(0.0, 20.0));
}

/// The grid-axis alignment values §6.2 inherits from Grid, one item per lane
/// so each is read on its own. `stretch` fills and transfers; `normal`,
/// `start`, `center` and `end` leave replaced content at 40x20 and only move
/// it inside the lane.
#[test]
fn grid_axis_self_alignment_decides_whether_a_ratio_item_fills_its_lane() {
    let cases = [
        (AlignFlags::STRETCH, Size::new(100.0, 50.0), 0.0),
        (AlignFlags::NORMAL, Size::new(40.0, 20.0), 0.0),
        (AlignFlags::START, Size::new(40.0, 20.0), 0.0),
        (AlignFlags::CENTER, Size::new(40.0, 20.0), 30.0),
        (AlignFlags::END, Size::new(40.0, 20.0), 60.0),
    ];
    for (alignment, expected, offset) in cases {
        let mut tree = TestTree::default();
        let item = wide_ratio_item(
            &mut tree,
            TestStyle {
                justify_self: self_align(alignment),
                ..TestStyle::default()
            },
        );
        let root = tree.push_grid_lanes(lanes_style(&[px(100.0)], &[]), vec![item]);

        sized_layout(&tree, root, Some(100.0), None);

        assert_size(tree.layout(item).size, expected);
        assert_point(tree.layout(item).location, Point::new(offset, 0.0));
    }
}

/// `justify-items` on the container is the same decision made once. With
/// `stretch` every lanes item fills, including the replaced ones `normal`
/// would have left alone.
#[test]
fn justify_items_stretch_fills_every_lane() {
    let mut tree = TestTree::default();
    let wide = wide_ratio_item(&mut tree, TestStyle::default());
    let tall = tall_ratio_item(&mut tree, TestStyle::default());
    let root = tree.push_grid_lanes(
        TestStyle {
            justify_items: justify_items(AlignFlags::STRETCH),
            ..lanes_style(&[px(100.0), px(100.0)], &[])
        },
        vec![wide, tall],
    );

    let output = sized_layout(&tree, root, Some(200.0), None);

    assert_size(tree.layout(wide).size, Size::new(100.0, 50.0));
    assert_size(tree.layout(tall).size, Size::new(100.0, 200.0));
    assert_size(output.size, Size::new(200.0, 200.0));
}

/// §2.3 puts the tracks in the block axis when `grid-template-rows` alone
/// names any, and §6.2 then reads `align-self` for the grid axis. A 100-tall
/// lane transfers into 200 of stacking width for the 2:1 item and 50 for the
/// 1:2 one, and the two stack along the inline axis.
#[test]
fn row_lanes_transfer_the_lane_height_into_the_stacking_width() {
    let mut tree = TestTree::default();
    let align_stretch = TestStyle {
        align_self: self_align(AlignFlags::STRETCH),
        ..TestStyle::default()
    };
    let wide = wide_ratio_item(&mut tree, align_stretch.clone());
    let tall = tall_ratio_item(&mut tree, align_stretch);
    let root = tree.push_grid_lanes(lanes_style(&[], &[px(100.0)]), vec![wide, tall]);

    let output = sized_layout(&tree, root, None, Some(100.0));

    assert_size(tree.layout(wide).size, Size::new(200.0, 100.0));
    assert_size(tree.layout(tall).size, Size::new(50.0, 100.0));
    assert_size(output.size, Size::new(250.0, 100.0));
    assert_point(tree.layout(tall).location, Point::new(200.0, 0.0));
}

/// §3.4 sizes the grid axis with regular Grid's §12, so an `auto` lane takes
/// the ratio item's max-content contribution — with nothing definite in the
/// stacking axis to transfer from, that is its natural 40 — and an `fr` lane
/// takes its share of the container and transfers that.
#[test]
fn auto_and_flexible_lanes_size_from_ratio_items() {
    let mut auto_tree = TestTree::default();
    let auto_item = wide_ratio_item(&mut auto_tree, stretched());
    let auto_root = auto_tree.push_grid_lanes(
        lanes_style(&[support::track_auto(), support::track_auto()], &[]),
        vec![auto_item],
    );
    let auto_output = auto_layout(&auto_tree, auto_root);
    assert_size(auto_tree.layout(auto_item).size, Size::new(40.0, 20.0));
    assert_size(auto_output.size, Size::new(80.0, 20.0));

    let mut flex_tree = TestTree::default();
    let flex_item = wide_ratio_item(&mut flex_tree, stretched());
    let flex_root =
        flex_tree.push_grid_lanes(lanes_style(&[fr(1.0), fr(1.0)], &[]), vec![flex_item]);
    let flex_output = sized_layout(&flex_tree, flex_root, Some(300.0), None);
    assert_size(flex_tree.layout(flex_item).size, Size::new(150.0, 75.0));
    assert_size(flex_output.size, Size::new(300.0, 75.0));
}

/// §4.1 lets an item span lanes, and §6.1's grid-axis gutter is part of the
/// area it then stretches into: 100 + 20 + 100 transfers to 110 of stacking
/// extent. The stacking gutter is the 20 between the spanning item and the one
/// that follows it in lane 0.
#[test]
fn a_spanning_ratio_item_stretches_across_its_lanes_and_their_gutter() {
    let mut tree = TestTree::default();
    let spanning = wide_ratio_item(
        &mut tree,
        TestStyle {
            grid_column: Line::new(line(1), span(2)),
            ..stretched()
        },
    );
    let following = wide_ratio_item(&mut tree, stretched());
    let root = tree.push_grid_lanes(
        TestStyle {
            gap: Size::new(gap_px(20.0), gap_px(20.0)),
            ..lanes_style(&[px(100.0), px(100.0)], &[])
        },
        vec![spanning, following],
    );

    let output = sized_layout(&tree, root, Some(220.0), None);

    assert_size(tree.layout(spanning).size, Size::new(220.0, 110.0));
    assert_size(tree.layout(following).size, Size::new(100.0, 50.0));
    assert_point(tree.layout(following).location, Point::new(0.0, 130.0));
    assert_size(output.size, Size::new(220.0, 180.0));
}

/// The lane size is clamped by the item's own min/max in the grid axis before
/// the ratio transfers from it, and a clamp on the stacking axis afterwards
/// simply breaks the ratio — the same order css-sizing-3 §5.1 gives a grid
/// item.
#[test]
fn item_min_and_max_sizes_clamp_a_lanes_transfer() {
    let cases = [
        (
            Size::new(size_auto(), size_auto()),
            Size::new(max_px(60.0), support::max_none()),
            Size::new(60.0, 30.0),
        ),
        (
            Size::new(size_px(160.0), size_auto()),
            Size::new(support::max_none(), support::max_none()),
            Size::new(160.0, 80.0),
        ),
        (
            Size::new(size_auto(), size_auto()),
            Size::new(support::max_none(), max_px(20.0)),
            Size::new(100.0, 20.0),
        ),
    ];
    for (min_size, max_size, expected) in cases {
        let mut tree = TestTree::default();
        let item = wide_ratio_item(
            &mut tree,
            TestStyle {
                min_size,
                max_size,
                ..stretched()
            },
        );
        let root = tree.push_grid_lanes(lanes_style(&[px(100.0)], &[]), vec![item]);

        sized_layout(&tree, root, Some(100.0), None);
        assert_size(tree.layout(item).size, expected);
    }
}

/// A ratio item sharing a lane with an authored-size box and a content-sized
/// one. Only the ratio item's stacking size follows the lane: the authored box
/// keeps its own 80x30, and the content-sized item is a *non-replaced* box, so
/// css-grid-1 §6.2 does stretch it to the lane — it is 100 wide at its own 16
/// of height. The three stack in order behind the §6.1 gutter.
#[test]
fn a_ratio_item_stacks_with_fixed_and_content_sized_neighbours() {
    let mut tree = TestTree::default();
    let image = wide_ratio_item(&mut tree, stretched());
    let box_item = fixed(&mut tree, 80.0, 30.0);
    let content_item = styled(&mut tree, TestStyle::default(), 30.0, 16.0);
    let root = tree.push_grid_lanes(
        TestStyle {
            gap: Size::new(gap_px(0.0), gap_px(10.0)),
            ..lanes_style(&[px(100.0)], &[])
        },
        vec![image, box_item, content_item],
    );

    let output = sized_layout(&tree, root, Some(100.0), None);

    assert_size(tree.layout(image).size, Size::new(100.0, 50.0));
    assert_size(tree.layout(box_item).size, Size::new(80.0, 30.0));
    assert_size(tree.layout(content_item).size, Size::new(100.0, 16.0));
    assert_point(tree.layout(box_item).location, Point::new(0.0, 60.0));
    assert_point(tree.layout(content_item).location, Point::new(0.0, 100.0));
    assert_size(output.size, Size::new(100.0, 116.0));
}

/// §4.2's tie threshold decides *which* lane an item joins and nothing else.
/// Under an infinite tolerance every lane counts as equally short, so the
/// cursor walks them in order instead of chasing the shortest — the four items
/// change places, and not one of them changes size.
#[test]
fn flow_tolerance_moves_ratio_items_without_resizing_them() {
    let wide_lane = Size::new(100.0, 50.0);
    let narrow_lane = Size::new(50.0, 25.0);
    let cases = [
        // Exact shortest-lane placement: after three items lane 1 holds 50 and
        // lane 0 holds 50 too, and the tie goes to the first lane.
        (
            tolerance_px(0.0),
            vec![
                (Point::new(0.0, 0.0), wide_lane),
                (Point::new(100.0, 0.0), narrow_lane),
                (Point::new(100.0, 25.0), narrow_lane),
                (Point::new(0.0, 50.0), wide_lane),
            ],
        ),
        // Every lane ties, so the cursor walks them in order and wraps.
        (
            tolerance_infinite(),
            vec![
                (Point::new(0.0, 0.0), wide_lane),
                (Point::new(100.0, 0.0), narrow_lane),
                (Point::new(0.0, 50.0), wide_lane),
                (Point::new(100.0, 25.0), narrow_lane),
            ],
        ),
    ];
    for (tolerance, expected) in cases {
        let mut tree = TestTree::default();
        let items = (0..4)
            .map(|_| wide_ratio_item(&mut tree, stretched()))
            .collect::<Vec<_>>();
        let root = tree.push_grid_lanes(
            TestStyle {
                flow_tolerance: tolerance,
                ..lanes_style(&[px(100.0), px(50.0)], &[])
            },
            items.clone(),
        );

        sized_layout(&tree, root, Some(150.0), None);

        // Two lanes, two sizes: whichever lane an item lands in, its size is
        // that lane's width through the ratio and nothing else.
        for (&item, (location, size)) in items.iter().zip(expected) {
            assert_point(tree.layout(item).location, location);
            assert_size(tree.layout(item).size, size);
        }
    }
}
