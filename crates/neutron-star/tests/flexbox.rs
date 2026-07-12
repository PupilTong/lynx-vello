//! Spec-focused flexbox integration tests over a plain `Vec`-backed host.
//!
//! The host deliberately has no styling engine. `TestStyle` is already the
//! computed style view, and leaf measurement is a deterministic intrinsic
//! size stored alongside each node.

mod support;

use neutron_star::compute::{
    FnLeafMeasurer, LeafMetrics, compute_absolute_layout, compute_leaf_layout,
};
use neutron_star::prelude::*;
use neutron_star::style::{
    AlignContent, AlignItems, AlignSelf, BoxGenerationMode, BoxSizing, Dimension, Direction,
    FlexDirection, FlexWrap, JustifyContent, LengthPercentage, LengthPercentageAuto, Overflow,
    Position,
};
use support::*;

#[test]
fn flex_grow_distributes_free_space_proportionally() {
    let mut tree = TestTree::default();
    let mut first_style = fixed_leaf_style(50.0, 20.0);
    first_style.flex_grow = 1.0;
    let first = tree.push_leaf(first_style, Size::new(50.0, 20.0), None);
    let mut second_style = fixed_leaf_style(50.0, 20.0);
    second_style.flex_grow = 2.0;
    let second = tree.push_leaf(second_style, Size::new(50.0, 20.0), None);
    let root = flex_container(&mut tree, TestStyle::default(), &[first, second]);

    definite_layout(&mut tree, root, 300.0, 20.0);

    assert_close(tree.layout(first).size.width, 350.0 / 3.0);
    assert_close(tree.layout(second).size.width, 550.0 / 3.0);
    assert_close(tree.layout(second).location.x, 350.0 / 3.0);
}

#[test]
fn flex_grow_sum_below_one_leaves_part_of_the_free_space() {
    let mut tree = TestTree::default();
    let mut first_style = fixed_leaf_style(50.0, 20.0);
    first_style.flex_grow = 0.2;
    let first = tree.push_leaf(first_style, Size::new(50.0, 20.0), None);
    let mut second_style = fixed_leaf_style(50.0, 20.0);
    second_style.flex_grow = 0.2;
    let second = tree.push_leaf(second_style, Size::new(50.0, 20.0), None);
    let root = flex_container(&mut tree, TestStyle::default(), &[first, second]);

    definite_layout(&mut tree, root, 300.0, 20.0);

    assert_close(tree.layout(first).size.width, 90.0);
    assert_close(tree.layout(second).size.width, 90.0);
    assert_close(tree.layout(second).location.x, 90.0);
}

#[test]
fn flex_shrink_uses_scaled_flex_shrink_factors() {
    let mut tree = TestTree::default();
    let mut first_style = fixed_leaf_style(100.0, 20.0);
    first_style.min_size.width = Dimension::ZERO;
    let first = tree.push_leaf(first_style, Size::new(100.0, 20.0), None);
    let mut second_style = fixed_leaf_style(200.0, 20.0);
    second_style.min_size.width = Dimension::ZERO;
    let second = tree.push_leaf(second_style, Size::new(200.0, 20.0), None);
    let root = flex_container(&mut tree, TestStyle::default(), &[first, second]);

    definite_layout(&mut tree, root, 180.0, 20.0);

    assert_close(tree.layout(first).size.width, 60.0);
    assert_close(tree.layout(second).size.width, 120.0);
    assert_close(tree.layout(second).location.x, 60.0);
}

#[test]
fn min_and_max_constraints_refreeze_flexible_items() {
    let mut grow_tree = TestTree::default();
    let mut capped_style = fixed_leaf_style(100.0, 20.0);
    capped_style.flex_grow = 1.0;
    capped_style.max_size.width = Dimension::Length(120.0);
    let capped = grow_tree.push_leaf(capped_style, Size::new(100.0, 20.0), None);
    let mut growing_style = fixed_leaf_style(100.0, 20.0);
    growing_style.flex_grow = 1.0;
    let growing = grow_tree.push_leaf(growing_style, Size::new(100.0, 20.0), None);
    let grow_root = flex_container(&mut grow_tree, TestStyle::default(), &[capped, growing]);

    definite_layout(&mut grow_tree, grow_root, 300.0, 20.0);
    assert_close(grow_tree.layout(capped).size.width, 120.0);
    assert_close(grow_tree.layout(growing).size.width, 180.0);

    let mut shrink_tree = TestTree::default();
    let mut floored_style = fixed_leaf_style(100.0, 20.0);
    floored_style.min_size.width = Dimension::Length(90.0);
    let floored = shrink_tree.push_leaf(floored_style, Size::new(100.0, 20.0), None);
    let mut shrinking_style = fixed_leaf_style(100.0, 20.0);
    shrinking_style.min_size.width = Dimension::ZERO;
    let shrinking = shrink_tree.push_leaf(shrinking_style, Size::new(100.0, 20.0), None);
    let shrink_root = flex_container(
        &mut shrink_tree,
        TestStyle::default(),
        &[floored, shrinking],
    );

    definite_layout(&mut shrink_tree, shrink_root, 160.0, 20.0);
    assert_close(shrink_tree.layout(floored).size.width, 90.0);
    assert_close(shrink_tree.layout(shrinking).size.width, 70.0);
}

#[test]
fn wrapping_accounts_for_column_and_row_gaps() {
    let mut tree = TestTree::default();
    let first = fixed_leaf(&mut tree, 100.0, 20.0);
    let second = fixed_leaf(&mut tree, 100.0, 20.0);
    let third = fixed_leaf(&mut tree, 100.0, 20.0);
    let container_style = TestStyle {
        flex_wrap: FlexWrap::Wrap,
        gap: Size::new(
            LengthPercentage::length(10.0),
            LengthPercentage::length(5.0),
        ),
        ..TestStyle::default()
    };
    let root = flex_container(&mut tree, container_style, &[first, second, third]);

    let output = perform_layout(
        &mut tree,
        root,
        Size::new(Some(210.0), None),
        Size::new(AvailableSpace::Definite(210.0), AvailableSpace::MaxContent),
    );

    assert_size(output.size, Size::new(210.0, 45.0));
    assert_point(tree.layout(first).location, Point::new(0.0, 0.0));
    assert_point(tree.layout(second).location, Point::new(110.0, 0.0));
    assert_point(tree.layout(third).location, Point::new(0.0, 25.0));
}

fn direction_fixture(
    flex_direction: FlexDirection,
    direction: Direction,
) -> (TestTree, NodeId, NodeId, NodeId) {
    let mut tree = TestTree::default();
    let first = fixed_leaf(&mut tree, 20.0, 20.0);
    let second = fixed_leaf(&mut tree, 30.0, 30.0);
    let root = flex_container(
        &mut tree,
        TestStyle {
            flex_direction,
            direction,
            align_items: Some(AlignItems::FlexStart),
            ..TestStyle::default()
        },
        &[first, second],
    );
    (tree, root, first, second)
}

#[test]
fn row_column_reverse_and_rtl_resolve_physical_main_axes() {
    let cases = [
        (
            FlexDirection::Row,
            Direction::Ltr,
            Point::new(0.0, 0.0),
            Point::new(20.0, 0.0),
        ),
        (
            FlexDirection::RowReverse,
            Direction::Ltr,
            Point::new(80.0, 0.0),
            Point::new(50.0, 0.0),
        ),
        (
            FlexDirection::Row,
            Direction::Rtl,
            Point::new(80.0, 0.0),
            Point::new(50.0, 0.0),
        ),
        (
            FlexDirection::RowReverse,
            Direction::Rtl,
            Point::new(0.0, 0.0),
            Point::new(20.0, 0.0),
        ),
        (
            FlexDirection::Column,
            Direction::Ltr,
            Point::new(0.0, 0.0),
            Point::new(0.0, 20.0),
        ),
        (
            FlexDirection::ColumnReverse,
            Direction::Ltr,
            Point::new(0.0, 80.0),
            Point::new(0.0, 50.0),
        ),
    ];

    for (flex_direction, direction, expected_first, expected_second) in cases {
        let (mut tree, root, first, second) = direction_fixture(flex_direction, direction);
        definite_layout(&mut tree, root, 100.0, 100.0);
        assert_point(tree.layout(first).location, expected_first);
        assert_point(tree.layout(second).location, expected_second);
    }
}

#[test]
fn order_is_stable_and_layout_order_is_the_sorted_index() {
    let mut tree = TestTree::default();
    let mut style_a = fixed_leaf_style(10.0, 10.0);
    style_a.order = 1;
    let a = tree.push_leaf(style_a, Size::new(10.0, 10.0), None);
    let mut style_b = fixed_leaf_style(10.0, 10.0);
    style_b.order = 0;
    let b = tree.push_leaf(style_b, Size::new(10.0, 10.0), None);
    let mut style_c = fixed_leaf_style(10.0, 10.0);
    style_c.order = 0;
    let c = tree.push_leaf(style_c, Size::new(10.0, 10.0), None);
    let mut style_d = fixed_leaf_style(10.0, 10.0);
    style_d.order = 1;
    let d = tree.push_leaf(style_d, Size::new(10.0, 10.0), None);
    let root = flex_container(&mut tree, TestStyle::default(), &[a, b, c, d]);

    definite_layout(&mut tree, root, 100.0, 20.0);

    for (node, expected_x, expected_order) in
        [(b, 0.0, 0), (c, 10.0, 1), (a, 20.0, 2), (d, 30.0, 3)]
    {
        assert_close(tree.layout(node).location.x, expected_x);
        assert_eq!(tree.layout(node).order, expected_order);
    }
}

#[test]
fn justify_content_and_main_axis_auto_margin_distribute_space() {
    let mut justify_tree = TestTree::default();
    let first = fixed_leaf(&mut justify_tree, 20.0, 10.0);
    let second = fixed_leaf(&mut justify_tree, 20.0, 10.0);
    let root = flex_container(
        &mut justify_tree,
        TestStyle {
            justify_content: Some(JustifyContent::SpaceBetween),
            ..TestStyle::default()
        },
        &[first, second],
    );

    definite_layout(&mut justify_tree, root, 100.0, 20.0);
    assert_close(justify_tree.layout(first).location.x, 0.0);
    assert_close(justify_tree.layout(second).location.x, 80.0);

    let mut margin_tree = TestTree::default();
    let mut auto_margin_style = fixed_leaf_style(20.0, 10.0);
    auto_margin_style.margin.left = LengthPercentageAuto::Auto;
    let auto_margin = margin_tree.push_leaf(auto_margin_style, Size::new(20.0, 10.0), None);
    let trailing = fixed_leaf(&mut margin_tree, 20.0, 10.0);
    let root = flex_container(
        &mut margin_tree,
        TestStyle {
            justify_content: Some(JustifyContent::Center),
            ..TestStyle::default()
        },
        &[auto_margin, trailing],
    );

    definite_layout(&mut margin_tree, root, 100.0, 20.0);
    assert_close(margin_tree.layout(auto_margin).margin.left, 60.0);
    assert_close(margin_tree.layout(auto_margin).location.x, 60.0);
    assert_close(margin_tree.layout(trailing).location.x, 80.0);
}

#[test]
fn cross_axis_alignment_auto_margins_and_stretch_are_applied() {
    let mut align_tree = TestTree::default();
    let centered = fixed_leaf(&mut align_tree, 20.0, 20.0);
    let mut end_style = fixed_leaf_style(20.0, 20.0);
    end_style.align_self = Some(AlignSelf::FlexEnd);
    let ended = align_tree.push_leaf(end_style, Size::new(20.0, 20.0), None);
    let root = flex_container(
        &mut align_tree,
        TestStyle {
            align_items: Some(AlignItems::Center),
            ..TestStyle::default()
        },
        &[centered, ended],
    );

    definite_layout(&mut align_tree, root, 100.0, 60.0);
    assert_close(align_tree.layout(centered).location.y, 20.0);
    assert_close(align_tree.layout(ended).location.y, 40.0);

    let mut stretch_tree = TestTree::default();
    let stretched = stretch_tree.push_leaf(TestStyle::default(), Size::new(20.0, 10.0), None);
    let root = flex_container(&mut stretch_tree, TestStyle::default(), &[stretched]);
    definite_layout(&mut stretch_tree, root, 100.0, 60.0);
    assert_close(stretch_tree.layout(stretched).size.height, 60.0);

    let mut margin_tree = TestTree::default();
    let mut auto_margin_style = fixed_leaf_style(20.0, 20.0);
    auto_margin_style.margin.top = LengthPercentageAuto::Auto;
    let auto_margin = margin_tree.push_leaf(auto_margin_style, Size::new(20.0, 20.0), None);
    let root = flex_container(
        &mut margin_tree,
        TestStyle {
            align_items: Some(AlignItems::Center),
            ..TestStyle::default()
        },
        &[auto_margin],
    );

    definite_layout(&mut margin_tree, root, 100.0, 60.0);
    assert_close(margin_tree.layout(auto_margin).margin.top, 40.0);
    assert_close(margin_tree.layout(auto_margin).location.y, 40.0);
}

#[test]
fn align_content_positions_multiple_lines_in_the_cross_axis() {
    let mut tree = TestTree::default();
    let first = fixed_leaf(&mut tree, 60.0, 10.0);
    let second = fixed_leaf(&mut tree, 60.0, 10.0);
    let root = flex_container(
        &mut tree,
        TestStyle {
            flex_wrap: FlexWrap::Wrap,
            gap: Size::new(LengthPercentage::ZERO, LengthPercentage::length(10.0)),
            align_content: Some(AlignContent::Center),
            align_items: Some(AlignItems::FlexStart),
            ..TestStyle::default()
        },
        &[first, second],
    );

    definite_layout(&mut tree, root, 100.0, 60.0);

    assert_close(tree.layout(first).location.y, 15.0);
    assert_close(tree.layout(second).location.y, 35.0);
}

#[test]
fn baseline_alignment_uses_child_first_baselines() {
    let mut tree = TestTree::default();
    let first = tree.push_leaf(
        fixed_leaf_style(20.0, 20.0),
        Size::new(20.0, 20.0),
        Some(15.0),
    );
    let second = tree.push_leaf(
        fixed_leaf_style(20.0, 30.0),
        Size::new(20.0, 30.0),
        Some(10.0),
    );
    let root = flex_container(
        &mut tree,
        TestStyle {
            align_items: Some(AlignItems::Baseline),
            ..TestStyle::default()
        },
        &[first, second],
    );

    let output = definite_layout(&mut tree, root, 100.0, 40.0);

    assert_close(tree.layout(first).location.y + 15.0, 15.0);
    assert_close(tree.layout(second).location.y + 10.0, 15.0);
    assert_eq!(output.first_baselines.y, Some(15.0));
}

#[test]
fn hidden_and_out_of_flow_children_do_not_participate_in_flexing() {
    let mut tree = TestTree::default();
    let first = fixed_leaf(&mut tree, 20.0, 10.0);

    let hidden = tree.push_leaf(
        TestStyle {
            box_generation_mode: BoxGenerationMode::None,
            ..fixed_leaf_style(1_000.0, 10.0)
        },
        Size::new(1_000.0, 10.0),
        None,
    );
    let absolute = tree.push_leaf(
        TestStyle {
            position: Position::Absolute,
            ..fixed_leaf_style(1_000.0, 10.0)
        },
        Size::new(1_000.0, 10.0),
        None,
    );
    let hoisted = tree.push_leaf(
        TestStyle {
            position: Position::AbsoluteHoisted,
            ..fixed_leaf_style(1_000.0, 10.0)
        },
        Size::new(1_000.0, 10.0),
        None,
    );
    let second = fixed_leaf(&mut tree, 20.0, 10.0);
    let root = flex_container(
        &mut tree,
        TestStyle {
            justify_content: Some(JustifyContent::SpaceBetween),
            ..TestStyle::default()
        },
        &[first, hidden, absolute, hoisted, second],
    );

    definite_layout(&mut tree, root, 100.0, 20.0);

    assert_close(tree.layout(first).location.x, 0.0);
    assert_close(tree.layout(second).location.x, 80.0);
    assert!(tree.static_position(hoisted).is_some());
}

#[test]
fn measure_goal_does_not_write_durable_layouts() {
    let mut tree = TestTree::default();
    let first = fixed_leaf(&mut tree, 40.0, 10.0);
    let second = fixed_leaf(&mut tree, 40.0, 20.0);
    let root = flex_container(&mut tree, TestStyle::default(), &[first, second]);
    let mut sentinel = Layout::default();
    sentinel.location = Point::new(123.0, 456.0);
    sentinel.size = Size::new(7.0, 8.0);
    tree.session_node_mut(first).layout = sentinel;
    tree.session_node_mut(second).layout = sentinel;
    tree.session_node_mut(root).layout = sentinel;

    let output = tree.session.compute_child_layout(
        &tree.source,
        root,
        LayoutInput::compute_size(
            Size::new(Some(100.0), None),
            Size::new(Some(100.0), None),
            Size::new(AvailableSpace::Definite(100.0), AvailableSpace::MaxContent),
            RequestedAxis::Both,
        ),
    );

    assert_size(output.size, Size::new(100.0, 20.0));
    assert_eq!(tree.session.layout_writes, 0);
    assert_eq!(tree.layout(first), sentinel);
    assert_eq!(tree.layout(second), sentinel);
    assert_eq!(tree.layout(root), sentinel);
}

#[test]
fn leaf_measure_goal_preserves_the_single_axis_fast_path() {
    let mut tree = TestTree::default();
    let leaf = fixed_leaf(&mut tree, 40.0, 20.0);
    let known = Size::new(Some(40.0), Some(20.0));
    let parent = Size::new(Some(100.0), Some(100.0));
    let available = Size::new(
        AvailableSpace::Definite(100.0),
        AvailableSpace::Definite(100.0),
    );

    let measured = tree.session.compute_child_layout(
        &tree.source,
        leaf,
        LayoutInput::compute_size(known, parent, available, RequestedAxis::Horizontal),
    );
    assert_size(measured.size, Size::new(40.0, 20.0));
    assert_eq!(tree.session.leaf_measure_calls, 0);

    let _ = tree.session.compute_child_layout(
        &tree.source,
        leaf,
        LayoutInput::compute_size(known, parent, available, RequestedAxis::Both),
    );
    assert_eq!(tree.session.leaf_measure_calls, 1);

    let _ = tree.session.compute_child_layout(
        &tree.source,
        leaf,
        LayoutInput::perform_layout(known, parent, available),
    );
    assert_eq!(tree.session.leaf_measure_calls, 2);
}

#[test]
fn relative_insets_shift_visual_positions_without_affecting_flow() {
    let mut tree = TestTree::default();
    let mut first_style = fixed_leaf_style(20.0, 10.0);
    first_style.inset.left = LengthPercentageAuto::Length(10.0);
    first_style.inset.top = LengthPercentageAuto::Length(5.0);
    let first = tree.push_leaf(first_style, Size::new(20.0, 10.0), None);

    let mut second_style = fixed_leaf_style(20.0, 10.0);
    second_style.inset.right = LengthPercentageAuto::Length(7.0);
    second_style.inset.bottom = LengthPercentageAuto::Length(3.0);
    let second = tree.push_leaf(second_style, Size::new(20.0, 10.0), None);
    let root = flex_container(
        &mut tree,
        TestStyle {
            align_items: Some(AlignItems::FlexStart),
            ..TestStyle::default()
        },
        &[first, second],
    );

    definite_layout(&mut tree, root, 100.0, 20.0);

    assert_point(tree.layout(first).location, Point::new(10.0, 5.0));
    assert_point(tree.layout(second).location, Point::new(13.0, -3.0));
}

#[test]
fn percentages_box_sizing_padding_and_gap_use_the_container_bases() {
    let mut tree = TestTree::default();
    let item_style = TestStyle {
        size: Size::new(Dimension::Length(20.0), Dimension::Length(10.0)),
        flex_basis: Dimension::Length(20.0),
        padding: Edges {
            left: LengthPercentage::length(10.0),
            right: LengthPercentage::length(10.0),
            top: LengthPercentage::ZERO,
            bottom: LengthPercentage::ZERO,
        },
        ..TestStyle::default()
    };
    let first = tree.push_leaf(item_style.clone(), Size::new(20.0, 10.0), None);
    let second = tree.push_leaf(item_style, Size::new(20.0, 10.0), None);
    let root = flex_container(
        &mut tree,
        TestStyle {
            padding: Edges::uniform(LengthPercentage::percent(0.1)),
            border: Edges::uniform(LengthPercentage::length(5.0)),
            gap: Size::new(LengthPercentage::percent(0.1), LengthPercentage::ZERO),
            align_items: Some(AlignItems::FlexStart),
            ..TestStyle::default()
        },
        &[first, second],
    );

    definite_layout(&mut tree, root, 200.0, 80.0);

    // Container padding percentages (including top/bottom) use its parent's
    // width: content width = 200 - 2*(20 padding + 5 border) = 150.
    // The 10% column gap is therefore 15.
    assert_point(tree.layout(first).location, Point::new(25.0, 25.0));
    assert_size(tree.layout(first).size, Size::new(40.0, 10.0));
    assert_point(tree.layout(second).location, Point::new(80.0, 25.0));
}

#[test]
fn automatic_minimum_size_depends_on_scroll_container_overflow() {
    let mut visible_tree = TestTree::default();
    let visible =
        visible_tree.push_leaf(fixed_leaf_style(100.0, 10.0), Size::new(100.0, 10.0), None);
    let root = flex_container(&mut visible_tree, TestStyle::default(), &[visible]);
    definite_layout(&mut visible_tree, root, 50.0, 10.0);
    assert_close(visible_tree.layout(visible).size.width, 100.0);

    let mut scroll_tree = TestTree::default();
    let mut scroll_style = fixed_leaf_style(100.0, 10.0);
    scroll_style.overflow = Point::new(Overflow::Hidden, Overflow::Hidden);
    let scroll = scroll_tree.push_leaf(scroll_style, Size::new(100.0, 10.0), None);
    let root = flex_container(&mut scroll_tree, TestStyle::default(), &[scroll]);
    definite_layout(&mut scroll_tree, root, 50.0, 10.0);
    assert_close(scroll_tree.layout(scroll).size.width, 50.0);
}

#[test]
fn column_wrapping_uses_rtl_and_wrap_reverse_for_cross_start() {
    for (wrap, expected) in [
        (
            FlexWrap::Wrap,
            [Point::new(50.0, 0.0), Point::new(40.0, 0.0)],
        ),
        (
            FlexWrap::WrapReverse,
            [Point::new(0.0, 0.0), Point::new(10.0, 0.0)],
        ),
    ] {
        let mut tree = TestTree::default();
        let mut child_style = fixed_leaf_style(10.0, 30.0);
        child_style.flex_basis = Dimension::Length(30.0);
        let first = tree.push_leaf(child_style.clone(), Size::new(10.0, 30.0), None);
        let second = tree.push_leaf(child_style, Size::new(10.0, 30.0), None);
        let root = flex_container(
            &mut tree,
            TestStyle {
                direction: Direction::Rtl,
                flex_direction: FlexDirection::Column,
                flex_wrap: wrap,
                align_content: Some(AlignContent::FlexStart),
                align_items: Some(AlignItems::FlexStart),
                ..TestStyle::default()
            },
            &[first, second],
        );

        definite_layout(&mut tree, root, 60.0, 50.0);
        assert_point(tree.layout(first).location, expected[0]);
        assert_point(tree.layout(second).location, expected[1]);
    }
}

#[test]
fn max_content_container_size_uses_flex_item_contributions() {
    fn intrinsic_fixture(grow: f32) -> (TestTree, NodeId, NodeId) {
        let mut tree = TestTree::default();
        let item = tree.push_leaf(
            TestStyle {
                flex_basis: Dimension::Length(100.0),
                flex_grow: grow,
                overflow: Point::new(Overflow::Hidden, Overflow::Hidden),
                ..TestStyle::default()
            },
            Size::new(200.0, 10.0),
            None,
        );
        let root = flex_container(&mut tree, TestStyle::default(), &[item]);
        (tree, root, item)
    }

    let (mut inflexible_tree, root, item) = intrinsic_fixture(0.0);
    let output = perform_layout(&mut inflexible_tree, root, Size::NONE, Size::MAX_CONTENT);
    assert_close(output.size.width, 100.0);
    assert_close(inflexible_tree.layout(item).size.width, 100.0);

    let (mut flexible_tree, root, item) = intrinsic_fixture(1.0);
    let output = perform_layout(&mut flexible_tree, root, Size::NONE, Size::MAX_CONTENT);
    assert_close(output.size.width, 200.0);
    assert_close(flexible_tree.layout(item).size.width, 200.0);
}

#[test]
fn indefinite_percentage_flex_basis_falls_back_to_content_not_width() {
    let mut tree = TestTree::default();
    let item = tree.push_leaf(
        TestStyle {
            size: Size::new(Dimension::Length(50.0), Dimension::Length(10.0)),
            flex_basis: Dimension::Percent(0.5),
            overflow: Point::new(Overflow::Hidden, Overflow::Hidden),
            ..TestStyle::default()
        },
        Size::new(80.0, 10.0),
        None,
    );
    let root = flex_container(&mut tree, TestStyle::default(), &[item]);

    let output = perform_layout(&mut tree, root, Size::NONE, Size::MAX_CONTENT);
    assert_close(output.size.width, 80.0);
    assert_close(tree.layout(item).size.width, 80.0);
}

#[test]
fn border_box_zero_basis_keeps_a_negative_inner_base_during_flexing() {
    let mut tree = TestTree::default();
    let item = tree.push_leaf(
        TestStyle {
            box_sizing: BoxSizing::BorderBox,
            flex_basis: Dimension::ZERO,
            flex_grow: 0.5,
            min_size: Size::new(Dimension::ZERO, Dimension::ZERO),
            padding: Edges {
                left: LengthPercentage::length(20.0),
                right: LengthPercentage::length(20.0),
                top: LengthPercentage::ZERO,
                bottom: LengthPercentage::ZERO,
            },
            overflow: Point::new(Overflow::Hidden, Overflow::Hidden),
            ..TestStyle::default()
        },
        Size::ZERO,
        None,
    );
    let root = flex_container(&mut tree, TestStyle::default(), &[item]);

    definite_layout(&mut tree, root, 100.0, 10.0);
    assert_close(tree.layout(item).size.width, 50.0);
}

#[test]
fn hoisted_static_position_is_the_aligned_margin_box_origin() {
    let mut tree = TestTree::default();
    let mut child_style = fixed_leaf_style(20.0, 10.0);
    child_style.position = Position::AbsoluteHoisted;
    child_style.margin.left = LengthPercentageAuto::Length(5.0);
    child_style.margin.top = LengthPercentageAuto::Length(3.0);
    let child = tree.push_leaf(child_style, Size::new(20.0, 10.0), None);
    let root = flex_container(
        &mut tree,
        TestStyle {
            justify_content: Some(JustifyContent::Center),
            align_items: Some(AlignItems::Center),
            ..TestStyle::default()
        },
        &[child],
    );

    definite_layout(&mut tree, root, 100.0, 50.0);
    assert_eq!(tree.static_position(child), Some(Point::new(37.5, 18.5)));
}

#[test]
fn aspect_ratio_does_not_disable_cross_axis_stretch() {
    let mut tree = TestTree::default();
    let item = tree.push_leaf(
        TestStyle {
            size: Size::new(Dimension::Length(50.0), Dimension::Auto),
            flex_basis: Dimension::Length(50.0),
            aspect_ratio: Some(1.0),
            ..TestStyle::default()
        },
        Size::new(50.0, 50.0),
        None,
    );
    let root = flex_container(&mut tree, TestStyle::default(), &[item]);

    definite_layout(&mut tree, root, 100.0, 100.0);
    assert_size(tree.layout(item).size, Size::new(50.0, 100.0));
}

#[test]
fn nowrap_auto_cross_size_clamped_by_min_stretches_its_line() {
    let mut tree = TestTree::default();
    let item = tree.push_leaf(TestStyle::default(), Size::new(20.0, 20.0), None);
    let root = flex_container(
        &mut tree,
        TestStyle {
            min_size: Size::new(Dimension::Auto, Dimension::Length(100.0)),
            ..TestStyle::default()
        },
        &[item],
    );

    let output = perform_layout(
        &mut tree,
        root,
        Size::new(Some(100.0), None),
        Size::new(AvailableSpace::Definite(100.0), AvailableSpace::MaxContent),
    );
    assert_size(output.size, Size::new(100.0, 100.0));
    assert_size(tree.layout(item).size, Size::new(20.0, 100.0));
}

#[test]
fn automatic_minimum_uses_aspect_ratio_transferred_size() {
    let mut tree = TestTree::default();
    let item = tree.push_intrinsic_leaf(
        TestStyle {
            size: Size::new(Dimension::Auto, Dimension::Length(80.0)),
            aspect_ratio: Some(2.0),
            ..TestStyle::default()
        },
        Size::new(10.0, 80.0),
        Size::new(10.0, 80.0),
    );
    let root = flex_container(&mut tree, TestStyle::default(), &[item]);

    definite_layout(&mut tree, root, 100.0, 80.0);
    assert_close(tree.layout(item).size.width, 160.0);
}

#[test]
fn intrinsic_main_size_keywords_use_content_contributions() {
    let mut tree = TestTree::default();
    let item = tree.push_intrinsic_leaf(
        TestStyle {
            size: Size::new(Dimension::MinContent, Dimension::Length(10.0)),
            flex_basis: Dimension::Auto,
            min_size: Size::new(Dimension::MinContent, Dimension::Auto),
            overflow: Point::new(Overflow::Hidden, Overflow::Hidden),
            ..TestStyle::default()
        },
        Size::new(50.0, 10.0),
        Size::new(200.0, 10.0),
    );
    let root = flex_container(&mut tree, TestStyle::default(), &[item]);

    definite_layout(&mut tree, root, 300.0, 10.0);
    assert_close(tree.layout(item).size.width, 50.0);
}

#[test]
fn multiline_column_min_content_cross_size_uses_largest_column() {
    let mut tree = TestTree::default();
    let mut child_style = fixed_leaf_style(30.0, 30.0);
    child_style.flex_basis = Dimension::Length(30.0);
    let first = tree.push_leaf(child_style.clone(), Size::new(30.0, 30.0), None);
    let second = tree.push_leaf(child_style, Size::new(30.0, 30.0), None);
    let root = flex_container(
        &mut tree,
        TestStyle {
            flex_direction: FlexDirection::Column,
            flex_wrap: FlexWrap::Wrap,
            ..TestStyle::default()
        },
        &[first, second],
    );

    let output = perform_layout(
        &mut tree,
        root,
        Size::new(None, Some(50.0)),
        Size::new(AvailableSpace::MinContent, AvailableSpace::Definite(50.0)),
    );
    assert_size(output.size, Size::new(30.0, 50.0));
}

#[test]
fn start_and_flex_start_remain_distinct_under_reversal() {
    for (alignment, expected) in [
        (AlignItems::Start, Point::new(0.0, 0.0)),
        (AlignItems::FlexStart, Point::new(80.0, 40.0)),
    ] {
        let mut tree = TestTree::default();
        let item = fixed_leaf(&mut tree, 20.0, 10.0);
        let root = flex_container(
            &mut tree,
            TestStyle {
                flex_direction: FlexDirection::RowReverse,
                flex_wrap: FlexWrap::WrapReverse,
                justify_content: Some(match alignment {
                    AlignItems::Start => JustifyContent::Start,
                    AlignItems::FlexStart => JustifyContent::FlexStart,
                    _ => unreachable!(),
                }),
                align_items: Some(alignment),
                ..TestStyle::default()
            },
            &[item],
        );
        definite_layout(&mut tree, root, 100.0, 50.0);
        assert_point(tree.layout(item).location, expected);
    }
}

#[test]
fn negative_margin_affects_line_breaking_without_being_clamped() {
    let mut tree = TestTree::default();
    let first = fixed_leaf(&mut tree, 80.0, 10.0);
    let mut second_style = fixed_leaf_style(40.0, 10.0);
    second_style.margin.left = LengthPercentageAuto::Length(-20.0);
    let second = tree.push_leaf(second_style, Size::new(40.0, 10.0), None);
    let third = fixed_leaf(&mut tree, 1.0, 10.0);
    let root = flex_container(
        &mut tree,
        TestStyle {
            flex_wrap: FlexWrap::Wrap,
            align_items: Some(AlignItems::FlexStart),
            ..TestStyle::default()
        },
        &[first, second, third],
    );

    definite_layout(&mut tree, root, 100.0, 20.0);
    assert_point(tree.layout(first).location, Point::new(0.0, 0.0));
    assert_point(tree.layout(second).location, Point::new(60.0, 0.0));
    assert_point(tree.layout(third).location, Point::new(0.0, 10.0));
}

#[test]
fn absolute_children_use_order_zero_for_paint_order() {
    let mut tree = TestTree::default();
    let mut inflow_style = fixed_leaf_style(20.0, 10.0);
    inflow_style.order = 5;
    let inflow = tree.push_leaf(inflow_style, Size::new(20.0, 10.0), None);
    let mut absolute_style = fixed_leaf_style(20.0, 10.0);
    absolute_style.position = Position::Absolute;
    let absolute = tree.push_leaf(absolute_style, Size::new(20.0, 10.0), None);
    let root = flex_container(&mut tree, TestStyle::default(), &[inflow, absolute]);

    definite_layout(&mut tree, root, 100.0, 20.0);
    assert_eq!(tree.layout(absolute).order, 0);
    assert_eq!(tree.layout(inflow).order, 1);
}

#[test]
fn leaf_measurement_reports_baselines_for_a_fully_sized_box() {
    let style = fixed_leaf_style(100.0, 20.0);
    let mut measurer = FnLeafMeasurer::new(|_input| {
        LeafMetrics::new(Size::new(100.0, 20.0)).with_first_baselines(Point::new(None, Some(15.0)))
    });
    let output = compute_leaf_layout(
        LayoutInput::perform_layout(Size::NONE, Size::NONE, Size::MAX_CONTENT),
        &style,
        |_calc, _basis| unreachable!(),
        &mut measurer,
    );
    assert_size(output.size, Size::new(100.0, 20.0));
    assert_size(output.content_size, Size::new(100.0, 20.0));
    assert_eq!(output.first_baselines.y, Some(15.0));
}

#[test]
fn leaf_max_width_constrains_measurement_and_preserves_overflow_extent() {
    let style = TestStyle {
        max_size: Size::new(Dimension::Length(100.0), Dimension::Auto),
        padding: Edges::uniform(LengthPercentage::length(10.0)),
        ..TestStyle::default()
    };
    let mut measurer = FnLeafMeasurer::new(|input| {
        assert_eq!(input.available_space.width, AvailableSpace::Definite(100.0));
        LeafMetrics::new(Size::new(200.0, 30.0)).with_first_baselines(Point::new(None, Some(15.0)))
    });
    let output = compute_leaf_layout(
        LayoutInput::perform_layout(
            Size::NONE,
            Size::new(Some(500.0), Some(500.0)),
            Size::new(
                AvailableSpace::Definite(500.0),
                AvailableSpace::Definite(500.0),
            ),
        ),
        &style,
        |_calc, _basis| unreachable!(),
        &mut measurer,
    );
    // max-width is content-box: 100 content + 20 padding.
    assert_size(output.size, Size::new(120.0, 50.0));
    assert_size(output.content_size, Size::new(220.0, 50.0));
    assert_eq!(output.first_baselines.y, Some(25.0));
}

#[test]
fn absolute_aspect_ratio_uses_vertical_inset_stretch_when_horizontal_is_auto() {
    let mut tree = TestTree::default();
    let child = tree.push_leaf(
        TestStyle {
            position: Position::Absolute,
            inset: Edges {
                left: LengthPercentageAuto::Auto,
                right: LengthPercentageAuto::Auto,
                top: LengthPercentageAuto::Length(10.0),
                bottom: LengthPercentageAuto::Length(10.0),
            },
            aspect_ratio: Some(2.0),
            ..TestStyle::default()
        },
        Size::ZERO,
        None,
    );

    let layout = compute_absolute_layout(
        &tree.source,
        &mut tree.session,
        child,
        Size::new(100.0, 100.0),
        Point::ZERO,
    );
    assert_size(layout.size, Size::new(160.0, 80.0));
    assert_point(layout.location, Point::new(0.0, 10.0));
}

#[test]
fn cyclic_percentage_item_margin_resolves_after_intrinsic_container_sizing() {
    let mut tree = TestTree::default();
    let mut child_style = fixed_leaf_style(100.0, 10.0);
    child_style.flex_basis = Dimension::Length(10.0);
    child_style.margin.left = LengthPercentageAuto::Percent(0.1);
    let child = tree.push_leaf(child_style, Size::new(100.0, 10.0), None);
    let root = flex_container(
        &mut tree,
        TestStyle {
            flex_direction: FlexDirection::Column,
            ..TestStyle::default()
        },
        &[child],
    );

    let output = perform_layout(
        &mut tree,
        root,
        Size::new(None, Some(20.0)),
        Size::new(AvailableSpace::MaxContent, AvailableSpace::Definite(20.0)),
    );
    assert_size(output.size, Size::new(100.0, 20.0));
    assert_close(tree.layout(child).margin.left, 10.0);
    assert_point(tree.layout(child).location, Point::new(10.0, 0.0));
    assert_close(output.content_size.width, 110.0);
}

#[test]
fn overflowing_auto_margins_follow_main_and_cross_axis_rules() {
    let mut tree = TestTree::default();
    let item = tree.push_leaf(
        TestStyle {
            size: Size::new(Dimension::Length(120.0), Dimension::Length(80.0)),
            flex_basis: Dimension::Length(120.0),
            flex_shrink: 0.0,
            margin: Edges::uniform(LengthPercentageAuto::Auto),
            ..TestStyle::default()
        },
        Size::new(120.0, 80.0),
        None,
    );
    let root = flex_container(
        &mut tree,
        TestStyle {
            justify_content: Some(JustifyContent::Center),
            ..TestStyle::default()
        },
        &[item],
    );

    definite_layout(&mut tree, root, 100.0, 50.0);
    let layout = tree.layout(item);
    assert_point(layout.location, Point::new(-10.0, 0.0));
    assert_close(layout.margin.left, 0.0);
    assert_close(layout.margin.right, 0.0);
    assert_close(layout.margin.top, 0.0);
    assert_close(layout.margin.bottom, -30.0);
}
