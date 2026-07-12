// Portions copyright 2026 The Lynx Authors. All rights reserved.
// Upstream fixtures are licensed under the Apache License, Version 2.0; see
// https://github.com/PupilTong/lynx/blob/dfeedeabfefca7ec5d77ea511071745361/LICENSE.
//
// Focused Rust-only adaptations of the Linear fixtures added by PupilTong/lynx#25,
// plus protocol-level checks for neutron-star's split source/session host.

#[path = "linear_support/mod.rs"]
mod support;

use neutron_star::compute::{LeafMeasureInput, LeafMetrics};
use neutron_star::prelude::*;
use neutron_star::style::{
    AlignContent, AlignItems, BoxGenerationMode, BoxSizing, Dimension, Direction, LengthPercentage,
    LengthPercentageAuto, LinearCrossGravity, LinearGravity, LinearLayoutGravity,
    LinearOrientation, Position, Visibility,
};
use support::{
    TestStyle, TestTree, assert_close, assert_point, assert_size, definite_layout, fixed_leaf,
    measure_layout, perform_layout,
};

fn edges<T>(left: T, right: T, top: T, bottom: T) -> Edges<T> {
    Edges {
        left,
        right,
        top,
        bottom,
    }
}

fn px(value: f32) -> Dimension {
    Dimension::Length(value)
}

fn fixed_style(width: f32, height: f32) -> TestStyle {
    TestStyle {
        size: Size::new(px(width), px(height)),
        ..TestStyle::default()
    }
}

fn axis_positions(
    orientation: LinearOrientation,
    direction: Direction,
) -> (Point<f32>, Point<f32>) {
    let mut tree = TestTree::default();
    let first = fixed_leaf(&mut tree, 10.0, 10.0);
    let second = fixed_leaf(&mut tree, 20.0, 10.0);
    let root = tree.push_linear(
        TestStyle {
            linear_orientation: orientation,
            direction,
            ..TestStyle::default()
        },
        vec![first, second],
    );
    definite_layout(&mut tree, root, 100.0, 100.0);
    (tree.layout(first).location, tree.layout(second).location)
}

#[test]
fn orientation_aliases_reverse_and_rtl_map_to_physical_axes() {
    assert_eq!(
        axis_positions(LinearOrientation::Row, Direction::Ltr),
        axis_positions(LinearOrientation::Horizontal, Direction::Ltr)
    );
    assert_eq!(
        axis_positions(LinearOrientation::Column, Direction::Ltr),
        axis_positions(LinearOrientation::Vertical, Direction::Ltr)
    );
    assert_eq!(
        axis_positions(LinearOrientation::RowReverse, Direction::Ltr),
        axis_positions(LinearOrientation::HorizontalReverse, Direction::Ltr)
    );
    assert_eq!(
        axis_positions(LinearOrientation::ColumnReverse, Direction::Ltr),
        axis_positions(LinearOrientation::VerticalReverse, Direction::Ltr)
    );

    assert_eq!(
        axis_positions(LinearOrientation::Horizontal, Direction::Ltr),
        (Point::new(0.0, 0.0), Point::new(10.0, 0.0))
    );
    assert_eq!(
        axis_positions(LinearOrientation::HorizontalReverse, Direction::Ltr),
        (Point::new(90.0, 0.0), Point::new(70.0, 0.0))
    );
    assert_eq!(
        axis_positions(LinearOrientation::Horizontal, Direction::Rtl),
        (Point::new(90.0, 0.0), Point::new(70.0, 0.0))
    );
    assert_eq!(
        axis_positions(LinearOrientation::HorizontalReverse, Direction::Rtl),
        (Point::new(0.0, 0.0), Point::new(10.0, 0.0))
    );
    assert_eq!(
        axis_positions(LinearOrientation::VerticalReverse, Direction::Rtl),
        (Point::new(90.0, 90.0), Point::new(80.0, 80.0))
    );
}

#[test]
fn order_is_stable_and_layout_order_is_exported() {
    let mut tree = TestTree::default();
    let source_first = tree.push_leaf(
        TestStyle {
            order: 2,
            ..fixed_style(10.0, 10.0)
        },
        Size::new(10.0, 10.0),
        None,
    );
    let equal_first = tree.push_leaf(
        TestStyle {
            order: -1,
            ..fixed_style(10.0, 10.0)
        },
        Size::new(10.0, 10.0),
        None,
    );
    let equal_second = tree.push_leaf(
        TestStyle {
            order: -1,
            ..fixed_style(10.0, 10.0)
        },
        Size::new(10.0, 10.0),
        None,
    );
    let middle = fixed_leaf(&mut tree, 10.0, 10.0);
    let root = tree.push_linear(
        TestStyle {
            linear_orientation: LinearOrientation::Horizontal,
            ..TestStyle::default()
        },
        vec![source_first, equal_first, equal_second, middle],
    );

    definite_layout(&mut tree, root, 100.0, 20.0);

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
    let first_absolute = tree.push_leaf(
        TestStyle {
            position: Position::Absolute,
            ..fixed_style(10.0, 10.0)
        },
        Size::new(10.0, 10.0),
        None,
    );
    let positive = tree.push_leaf(
        TestStyle {
            order: 1,
            ..fixed_style(10.0, 10.0)
        },
        Size::new(10.0, 10.0),
        None,
    );
    let negative = tree.push_leaf(
        TestStyle {
            order: -1,
            ..fixed_style(10.0, 10.0)
        },
        Size::new(10.0, 10.0),
        None,
    );
    let last_absolute = tree.push_leaf(
        TestStyle {
            position: Position::Absolute,
            ..fixed_style(10.0, 10.0)
        },
        Size::new(10.0, 10.0),
        None,
    );
    let root = tree.push_linear(
        TestStyle {
            linear_orientation: LinearOrientation::Horizontal,
            ..TestStyle::default()
        },
        vec![first_absolute, positive, negative, last_absolute],
    );

    definite_layout(&mut tree, root, 100.0, 20.0);

    assert_eq!(tree.layout(negative).order, 0);
    assert_eq!(tree.layout(first_absolute).order, 1);
    assert_eq!(tree.layout(last_absolute).order, 2);
    assert_eq!(tree.layout(positive).order, 3);
}

#[test]
fn display_none_is_zeroed_while_hidden_and_collapse_stay_in_flow() {
    let mut tree = TestTree::default();
    let first = fixed_leaf(&mut tree, 10.0, 10.0);
    let hidden_descendant = fixed_leaf(&mut tree, 8.0, 8.0);
    let display_none = tree.push_linear(
        TestStyle {
            box_generation_mode: BoxGenerationMode::None,
            ..fixed_style(10.0, 10.0)
        },
        vec![hidden_descendant],
    );
    let visibility_hidden = tree.push_leaf(
        TestStyle {
            visibility: Visibility::Hidden,
            ..fixed_style(10.0, 10.0)
        },
        Size::new(10.0, 10.0),
        None,
    );
    let visibility_collapse = tree.push_leaf(
        TestStyle {
            visibility: Visibility::Collapse,
            ..fixed_style(10.0, 10.0)
        },
        Size::new(10.0, 10.0),
        None,
    );
    let last = fixed_leaf(&mut tree, 10.0, 10.0);
    let root = tree.push_linear(
        TestStyle::default(),
        vec![
            first,
            display_none,
            visibility_hidden,
            visibility_collapse,
            last,
        ],
    );

    definite_layout(&mut tree, root, 50.0, 50.0);

    assert_eq!(tree.layout(display_none), Layout::with_order(1));
    assert_eq!(tree.layout(hidden_descendant), Layout::default());
    assert_close(tree.layout(first).location.y, 0.0);
    assert_close(tree.layout(visibility_hidden).location.y, 10.0);
    assert_close(tree.layout(visibility_collapse).location.y, 20.0);
    assert_close(tree.layout(last).location.y, 30.0);
}

fn single_item_main_offset(
    gravity: LinearGravity,
    justify_content: Option<AlignContent>,
    direction: Direction,
) -> f32 {
    let mut tree = TestTree::default();
    let child = fixed_leaf(&mut tree, 20.0, 10.0);
    let root = tree.push_linear(
        TestStyle {
            direction,
            linear_orientation: LinearOrientation::Horizontal,
            linear_gravity: gravity,
            justify_content,
            ..TestStyle::default()
        },
        vec![child],
    );
    definite_layout(&mut tree, root, 100.0, 20.0);
    tree.layout(child).location.x
}

#[test]
fn main_gravity_overrides_justify_and_maps_distribution_fallbacks() {
    assert_close(
        single_item_main_offset(
            LinearGravity::End,
            Some(AlignContent::Start),
            Direction::Ltr,
        ),
        80.0,
    );
    assert_close(
        single_item_main_offset(
            LinearGravity::Center,
            Some(AlignContent::Start),
            Direction::Ltr,
        ),
        40.0,
    );
    assert_close(
        single_item_main_offset(
            LinearGravity::None,
            Some(AlignContent::FlexEnd),
            Direction::Ltr,
        ),
        80.0,
    );
    assert_close(
        single_item_main_offset(
            LinearGravity::None,
            Some(AlignContent::SpaceAround),
            Direction::Ltr,
        ),
        0.0,
    );
    assert_close(
        single_item_main_offset(LinearGravity::Right, None, Direction::Rtl),
        80.0,
    );
}

#[test]
fn space_between_distributes_only_non_negative_free_space() {
    let mut tree = TestTree::default();
    let first = fixed_leaf(&mut tree, 10.0, 10.0);
    let second = fixed_leaf(&mut tree, 20.0, 10.0);
    let root = tree.push_linear(
        TestStyle {
            linear_orientation: LinearOrientation::Horizontal,
            linear_gravity: LinearGravity::SpaceBetween,
            ..TestStyle::default()
        },
        vec![first, second],
    );
    definite_layout(&mut tree, root, 100.0, 20.0);
    assert_close(tree.layout(first).location.x, 0.0);
    assert_close(tree.layout(second).location.x, 80.0);

    definite_layout(&mut tree, root, 20.0, 20.0);
    assert_close(tree.layout(first).location.x, 0.0);
    assert_close(tree.layout(second).location.x, 10.0);
}

#[test]
fn end_and_center_preserve_negative_free_space_offsets() {
    for (gravity, expected_first, expected_second) in [
        (LinearGravity::End, -20.0, 0.0),
        (LinearGravity::Center, -10.0, 10.0),
    ] {
        let mut tree = TestTree::default();
        let first = fixed_leaf(&mut tree, 20.0, 10.0);
        let second = fixed_leaf(&mut tree, 20.0, 10.0);
        let root = tree.push_linear(
            TestStyle {
                linear_orientation: LinearOrientation::Horizontal,
                linear_gravity: gravity,
                ..TestStyle::default()
            },
            vec![first, second],
        );
        definite_layout(&mut tree, root, 20.0, 20.0);
        assert_close(tree.layout(first).location.x, expected_first);
        assert_close(tree.layout(second).location.x, expected_second);
    }
}

fn cross_case(container: TestStyle, child: TestStyle) -> Layout {
    let mut tree = TestTree::default();
    let child = tree.push_leaf(child, Size::new(20.0, 10.0), None);
    let root = tree.push_linear(container, vec![child]);
    definite_layout(&mut tree, root, 100.0, 50.0);
    tree.layout(child)
}

#[test]
fn cross_gravity_precedence_and_stretch_follow_linear_rules() {
    let item_override = cross_case(
        TestStyle {
            linear_cross_gravity: LinearCrossGravity::Center,
            align_items: Some(AlignItems::Start),
            ..TestStyle::default()
        },
        TestStyle {
            linear_layout_gravity: LinearLayoutGravity::End,
            align_self: Some(AlignItems::Start),
            ..fixed_style(20.0, 10.0)
        },
    );
    assert_close(item_override.location.x, 80.0);

    let align_self = cross_case(
        TestStyle {
            linear_cross_gravity: LinearCrossGravity::End,
            align_items: Some(AlignItems::End),
            ..TestStyle::default()
        },
        TestStyle {
            align_self: Some(AlignItems::Center),
            ..fixed_style(20.0, 10.0)
        },
    );
    assert_close(align_self.location.x, 40.0);

    let container_linear = cross_case(
        TestStyle {
            linear_cross_gravity: LinearCrossGravity::Center,
            align_items: Some(AlignItems::End),
            ..TestStyle::default()
        },
        fixed_style(20.0, 10.0),
    );
    assert_close(container_linear.location.x, 40.0);

    let align_items_stretch_is_not_mapped = cross_case(
        TestStyle {
            align_items: Some(AlignItems::Stretch),
            ..TestStyle::default()
        },
        fixed_style(20.0, 10.0),
    );
    assert_close(align_items_stretch_is_not_mapped.location.x, 0.0);
    assert_close(align_items_stretch_is_not_mapped.size.width, 20.0);

    let explicit_stretch = cross_case(
        TestStyle::default(),
        TestStyle {
            linear_layout_gravity: LinearLayoutGravity::Stretch,
            ..fixed_style(20.0, 10.0)
        },
    );
    assert_close(explicit_stretch.location.x, 0.0);
    assert_close(explicit_stretch.size.width, 100.0);
}

#[test]
fn physical_cross_gravity_stays_physical_in_vertical_rtl() {
    for (gravity, expected_x) in [
        (LinearLayoutGravity::Left, 0.0),
        (LinearLayoutGravity::Right, 80.0),
    ] {
        let layout = cross_case(
            TestStyle {
                direction: Direction::Rtl,
                ..TestStyle::default()
            },
            TestStyle {
                linear_layout_gravity: gravity,
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
            if left {
                LengthPercentageAuto::Auto
            } else {
                LengthPercentageAuto::ZERO
            },
            if right {
                LengthPercentageAuto::Auto
            } else {
                LengthPercentageAuto::ZERO
            },
            LengthPercentageAuto::ZERO,
            LengthPercentageAuto::ZERO,
        );
        let layout = cross_case(
            TestStyle {
                linear_cross_gravity: LinearCrossGravity::End,
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
            margin: Edges::uniform(LengthPercentageAuto::Auto),
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
        let mut tree = TestTree::default();
        let child = tree.push_leaf(
            TestStyle {
                margin: edges(
                    LengthPercentageAuto::ZERO,
                    LengthPercentageAuto::ZERO,
                    if top {
                        LengthPercentageAuto::Auto
                    } else {
                        LengthPercentageAuto::ZERO
                    },
                    if bottom {
                        LengthPercentageAuto::Auto
                    } else {
                        LengthPercentageAuto::ZERO
                    },
                ),
                ..fixed_style(20.0, 10.0)
            },
            Size::new(20.0, 10.0),
            None,
        );
        let root = tree.push_linear(
            TestStyle {
                linear_orientation: LinearOrientation::Horizontal,
                linear_cross_gravity: LinearCrossGravity::End,
                ..TestStyle::default()
            },
            vec![child],
        );
        definite_layout(&mut tree, root, 100.0, 50.0);
        let layout = tree.layout(child);
        assert_close(layout.location.y, expected_y);
        assert_close(layout.margin.top, expected_top);
        assert_close(layout.margin.bottom, expected_bottom);
    }
}

fn weighted_leaf(tree: &mut TestTree, weight: f32, min: Dimension, max: Dimension) -> NodeId {
    tree.push_leaf(
        TestStyle {
            size: Size::new(Dimension::Auto, px(10.0)),
            min_size: Size::new(min, Dimension::Auto),
            max_size: Size::new(max, Dimension::Auto),
            linear_weight: weight,
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
    first_min: Dimension,
    first_max: Dimension,
) -> (Layout, Layout) {
    let mut tree = TestTree::default();
    let first = weighted_leaf(&mut tree, first_weight, first_min, first_max);
    let second = weighted_leaf(&mut tree, second_weight, Dimension::Auto, Dimension::Auto);
    let root = tree.push_linear(
        TestStyle {
            linear_orientation: LinearOrientation::Horizontal,
            linear_weight_sum: weight_sum,
            ..TestStyle::default()
        },
        vec![first, second],
    );
    definite_layout(&mut tree, root, width, 20.0);
    (tree.layout(first), tree.layout(second))
}

#[test]
fn weights_split_space_and_explicit_sum_can_reserve_space() {
    let (first, second) = weighted_pair(90.0, 0.0, 1.0, 2.0, Dimension::Auto, Dimension::Auto);
    assert_close(first.size.width, 30.0);
    assert_close(second.size.width, 60.0);
    assert_close(second.location.x, 30.0);

    let (first, second) = weighted_pair(100.0, 4.0, 1.0, 1.0, Dimension::Auto, Dimension::Auto);
    assert_close(first.size.width, 25.0);
    assert_close(second.size.width, 25.0);
    assert_close(second.location.x, 25.0);

    let (first, second) = weighted_pair(100.0, 0.0, 0.25, 0.25, Dimension::Auto, Dimension::Auto);
    assert_close(first.size.width, 25.0);
    assert_close(second.size.width, 25.0);
}

#[test]
fn weighted_min_and_max_violations_freeze_and_redistribute() {
    let (first, second) = weighted_pair(100.0, 0.0, 1.0, 1.0, Dimension::Auto, px(30.0));
    assert_close(first.size.width, 30.0);
    assert_close(second.size.width, 70.0);

    let (first, second) = weighted_pair(100.0, 0.0, 1.0, 1.0, px(70.0), Dimension::Auto);
    assert_close(first.size.width, 70.0);
    assert_close(second.size.width, 30.0);

    let (first, second) = weighted_pair(
        100.0,
        0.0,
        1.0,
        1.0,
        Dimension::Percent(0.7),
        Dimension::Auto,
    );
    assert_close(first.size.width, 70.0);
    assert_close(second.size.width, 30.0);
}

#[test]
fn indefinite_main_axis_disables_weight_distribution() {
    let mut tree = TestTree::default();
    let first = tree.push_leaf(
        TestStyle {
            linear_weight: 1.0,
            ..fixed_style(15.0, 10.0)
        },
        Size::new(15.0, 10.0),
        None,
    );
    let second = tree.push_leaf(
        TestStyle {
            linear_weight: 2.0,
            ..fixed_style(25.0, 10.0)
        },
        Size::new(25.0, 10.0),
        None,
    );
    let root = tree.push_linear(
        TestStyle {
            linear_orientation: LinearOrientation::Horizontal,
            ..TestStyle::default()
        },
        vec![first, second],
    );
    let output = perform_layout(
        &mut tree,
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
            size: Size::new(Dimension::Auto, Dimension::Auto),
            aspect_ratio: Some(2.0),
            linear_weight: 1.0,
            linear_layout_gravity: LinearLayoutGravity::Start,
            ..TestStyle::default()
        },
        Size::new(10.0, 10.0),
        None,
    );
    let root = tree.push_linear(
        TestStyle {
            linear_orientation: LinearOrientation::Horizontal,
            ..TestStyle::default()
        },
        vec![child],
    );
    definite_layout(&mut tree, root, 100.0, 100.0);
    assert_size(tree.layout(child).size, Size::new(100.0, 50.0));
}

#[test]
fn container_aspect_ratio_derives_unknown_axis_from_caller_known_axis() {
    let mut tree = TestTree::default();
    let child = fixed_leaf(&mut tree, 10.0, 10.0);
    let root = tree.push_linear(
        TestStyle {
            aspect_ratio: Some(2.0),
            ..TestStyle::default()
        },
        vec![child],
    );
    let output = perform_layout(
        &mut tree,
        root,
        Size::new(Some(100.0), None),
        Size::new(AvailableSpace::Definite(100.0), AvailableSpace::MaxContent),
    );
    assert_size(output.size, Size::new(100.0, 50.0));
}

#[test]
fn padding_border_margins_and_box_sizing_use_border_box_geometry() {
    let mut tree = TestTree::default();
    let child = tree.push_leaf(
        TestStyle {
            margin: edges(
                LengthPercentageAuto::Length(3.0),
                LengthPercentageAuto::Length(7.0),
                LengthPercentageAuto::Length(4.0),
                LengthPercentageAuto::Length(6.0),
            ),
            box_sizing: BoxSizing::BorderBox,
            ..fixed_style(20.0, 10.0)
        },
        Size::new(20.0, 10.0),
        None,
    );
    let root = tree.push_linear(
        TestStyle {
            linear_orientation: LinearOrientation::Horizontal,
            padding: Edges::uniform(LengthPercentage::Length(10.0)),
            border: Edges::uniform(LengthPercentage::Length(2.0)),
            ..TestStyle::default()
        },
        vec![child],
    );
    definite_layout(&mut tree, root, 120.0, 60.0);
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

#[test]
fn fit_content_and_definite_cross_stretch_reach_leaf_measurement() {
    let mut tree = TestTree::default();
    let fit = tree.push_measured_leaf(
        TestStyle {
            size: Size::new(
                Dimension::FitContent(LengthPercentage::Length(30.0)),
                px(10.0),
            ),
            linear_layout_gravity: LinearLayoutGravity::Start,
            ..TestStyle::default()
        },
        responsive_measure,
    );
    let stretched = tree.push_measured_leaf(TestStyle::default(), responsive_measure);
    let root = tree.push_linear(TestStyle::default(), vec![fit, stretched]);
    definite_layout(&mut tree, root, 100.0, 40.0);

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
            linear_orientation: LinearOrientation::Horizontal,
            ..TestStyle::default()
        },
        vec![child],
    );

    let min_content = perform_layout(
        &mut tree,
        root,
        Size::NONE,
        Size::new(AvailableSpace::MinContent, AvailableSpace::MaxContent),
    );
    let max_content = perform_layout(
        &mut tree,
        root,
        Size::NONE,
        Size::new(AvailableSpace::MaxContent, AvailableSpace::MaxContent),
    );

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
            size: Size::new(Dimension::MinContent, px(30.0)),
            linear_layout_gravity: LinearLayoutGravity::Start,
            ..TestStyle::default()
        },
        responsive_measure,
    );
    let root = tree.push_linear(
        TestStyle {
            linear_orientation: LinearOrientation::Horizontal,
            ..TestStyle::default()
        },
        vec![child],
    );

    definite_layout(&mut tree, root, 100.0, 50.0);

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
            size: Size::new(Dimension::MaxContent, px(30.0)),
            linear_layout_gravity: LinearLayoutGravity::Start,
            ..TestStyle::default()
        },
        responsive_measure,
    );
    let root = tree.push_linear(
        TestStyle {
            linear_orientation: LinearOrientation::Horizontal,
            ..TestStyle::default()
        },
        vec![child],
    );

    definite_layout(&mut tree, root, 100.0, 50.0);

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

#[test]
fn horizontal_and_vertical_baselines_follow_linear_item_rules() {
    let mut row_tree = TestTree::default();
    let early = row_tree.push_leaf(fixed_style(10.0, 30.0), Size::new(10.0, 30.0), Some(5.0));
    let late = row_tree.push_leaf(fixed_style(10.0, 20.0), Size::new(10.0, 20.0), Some(15.0));
    let row = row_tree.push_linear(
        TestStyle {
            linear_orientation: LinearOrientation::Horizontal,
            ..TestStyle::default()
        },
        vec![early, late],
    );
    let output = definite_layout(&mut row_tree, row, 100.0, 40.0);
    assert_close(output.first_baselines.y.unwrap(), 15.0);

    let mut column_tree = TestTree::default();
    let first = column_tree.push_leaf(fixed_style(10.0, 20.0), Size::new(10.0, 20.0), Some(5.0));
    let second = fixed_leaf(&mut column_tree, 10.0, 10.0);
    let column = column_tree.push_linear(
        TestStyle {
            linear_gravity: LinearGravity::Center,
            ..TestStyle::default()
        },
        vec![first, second],
    );
    let output = definite_layout(&mut column_tree, column, 20.0, 100.0);
    assert_close(output.first_baselines.y.unwrap(), 40.0);

    let empty = column_tree.push_linear(TestStyle::default(), Vec::new());
    let output = definite_layout(&mut column_tree, empty, 20.0, 20.0);
    assert_eq!(output.first_baselines.y, None);
}

#[test]
fn committed_child_without_a_baseline_does_not_export_its_probe_baseline() {
    let mut tree = TestTree::default();
    let child = tree.push_measured_leaf(TestStyle::default(), probe_only_baseline);
    let root = tree.push_linear(TestStyle::default(), vec![child]);

    let output = definite_layout(&mut tree, root, 20.0, 20.0);

    assert_eq!(output.first_baselines.y, None);
}

#[test]
fn nested_linear_containers_round_trip_through_static_host_dispatch() {
    let mut tree = TestTree::default();
    let first = fixed_leaf(&mut tree, 10.0, 10.0);
    let second = fixed_leaf(&mut tree, 20.0, 10.0);
    let inner = tree.push_linear(
        TestStyle {
            size: Size::new(px(100.0), px(20.0)),
            linear_orientation: LinearOrientation::Horizontal,
            linear_gravity: LinearGravity::End,
            ..TestStyle::default()
        },
        vec![first, second],
    );
    let outer = tree.push_linear(TestStyle::default(), vec![inner]);
    definite_layout(&mut tree, outer, 100.0, 100.0);
    assert_close(tree.layout(inner).location.y, 0.0);
    assert_close(tree.layout(first).location.x, 70.0);
    assert_close(tree.layout(second).location.x, 80.0);
}

#[test]
fn grid_and_linear_share_static_host_dispatch_in_both_directions() {
    let mut linear_root_tree = TestTree::default();
    let grid_leaf = fixed_leaf(&mut linear_root_tree, 12.0, 8.0);
    let grid = linear_root_tree.push_grid(fixed_style(30.0, 20.0), vec![grid_leaf]);
    let linear_root = linear_root_tree.push_linear(
        TestStyle {
            linear_orientation: LinearOrientation::Horizontal,
            ..TestStyle::default()
        },
        vec![grid],
    );
    definite_layout(&mut linear_root_tree, linear_root, 100.0, 40.0);
    assert_size(linear_root_tree.layout(grid).size, Size::new(30.0, 20.0));
    assert_size(
        linear_root_tree.layout(grid_leaf).size,
        Size::new(12.0, 8.0),
    );

    let mut grid_root_tree = TestTree::default();
    let linear_leaf = fixed_leaf(&mut grid_root_tree, 10.0, 6.0);
    let linear = grid_root_tree.push_linear(
        TestStyle {
            size: Size::new(px(30.0), px(20.0)),
            linear_orientation: LinearOrientation::Horizontal,
            linear_gravity: LinearGravity::End,
            ..TestStyle::default()
        },
        vec![linear_leaf],
    );
    let grid_root = grid_root_tree.push_grid(fixed_style(100.0, 40.0), vec![linear]);
    definite_layout(&mut grid_root_tree, grid_root, 100.0, 40.0);
    assert_size(grid_root_tree.layout(linear).size, Size::new(30.0, 20.0));
    assert_point(
        grid_root_tree.layout(linear_leaf).location,
        Point::new(20.0, 0.0),
    );
}

#[test]
fn flex_and_linear_share_static_host_dispatch_in_both_directions() {
    let mut linear_root_tree = TestTree::default();
    let flex_leaf = fixed_leaf(&mut linear_root_tree, 12.0, 8.0);
    let flex = linear_root_tree.push_flex(fixed_style(30.0, 20.0), vec![flex_leaf]);
    let linear_root = linear_root_tree.push_linear(
        TestStyle {
            linear_orientation: LinearOrientation::Horizontal,
            ..TestStyle::default()
        },
        vec![flex],
    );
    definite_layout(&mut linear_root_tree, linear_root, 100.0, 40.0);
    assert_size(linear_root_tree.layout(flex).size, Size::new(30.0, 20.0));
    assert_size(
        linear_root_tree.layout(flex_leaf).size,
        Size::new(12.0, 8.0),
    );

    let mut flex_root_tree = TestTree::default();
    let linear_leaf = fixed_leaf(&mut flex_root_tree, 10.0, 6.0);
    let linear = flex_root_tree.push_linear(
        TestStyle {
            size: Size::new(px(30.0), px(20.0)),
            linear_orientation: LinearOrientation::Horizontal,
            linear_gravity: LinearGravity::End,
            ..TestStyle::default()
        },
        vec![linear_leaf],
    );
    let flex_root = flex_root_tree.push_flex(fixed_style(100.0, 40.0), vec![linear]);
    definite_layout(&mut flex_root_tree, flex_root, 100.0, 40.0);
    assert_size(flex_root_tree.layout(linear).size, Size::new(30.0, 20.0));
    assert_point(
        flex_root_tree.layout(linear_leaf).location,
        Point::new(20.0, 0.0),
    );
}

#[test]
fn flex_max_content_target_enables_linear_weight_distribution() {
    let mut tree = TestTree::default();
    let narrow = tree.push_leaf(
        TestStyle {
            linear_weight: 1.0,
            ..fixed_style(10.0, 10.0)
        },
        Size::new(10.0, 10.0),
        None,
    );
    let wide = tree.push_leaf(
        TestStyle {
            linear_weight: 1.0,
            ..fixed_style(90.0, 10.0)
        },
        Size::new(90.0, 10.0),
        None,
    );
    let linear = tree.push_linear(
        TestStyle {
            linear_orientation: LinearOrientation::Horizontal,
            ..TestStyle::default()
        },
        vec![narrow, wide],
    );
    let flex = tree.push_flex(TestStyle::default(), vec![linear]);

    let output = perform_layout(
        &mut tree,
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
            size: Size::new(Dimension::Auto, px(10.0)),
            ..TestStyle::default()
        },
        Size::new(10.0, 10.0),
        None,
    );
    let wide = tree.push_leaf(
        TestStyle {
            size: Size::new(Dimension::Auto, px(10.0)),
            ..TestStyle::default()
        },
        Size::new(90.0, 10.0),
        None,
    );
    let linear = tree.push_linear(TestStyle::default(), vec![narrow, wide]);
    let flex = tree.push_flex(TestStyle::default(), vec![linear]);

    let output = perform_layout(
        &mut tree,
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
            size: Size::new(Dimension::Percent(0.5), px(10.0)),
            ..TestStyle::default()
        },
        Size::new(80.0, 10.0),
        None,
    );
    let fixed = fixed_leaf(&mut tree, 20.0, 10.0);
    let linear = tree.push_linear(
        TestStyle {
            linear_orientation: LinearOrientation::Horizontal,
            ..TestStyle::default()
        },
        vec![percentage, fixed],
    );
    let flex = tree.push_flex(TestStyle::default(), vec![linear]);

    let output = perform_layout(
        &mut tree,
        flex,
        Size::new(None, Some(20.0)),
        Size::new(AvailableSpace::MaxContent, AvailableSpace::Definite(20.0)),
    );

    // Flex decides a 100px target geometry but §9.8 keeps it indefinite for
    // descendant percentages. Linear therefore preserves the child's 80px
    // intrinsic measurement instead of initially resolving width:50% to 50px.
    assert_close(output.size.width, 100.0);
    assert_close(tree.layout(linear).size.width, 100.0);
    assert_close(tree.layout(percentage).size.width, 80.0);
    assert_close(tree.layout(fixed).location.x, 80.0);
}

#[test]
fn relative_insets_move_visual_box_without_advancing_following_flow() {
    let mut tree = TestTree::default();
    let shifted = tree.push_leaf(
        TestStyle {
            inset: edges(
                LengthPercentageAuto::Length(5.0),
                LengthPercentageAuto::Auto,
                LengthPercentageAuto::Length(3.0),
                LengthPercentageAuto::Auto,
            ),
            ..fixed_style(10.0, 10.0)
        },
        Size::new(10.0, 10.0),
        None,
    );
    let following = fixed_leaf(&mut tree, 10.0, 10.0);
    let root = tree.push_linear(
        TestStyle {
            linear_orientation: LinearOrientation::Horizontal,
            ..TestStyle::default()
        },
        vec![shifted, following],
    );
    definite_layout(&mut tree, root, 100.0, 20.0);
    assert_point(tree.layout(shifted).location, Point::new(5.0, 3.0));
    assert_point(tree.layout(following).location, Point::new(10.0, 0.0));
}

#[test]
fn absolute_children_use_linear_static_alignment_and_insets_override_it() {
    let mut tree = TestTree::default();
    let centered = tree.push_leaf(
        TestStyle {
            position: Position::Absolute,
            linear_layout_gravity: LinearLayoutGravity::Center,
            ..fixed_style(10.0, 8.0)
        },
        Size::new(10.0, 8.0),
        None,
    );
    let inset = tree.push_leaf(
        TestStyle {
            position: Position::Absolute,
            inset: edges(
                LengthPercentageAuto::Length(5.0),
                LengthPercentageAuto::Auto,
                LengthPercentageAuto::Length(7.0),
                LengthPercentageAuto::Auto,
            ),
            ..fixed_style(10.0, 8.0)
        },
        Size::new(10.0, 8.0),
        None,
    );
    let root = tree.push_linear(
        TestStyle {
            linear_orientation: LinearOrientation::Horizontal,
            linear_gravity: LinearGravity::Center,
            ..TestStyle::default()
        },
        vec![centered, inset],
    );
    definite_layout(&mut tree, root, 100.0, 50.0);
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
    let child = tree.push_leaf(
        TestStyle {
            position: Position::Absolute,
            linear_layout_gravity: LinearLayoutGravity::Center,
            ..fixed_style(10.0, 10.0)
        },
        Size::new(10.0, 10.0),
        None,
    );
    let root = tree.push_linear(
        TestStyle {
            linear_orientation: LinearOrientation::Horizontal,
            linear_gravity: LinearGravity::Center,
            padding: edges(
                LengthPercentage::Length(10.0),
                LengthPercentage::Length(20.0),
                LengthPercentage::Length(5.0),
                LengthPercentage::Length(15.0),
            ),
            border: Edges::uniform(LengthPercentage::Length(2.0)),
            ..TestStyle::default()
        },
        vec![child],
    );
    definite_layout(&mut tree, root, 100.0, 60.0);
    assert_point(tree.layout(child).location, Point::new(45.0, 25.0));
}

#[test]
fn absolute_static_alignment_uses_common_inset_and_aspect_ratio_sizing() {
    let mut tree = TestTree::default();
    let child = tree.push_leaf(
        TestStyle {
            position: Position::Absolute,
            inset: edges(
                LengthPercentageAuto::Length(10.0),
                LengthPercentageAuto::Length(10.0),
                LengthPercentageAuto::Auto,
                LengthPercentageAuto::Auto,
            ),
            aspect_ratio: Some(2.0),
            linear_layout_gravity: LinearLayoutGravity::Center,
            ..TestStyle::default()
        },
        Size::new(12.0, 12.0),
        None,
    );
    let root = tree.push_linear(
        TestStyle {
            linear_orientation: LinearOrientation::Horizontal,
            ..TestStyle::default()
        },
        vec![child],
    );
    definite_layout(&mut tree, root, 100.0, 100.0);
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
    let child = tree.push_leaf(
        TestStyle {
            position: Position::Absolute,
            margin: edges(
                LengthPercentageAuto::Length(3.0),
                LengthPercentageAuto::Length(7.0),
                LengthPercentageAuto::Length(2.0),
                LengthPercentageAuto::Length(4.0),
            ),
            linear_layout_gravity: LinearLayoutGravity::End,
            ..fixed_style(10.0, 8.0)
        },
        Size::new(10.0, 8.0),
        None,
    );
    let root = tree.push_linear(
        TestStyle {
            linear_orientation: LinearOrientation::HorizontalReverse,
            linear_gravity: LinearGravity::End,
            ..TestStyle::default()
        },
        vec![child],
    );

    definite_layout(&mut tree, root, 100.0, 50.0);

    assert_point(tree.layout(child).location, Point::new(3.0, 38.0));
}

#[test]
fn hoisted_children_record_static_position_without_local_commit() {
    let mut tree = TestTree::default();
    let hoisted = tree.push_measured_leaf(
        TestStyle {
            position: Position::AbsoluteHoisted,
            linear_layout_gravity: LinearLayoutGravity::End,
            ..fixed_style(20.0, 10.0)
        },
        responsive_measure,
    );
    let root = tree.push_linear(
        TestStyle {
            linear_orientation: LinearOrientation::Horizontal,
            linear_gravity: LinearGravity::Center,
            ..TestStyle::default()
        },
        vec![hoisted],
    );
    definite_layout(&mut tree, root, 100.0, 50.0);

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
            position: Position::AbsoluteHoisted,
            inset: edges(
                LengthPercentageAuto::Length(10.0),
                LengthPercentageAuto::Length(10.0),
                LengthPercentageAuto::Auto,
                LengthPercentageAuto::Auto,
            ),
            linear_layout_gravity: LinearLayoutGravity::End,
            size: Size::new(px(20.0), Dimension::Auto),
            ..TestStyle::default()
        },
        responsive_measure,
    );
    let root = tree.push_linear(
        TestStyle {
            linear_orientation: LinearOrientation::Horizontal,
            linear_gravity: LinearGravity::Center,
            ..TestStyle::default()
        },
        vec![hoisted],
    );

    definite_layout(&mut tree, root, 100.0, 50.0);

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
            position: Position::AbsoluteHoisted,
            ..fixed_style(10.0, 10.0)
        },
        responsive_measure,
    );
    let root = tree.push_linear(TestStyle::default(), vec![measured, hoisted]);
    tree.session.enable_cache();

    let output = measure_layout(
        &mut tree,
        root,
        Size::new(Some(100.0), None),
        Size::new(AvailableSpace::Definite(100.0), AvailableSpace::MaxContent),
    );
    assert!(output.size.width.is_finite() && output.size.height.is_finite());
    assert_eq!(tree.session.layout_writes, 0);
    assert_eq!(tree.session.static_position_writes, 0);
    assert!(
        tree.measure_inputs(measured)
            .iter()
            .all(|input| matches!(input.goal, LayoutGoal::Measure(_)))
    );

    let measured_calls = tree.measure_inputs(measured).len();
    let cached = measure_layout(
        &mut tree,
        root,
        Size::new(Some(100.0), None),
        Size::new(AvailableSpace::Definite(100.0), AvailableSpace::MaxContent),
    );
    assert_eq!(cached, output);
    assert_eq!(tree.measure_inputs(measured).len(), measured_calls);

    let resized = measure_layout(
        &mut tree,
        root,
        Size::new(Some(80.0), None),
        Size::new(AvailableSpace::Definite(80.0), AvailableSpace::MaxContent),
    );
    assert_close(resized.size.width, 80.0);
    assert!(tree.measure_inputs(measured).len() > measured_calls);
    let before_commit = tree.measure_inputs(measured).len();

    perform_layout(
        &mut tree,
        root,
        Size::new(Some(80.0), None),
        Size::new(AvailableSpace::Definite(80.0), AvailableSpace::MaxContent),
    );
    assert!(tree.session.layout_writes > 0);
    assert!(tree.session.static_position_writes > 0);
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
            linear_orientation: LinearOrientation::Horizontal,
            min_size: Size::new(px(50.0), Dimension::Auto),
            max_size: Size::new(Dimension::Auto, px(30.0)),
            ..TestStyle::default()
        },
        vec![first, second],
    );
    let output = perform_layout(
        &mut tree,
        root,
        Size::NONE,
        Size::new(AvailableSpace::MaxContent, AvailableSpace::MaxContent),
    );
    assert_size(output.size, Size::new(50.0, 30.0));
}

#[test]
fn calc_item_size_and_percent_min_max_resolve_against_container_content_box() {
    let mut tree = TestTree::default();
    let width = tree.push_calc(5.0, 0.5);
    let child = tree.push_leaf(
        TestStyle {
            size: Size::new(Dimension::Calc(width), px(10.0)),
            min_size: Size::new(Dimension::Percent(0.4), Dimension::Auto),
            max_size: Size::new(Dimension::Percent(0.6), Dimension::Auto),
            ..TestStyle::default()
        },
        Size::new(1.0, 10.0),
        None,
    );
    let root = tree.push_linear(
        TestStyle {
            padding: edges(
                LengthPercentage::Length(10.0),
                LengthPercentage::Length(10.0),
                LengthPercentage::ZERO,
                LengthPercentage::ZERO,
            ),
            border: edges(
                LengthPercentage::Length(2.0),
                LengthPercentage::Length(2.0),
                LengthPercentage::ZERO,
                LengthPercentage::ZERO,
            ),
            ..TestStyle::default()
        },
        vec![child],
    );
    definite_layout(&mut tree, root, 124.0, 40.0);
    // The content box is 100px wide, so calc(5px + 50%) resolves to 55px.
    assert_close(tree.layout(child).size.width, 55.0);
    assert_close(tree.layout(child).location.x, 12.0);
}

#[test]
fn intrinsic_inline_percentage_edges_resolve_without_changing_their_basis() {
    let mut tree = TestTree::default();
    let child = tree.push_leaf(
        TestStyle {
            margin: edges(
                LengthPercentageAuto::Percent(0.5),
                LengthPercentageAuto::ZERO,
                LengthPercentageAuto::ZERO,
                LengthPercentageAuto::ZERO,
            ),
            ..fixed_style(20.0, 10.0)
        },
        Size::new(20.0, 10.0),
        None,
    );
    let root = tree.push_linear(TestStyle::default(), vec![child]);
    let output = perform_layout(
        &mut tree,
        root,
        Size::NONE,
        Size::new(AvailableSpace::MaxContent, AvailableSpace::MaxContent),
    );
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
            size: Size::new(Dimension::Percent(0.5), px(10.0)),
            margin: edges(
                LengthPercentageAuto::Percent(0.5),
                LengthPercentageAuto::ZERO,
                LengthPercentageAuto::ZERO,
                LengthPercentageAuto::ZERO,
            ),
            ..TestStyle::default()
        },
        Size::new(80.0, 10.0),
        None,
    );
    let independent = fixed_leaf(&mut tree, 20.0, 10.0);
    let root = tree.push_linear(
        TestStyle {
            linear_orientation: LinearOrientation::Horizontal,
            // End alignment also locks in Starlight's original intrinsic main
            // total: feeding the refreshed 50px margin back into that total
            // would shift both children 50px toward main-start.
            linear_gravity: LinearGravity::End,
            ..TestStyle::default()
        },
        vec![dependent, independent],
    );

    let output = perform_layout(
        &mut tree,
        root,
        Size::NONE,
        Size::new(AvailableSpace::MaxContent, AvailableSpace::MaxContent),
    );

    // The intrinsic pass measures 80px + 20px. Once the 100px container is
    // known, Starlight refreshes the 50% margin but does not issue a second
    // sizing probe for the percentage-sized child at 50px.
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
        1
    );
}

#[test]
fn intrinsic_percentage_box_refresh_precedes_container_min_clamp() {
    let mut tree = TestTree::default();
    let child = tree.push_leaf(
        TestStyle {
            margin: edges(
                LengthPercentageAuto::Percent(0.5),
                LengthPercentageAuto::ZERO,
                LengthPercentageAuto::ZERO,
                LengthPercentageAuto::ZERO,
            ),
            inset: edges(
                LengthPercentageAuto::Auto,
                LengthPercentageAuto::Auto,
                LengthPercentageAuto::Percent(0.5),
                LengthPercentageAuto::Auto,
            ),
            ..fixed_style(20.0, 10.0)
        },
        Size::new(20.0, 10.0),
        None,
    );
    let root = tree.push_linear(
        TestStyle {
            min_size: Size::new(px(100.0), px(100.0)),
            ..TestStyle::default()
        },
        vec![child],
    );

    let output = perform_layout(
        &mut tree,
        root,
        Size::NONE,
        Size::new(AvailableSpace::MaxContent, AvailableSpace::MaxContent),
    );

    // Starlight resolves the 50% margin against the provisional 20px
    // intrinsic width, then applies the container's 100px min clamps. Relative
    // positioning happens later and therefore resolves top:50% against the
    // final 100px containing-block height.
    assert_size(output.size, Size::new(100.0, 100.0));
    assert_close(tree.layout(child).margin.left, 10.0);
    assert_point(tree.layout(child).location, Point::new(10.0, 50.0));
}
