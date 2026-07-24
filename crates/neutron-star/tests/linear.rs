#[path = "support/mod.rs"]
mod support;

use neutron_star::compute::{LeafMeasureInput, LeafMetrics};
use neutron_star::prelude::*;
use stylo::computed_values::{box_sizing, direction, linear_direction, visibility};
use stylo::values::computed::{
    Contain, ContentDistribution, Display, MaxSize, PositionProperty, Size as StyleSize,
};
use stylo::values::specified::align::AlignFlags;
use support::{
    TestId, TestStyle, TestTree, assert_close, assert_point, assert_size, border_px, content,
    definite_layout, fixed_leaf, inset_auto, inset_pct, inset_px, items, margin_auto, margin_pct,
    margin_px, max_fit_content_px, max_max_content, max_min_content, max_none, max_pct, max_px,
    measure_layout, nn, npx, perform_layout, ratio, self_align, size_auto, size_calc,
    size_fit_content_px, size_max_content, size_min_content, size_pct, size_px,
};

fn edges<T>(left: T, right: T, top: T, bottom: T) -> Edges<T> {
    Edges {
        left,
        right,
        top,
        bottom,
    }
}

fn fixed_style(width: f32, height: f32) -> TestStyle {
    TestStyle {
        size: Size::new(size_px(width), size_px(height)),
        ..TestStyle::default()
    }
}

fn fixed_leaf_with(
    tree: &mut TestTree,
    width: f32,
    height: f32,
    update: impl FnOnce(&mut TestStyle),
) -> TestId {
    let mut style = fixed_style(width, height);
    update(&mut style);
    tree.push_leaf(style, Size::new(width, height), None)
}

fn max_content_layout(tree: &TestTree, root: TestId) -> LayoutOutput {
    perform_layout(tree, root, Size::NONE, Size::MAX_CONTENT)
}

fn assert_case_close(case: &str, actual: f32, expected: f32) {
    let error = (actual - expected).abs();
    assert!(
        error <= 0.01,
        "{case}: expected {expected}, got {actual} (absolute error {error})"
    );
}

mod flow_direction {
    use super::*;

    fn axis_positions(
        linear_direction: linear_direction::T,
        direction: direction::T,
    ) -> (Point<f32>, Point<f32>) {
        let mut tree = TestTree::default();
        let first = fixed_leaf(&mut tree, 10.0, 10.0);
        let second = fixed_leaf(&mut tree, 20.0, 10.0);
        let root = tree.push_linear(
            TestStyle {
                direction,
                linear_direction,
                ..TestStyle::default()
            },
            vec![first, second],
        );
        definite_layout(&tree, root, 100.0, 100.0);
        (tree.layout(first).location, tree.layout(second).location)
    }

    #[test]
    fn reversal_and_rtl_map_main_and_cross_starts() {
        for (case, linear, direction, expected) in [
            (
                "row/ltr",
                linear_direction::T::Row,
                direction::T::Ltr,
                (Point::new(0.0, 0.0), Point::new(10.0, 0.0)),
            ),
            (
                "row-reverse/ltr",
                linear_direction::T::RowReverse,
                direction::T::Ltr,
                (Point::new(90.0, 0.0), Point::new(70.0, 0.0)),
            ),
            (
                "row/rtl",
                linear_direction::T::Row,
                direction::T::Rtl,
                (Point::new(90.0, 0.0), Point::new(70.0, 0.0)),
            ),
            (
                "row-reverse/rtl",
                linear_direction::T::RowReverse,
                direction::T::Rtl,
                (Point::new(0.0, 0.0), Point::new(10.0, 0.0)),
            ),
            (
                "column-reverse/rtl",
                linear_direction::T::ColumnReverse,
                direction::T::Rtl,
                (Point::new(90.0, 90.0), Point::new(80.0, 80.0)),
            ),
        ] {
            assert_eq!(axis_positions(linear, direction), expected, "{case}");
        }
    }
}

#[test]
fn order_is_stable_and_layout_order_is_exported() {
    let mut tree = TestTree::default();
    let source_first = fixed_leaf_with(&mut tree, 10.0, 10.0, |style| style.order = 2);
    let equal_first = fixed_leaf_with(&mut tree, 10.0, 10.0, |style| style.order = -1);
    let equal_second = fixed_leaf_with(&mut tree, 10.0, 10.0, |style| style.order = -1);
    let middle = fixed_leaf(&mut tree, 10.0, 10.0);
    let root = tree.push_linear(
        TestStyle {
            linear_direction: linear_direction::T::Row,
            ..TestStyle::default()
        },
        vec![source_first, equal_first, equal_second, middle],
    );

    definite_layout(&tree, root, 100.0, 20.0);

    for (node, x, order) in [
        (equal_first, 0.0, 0),
        (equal_second, 10.0, 1),
        (middle, 20.0, 2),
        (source_first, 30.0, 3),
    ] {
        assert_close(tree.layout(node).location.x, x);
        assert_eq!(tree.layout(node).order, order);
    }
}

#[test]
fn absolute_and_in_flow_children_share_merged_layout_order() {
    let mut tree = TestTree::default();
    let first_absolute = fixed_leaf_with(&mut tree, 10.0, 10.0, |style| {
        style.position = PositionProperty::Absolute;
    });
    let positive = fixed_leaf_with(&mut tree, 10.0, 10.0, |style| style.order = 1);
    let negative = fixed_leaf_with(&mut tree, 10.0, 10.0, |style| style.order = -1);
    let last_absolute = fixed_leaf_with(&mut tree, 10.0, 10.0, |style| {
        style.position = PositionProperty::Absolute;
    });
    let root = tree.push_linear(
        TestStyle {
            linear_direction: linear_direction::T::Row,
            ..TestStyle::default()
        },
        vec![first_absolute, positive, negative, last_absolute],
    );

    definite_layout(&tree, root, 100.0, 20.0);

    assert_eq!(tree.layout(negative).order, 0);
    assert_eq!(tree.layout(first_absolute).order, 1);
    assert_eq!(tree.layout(last_absolute).order, 2);
    assert_eq!(tree.layout(positive).order, 3);
}

#[test]
fn display_none_is_zeroed_while_hidden_stays_in_flow() {
    let mut tree = TestTree::default();
    let first = fixed_leaf(&mut tree, 10.0, 10.0);
    let hidden_descendant = fixed_leaf(&mut tree, 8.0, 8.0);
    let display_none = tree.push_linear(
        TestStyle {
            display: Display::None,
            ..fixed_style(10.0, 10.0)
        },
        vec![hidden_descendant],
    );
    let visibility_hidden = fixed_leaf_with(&mut tree, 10.0, 10.0, |style| {
        style.visibility = visibility::T::Hidden;
    });
    let last = fixed_leaf(&mut tree, 10.0, 10.0);
    let root = tree.push_linear(
        TestStyle::default(),
        vec![first, display_none, visibility_hidden, last],
    );

    definite_layout(&tree, root, 50.0, 50.0);

    assert_eq!(tree.layout(display_none), Layout::with_order(1));
    assert_eq!(tree.layout(hidden_descendant), Layout::default());
    assert_close(tree.layout(first).location.y, 0.0);
    assert_close(tree.layout(visibility_hidden).location.y, 10.0);
    assert_close(tree.layout(last).location.y, 20.0);
}

fn single_item_main_offset(justify_content: ContentDistribution, direction: direction::T) -> f32 {
    let mut tree = TestTree::default();
    let child = fixed_leaf(&mut tree, 20.0, 10.0);
    let root = tree.push_linear(
        TestStyle {
            direction,
            linear_direction: linear_direction::T::Row,
            justify_content,
            ..TestStyle::default()
        },
        vec![child],
    );
    definite_layout(&tree, root, 100.0, 20.0);
    tree.layout(child).location.x
}

#[test]
fn justify_content_maps_gravity_keywords_and_distribution_fallbacks() {
    for (case, flags, direction, expected) in [
        ("end", AlignFlags::END, direction::T::Ltr, 80.0),
        ("center", AlignFlags::CENTER, direction::T::Ltr, 40.0),
        ("flex-end", AlignFlags::FLEX_END, direction::T::Ltr, 80.0),
        (
            "space-around fallback",
            AlignFlags::SPACE_AROUND,
            direction::T::Ltr,
            0.0,
        ),
        (
            "physical right in rtl",
            AlignFlags::RIGHT,
            direction::T::Rtl,
            80.0,
        ),
    ] {
        assert_case_close(
            case,
            single_item_main_offset(content(flags), direction),
            expected,
        );
    }
}

#[test]
fn space_between_distributes_only_non_negative_free_space() {
    let mut tree = TestTree::default();
    let first = fixed_leaf(&mut tree, 10.0, 10.0);
    let second = fixed_leaf(&mut tree, 20.0, 10.0);
    let root = tree.push_linear(
        TestStyle {
            linear_direction: linear_direction::T::Row,
            justify_content: content(AlignFlags::SPACE_BETWEEN),
            ..TestStyle::default()
        },
        vec![first, second],
    );
    definite_layout(&tree, root, 100.0, 20.0);
    assert_close(tree.layout(first).location.x, 0.0);
    assert_close(tree.layout(second).location.x, 80.0);

    definite_layout(&tree, root, 20.0, 20.0);
    assert_close(tree.layout(first).location.x, 0.0);
    assert_close(tree.layout(second).location.x, 10.0);
}

#[test]
fn end_and_center_preserve_negative_free_space_offsets() {
    for (justify, expected_first, expected_second) in [
        (AlignFlags::END, -20.0, 0.0),
        (AlignFlags::CENTER, -10.0, 10.0),
    ] {
        let mut tree = TestTree::default();
        let first = fixed_leaf(&mut tree, 20.0, 10.0);
        let second = fixed_leaf(&mut tree, 20.0, 10.0);
        let root = tree.push_linear(
            TestStyle {
                linear_direction: linear_direction::T::Row,
                justify_content: content(justify),
                ..TestStyle::default()
            },
            vec![first, second],
        );
        definite_layout(&tree, root, 20.0, 20.0);
        assert_close(tree.layout(first).location.x, expected_first);
        assert_close(tree.layout(second).location.x, expected_second);
    }
}

fn cross_case(container: TestStyle, child: TestStyle) -> Layout {
    let mut tree = TestTree::default();
    let child = tree.push_leaf(child, Size::new(20.0, 10.0), None);
    let root = tree.push_linear(container, vec![child]);
    definite_layout(&tree, root, 100.0, 50.0);
    tree.layout(child)
}

#[test]
fn cross_alignment_honors_container_and_self_values() {
    for (case, container_align, child_align, expected_x, expected_width) in [
        (
            "self end overrides container start",
            Some(AlignFlags::START),
            Some(AlignFlags::END),
            80.0,
            20.0,
        ),
        (
            "self center overrides container end",
            Some(AlignFlags::END),
            Some(AlignFlags::CENTER),
            40.0,
            20.0,
        ),
        (
            "container center",
            Some(AlignFlags::CENTER),
            None,
            40.0,
            20.0,
        ),
        ("normal preserves explicit size", None, None, 0.0, 20.0),
        (
            "container stretch maps to fill",
            Some(AlignFlags::STRETCH),
            None,
            0.0,
            100.0,
        ),
        (
            "self stretch overrides explicit size",
            None,
            Some(AlignFlags::STRETCH),
            0.0,
            100.0,
        ),
    ] {
        let mut container = TestStyle::default();
        if let Some(align) = container_align {
            container.align_items = items(align);
        }
        let mut child = fixed_style(20.0, 10.0);
        if let Some(align) = child_align {
            child.align_self = self_align(align);
        }
        let layout = cross_case(container, child);
        assert_case_close(case, layout.location.x, expected_x);
        assert_case_close(case, layout.size.width, expected_width);
    }
}

#[test]
fn physical_cross_alignment_stays_physical_in_vertical_rtl() {
    for (flags, expected_x) in [(AlignFlags::LEFT, 0.0), (AlignFlags::RIGHT, 80.0)] {
        let layout = cross_case(
            TestStyle {
                direction: direction::T::Rtl,
                ..TestStyle::default()
            },
            TestStyle {
                align_self: self_align(flags),
                ..fixed_style(20.0, 10.0)
            },
        );
        assert_close(layout.location.x, expected_x);
    }
}

#[test]
fn cross_axis_auto_margins_override_alignment_and_export_used_values() {
    for (left, right, expected_x, expected_left, expected_right) in [
        (true, true, 40.0, 40.0, 40.0),
        (true, false, 80.0, 80.0, 0.0),
        (false, true, 0.0, 0.0, 80.0),
    ] {
        let margin = edges(
            if left { margin_auto() } else { margin_px(0.0) },
            if right { margin_auto() } else { margin_px(0.0) },
            margin_px(0.0),
            margin_px(0.0),
        );
        let layout = cross_case(
            TestStyle {
                align_items: items(AlignFlags::END),
                ..TestStyle::default()
            },
            TestStyle {
                margin,
                ..fixed_style(20.0, 10.0)
            },
        );
        assert_close(layout.location.x, expected_x);
        assert_close(layout.margin.left, expected_left);
        assert_close(layout.margin.right, expected_right);
    }

    let overflow = cross_case(
        TestStyle::default(),
        TestStyle {
            margin: Edges::uniform(margin_auto()),
            ..fixed_style(120.0, 10.0)
        },
    );
    assert_close(overflow.location.x, 0.0);
    assert_close(overflow.margin.left, 0.0);
    assert_close(overflow.margin.right, 0.0);
}

#[test]
fn vertical_cross_axis_auto_margins_export_the_correct_edges() {
    for (top, bottom, expected_y, expected_top, expected_bottom) in [
        (true, true, 20.0, 20.0, 20.0),
        (true, false, 40.0, 40.0, 0.0),
        (false, true, 0.0, 0.0, 40.0),
    ] {
        let layout = cross_case(
            TestStyle {
                linear_direction: linear_direction::T::Row,
                align_items: items(AlignFlags::END),
                ..TestStyle::default()
            },
            TestStyle {
                margin: edges(
                    margin_px(0.0),
                    margin_px(0.0),
                    if top { margin_auto() } else { margin_px(0.0) },
                    if bottom {
                        margin_auto()
                    } else {
                        margin_px(0.0)
                    },
                ),
                ..fixed_style(20.0, 10.0)
            },
        );
        assert_close(layout.location.y, expected_y);
        assert_close(layout.margin.top, expected_top);
        assert_close(layout.margin.bottom, expected_bottom);
    }
}

fn weighted_leaf(
    tree: &mut TestTree,
    weight: f32,
    min: stylo::values::computed::Size,
    max: stylo::values::computed::MaxSize,
) -> TestId {
    tree.push_leaf(
        TestStyle {
            size: Size::new(size_auto(), size_px(10.0)),
            min_size: Size::new(min, size_auto()),
            max_size: Size::new(max, max_none()),
            linear_weight: nn(weight),
            ..TestStyle::default()
        },
        Size::new(13.0, 10.0),
        None,
    )
}

fn weighted_pair(
    width: f32,
    weight_sum: f32,
    first_weight: f32,
    second_weight: f32,
    first_min: stylo::values::computed::Size,
    first_max: stylo::values::computed::MaxSize,
) -> (Layout, Layout) {
    let mut tree = TestTree::default();
    let first = weighted_leaf(&mut tree, first_weight, first_min, first_max);
    let second = weighted_leaf(&mut tree, second_weight, size_auto(), max_none());
    let root = tree.push_linear(
        TestStyle {
            linear_direction: linear_direction::T::Row,
            linear_weight_sum: nn(weight_sum),
            ..TestStyle::default()
        },
        vec![first, second],
    );
    definite_layout(&tree, root, width, 20.0);
    (tree.layout(first), tree.layout(second))
}

#[test]
fn weights_split_main_space_in_proportion_to_their_values() {
    let (first, second) = weighted_pair(90.0, 0.0, 1.0, 2.0, size_auto(), max_none());
    assert_close(first.size.width, 30.0);
    assert_close(second.size.width, 60.0);
    assert_close(second.location.x, 30.0);
}

#[test]
fn an_explicit_weight_sum_can_reserve_unallocated_main_space() {
    let (first, second) = weighted_pair(100.0, 4.0, 1.0, 1.0, size_auto(), max_none());
    assert_close(first.size.width, 25.0);
    assert_close(second.size.width, 25.0);
    assert_close(second.location.x, 25.0);
}

#[test]
fn a_total_weight_below_one_leaves_part_of_the_main_space_unallocated() {
    let (first, second) = weighted_pair(100.0, 0.0, 0.25, 0.25, size_auto(), max_none());
    assert_close(first.size.width, 25.0);
    assert_close(second.size.width, 25.0);
    assert_close(second.location.x, 25.0);
}

#[test]
fn weighted_min_and_max_violations_freeze_and_redistribute() {
    let (first, second) = weighted_pair(100.0, 0.0, 1.0, 1.0, size_auto(), max_px(30.0));
    assert_close(first.size.width, 30.0);
    assert_close(second.size.width, 70.0);

    let (first, second) = weighted_pair(100.0, 0.0, 1.0, 1.0, size_px(70.0), max_none());
    assert_close(first.size.width, 70.0);
    assert_close(second.size.width, 30.0);

    let (first, second) = weighted_pair(100.0, 0.0, 1.0, 1.0, size_pct(0.7), max_none());
    assert_close(first.size.width, 70.0);
    assert_close(second.size.width, 30.0);
}

#[test]
fn an_exhausted_main_axis_assigns_zero_size_to_a_weighted_item() {
    let mut tree = TestTree::default();
    let fixed = fixed_leaf(&mut tree, 20.0, 30.0);
    let weighted = tree.push_leaf(
        TestStyle {
            size: Size::new(size_px(20.0), size_auto()),
            linear_weight: nn(1.0),
            ..TestStyle::default()
        },
        Size::new(20.0, 10.0),
        None,
    );
    let root = tree.push_linear(TestStyle::default(), vec![fixed, weighted]);

    definite_layout(&tree, root, 100.0, 20.0);

    assert_size(tree.layout(fixed).size, Size::new(20.0, 30.0));
    assert_point(tree.layout(weighted).location, Point::new(0.0, 30.0));
    assert_size(tree.layout(weighted).size, Size::new(20.0, 0.0));
}

#[test]
fn indefinite_main_axis_disables_weight_distribution() {
    let mut tree = TestTree::default();
    let first = fixed_leaf_with(&mut tree, 15.0, 10.0, |style| {
        style.linear_weight = nn(1.0);
    });
    let second = fixed_leaf_with(&mut tree, 25.0, 10.0, |style| {
        style.linear_weight = nn(2.0);
    });
    let root = tree.push_linear(
        TestStyle {
            linear_direction: linear_direction::T::Row,
            ..TestStyle::default()
        },
        vec![first, second],
    );
    let output = perform_layout(
        &tree,
        root,
        Size::new(None, Some(20.0)),
        Size::new(
            AvailableSpace::Definite(100.0),
            AvailableSpace::Definite(20.0),
        ),
    );
    assert_close(output.size.width, 40.0);
    assert_close(tree.layout(first).size.width, 15.0);
    assert_close(tree.layout(second).size.width, 25.0);
}

#[test]
fn weighted_main_size_reapplies_aspect_ratio_to_cross_axis() {
    let mut tree = TestTree::default();
    let child = tree.push_leaf(
        TestStyle {
            size: Size::new(size_auto(), size_auto()),
            aspect_ratio: ratio(2.0),
            linear_weight: nn(1.0),
            align_self: self_align(AlignFlags::START),
            ..TestStyle::default()
        },
        Size::new(10.0, 10.0),
        None,
    );
    let root = tree.push_linear(
        TestStyle {
            linear_direction: linear_direction::T::Row,
            ..TestStyle::default()
        },
        vec![child],
    );
    definite_layout(&tree, root, 100.0, 100.0);
    assert_size(tree.layout(child).size, Size::new(100.0, 50.0));
}

#[test]
fn container_aspect_ratio_derives_unknown_axis_from_caller_known_axis() {
    let mut tree = TestTree::default();
    let child = fixed_leaf(&mut tree, 10.0, 10.0);
    let root = tree.push_linear(
        TestStyle {
            aspect_ratio: ratio(2.0),
            ..TestStyle::default()
        },
        vec![child],
    );
    let output = perform_layout(
        &tree,
        root,
        Size::new(Some(100.0), None),
        Size::new(AvailableSpace::Definite(100.0), AvailableSpace::MaxContent),
    );
    assert_size(output.size, Size::new(100.0, 50.0));
}

#[test]
fn padding_border_margins_and_box_sizing_use_border_box_geometry() {
    let mut tree = TestTree::default();
    let child = fixed_leaf_with(&mut tree, 20.0, 10.0, |style| {
        style.margin = edges(
            margin_px(3.0),
            margin_px(7.0),
            margin_px(4.0),
            margin_px(6.0),
        );
        style.box_sizing = box_sizing::T::BorderBox;
    });
    let root = tree.push_linear(
        TestStyle {
            linear_direction: linear_direction::T::Row,
            padding: Edges::uniform(npx(10.0)),
            border: Edges::uniform(border_px(2.0)),
            ..TestStyle::default()
        },
        vec![child],
    );
    definite_layout(&tree, root, 120.0, 60.0);
    assert_point(tree.layout(child).location, Point::new(15.0, 16.0));
    assert_size(tree.layout(child).size, Size::new(20.0, 10.0));
    assert_eq!(tree.layout(child).margin, edges(3.0, 7.0, 4.0, 6.0));
}

fn responsive_measure(input: LeafMeasureInput) -> LeafMetrics {
    let width = match input.available_space.width {
        AvailableSpace::Definite(width) => width.min(80.0),
        AvailableSpace::MinContent => 12.0,
        AvailableSpace::MaxContent => 80.0,
    };
    let height = input.known_dimensions.height.unwrap_or(10.0);
    LeafMetrics::new(Size::new(width, height)).with_first_baselines(Point::new(None, Some(7.0)))
}

fn probe_only_baseline(input: LeafMeasureInput) -> LeafMetrics {
    let baseline = matches!(input.goal, LayoutGoal::Measure(_)).then_some(7.0);
    LeafMetrics::new(Size::new(10.0, 10.0)).with_first_baselines(Point::new(None, baseline))
}

fn height_from_width(input: LeafMeasureInput) -> LeafMetrics {
    let width = input.known_dimensions.width.unwrap_or(10.0);
    LeafMetrics::new(Size::new(width, 100.0 / width))
        .with_first_baselines(Point::new(None, Some(4.0)))
}

#[test]
fn fully_known_commit_elides_the_probe_but_still_commits_the_child() {
    let mut tree = TestTree::default();
    let child = tree.push_measured_leaf(fixed_style(20.0, 10.0), responsive_measure);
    let root = tree.push_linear(TestStyle::default(), vec![child]);
    tree.enable_cache();

    definite_layout(&tree, root, 100.0, 40.0);

    assert_eq!(tree.child_layout_calls.get(), 2);
    assert_eq!(
        tree.committed_input(child)
            .expect("the final child Commit must remain")
            .goal,
        LayoutGoal::Commit
    );
    assert_size(tree.layout(child).size, Size::new(20.0, 10.0));
}

#[test]
fn known_width_auto_height_retains_the_height_for_width_probe() {
    let mut tree = TestTree::default();
    let child = tree.push_measured_leaf(
        TestStyle {
            size: Size::new(size_px(20.0), size_auto()),
            align_self: self_align(AlignFlags::START),
            ..TestStyle::default()
        },
        height_from_width,
    );
    let root = tree.push_linear(
        TestStyle {
            linear_direction: linear_direction::T::Row,
            ..TestStyle::default()
        },
        vec![child],
    );
    tree.enable_cache();

    definite_layout(&tree, root, 100.0, 40.0);

    assert!(tree.measure_inputs(child).iter().any(|input| {
        matches!(input.goal, LayoutGoal::Measure(_))
            && input.known_dimensions == Size::new(Some(20.0), None)
    }));
    let committed = tree
        .committed_input(child)
        .expect("height-for-width measurement must still reach Commit");
    assert_eq!(committed.known_dimensions, Size::new(Some(20.0), Some(5.0)));
    assert_size(tree.layout(child).size, Size::new(20.0, 5.0));
}

#[test]
fn layout_contained_auto_measure_uses_known_child_size_without_a_probe() {
    let mut tree = TestTree::default();
    let child = tree.push_measured_leaf(fixed_style(20.0, 10.0), responsive_measure);
    let root = tree.push_linear(
        TestStyle {
            containment: Contain::LAYOUT,
            ..TestStyle::default()
        },
        vec![child],
    );

    let output = measure_layout(&tree, root, Size::NONE, Size::MAX_CONTENT);

    assert_size(output.size, Size::new(20.0, 10.0));
    assert_eq!(output.first_baselines.y, None);
    assert_eq!(tree.child_layout_calls.get(), 1);
    assert!(tree.measure_inputs(child).is_empty());
}

#[test]
fn content_independent_contained_measure_skips_children_but_noncontained_keeps_baseline() {
    let run = |containment| {
        let mut tree = TestTree::default();
        let child = tree.push_leaf(fixed_style(10.0, 10.0), Size::new(10.0, 10.0), Some(7.0));
        let root = tree.push_linear(
            TestStyle {
                size: Size::new(size_px(100.0), size_px(40.0)),
                containment,
                ..TestStyle::default()
            },
            vec![child],
        );
        let output = measure_layout(&tree, root, Size::NONE, Size::MAX_CONTENT);
        (output, tree.child_layout_calls.get())
    };

    let (noncontained, noncontained_calls) = run(Contain::empty());
    assert_eq!(noncontained.first_baselines.y, Some(7.0));
    assert_eq!(noncontained_calls, 2);

    let (contained, contained_calls) = run(Contain::LAYOUT);
    assert_eq!(contained.first_baselines.y, None);
    assert_eq!(contained_calls, 1);
}

#[test]
fn fit_content_and_definite_cross_stretch_reach_leaf_measurement() {
    let mut tree = TestTree::default();
    let fit = tree.push_measured_leaf(
        TestStyle {
            size: Size::new(size_fit_content_px(30.0), size_px(10.0)),
            align_self: self_align(AlignFlags::START),
            ..TestStyle::default()
        },
        responsive_measure,
    );
    let stretched = tree.push_measured_leaf(TestStyle::default(), responsive_measure);
    let root = tree.push_linear(TestStyle::default(), vec![fit, stretched]);
    definite_layout(&tree, root, 100.0, 40.0);

    assert_close(tree.layout(fit).size.width, 30.0);
    assert_close(tree.layout(stretched).size.width, 100.0);
    assert!(
        tree.measure_inputs(stretched)
            .iter()
            .any(|input| input.known_dimensions.width == Some(100.0))
    );
}

#[test]
fn auto_main_axis_preserves_grid_min_and_max_content_probes() {
    let mut tree = TestTree::default();
    let child = tree.push_measured_leaf(TestStyle::default(), responsive_measure);
    let root = tree.push_linear(
        TestStyle {
            linear_direction: linear_direction::T::Row,
            ..TestStyle::default()
        },
        vec![child],
    );

    let min_content = perform_layout(
        &tree,
        root,
        Size::NONE,
        Size::new(AvailableSpace::MinContent, AvailableSpace::MaxContent),
    );
    let max_content = max_content_layout(&tree, root);

    assert_close(min_content.size.width, 12.0);
    assert_close(max_content.size.width, 80.0);
    assert!(
        tree.measure_inputs(child)
            .iter()
            .any(|input| input.available_space.width == AvailableSpace::MinContent)
    );
    assert!(
        tree.measure_inputs(child)
            .iter()
            .any(|input| input.available_space.width == AvailableSpace::MaxContent)
    );
}

#[test]
fn intrinsic_keyword_probe_requests_one_axis_and_preserves_the_cross_size() {
    let mut tree = TestTree::default();
    let child = tree.push_measured_leaf(
        TestStyle {
            size: Size::new(size_min_content(), size_px(30.0)),
            align_self: self_align(AlignFlags::START),
            ..TestStyle::default()
        },
        responsive_measure,
    );
    let root = tree.push_linear(
        TestStyle {
            linear_direction: linear_direction::T::Row,
            ..TestStyle::default()
        },
        vec![child],
    );

    definite_layout(&tree, root, 100.0, 50.0);

    assert!(tree.measure_inputs(child).iter().any(|input| {
        input.goal == LayoutGoal::Measure(RequestedAxis::Horizontal)
            && input.available_space.width == AvailableSpace::MinContent
            && input.available_space.height == AvailableSpace::Definite(30.0)
            && input.known_dimensions.height == Some(30.0)
    }));
}

#[test]
fn max_content_keyword_does_not_issue_a_min_content_probe() {
    let mut tree = TestTree::default();
    let child = tree.push_measured_leaf(
        TestStyle {
            size: Size::new(size_max_content(), size_px(30.0)),
            align_self: self_align(AlignFlags::START),
            ..TestStyle::default()
        },
        responsive_measure,
    );
    let root = tree.push_linear(
        TestStyle {
            linear_direction: linear_direction::T::Row,
            ..TestStyle::default()
        },
        vec![child],
    );

    definite_layout(&tree, root, 100.0, 50.0);

    assert!(tree.measure_inputs(child).iter().any(|input| {
        input.goal == LayoutGoal::Measure(RequestedAxis::Horizontal)
            && input.available_space.width == AvailableSpace::MaxContent
    }));
    assert!(
        !tree
            .measure_inputs(child)
            .iter()
            .any(|input| input.available_space.width == AvailableSpace::MinContent)
    );
}

mod baseline {
    use super::*;

    #[test]
    fn horizontal_linear_baseline_uses_the_largest_child_baseline() {
        let mut row_tree = TestTree::default();
        let early = row_tree.push_leaf(fixed_style(10.0, 30.0), Size::new(10.0, 30.0), Some(5.0));
        let late = row_tree.push_leaf(fixed_style(10.0, 20.0), Size::new(10.0, 20.0), Some(15.0));
        let row = row_tree.push_linear(
            TestStyle {
                linear_direction: linear_direction::T::Row,
                ..TestStyle::default()
            },
            vec![early, late],
        );
        let output = definite_layout(&row_tree, row, 100.0, 40.0);
        assert_close(output.first_baselines.y.unwrap(), 15.0);
    }

    #[test]
    fn vertical_linear_baseline_includes_main_axis_alignment_offset() {
        let mut column_tree = TestTree::default();
        let first =
            column_tree.push_leaf(fixed_style(10.0, 20.0), Size::new(10.0, 20.0), Some(5.0));
        let second = fixed_leaf(&mut column_tree, 10.0, 10.0);
        let column = column_tree.push_linear(
            TestStyle {
                justify_content: content(AlignFlags::CENTER),
                ..TestStyle::default()
            },
            vec![first, second],
        );
        let output = definite_layout(&column_tree, column, 20.0, 100.0);
        assert_close(output.first_baselines.y.unwrap(), 40.0);
    }

    #[test]
    fn empty_linear_container_has_no_first_baseline() {
        let mut tree = TestTree::default();
        let empty = tree.push_linear(TestStyle::default(), Vec::new());
        let output = definite_layout(&tree, empty, 20.0, 20.0);
        assert_eq!(output.first_baselines.y, None);
    }

    #[test]
    fn a_missing_child_baseline_uses_its_bottom_edge_without_leaking_probe_data() {
        let mut tree = TestTree::default();
        let child = tree.push_measured_leaf(TestStyle::default(), probe_only_baseline);
        let root = tree.push_linear(TestStyle::default(), vec![child]);

        let output = definite_layout(&tree, root, 20.0, 20.0);

        assert_eq!(output.first_baselines.y, Some(10.0));
    }
}

#[test]
fn nested_linear_container_applies_its_inner_justify_content() {
    let mut tree = TestTree::default();
    let first = fixed_leaf(&mut tree, 10.0, 10.0);
    let second = fixed_leaf(&mut tree, 20.0, 10.0);
    let inner = tree.push_linear(
        TestStyle {
            size: Size::new(size_px(100.0), size_px(20.0)),
            linear_direction: linear_direction::T::Row,
            justify_content: content(AlignFlags::END),
            ..TestStyle::default()
        },
        vec![first, second],
    );
    let outer = tree.push_linear(TestStyle::default(), vec![inner]);
    definite_layout(&tree, outer, 100.0, 100.0);
    assert_close(tree.layout(inner).location.y, 0.0);
    assert_close(tree.layout(first).location.x, 70.0);
    assert_close(tree.layout(second).location.x, 80.0);
}

#[test]
fn a_linear_item_can_be_a_grid_container() {
    let mut linear_root_tree = TestTree::default();
    let grid_leaf = fixed_leaf(&mut linear_root_tree, 12.0, 8.0);
    let grid = linear_root_tree.push_grid(fixed_style(30.0, 20.0), vec![grid_leaf]);
    let linear_root = linear_root_tree.push_linear(
        TestStyle {
            linear_direction: linear_direction::T::Row,
            ..TestStyle::default()
        },
        vec![grid],
    );
    definite_layout(&linear_root_tree, linear_root, 100.0, 40.0);
    assert_size(linear_root_tree.layout(grid).size, Size::new(30.0, 20.0));
    assert_size(
        linear_root_tree.layout(grid_leaf).size,
        Size::new(12.0, 8.0),
    );
}

#[test]
fn a_grid_item_can_be_a_linear_container() {
    let mut grid_root_tree = TestTree::default();
    let linear_leaf = fixed_leaf(&mut grid_root_tree, 10.0, 6.0);
    let linear = grid_root_tree.push_linear(
        TestStyle {
            size: Size::new(size_px(30.0), size_px(20.0)),
            linear_direction: linear_direction::T::Row,
            justify_content: content(AlignFlags::END),
            ..TestStyle::default()
        },
        vec![linear_leaf],
    );
    let grid_root = grid_root_tree.push_grid(fixed_style(100.0, 40.0), vec![linear]);
    definite_layout(&grid_root_tree, grid_root, 100.0, 40.0);
    assert_size(grid_root_tree.layout(linear).size, Size::new(30.0, 20.0));
    assert_point(
        grid_root_tree.layout(linear_leaf).location,
        Point::new(20.0, 0.0),
    );
}

#[test]
fn a_linear_item_can_be_a_flex_container() {
    let mut linear_root_tree = TestTree::default();
    let flex_leaf = fixed_leaf(&mut linear_root_tree, 12.0, 8.0);
    let flex = linear_root_tree.push_flex(fixed_style(30.0, 20.0), vec![flex_leaf]);
    let linear_root = linear_root_tree.push_linear(
        TestStyle {
            linear_direction: linear_direction::T::Row,
            ..TestStyle::default()
        },
        vec![flex],
    );
    definite_layout(&linear_root_tree, linear_root, 100.0, 40.0);
    assert_size(linear_root_tree.layout(flex).size, Size::new(30.0, 20.0));
    assert_size(
        linear_root_tree.layout(flex_leaf).size,
        Size::new(12.0, 8.0),
    );
}

#[test]
fn a_flex_item_can_be_a_linear_container() {
    let mut flex_root_tree = TestTree::default();
    let linear_leaf = fixed_leaf(&mut flex_root_tree, 10.0, 6.0);
    let linear = flex_root_tree.push_linear(
        TestStyle {
            size: Size::new(size_px(30.0), size_px(20.0)),
            linear_direction: linear_direction::T::Row,
            justify_content: content(AlignFlags::END),
            ..TestStyle::default()
        },
        vec![linear_leaf],
    );
    let flex_root = flex_root_tree.push_flex(fixed_style(100.0, 40.0), vec![linear]);
    definite_layout(&flex_root_tree, flex_root, 100.0, 40.0);
    assert_size(flex_root_tree.layout(linear).size, Size::new(30.0, 20.0));
    assert_point(
        flex_root_tree.layout(linear_leaf).location,
        Point::new(20.0, 0.0),
    );
}

#[test]
fn flex_max_content_target_enables_linear_weight_distribution() {
    let mut tree = TestTree::default();
    let narrow = fixed_leaf_with(&mut tree, 10.0, 10.0, |style| {
        style.linear_weight = nn(1.0);
    });
    let wide = fixed_leaf_with(&mut tree, 90.0, 10.0, |style| {
        style.linear_weight = nn(1.0);
    });
    let linear = tree.push_linear(
        TestStyle {
            linear_direction: linear_direction::T::Row,
            ..TestStyle::default()
        },
        vec![narrow, wide],
    );
    let flex = tree.push_flex(TestStyle::default(), vec![linear]);

    let output = perform_layout(
        &tree,
        flex,
        Size::new(None, Some(20.0)),
        Size::new(AvailableSpace::MaxContent, AvailableSpace::Definite(20.0)),
    );

    assert_close(output.size.width, 100.0);
    assert_close(tree.layout(linear).size.width, 100.0);
    assert_close(tree.layout(narrow).size.width, 50.0);
    assert_close(tree.layout(wide).size.width, 50.0);
}

#[test]
fn flex_max_content_target_enables_linear_default_cross_stretch() {
    let mut tree = TestTree::default();
    let narrow = tree.push_leaf(
        TestStyle {
            size: Size::new(size_auto(), size_px(10.0)),
            ..TestStyle::default()
        },
        Size::new(10.0, 10.0),
        None,
    );
    let wide = tree.push_leaf(
        TestStyle {
            size: Size::new(size_auto(), size_px(10.0)),
            ..TestStyle::default()
        },
        Size::new(90.0, 10.0),
        None,
    );
    let linear = tree.push_linear(TestStyle::default(), vec![narrow, wide]);
    let flex = tree.push_flex(TestStyle::default(), vec![linear]);

    let output = perform_layout(
        &tree,
        flex,
        Size::new(None, Some(20.0)),
        Size::new(AvailableSpace::MaxContent, AvailableSpace::Definite(20.0)),
    );

    assert_close(output.size.width, 90.0);
    assert_close(tree.layout(linear).size.width, 90.0);
    assert_close(tree.layout(narrow).size.width, 90.0);
    assert_close(tree.layout(wide).size.width, 90.0);
}

#[test]
fn flex_known_but_indefinite_linear_size_is_not_a_percentage_basis() {
    let mut tree = TestTree::default();
    let percentage = tree.push_leaf(
        TestStyle {
            size: Size::new(size_pct(0.5), size_px(10.0)),
            ..TestStyle::default()
        },
        Size::new(80.0, 10.0),
        None,
    );
    let fixed = fixed_leaf(&mut tree, 20.0, 10.0);
    let linear = tree.push_linear(
        TestStyle {
            linear_direction: linear_direction::T::Row,
            ..TestStyle::default()
        },
        vec![percentage, fixed],
    );
    let flex = tree.push_flex(TestStyle::default(), vec![linear]);

    let output = perform_layout(
        &tree,
        flex,
        Size::new(None, Some(20.0)),
        Size::new(AvailableSpace::MaxContent, AvailableSpace::Definite(20.0)),
    );

    assert_close(output.size.width, 100.0);
    assert_close(tree.layout(linear).size.width, 100.0);
    assert_close(tree.layout(percentage).size.width, 80.0);
    assert_close(tree.layout(fixed).location.x, 80.0);
}

#[test]
fn relative_insets_move_visual_box_without_advancing_following_flow() {
    let mut tree = TestTree::default();
    let shifted = fixed_leaf_with(&mut tree, 10.0, 10.0, |style| {
        style.inset = edges(inset_px(5.0), inset_auto(), inset_px(3.0), inset_auto());
    });
    let following = fixed_leaf(&mut tree, 10.0, 10.0);
    let root = tree.push_linear(
        TestStyle {
            linear_direction: linear_direction::T::Row,
            ..TestStyle::default()
        },
        vec![shifted, following],
    );
    definite_layout(&tree, root, 100.0, 20.0);
    assert_point(tree.layout(shifted).location, Point::new(5.0, 3.0));
    assert_point(tree.layout(following).location, Point::new(10.0, 0.0));
}

#[test]
fn absolute_children_use_linear_static_alignment_and_insets_override_it() {
    let mut tree = TestTree::default();
    let centered = fixed_leaf_with(&mut tree, 10.0, 8.0, |style| {
        style.position = PositionProperty::Absolute;
        style.align_self = self_align(AlignFlags::CENTER);
    });
    let inset = fixed_leaf_with(&mut tree, 10.0, 8.0, |style| {
        style.position = PositionProperty::Absolute;
        style.inset = edges(inset_px(5.0), inset_auto(), inset_px(7.0), inset_auto());
    });
    let root = tree.push_linear(
        TestStyle {
            linear_direction: linear_direction::T::Row,
            justify_content: content(AlignFlags::CENTER),
            ..TestStyle::default()
        },
        vec![centered, inset],
    );
    definite_layout(&tree, root, 100.0, 50.0);
    assert_point(tree.layout(centered).location, Point::new(45.0, 21.0));
    assert_point(tree.layout(inset).location, Point::new(5.0, 7.0));
    assert!(
        tree.measure_inputs(centered)
            .iter()
            .all(|input| !matches!(input.goal, LayoutGoal::Measure(_)))
    );
    assert!(
        tree.measure_inputs(inset)
            .iter()
            .all(|input| !matches!(input.goal, LayoutGoal::Measure(_)))
    );
}

#[test]
fn absolute_static_alignment_uses_the_padding_box() {
    let mut tree = TestTree::default();
    let child = fixed_leaf_with(&mut tree, 10.0, 10.0, |style| {
        style.position = PositionProperty::Absolute;
        style.align_self = self_align(AlignFlags::CENTER);
    });
    let root = tree.push_linear(
        TestStyle {
            linear_direction: linear_direction::T::Row,
            justify_content: content(AlignFlags::CENTER),
            padding: edges(npx(10.0), npx(20.0), npx(5.0), npx(15.0)),
            border: Edges::uniform(border_px(2.0)),
            ..TestStyle::default()
        },
        vec![child],
    );
    definite_layout(&tree, root, 100.0, 60.0);
    assert_point(tree.layout(child).location, Point::new(45.0, 25.0));
}

#[test]
fn absolute_static_alignment_uses_common_inset_and_aspect_ratio_sizing() {
    let mut tree = TestTree::default();
    let child = tree.push_leaf(
        TestStyle {
            position: PositionProperty::Absolute,
            inset: edges(inset_px(10.0), inset_px(10.0), inset_auto(), inset_auto()),
            aspect_ratio: ratio(2.0),
            align_self: self_align(AlignFlags::CENTER),
            ..TestStyle::default()
        },
        Size::new(12.0, 12.0),
        None,
    );
    let root = tree.push_linear(
        TestStyle {
            linear_direction: linear_direction::T::Row,
            ..TestStyle::default()
        },
        vec![child],
    );
    definite_layout(&tree, root, 100.0, 100.0);
    assert_size(tree.layout(child).size, Size::new(80.0, 40.0));
    assert_point(tree.layout(child).location, Point::new(10.0, 30.0));
    assert!(
        tree.measure_inputs(child)
            .iter()
            .all(|input| !matches!(input.goal, LayoutGoal::Measure(_)))
    );
}

#[test]
fn absolute_static_alignment_preserves_reversal_and_margin_box_edges() {
    let mut tree = TestTree::default();
    let child = fixed_leaf_with(&mut tree, 10.0, 8.0, |style| {
        style.position = PositionProperty::Absolute;
        style.margin = edges(
            margin_px(3.0),
            margin_px(7.0),
            margin_px(2.0),
            margin_px(4.0),
        );
        style.align_self = self_align(AlignFlags::END);
    });
    let root = tree.push_linear(
        TestStyle {
            linear_direction: linear_direction::T::RowReverse,
            justify_content: content(AlignFlags::END),
            ..TestStyle::default()
        },
        vec![child],
    );

    definite_layout(&tree, root, 100.0, 50.0);

    assert_point(tree.layout(child).location, Point::new(3.0, 38.0));
}

#[test]
fn hoisted_children_record_static_position_without_local_commit() {
    let mut tree = TestTree::default();
    let hoisted = tree.push_measured_leaf(
        TestStyle {
            position: PositionProperty::Fixed,
            align_self: self_align(AlignFlags::END),
            ..fixed_style(20.0, 10.0)
        },
        responsive_measure,
    );
    let root = tree.push_linear(
        TestStyle {
            linear_direction: linear_direction::T::Row,
            justify_content: content(AlignFlags::CENTER),
            ..TestStyle::default()
        },
        vec![hoisted],
    );
    definite_layout(&tree, root, 100.0, 50.0);

    assert_point(
        tree.static_position(hoisted)
            .expect("hoisted static position"),
        Point::new(40.0, 40.0),
    );
    assert_eq!(tree.layout(hoisted), Layout::default());
    assert!(!tree.measure_inputs(hoisted).is_empty());
    assert!(
        tree.measure_inputs(hoisted)
            .iter()
            .all(|input| matches!(input.goal, LayoutGoal::Measure(_)))
    );
}

#[test]
fn hoisted_static_fallback_measures_only_the_axis_with_auto_insets() {
    let mut tree = TestTree::default();
    let hoisted = tree.push_measured_leaf(
        TestStyle {
            position: PositionProperty::Fixed,
            inset: edges(inset_px(10.0), inset_px(10.0), inset_auto(), inset_auto()),
            align_self: self_align(AlignFlags::END),
            size: Size::new(size_px(20.0), size_auto()),
            ..TestStyle::default()
        },
        responsive_measure,
    );
    let root = tree.push_linear(
        TestStyle {
            linear_direction: linear_direction::T::Row,
            justify_content: content(AlignFlags::CENTER),
            ..TestStyle::default()
        },
        vec![hoisted],
    );

    definite_layout(&tree, root, 100.0, 50.0);

    assert!(
        tree.measure_inputs(hoisted)
            .iter()
            .any(|input| { input.goal == LayoutGoal::Measure(RequestedAxis::Vertical) })
    );
    assert!(tree.measure_inputs(hoisted).iter().all(|input| {
        !matches!(
            input.goal,
            LayoutGoal::Measure(RequestedAxis::Horizontal | RequestedAxis::Both)
        )
    }));
}

#[test]
fn real_cache_does_not_let_linear_measurement_satisfy_commit() {
    let mut tree = TestTree::default();
    let measured = tree.push_measured_leaf(TestStyle::default(), responsive_measure);
    let hoisted = tree.push_measured_leaf(
        TestStyle {
            position: PositionProperty::Fixed,
            ..fixed_style(10.0, 10.0)
        },
        responsive_measure,
    );
    let root = tree.push_linear(TestStyle::default(), vec![measured, hoisted]);
    tree.enable_cache();
    let sentinel = || {
        let mut layout = Layout::default();
        layout.location = Point::new(123.0, 456.0);
        layout.size = Size::new(7.0, 8.0);
        layout
    };
    tree.set_layout_for_testing(measured, sentinel());
    tree.set_layout_for_testing(hoisted, sentinel());
    tree.set_layout_for_testing(root, sentinel());

    let output = measure_layout(
        &tree,
        root,
        Size::new(Some(100.0), None),
        Size::new(AvailableSpace::Definite(100.0), AvailableSpace::MaxContent),
    );
    assert!(output.size.width.is_finite() && output.size.height.is_finite());
    assert_eq!(tree.layout_writes.get(), 0);
    assert_eq!(tree.static_position_writes.get(), 0);
    assert_eq!(tree.layout(measured), sentinel());
    assert_eq!(tree.layout(hoisted), sentinel());
    assert_eq!(tree.layout(root), sentinel());
    assert!(
        tree.measure_inputs(measured)
            .iter()
            .all(|input| matches!(input.goal, LayoutGoal::Measure(_)))
    );

    let measured_calls = tree.measure_inputs(measured).len();
    let cached = measure_layout(
        &tree,
        root,
        Size::new(Some(100.0), None),
        Size::new(AvailableSpace::Definite(100.0), AvailableSpace::MaxContent),
    );
    assert_eq!(cached, output);
    assert_eq!(tree.measure_inputs(measured).len(), measured_calls);

    let resized = measure_layout(
        &tree,
        root,
        Size::new(Some(80.0), None),
        Size::new(AvailableSpace::Definite(80.0), AvailableSpace::MaxContent),
    );
    assert_close(resized.size.width, 80.0);
    assert!(tree.measure_inputs(measured).len() > measured_calls);
    let before_commit = tree.measure_inputs(measured).len();

    perform_layout(
        &tree,
        root,
        Size::new(Some(80.0), None),
        Size::new(AvailableSpace::Definite(80.0), AvailableSpace::MaxContent),
    );
    assert!(tree.layout_writes.get() > 0);
    assert!(tree.static_position_writes.get() > 0);
    assert!(
        tree.measure_inputs(measured)[before_commit..]
            .iter()
            .any(|input| input.goal == LayoutGoal::Commit)
    );
}

#[test]
fn content_sized_container_applies_min_max_after_natural_size() {
    let mut tree = TestTree::default();
    let first = fixed_leaf(&mut tree, 20.0, 40.0);
    let second = fixed_leaf(&mut tree, 10.0, 20.0);
    let root = tree.push_linear(
        TestStyle {
            linear_direction: linear_direction::T::Row,
            min_size: Size::new(size_px(50.0), size_auto()),
            max_size: Size::new(max_none(), max_px(30.0)),
            ..TestStyle::default()
        },
        vec![first, second],
    );
    let output = max_content_layout(&tree, root);
    assert_size(output.size, Size::new(50.0, 30.0));
}

#[test]
fn calc_item_size_and_percent_min_max_resolve_against_container_content_box() {
    let mut tree = TestTree::default();
    let child = tree.push_leaf(
        TestStyle {
            size: Size::new(size_calc(5.0, 0.5), size_px(10.0)),
            min_size: Size::new(size_pct(0.4), size_auto()),
            max_size: Size::new(max_pct(0.6), max_none()),
            ..TestStyle::default()
        },
        Size::new(1.0, 10.0),
        None,
    );
    let root = tree.push_linear(
        TestStyle {
            padding: edges(npx(10.0), npx(10.0), npx(0.0), npx(0.0)),
            border: edges(
                border_px(2.0),
                border_px(2.0),
                border_px(0.0),
                border_px(0.0),
            ),
            ..TestStyle::default()
        },
        vec![child],
    );
    definite_layout(&tree, root, 124.0, 40.0);
    assert_close(tree.layout(child).size.width, 55.0);
    assert_close(tree.layout(child).location.x, 12.0);
}

#[test]
fn intrinsic_inline_percentage_edges_resolve_without_changing_their_basis() {
    let mut tree = TestTree::default();
    let child = fixed_leaf_with(&mut tree, 20.0, 10.0, |style| {
        style.margin = edges(
            margin_pct(0.5),
            margin_px(0.0),
            margin_px(0.0),
            margin_px(0.0),
        );
    });
    let root = tree.push_linear(TestStyle::default(), vec![child]);
    let output = max_content_layout(&tree, root);
    assert_close(output.size.width, 20.0);
    assert_close(output.content_size.width, 30.0);
    assert_close(tree.layout(child).location.x, 10.0);
    assert_close(tree.layout(child).margin.left, 10.0);
}

#[test]
fn intrinsic_percentage_box_refresh_does_not_remeasure_children() {
    let mut tree = TestTree::default();
    let dependent = tree.push_leaf(
        TestStyle {
            size: Size::new(size_pct(0.5), size_px(10.0)),
            margin: edges(
                margin_pct(0.5),
                margin_px(0.0),
                margin_px(0.0),
                margin_px(0.0),
            ),
            ..TestStyle::default()
        },
        Size::new(80.0, 10.0),
        None,
    );
    let independent = fixed_leaf(&mut tree, 20.0, 10.0);
    let root = tree.push_linear(
        TestStyle {
            linear_direction: linear_direction::T::Row,
            justify_content: content(AlignFlags::END),
            ..TestStyle::default()
        },
        vec![dependent, independent],
    );

    let output = max_content_layout(&tree, root);

    assert_close(output.size.width, 100.0);
    assert_close(output.content_size.width, 150.0);
    assert_close(tree.layout(dependent).size.width, 80.0);
    assert_close(tree.layout(dependent).margin.left, 50.0);
    assert_close(tree.layout(dependent).location.x, 50.0);
    assert_close(tree.layout(independent).location.x, 130.0);
    assert_eq!(
        tree.measure_inputs(dependent)
            .iter()
            .filter(|input| matches!(input.goal, LayoutGoal::Measure(_)))
            .count(),
        1
    );
    assert_eq!(
        tree.measure_inputs(independent)
            .iter()
            .filter(|input| matches!(input.goal, LayoutGoal::Measure(_)))
            .count(),
        0
    );
}

#[test]
fn intrinsic_percentage_box_refresh_precedes_container_min_clamp() {
    let mut tree = TestTree::default();
    let child = fixed_leaf_with(&mut tree, 20.0, 10.0, |style| {
        style.margin = edges(
            margin_pct(0.5),
            margin_px(0.0),
            margin_px(0.0),
            margin_px(0.0),
        );
        style.inset = edges(inset_auto(), inset_auto(), inset_pct(0.5), inset_auto());
    });
    let root = tree.push_linear(
        TestStyle {
            min_size: Size::new(size_px(100.0), size_px(100.0)),
            ..TestStyle::default()
        },
        vec![child],
    );

    let output = max_content_layout(&tree, root);

    assert_size(output.size, Size::new(100.0, 100.0));
    assert_close(tree.layout(child).margin.left, 10.0);
    assert_point(tree.layout(child).location, Point::new(10.0, 50.0));
}

#[test]
fn weighted_minima_freeze_every_item_and_overflow_the_axis() {
    let mut tree = TestTree::default();
    let make_item = |tree: &mut TestTree| {
        tree.push_leaf(
            TestStyle {
                linear_weight: nn(1.0),
                min_size: Size::new(size_auto(), size_px(80.0)),
                ..TestStyle::default()
            },
            Size::ZERO,
            None,
        )
    };
    let first = make_item(&mut tree);
    let second = make_item(&mut tree);
    let root = tree.push_linear(TestStyle::default(), vec![first, second]);

    definite_layout(&tree, root, 50.0, 100.0);

    assert_close(tree.layout(first).size.height, 80.0);
    assert_close(tree.layout(second).size.height, 80.0);
    assert_close(tree.layout(first).location.y, 0.0);
    assert_close(tree.layout(second).location.y, 80.0);
}

#[test]
fn intrinsic_keywords_resolve_on_linear_items() {
    let run = |style: TestStyle| -> Size<f32> {
        let mut tree = TestTree::default();
        let item = tree.push_intrinsic_leaf(style, Size::new(30.0, 12.0), Size::new(90.0, 48.0));
        let root = tree.push_linear(TestStyle::default(), vec![item]);
        definite_layout(&tree, root, 200.0, 100.0);
        tree.layout(item).size
    };

    assert_close(
        run(TestStyle {
            size: Size::new(size_min_content(), size_auto()),
            ..TestStyle::default()
        })
        .width,
        30.0,
    );
    assert_close(
        run(TestStyle {
            size: Size::new(size_max_content(), size_auto()),
            ..TestStyle::default()
        })
        .width,
        90.0,
    );
    assert_close(
        run(TestStyle {
            size: Size::new(size_px(40.0), size_max_content()),
            ..TestStyle::default()
        })
        .height,
        48.0,
    );
    let both = run(TestStyle {
        size: Size::new(size_min_content(), size_min_content()),
        ..TestStyle::default()
    });
    assert_size(both, Size::new(30.0, 12.0));

    let stretched = run(TestStyle {
        size: Size::new(StyleSize::Stretch, size_auto()),
        min_size: Size::new(size_min_content(), size_auto()),
        ..TestStyle::default()
    });
    assert_close(stretched.width, 200.0);
    let fit_keyword = run(TestStyle {
        size: Size::new(StyleSize::FitContent, size_auto()),
        min_size: Size::new(size_max_content(), size_auto()),
        ..TestStyle::default()
    });
    assert_close(fit_keyword.width, 200.0);

    assert_close(
        run(TestStyle {
            max_size: Size::new(max_min_content(), max_none()),
            ..TestStyle::default()
        })
        .width,
        30.0,
    );
    assert_close(
        run(TestStyle {
            max_size: Size::new(max_max_content(), max_none()),
            ..TestStyle::default()
        })
        .width,
        90.0,
    );
    assert_close(
        run(TestStyle {
            max_size: Size::new(max_fit_content_px(40.0), max_none()),
            ..TestStyle::default()
        })
        .width,
        40.0,
    );
    assert_close(
        run(TestStyle {
            min_size: Size::new(size_min_content(), size_auto()),
            max_size: Size::new(max_px(50.0), max_none()),
            ..TestStyle::default()
        })
        .width,
        50.0,
    );
    assert_close(
        run(TestStyle {
            max_size: Size::new(MaxSize::FitContent, max_none()),
            min_size: Size::new(size_min_content(), size_auto()),
            ..TestStyle::default()
        })
        .width,
        200.0,
    );
    assert_close(
        run(TestStyle {
            max_size: Size::new(MaxSize::Stretch, max_none()),
            min_size: Size::new(size_min_content(), size_auto()),
            ..TestStyle::default()
        })
        .width,
        200.0,
    );
}

#[test]
fn percentage_padding_and_auto_margins_refresh_after_container_sizing() {
    let mut tree = TestTree::default();
    let wide = fixed_leaf(&mut tree, 100.0, 10.0);
    let padded = tree.push_leaf(
        TestStyle {
            size: Size::new(size_px(20.0), size_px(10.0)),
            padding: Edges {
                left: support::npct(0.1),
                ..Edges::uniform(npx(0.0))
            },
            margin: Edges {
                left: margin_auto(),
                right: margin_auto(),
                top: margin_px(0.0),
                bottom: margin_px(0.0),
            },
            ..TestStyle::default()
        },
        Size::new(20.0, 10.0),
        None,
    );
    let root = tree.push_linear(TestStyle::default(), vec![wide, padded]);

    let output = max_content_layout(&tree, root);

    assert_close(output.size.width, 100.0);
    let layout = tree.layout(padded);
    assert_close(layout.padding.left, 10.0);
    assert_close(layout.size.width, 20.0);
    assert_close(layout.location.x, 40.0);
}

#[test]
fn relative_insets_follow_item_direction_and_refresh_percentages() {
    let mut tree = TestTree::default();
    let rtl_item = fixed_leaf_with(&mut tree, 20.0, 10.0, |style| {
        style.direction = direction::T::Rtl;
        style.inset = edges(inset_pct(0.1), inset_px(10.0), inset_auto(), inset_auto());
    });
    let right_only = fixed_leaf_with(&mut tree, 20.0, 10.0, |style| {
        style.inset = edges(inset_auto(), inset_px(5.0), inset_auto(), inset_auto());
    });
    let px_item = fixed_leaf_with(&mut tree, 20.0, 10.0, |style| {
        style.inset = edges(inset_px(7.0), inset_auto(), inset_auto(), inset_auto());
    });
    let root = tree.push_linear(TestStyle::default(), vec![rtl_item, right_only, px_item]);

    definite_layout(&tree, root, 200.0, 100.0);

    assert_close(tree.layout(rtl_item).location.x, -10.0);
    assert_close(tree.layout(right_only).location.x, -5.0);
    assert_close(tree.layout(px_item).location.x, 7.0);
    assert_close(tree.layout(right_only).location.y, 10.0);
}

#[test]
fn container_aspect_ratio_derives_missing_axis_with_clamps() {
    let mut tree = TestTree::default();
    let root = tree.push_linear(
        TestStyle {
            aspect_ratio: ratio(2.0),
            padding: Edges::uniform(npx(10.0)),
            ..TestStyle::default()
        },
        Vec::new(),
    );
    let output = perform_layout(
        &tree,
        root,
        Size::new(None, Some(100.0)),
        Size::new(AvailableSpace::MaxContent, AvailableSpace::Definite(100.0)),
    );
    assert_size(output.size, Size::new(180.0, 100.0));

    let mut tree = TestTree::default();
    let root = tree.push_linear(
        TestStyle {
            aspect_ratio: ratio(2.0),
            max_size: Size::new(max_px(150.0), max_none()),
            ..TestStyle::default()
        },
        Vec::new(),
    );
    let output = perform_layout(
        &tree,
        root,
        Size::new(None, Some(100.0)),
        Size::new(AvailableSpace::MaxContent, AvailableSpace::Definite(100.0)),
    );
    assert_size(output.size, Size::new(150.0, 100.0));

    let mut tree = TestTree::default();
    let root = tree.push_linear(
        TestStyle {
            aspect_ratio: ratio(2.0),
            box_sizing: box_sizing::T::BorderBox,
            padding: Edges::uniform(npx(10.0)),
            size: Size::new(size_px(100.0), size_auto()),
            ..TestStyle::default()
        },
        Vec::new(),
    );
    let output = perform_layout(
        &tree,
        root,
        Size::new(Some(100.0), None),
        Size::new(AvailableSpace::Definite(100.0), AvailableSpace::MaxContent),
    );
    assert_size(output.size, Size::new(100.0, 50.0));
}

#[test]
fn weighted_ratio_items_derive_cross_from_forced_main() {
    let mut tree = TestTree::default();
    let derived = tree.push_leaf(
        TestStyle {
            linear_weight: nn(1.0),
            aspect_ratio: ratio(2.0),
            box_sizing: box_sizing::T::BorderBox,
            ..TestStyle::default()
        },
        Size::ZERO,
        None,
    );
    let explicit = tree.push_leaf(
        TestStyle {
            linear_weight: nn(1.0),
            aspect_ratio: ratio(2.0),
            size: Size::new(size_px(40.0), size_auto()),
            ..TestStyle::default()
        },
        Size::ZERO,
        None,
    );
    let root = tree.push_linear(TestStyle::default(), vec![derived, explicit]);

    definite_layout(&tree, root, 300.0, 100.0);

    assert_size(tree.layout(derived).size, Size::new(100.0, 50.0));
    assert_close(tree.layout(explicit).size.width, 40.0);
}

#[test]
fn static_children_skip_relative_inset_nudges() {
    let mut tree = TestTree::default();
    let static_item = fixed_leaf_with(&mut tree, 20.0, 10.0, |style| {
        style.position = PositionProperty::Static;
        style.inset = edges(inset_px(15.0), inset_auto(), inset_px(5.0), inset_auto());
    });
    let root = tree.push_linear(TestStyle::default(), vec![static_item]);

    definite_layout(&tree, root, 100.0, 100.0);

    assert_point(tree.layout(static_item).location, Point::new(0.0, 0.0));
}

#[test]
fn fixed_child_with_pinned_axes_skips_static_measurement() {
    let mut tree = TestTree::default();
    let fixed_child = fixed_leaf_with(&mut tree, 20.0, 10.0, |style| {
        style.position = PositionProperty::Fixed;
        style.inset = edges(inset_px(4.0), inset_auto(), inset_px(6.0), inset_auto());
    });
    let root = tree.push_linear(
        TestStyle {
            border: Edges::uniform(border_px(2.0)),
            padding: Edges::uniform(npx(5.0)),
            ..TestStyle::default()
        },
        vec![fixed_child],
    );

    definite_layout(&tree, root, 100.0, 100.0);

    assert_eq!(
        tree.static_position(fixed_child),
        Some(Point::new(2.0, 2.0))
    );
    assert_eq!(
        tree.session_node(fixed_child).measure_inputs.borrow().len(),
        0
    );
}

#[test]
fn margin_refresh_preserves_auto_edges_alongside_percentages() {
    let mut tree = TestTree::default();
    let wide = fixed_leaf(&mut tree, 100.0, 10.0);
    let item = tree.push_leaf(
        TestStyle {
            size: Size::new(size_px(20.0), size_px(10.0)),
            margin: Edges {
                left: margin_pct(0.1),
                right: margin_auto(),
                top: margin_px(0.0),
                bottom: margin_px(0.0),
            },
            ..TestStyle::default()
        },
        Size::new(20.0, 10.0),
        None,
    );
    let root = tree.push_linear(TestStyle::default(), vec![wide, item]);

    let output = max_content_layout(&tree, root);

    assert_close(output.size.width, 100.0);
    let layout = tree.layout(item);
    assert_close(layout.margin.left, 10.0);
    assert_close(layout.location.x, 10.0);
    assert_close(layout.margin.right, 70.0);
}

#[test]
fn aspect_ratio_transfers_percentage_definiteness_across_axes() {
    let mut tree = TestTree::default();
    let item = tree.push_leaf(
        TestStyle {
            size: Size::new(size_auto(), size_pct(0.5)),
            aspect_ratio: ratio(2.0),
            align_self: self_align(AlignFlags::START),
            ..TestStyle::default()
        },
        Size::ZERO,
        None,
    );
    let root = tree.push_linear(TestStyle::default(), vec![item]);

    definite_layout(&tree, root, 300.0, 100.0);

    assert_size(tree.layout(item).size, Size::new(100.0, 50.0));
}
