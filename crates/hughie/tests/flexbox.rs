//! Spec-focused flexbox integration tests over a plain `Vec`-backed host.

mod support;

use hughie::compute::{compute_absolute_layout, compute_leaf_layout};
use hughie::prelude::*;
use hughie::style::StyleSize;
use stylo::computed_values::{box_sizing, direction, flex_direction, flex_wrap};
use stylo::values::computed::{Display, FlexBasis, Margin, MaxSize, Overflow, PositionProperty};
use stylo::values::specified::align::AlignFlags;
use support::*;

#[test]
fn flex_grow_distributes_free_space_proportionally() {
    let mut tree = TestTree::default();
    let mut first_style = fixed_leaf_style(50.0, 20.0);
    first_style.flex_grow = nn(1.0);
    let first = tree.push_leaf(first_style, Size::new(50.0, 20.0), None);
    let mut second_style = fixed_leaf_style(50.0, 20.0);
    second_style.flex_grow = nn(2.0);
    let second = tree.push_leaf(second_style, Size::new(50.0, 20.0), None);
    let root = flex_container(&mut tree, TestStyle::default(), &[first, second]);

    definite_layout(&tree, root, 300.0, 20.0);

    assert_close(tree.layout(first).size.width, 350.0 / 3.0);
    assert_close(tree.layout(second).size.width, 550.0 / 3.0);
    assert_close(tree.layout(second).location.x, 350.0 / 3.0);
}

#[test]
fn flex_grow_sum_below_one_leaves_part_of_the_free_space() {
    let mut tree = TestTree::default();
    let mut first_style = fixed_leaf_style(50.0, 20.0);
    first_style.flex_grow = nn(0.2);
    let first = tree.push_leaf(first_style, Size::new(50.0, 20.0), None);
    let mut second_style = fixed_leaf_style(50.0, 20.0);
    second_style.flex_grow = nn(0.2);
    let second = tree.push_leaf(second_style, Size::new(50.0, 20.0), None);
    let root = flex_container(&mut tree, TestStyle::default(), &[first, second]);

    definite_layout(&tree, root, 300.0, 20.0);

    assert_close(tree.layout(first).size.width, 90.0);
    assert_close(tree.layout(second).size.width, 90.0);
    assert_close(tree.layout(second).location.x, 90.0);
}

#[test]
fn flex_shrink_uses_scaled_flex_shrink_factors() {
    let mut tree = TestTree::default();
    let mut first_style = fixed_leaf_style(100.0, 20.0);
    first_style.min_size.width = size_px(0.0);
    let first = tree.push_leaf(first_style, Size::new(100.0, 20.0), None);
    let mut second_style = fixed_leaf_style(200.0, 20.0);
    second_style.min_size.width = size_px(0.0);
    let second = tree.push_leaf(second_style, Size::new(200.0, 20.0), None);
    let root = flex_container(&mut tree, TestStyle::default(), &[first, second]);

    definite_layout(&tree, root, 180.0, 20.0);

    assert_close(tree.layout(first).size.width, 60.0);
    assert_close(tree.layout(second).size.width, 120.0);
    assert_close(tree.layout(second).location.x, 60.0);
}

#[test]
fn min_and_max_constraints_refreeze_flexible_items() {
    let mut grow_tree = TestTree::default();
    let mut capped_style = fixed_leaf_style(100.0, 20.0);
    capped_style.flex_grow = nn(1.0);
    capped_style.max_size.width = max_px(120.0);
    let capped = grow_tree.push_leaf(capped_style, Size::new(100.0, 20.0), None);
    let mut growing_style = fixed_leaf_style(100.0, 20.0);
    growing_style.flex_grow = nn(1.0);
    let growing = grow_tree.push_leaf(growing_style, Size::new(100.0, 20.0), None);
    let grow_root = flex_container(&mut grow_tree, TestStyle::default(), &[capped, growing]);

    definite_layout(&grow_tree, grow_root, 300.0, 20.0);
    assert_close(grow_tree.layout(capped).size.width, 120.0);
    assert_close(grow_tree.layout(growing).size.width, 180.0);

    let mut shrink_tree = TestTree::default();
    let mut floored_style = fixed_leaf_style(100.0, 20.0);
    floored_style.min_size.width = size_px(90.0);
    let floored = shrink_tree.push_leaf(floored_style, Size::new(100.0, 20.0), None);
    let mut shrinking_style = fixed_leaf_style(100.0, 20.0);
    shrinking_style.min_size.width = size_px(0.0);
    let shrinking = shrink_tree.push_leaf(shrinking_style, Size::new(100.0, 20.0), None);
    let shrink_root = flex_container(
        &mut shrink_tree,
        TestStyle::default(),
        &[floored, shrinking],
    );

    definite_layout(&shrink_tree, shrink_root, 160.0, 20.0);
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
        flex_wrap: flex_wrap::T::WRAP,
        gap: Size::new(gap_px(10.0), gap_px(5.0)),
        ..TestStyle::default()
    };
    let root = flex_container(&mut tree, container_style, &[first, second, third]);

    let output = perform_layout(
        &tree,
        root,
        Size::new(Some(210.0), None),
        Size::new(AvailableSpace::Definite(210.0), AvailableSpace::MaxContent),
    );

    assert_size(output.size, Size::new(210.0, 45.0));
    assert_point(tree.layout(first).location, Point::new(0.0, 0.0));
    assert_point(tree.layout(second).location, Point::new(110.0, 0.0));
    assert_point(tree.layout(third).location, Point::new(0.0, 25.0));
}

mod line_collection {
    use super::*;

    #[test]
    fn a_zero_sized_item_after_an_exact_fit_remains_on_the_current_line() {
        let mut tree = TestTree::default();
        let exact_fit = fixed_leaf(&mut tree, 50.0, 10.0);
        let zero_sized = fixed_leaf(&mut tree, 0.0, 6.0);
        let next_line = fixed_leaf(&mut tree, 10.0, 10.0);
        let root = flex_container(
            &mut tree,
            TestStyle {
                flex_wrap: flex_wrap::T::WRAP,
                align_items: items(AlignFlags::FLEX_START),
                ..TestStyle::default()
            },
            &[exact_fit, zero_sized, next_line],
        );

        let output = perform_layout(
            &tree,
            root,
            Size::new(Some(50.0), None),
            Size::new(AvailableSpace::Definite(50.0), AvailableSpace::MaxContent),
        );

        assert_size(output.size, Size::new(50.0, 20.0));
        assert_point(tree.layout(exact_fit).location, Point::new(0.0, 0.0));
        assert_size(tree.layout(zero_sized).size, Size::new(0.0, 6.0));
        assert_point(tree.layout(zero_sized).location, Point::new(50.0, 0.0));
        assert_point(tree.layout(next_line).location, Point::new(0.0, 10.0));
    }

    #[test]
    fn an_oversized_first_item_forms_its_own_line() {
        let mut tree = TestTree::default();
        let oversized = tree.push_leaf(
            TestStyle {
                flex_shrink: nn(0.0),
                ..fixed_leaf_style(50.0, 10.0)
            },
            Size::new(50.0, 10.0),
            None,
        );
        let next = fixed_leaf(&mut tree, 10.0, 10.0);
        let root = flex_container(
            &mut tree,
            TestStyle {
                flex_wrap: flex_wrap::T::WRAP,
                align_items: items(AlignFlags::FLEX_START),
                ..TestStyle::default()
            },
            &[oversized, next],
        );

        let output = perform_layout(
            &tree,
            root,
            Size::new(Some(30.0), None),
            Size::new(AvailableSpace::Definite(30.0), AvailableSpace::MaxContent),
        );

        assert_size(output.size, Size::new(30.0, 20.0));
        assert_size(tree.layout(oversized).size, Size::new(50.0, 10.0));
        assert_point(tree.layout(oversized).location, Point::new(0.0, 0.0));
        assert_point(tree.layout(next).location, Point::new(0.0, 10.0));
    }

    #[test]
    fn flexible_lengths_are_resolved_independently_for_each_line() {
        let mut tree = TestTree::default();
        let mut growing = |basis| {
            tree.push_leaf(
                TestStyle {
                    flex_grow: nn(1.0),
                    ..fixed_leaf_style(basis, 10.0)
                },
                Size::new(basis, 10.0),
                None,
            )
        };
        let first = growing(40.0);
        let second = growing(40.0);
        let third = growing(40.0);
        let fourth = growing(20.0);
        let root = flex_container(
            &mut tree,
            TestStyle {
                flex_wrap: flex_wrap::T::WRAP,
                align_items: items(AlignFlags::FLEX_START),
                ..TestStyle::default()
            },
            &[first, second, third, fourth],
        );

        let output = perform_layout(
            &tree,
            root,
            Size::new(Some(100.0), None),
            Size::new(AvailableSpace::Definite(100.0), AvailableSpace::MaxContent),
        );

        assert_size(output.size, Size::new(100.0, 20.0));
        assert_size(tree.layout(first).size, Size::new(50.0, 10.0));
        assert_size(tree.layout(second).size, Size::new(50.0, 10.0));
        assert_size(tree.layout(third).size, Size::new(60.0, 10.0));
        assert_size(tree.layout(fourth).size, Size::new(40.0, 10.0));
        assert_point(tree.layout(third).location, Point::new(0.0, 10.0));
        assert_point(tree.layout(fourth).location, Point::new(60.0, 10.0));
    }
}

fn direction_fixture(
    flex_direction: flex_direction::T,
    direction: direction::T,
) -> (TestTree, TestId, TestId, TestId) {
    let mut tree = TestTree::default();
    let first = fixed_leaf(&mut tree, 20.0, 20.0);
    let second = fixed_leaf(&mut tree, 30.0, 30.0);
    let root = flex_container(
        &mut tree,
        TestStyle {
            flex_direction,
            direction,
            align_items: items(AlignFlags::FLEX_START),
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
            flex_direction::T::Row,
            direction::T::Ltr,
            Point::new(0.0, 0.0),
            Point::new(20.0, 0.0),
        ),
        (
            flex_direction::T::RowReverse,
            direction::T::Ltr,
            Point::new(80.0, 0.0),
            Point::new(50.0, 0.0),
        ),
        (
            flex_direction::T::Row,
            direction::T::Rtl,
            Point::new(80.0, 0.0),
            Point::new(50.0, 0.0),
        ),
        (
            flex_direction::T::RowReverse,
            direction::T::Rtl,
            Point::new(0.0, 0.0),
            Point::new(20.0, 0.0),
        ),
        (
            flex_direction::T::Column,
            direction::T::Ltr,
            Point::new(0.0, 0.0),
            Point::new(0.0, 20.0),
        ),
        (
            flex_direction::T::ColumnReverse,
            direction::T::Ltr,
            Point::new(0.0, 80.0),
            Point::new(0.0, 50.0),
        ),
    ];

    for (flex_direction, direction, expected_first, expected_second) in cases {
        let (tree, root, first, second) = direction_fixture(flex_direction, direction);
        definite_layout(&tree, root, 100.0, 100.0);
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

    definite_layout(&tree, root, 100.0, 20.0);

    for (node, expected_x, expected_order) in
        [(b, 0.0, 0), (c, 10.0, 1), (a, 20.0, 2), (d, 30.0, 3)]
    {
        assert_close(tree.layout(node).location.x, expected_x);
        assert_eq!(tree.layout(node).order, expected_order);
    }
}

#[test]
fn space_between_places_two_items_at_opposite_main_edges() {
    let mut tree = TestTree::default();
    let first = fixed_leaf(&mut tree, 20.0, 10.0);
    let second = fixed_leaf(&mut tree, 20.0, 10.0);
    let root = flex_container(
        &mut tree,
        TestStyle {
            justify_content: content(AlignFlags::SPACE_BETWEEN),
            ..TestStyle::default()
        },
        &[first, second],
    );

    definite_layout(&tree, root, 100.0, 20.0);
    assert_close(tree.layout(first).location.x, 0.0);
    assert_close(tree.layout(second).location.x, 80.0);
}

#[test]
fn a_single_space_between_item_falls_back_to_main_start() {
    let mut tree = TestTree::default();
    let child = fixed_leaf(&mut tree, 20.0, 10.0);
    let root = flex_container(
        &mut tree,
        TestStyle {
            justify_content: content(AlignFlags::SPACE_BETWEEN),
            ..TestStyle::default()
        },
        &[child],
    );

    definite_layout(&tree, root, 100.0, 20.0);
    assert_point(tree.layout(child).location, Point::new(0.0, 0.0));
}

#[test]
fn a_main_start_auto_margin_consumes_space_before_justify_content() {
    let mut tree = TestTree::default();
    let mut auto_margin_style = fixed_leaf_style(20.0, 10.0);
    auto_margin_style.margin.left = margin_auto();
    let auto_margin = tree.push_leaf(auto_margin_style, Size::new(20.0, 10.0), None);
    let trailing = fixed_leaf(&mut tree, 20.0, 10.0);
    let root = flex_container(
        &mut tree,
        TestStyle {
            justify_content: content(AlignFlags::CENTER),
            ..TestStyle::default()
        },
        &[auto_margin, trailing],
    );

    definite_layout(&tree, root, 100.0, 20.0);
    assert_close(tree.layout(auto_margin).margin.left, 60.0);
    assert_close(tree.layout(auto_margin).location.x, 60.0);
    assert_close(tree.layout(trailing).location.x, 80.0);
}

mod alignment {
    use super::*;

    #[test]
    fn align_items_center_and_align_self_end_position_items_on_the_cross_axis() {
        let mut align_tree = TestTree::default();
        let centered = fixed_leaf(&mut align_tree, 20.0, 20.0);
        let mut end_style = fixed_leaf_style(20.0, 20.0);
        end_style.align_self = self_align(AlignFlags::FLEX_END);
        let ended = align_tree.push_leaf(end_style, Size::new(20.0, 20.0), None);
        let root = flex_container(
            &mut align_tree,
            TestStyle {
                align_items: items(AlignFlags::CENTER),
                ..TestStyle::default()
            },
            &[centered, ended],
        );

        definite_layout(&align_tree, root, 100.0, 60.0);
        assert_close(align_tree.layout(centered).location.y, 20.0);
        assert_close(align_tree.layout(ended).location.y, 40.0);
    }

    #[test]
    fn an_auto_cross_size_stretches_to_the_flex_line() {
        let mut stretch_tree = TestTree::default();
        let stretched = stretch_tree.push_leaf(TestStyle::default(), Size::new(20.0, 10.0), None);
        let root = flex_container(&mut stretch_tree, TestStyle::default(), &[stretched]);
        definite_layout(&stretch_tree, root, 100.0, 60.0);
        assert_close(stretch_tree.layout(stretched).size.height, 60.0);
    }

    #[test]
    fn a_cross_start_auto_margin_absorbs_free_space_before_alignment() {
        let mut margin_tree = TestTree::default();
        let mut auto_margin_style = fixed_leaf_style(20.0, 20.0);
        auto_margin_style.margin.top = margin_auto();
        let auto_margin = margin_tree.push_leaf(auto_margin_style, Size::new(20.0, 20.0), None);
        let root = flex_container(
            &mut margin_tree,
            TestStyle {
                align_items: items(AlignFlags::CENTER),
                ..TestStyle::default()
            },
            &[auto_margin],
        );

        definite_layout(&margin_tree, root, 100.0, 60.0);
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
                flex_wrap: flex_wrap::T::WRAP,
                gap: Size::new(gap_px(0.0), gap_px(10.0)),
                align_content: content(AlignFlags::CENTER),
                align_items: items(AlignFlags::FLEX_START),
                ..TestStyle::default()
            },
            &[first, second],
        );

        definite_layout(&tree, root, 100.0, 60.0);

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
                align_items: items(AlignFlags::BASELINE),
                ..TestStyle::default()
            },
            &[first, second],
        );

        let output = definite_layout(&tree, root, 100.0, 40.0);

        assert_close(tree.layout(first).location.y + 15.0, 15.0);
        assert_close(tree.layout(second).location.y + 10.0, 15.0);
        assert_eq!(output.first_baselines.y, Some(15.0));
    }
}

#[test]
fn hidden_and_out_of_flow_children_do_not_participate_in_flexing() {
    let mut tree = TestTree::default();
    let first = fixed_leaf(&mut tree, 20.0, 10.0);

    let hidden = tree.push_leaf(
        TestStyle {
            display: Display::None,
            ..fixed_leaf_style(1_000.0, 10.0)
        },
        Size::new(1_000.0, 10.0),
        None,
    );
    let absolute = tree.push_leaf(
        TestStyle {
            position: PositionProperty::Absolute,
            ..fixed_leaf_style(1_000.0, 10.0)
        },
        Size::new(1_000.0, 10.0),
        None,
    );
    let hoisted = tree.push_leaf(
        TestStyle {
            position: PositionProperty::Fixed,
            ..fixed_leaf_style(1_000.0, 10.0)
        },
        Size::new(1_000.0, 10.0),
        None,
    );
    let second = fixed_leaf(&mut tree, 20.0, 10.0);
    let root = flex_container(
        &mut tree,
        TestStyle {
            justify_content: content(AlignFlags::SPACE_BETWEEN),
            ..TestStyle::default()
        },
        &[first, hidden, absolute, hoisted, second],
    );

    definite_layout(&tree, root, 100.0, 20.0);

    assert_close(tree.layout(first).location.x, 0.0);
    assert_close(tree.layout(second).location.x, 80.0);
    assert!(tree.static_position(hoisted).is_some());
}

#[test]
fn display_contents_children_flex_as_items_of_the_box_ancestor() {
    let mut tree = TestTree::default();
    let grow = || TestStyle {
        flex_grow: nn(1.0),
        ..fixed_leaf_style(50.0, 20.0)
    };
    let first = tree.push_leaf(grow(), Size::new(50.0, 20.0), None);
    let lifted = tree.push_leaf(grow(), Size::new(50.0, 20.0), None);
    let nested = tree.push_leaf(grow(), Size::new(50.0, 20.0), None);
    let inner_wrapper = tree.push_contents(vec![nested]);
    let wrapper = tree.push_contents(vec![lifted, inner_wrapper]);
    let root = flex_container(&mut tree, TestStyle::default(), &[first, wrapper]);

    definite_layout(&tree, root, 300.0, 20.0);

    // The three flattened leaves share the root's free space.
    for (item, x) in [(first, 0.0), (lifted, 100.0), (nested, 200.0)] {
        assert_close(tree.layout(item).size.width, 100.0);
        assert_close(tree.layout(item).location.x, x);
    }
    for box_less in [wrapper, inner_wrapper] {
        assert_eq!(tree.layout(box_less), Layout::default());
    }
}

#[test]
fn display_contents_items_keep_their_own_order_and_hidden_handling() {
    let mut tree = TestTree::default();
    let last = tree.push_leaf(
        TestStyle {
            order: 1,
            ..fixed_leaf_style(20.0, 10.0)
        },
        Size::new(20.0, 10.0),
        None,
    );
    let hidden = tree.push_leaf(
        TestStyle {
            display: Display::None,
            ..fixed_leaf_style(1_000.0, 10.0)
        },
        Size::new(1_000.0, 10.0),
        None,
    );
    let wrapper = tree.push_contents(vec![last, hidden]);
    let first = fixed_leaf(&mut tree, 20.0, 10.0);
    let root = flex_container(&mut tree, TestStyle::default(), &[wrapper, first]);

    definite_layout(&tree, root, 100.0, 10.0);

    // Lifted items keep their own order and flattened paint slot.
    assert_close(tree.layout(first).location.x, 0.0);
    assert_close(tree.layout(last).location.x, 20.0);
    assert_eq!(tree.layout(hidden), Layout::with_order(1));
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
    tree.set_layout_for_testing(first, snapshot_layout(&sentinel));
    tree.set_layout_for_testing(second, snapshot_layout(&sentinel));
    tree.set_layout_for_testing(root, snapshot_layout(&sentinel));

    let output = tree.compute_layout(
        root,
        LayoutInput::measure(
            Size::new(Some(100.0), None),
            Size::new(Some(100.0), None),
            Size::new(AvailableSpace::Definite(100.0), AvailableSpace::MaxContent),
            RequestedAxis::Both,
        ),
    );

    assert_size(output.size, Size::new(100.0, 20.0));
    assert_eq!(tree.layout_writes.get(), 0);
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

    let measured = tree.compute_layout(
        leaf,
        LayoutInput::measure(known, parent, available, RequestedAxis::Horizontal),
    );
    assert_size(measured.size, Size::new(40.0, 20.0));
    assert_eq!(tree.leaf_measure_calls.get(), 0);

    let _ = tree.compute_layout(
        leaf,
        LayoutInput::measure(known, parent, available, RequestedAxis::Both),
    );
    assert_eq!(tree.leaf_measure_calls.get(), 1);

    let _ = tree.compute_layout(
        leaf,
        LayoutInput::commit(known, parent, available, Size::new(false, false)),
    );
    assert_eq!(tree.leaf_measure_calls.get(), 2);
}

#[test]
fn relative_insets_shift_visual_positions_without_affecting_flow() {
    let mut tree = TestTree::default();
    let mut first_style = fixed_leaf_style(20.0, 10.0);
    first_style.inset.left = inset_px(10.0);
    first_style.inset.top = inset_px(5.0);
    let first = tree.push_leaf(first_style, Size::new(20.0, 10.0), None);

    let mut second_style = fixed_leaf_style(20.0, 10.0);
    second_style.inset.right = inset_px(7.0);
    second_style.inset.bottom = inset_px(3.0);
    let second = tree.push_leaf(second_style, Size::new(20.0, 10.0), None);
    let root = flex_container(
        &mut tree,
        TestStyle {
            align_items: items(AlignFlags::FLEX_START),
            ..TestStyle::default()
        },
        &[first, second],
    );

    definite_layout(&tree, root, 100.0, 20.0);

    assert_point(tree.layout(first).location, Point::new(10.0, 5.0));
    assert_point(tree.layout(second).location, Point::new(13.0, -3.0));
}

#[test]
fn percentages_box_sizing_padding_and_gap_use_the_container_bases() {
    let mut tree = TestTree::default();
    let item_style = TestStyle {
        size: Size::new(size_px(20.0), size_px(10.0)),
        flex_basis: basis_px(20.0),
        padding: Edges {
            left: npx(10.0),
            right: npx(10.0),
            top: npx(0.0),
            bottom: npx(0.0),
        },
        ..TestStyle::default()
    };
    let first = tree.push_leaf(item_style.clone(), Size::new(20.0, 10.0), None);
    let second = tree.push_leaf(item_style, Size::new(20.0, 10.0), None);
    let root = flex_container(
        &mut tree,
        TestStyle {
            padding: Edges::uniform(npct(0.1)),
            border: Edges::uniform(border_px(5.0)),
            gap: Size::new(gap_pct(0.1), gap_px(0.0)),
            align_items: items(AlignFlags::FLEX_START),
            ..TestStyle::default()
        },
        &[first, second],
    );

    definite_layout(&tree, root, 200.0, 80.0);

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
    definite_layout(&visible_tree, root, 50.0, 10.0);
    assert_close(visible_tree.layout(visible).size.width, 100.0);

    let mut scroll_tree = TestTree::default();
    let mut scroll_style = fixed_leaf_style(100.0, 10.0);
    scroll_style.overflow = Point::new(Overflow::Hidden, Overflow::Hidden);
    let scroll = scroll_tree.push_leaf(scroll_style, Size::new(100.0, 10.0), None);
    let root = flex_container(&mut scroll_tree, TestStyle::default(), &[scroll]);
    definite_layout(&scroll_tree, root, 50.0, 10.0);
    assert_close(scroll_tree.layout(scroll).size.width, 50.0);
}

#[test]
fn column_wrapping_uses_rtl_and_wrap_reverse_for_cross_start() {
    for (wrap, expected) in [
        (
            flex_wrap::T::WRAP,
            [Point::new(50.0, 0.0), Point::new(40.0, 0.0)],
        ),
        (
            flex_wrap::T::WRAP_REVERSE,
            [Point::new(0.0, 0.0), Point::new(10.0, 0.0)],
        ),
    ] {
        let mut tree = TestTree::default();
        let mut child_style = fixed_leaf_style(10.0, 30.0);
        child_style.flex_basis = basis_px(30.0);
        let first = tree.push_leaf(child_style.clone(), Size::new(10.0, 30.0), None);
        let second = tree.push_leaf(child_style, Size::new(10.0, 30.0), None);
        let root = flex_container(
            &mut tree,
            TestStyle {
                direction: direction::T::Rtl,
                flex_direction: flex_direction::T::Column,
                flex_wrap: wrap,
                align_content: content(AlignFlags::FLEX_START),
                align_items: items(AlignFlags::FLEX_START),
                ..TestStyle::default()
            },
            &[first, second],
        );

        definite_layout(&tree, root, 60.0, 50.0);
        assert_point(tree.layout(first).location, expected[0]);
        assert_point(tree.layout(second).location, expected[1]);
    }
}

#[test]
fn max_content_container_size_uses_flex_item_contributions() {
    fn intrinsic_fixture(grow: f32) -> (TestTree, TestId, TestId) {
        let mut tree = TestTree::default();
        let item = tree.push_leaf(
            TestStyle {
                flex_basis: basis_px(100.0),
                flex_grow: nn(grow),
                overflow: Point::new(Overflow::Hidden, Overflow::Hidden),
                ..TestStyle::default()
            },
            Size::new(200.0, 10.0),
            None,
        );
        let root = flex_container(&mut tree, TestStyle::default(), &[item]);
        (tree, root, item)
    }

    let (inflexible_tree, root, item) = intrinsic_fixture(0.0);
    let output = perform_layout(&inflexible_tree, root, Size::NONE, Size::MAX_CONTENT);
    assert_close(output.size.width, 100.0);
    assert_close(inflexible_tree.layout(item).size.width, 100.0);

    let (flexible_tree, root, item) = intrinsic_fixture(1.0);
    let output = perform_layout(&flexible_tree, root, Size::NONE, Size::MAX_CONTENT);
    assert_close(output.size.width, 200.0);
    assert_close(flexible_tree.layout(item).size.width, 200.0);
}

/// A percentage flex basis against an indefinite container behaves as
/// `content` (css-flexbox-1 §7.2.3), not as the item's `width`: the flex base
/// size is the 80px content. The item's max-content contribution is still
/// its definite 50px `width` (css-sizing-3 §5.2), and a flex base size from
/// the content neither caps nor floors it, so the container is 50 either
/// way: the item shrinks to it, or, unable to shrink, keeps its 80px flex
/// base size and overflows. (Chrome gives the same containers; it then
/// resolves the percentage against the decided 50 and makes the shrinkable
/// item 25.)
#[test]
fn indefinite_percentage_flex_basis_falls_back_to_content_not_width() {
    for (flex_shrink, width) in [(1.0, 50.0), (0.0, 80.0)] {
        let mut tree = TestTree::default();
        let item = tree.push_leaf(
            TestStyle {
                size: Size::new(size_px(50.0), size_px(10.0)),
                flex_basis: basis_pct(0.5),
                flex_shrink: nn(flex_shrink),
                overflow: Point::new(Overflow::Hidden, Overflow::Hidden),
                ..TestStyle::default()
            },
            Size::new(80.0, 10.0),
            None,
        );
        let root = flex_container(&mut tree, TestStyle::default(), &[item]);

        let output = perform_layout(&tree, root, Size::NONE, Size::MAX_CONTENT);
        assert_close(output.size.width, 50.0);
        assert_close(tree.layout(item).size.width, width);
    }
}

#[test]
fn border_box_zero_basis_keeps_a_negative_inner_base_during_flexing() {
    let mut tree = TestTree::default();
    let item = tree.push_leaf(
        TestStyle {
            box_sizing: box_sizing::T::BorderBox,
            flex_basis: basis_px(0.0),
            flex_grow: nn(0.5),
            min_size: Size::new(size_px(0.0), size_px(0.0)),
            padding: Edges {
                left: npx(20.0),
                right: npx(20.0),
                top: npx(0.0),
                bottom: npx(0.0),
            },
            overflow: Point::new(Overflow::Hidden, Overflow::Hidden),
            ..TestStyle::default()
        },
        Size::ZERO,
        None,
    );
    let root = flex_container(&mut tree, TestStyle::default(), &[item]);

    definite_layout(&tree, root, 100.0, 10.0);
    assert_close(tree.layout(item).size.width, 50.0);
}

#[test]
fn hoisted_static_position_is_the_aligned_margin_box_origin() {
    let mut tree = TestTree::default();
    let mut child_style = fixed_leaf_style(20.0, 10.0);
    child_style.position = PositionProperty::Fixed;
    child_style.margin.left = margin_px(5.0);
    child_style.margin.top = margin_px(3.0);
    let child = tree.push_leaf(child_style, Size::new(20.0, 10.0), None);
    let root = flex_container(
        &mut tree,
        TestStyle {
            justify_content: content(AlignFlags::CENTER),
            align_items: items(AlignFlags::CENTER),
            ..TestStyle::default()
        },
        &[child],
    );

    definite_layout(&tree, root, 100.0, 50.0);
    assert_eq!(tree.static_position(child), Some(Point::new(37.5, 18.5)));
}

#[test]
fn aspect_ratio_does_not_disable_cross_axis_stretch() {
    let mut tree = TestTree::default();
    let item = tree.push_leaf(
        TestStyle {
            size: Size::new(size_px(50.0), size_auto()),
            flex_basis: basis_px(50.0),
            aspect_ratio: ratio(1.0),
            ..TestStyle::default()
        },
        Size::new(50.0, 50.0),
        None,
    );
    let root = flex_container(&mut tree, TestStyle::default(), &[item]);

    definite_layout(&tree, root, 100.0, 100.0);
    assert_size(tree.layout(item).size, Size::new(50.0, 100.0));
}

/// A replaced 40x20 leaf: it answers a known axis through its own ratio, the
/// way `NaturalSize::measure` answers for a decoded bitmap.
fn bitmap_leaf(input: LeafMeasureInput) -> LeafMetrics {
    let known = input.known_dimensions;
    LeafMetrics::new(match (known.width, known.height) {
        (Some(width), Some(height)) => Size::new(width, height),
        (Some(width), None) => Size::new(width, width / 2.0),
        (None, Some(height)) => Size::new(height * 2.0, height),
        (None, None) => Size::new(40.0, 20.0),
    })
}

/// The same leaf standing up: 20x40, ratio 1:2. Every transfer below is
/// asserted in both ratio directions, because a rule that reads the ratio the
/// wrong way round still passes on a square-ish one.
fn tall_bitmap_leaf(input: LeafMeasureInput) -> LeafMetrics {
    let known = input.known_dimensions;
    LeafMetrics::new(match (known.width, known.height) {
        (Some(width), Some(height)) => Size::new(width, height),
        (Some(width), None) => Size::new(width, width * 2.0),
        (None, Some(height)) => Size::new(height / 2.0, height),
        (None, None) => Size::new(20.0, 40.0),
    })
}

/// The `40x20` leaf under a replaced element's own natural size, which is what
/// `dom` reports for a decoded bitmap. Flexbox never reads it — css-align-3
/// §6.2.4 makes `normal` behave as `stretch` for flex items with no exception
/// for replaced content — and the tests carry it anyway so that they prove
/// that, rather than merely failing to exercise it.
fn replaced(style: TestStyle) -> TestStyle {
    TestStyle {
        natural_size: Size::new(Some(40.0), Some(20.0)),
        ..style
    }
}

/// css-flexbox-1 §9.8 makes the cross size of an item the container will
/// stretch *definite*, and §9.2 steps B/E then produce that item's flex base
/// size from a measurement that knows it — so an auto-sized replaced item ends
/// up as large as the stretch made it, not as large as its own pixels.
///
/// Both directions are asserted, because the whole of the rule is that the
/// transfer follows the *cross* axis wherever that is.
#[test]
fn a_stretched_cross_size_reaches_the_flex_base_measurement() {
    for (flex_direction, expected) in [
        (flex_direction::T::Row, Size::new(200.0, 100.0)),
        (flex_direction::T::Column, Size::new(100.0, 50.0)),
    ] {
        let mut tree = TestTree::default();
        let item = tree.push_measured_leaf(TestStyle::default(), bitmap_leaf);
        let root = flex_container(
            &mut tree,
            TestStyle {
                flex_direction,
                ..TestStyle::default()
            },
            &[item],
        );

        definite_layout(&tree, root, 100.0, 100.0);
        assert_size(tree.layout(item).size, expected);
    }
}

/// The stretched cross size handed to that measurement is the item's own
/// clamped one, not the line's: a cross-axis `max-*` caps what it sees.
#[test]
fn a_stretched_cross_size_is_clamped_before_the_measurement_reads_it() {
    let mut tree = TestTree::default();
    let item = tree.push_measured_leaf(
        TestStyle {
            max_size: Size::new(max_px(60.0), max_none()),
            ..TestStyle::default()
        },
        bitmap_leaf,
    );
    let root = flex_container(
        &mut tree,
        TestStyle {
            flex_direction: flex_direction::T::Column,
            ..TestStyle::default()
        },
        &[item],
    );

    definite_layout(&tree, root, 100.0, 100.0);
    assert_size(tree.layout(item).size, Size::new(60.0, 30.0));
}

/// An item the container will *not* stretch measures at its own size, which is
/// what keeps the rule above from reaching every flex item.
#[test]
fn an_unstretched_item_measures_at_its_own_size() {
    let mut tree = TestTree::default();
    let item = tree.push_measured_leaf(TestStyle::default(), bitmap_leaf);
    let root = flex_container(
        &mut tree,
        TestStyle {
            flex_direction: flex_direction::T::Column,
            align_items: items(AlignFlags::FLEX_START),
            ..TestStyle::default()
        },
        &[item],
    );

    definite_layout(&tree, root, 100.0, 100.0);
    assert_size(tree.layout(item).size, Size::new(40.0, 20.0));
}

/// §9.8 grants the definite stretched cross size to *single-line* containers
/// alone, so a wrapping one measures its items at their own size and only
/// stretches them afterwards — the item ends up 100 wide and 20 tall rather
/// than 100x50.
#[test]
fn a_wrapping_container_does_not_pre_impose_the_stretched_cross_size() {
    let mut tree = TestTree::default();
    let item = tree.push_measured_leaf(TestStyle::default(), bitmap_leaf);
    let root = flex_container(
        &mut tree,
        TestStyle {
            flex_direction: flex_direction::T::Column,
            flex_wrap: flex_wrap::T::WRAP,
            ..TestStyle::default()
        },
        &[item],
    );

    definite_layout(&tree, root, 100.0, 100.0);
    assert_size(tree.layout(item).size, Size::new(100.0, 20.0));
}

/// css-align-3 §6.2.4: `align-self: normal` on a flex item behaves as
/// `stretch` with no replaced-element exception — unlike a grid item, where
/// css-grid-1 §6.2 makes `normal` leave replaced content at its natural size.
/// The item below reports one, and is stretched and transferred all the same.
#[test]
fn normal_cross_alignment_stretches_replaced_flex_items() {
    let mut tree = TestTree::default();
    let item = tree.push_measured_leaf(replaced(TestStyle::default()), bitmap_leaf);
    let root = flex_container(&mut tree, TestStyle::default(), &[item]);

    definite_layout(&tree, root, 300.0, 100.0);
    assert_size(tree.layout(item).size, Size::new(200.0, 100.0));
}

/// The 1:2 leaf takes the same route as the 2:1 one in the cases above: the
/// stretched cross size divides instead of multiplying. Row: cross 100 → main
/// 100/2. Column: cross 100 → main 100*2.
#[test]
fn a_stretched_cross_size_transfers_through_a_tall_ratio_too() {
    for (flex_direction, expected) in [
        (flex_direction::T::Row, Size::new(50.0, 100.0)),
        (flex_direction::T::Column, Size::new(100.0, 200.0)),
    ] {
        let mut tree = TestTree::default();
        let item = tree.push_measured_leaf(TestStyle::default(), tall_bitmap_leaf);
        let root = flex_container(
            &mut tree,
            TestStyle {
                flex_direction,
                ..TestStyle::default()
            },
            &[item],
        );

        definite_layout(&tree, root, 100.0, 100.0);
        assert_size(tree.layout(item).size, expected);
    }
}

/// §9.8 only makes the cross size definite for an item the container is going
/// to *stretch*. Every other cross alignment, and an auto cross margin — which
/// css-flexbox-1 §8.1 says blocks the stretch outright — leaves the item
/// measuring at its own 40x20.
#[test]
fn an_item_the_container_will_not_stretch_measures_at_its_own_size() {
    let cases = [
        (AlignFlags::CENTER, margin_px(0.0)),
        (AlignFlags::FLEX_END, margin_px(0.0)),
        (AlignFlags::STRETCH, margin_auto()),
    ];
    for (align, cross_margin) in cases {
        let mut tree = TestTree::default();
        let item = tree.push_measured_leaf(
            TestStyle {
                align_self: self_align(align),
                margin: Edges {
                    left: cross_margin,
                    ..Edges::uniform(margin_px(0.0))
                },
                ..TestStyle::default()
            },
            bitmap_leaf,
        );
        let root = flex_container(
            &mut tree,
            TestStyle {
                flex_direction: flex_direction::T::Column,
                ..TestStyle::default()
            },
            &[item],
        );

        definite_layout(&tree, root, 100.0, 100.0);
        assert_size(tree.layout(item).size, Size::new(40.0, 20.0));
    }
}

/// §9.8 needs a *definite* container cross size to hand over. A row container
/// sized by its own content has none while its items are being measured, so
/// the item reports 40x20 and the line is 20 tall.
#[test]
fn an_indefinite_container_cross_size_leaves_the_item_at_its_own_size() {
    let mut tree = TestTree::default();
    let item = tree.push_measured_leaf(TestStyle::default(), bitmap_leaf);
    let root = flex_container(&mut tree, TestStyle::default(), &[item]);

    let output = perform_layout(
        &tree,
        root,
        Size::new(Some(300.0), None),
        Size::new(AvailableSpace::Definite(300.0), AvailableSpace::MaxContent),
    );
    assert_size(tree.layout(item).size, Size::new(40.0, 20.0));
    assert_close(output.size.height, 20.0);
}

/// An authored cross size is the item's own used cross size, so §9.2 step B
/// transfers from *it* and the container's stretch never applies: 30 wide in a
/// 100-wide column gives 30/2 = 15 tall.
#[test]
fn an_authored_cross_size_outranks_the_containers_stretch() {
    let mut tree = TestTree::default();
    let item = tree.push_measured_leaf(
        TestStyle {
            size: Size::new(size_px(30.0), size_auto()),
            ..TestStyle::default()
        },
        bitmap_leaf,
    );
    let root = flex_container(
        &mut tree,
        TestStyle {
            flex_direction: flex_direction::T::Column,
            ..TestStyle::default()
        },
        &[item],
    );

    definite_layout(&tree, root, 100.0, 100.0);
    assert_size(tree.layout(item).size, Size::new(30.0, 15.0));
}

/// The stretched cross size §9.8 hands to the measurement is clamped by the
/// item's own cross min/max first. A cross-axis minimum above the container's
/// inner cross size raises it: 160 wide in a 100-wide column gives 80 tall.
/// (The `max-*` half is `a_stretched_cross_size_is_clamped_before_the_
/// measurement_reads_it`.)
#[test]
fn a_cross_axis_minimum_raises_the_size_the_measurement_reads() {
    let mut tree = TestTree::default();
    let item = tree.push_measured_leaf(
        TestStyle {
            min_size: Size::new(size_px(160.0), size_auto()),
            ..TestStyle::default()
        },
        bitmap_leaf,
    );
    let root = flex_container(
        &mut tree,
        TestStyle {
            flex_direction: flex_direction::T::Column,
            ..TestStyle::default()
        },
        &[item],
    );

    definite_layout(&tree, root, 100.0, 300.0);
    assert_size(tree.layout(item).size, Size::new(160.0, 80.0));
}

/// §9.2 reaches step B only when `flex-basis` is `content` and the main size
/// is `auto`: step A takes a definite `flex-basis`, and a main-axis size
/// answers `auto` before any measurement runs. Either one outranks the
/// transfer, which the cross axis still performs around it.
#[test]
fn an_authored_main_size_or_flex_basis_outranks_the_transfer() {
    for style in [
        TestStyle {
            flex_basis: basis_px(70.0),
            ..TestStyle::default()
        },
        TestStyle {
            size: Size::new(size_auto(), size_px(70.0)),
            ..TestStyle::default()
        },
    ] {
        let mut tree = TestTree::default();
        let item = tree.push_measured_leaf(style, bitmap_leaf);
        let root = flex_container(
            &mut tree,
            TestStyle {
                flex_direction: flex_direction::T::Column,
                ..TestStyle::default()
            },
            &[item],
        );

        definite_layout(&tree, root, 100.0, 300.0);
        assert_size(tree.layout(item).size, Size::new(100.0, 70.0));
    }
}

/// A transferred base size cannot be shrunk away: css-flexbox-1 §4.5 gives an
/// auto-minimum item a content-based minimum, and for a box with a ratio and a
/// definite cross size that is the transferred size itself. The 100-wide
/// column stretches the item to a 50-tall base, and a 40-tall container cannot
/// pull it below 50 — only a `max-height` can, which is the second case.
#[test]
fn shrinking_cannot_pull_a_transferred_base_size_below_its_automatic_minimum() {
    for (max_height, expected) in [(max_none(), 50.0), (max_px(40.0), 40.0)] {
        let mut tree = TestTree::default();
        let item = tree.push_measured_leaf(
            TestStyle {
                max_size: Size::new(max_none(), max_height),
                ..TestStyle::default()
            },
            bitmap_leaf,
        );
        let root = flex_container(
            &mut tree,
            TestStyle {
                flex_direction: flex_direction::T::Column,
                ..TestStyle::default()
            },
            &[item],
        );

        definite_layout(&tree, root, 100.0, 40.0);
        assert_size(tree.layout(item).size, Size::new(100.0, expected));
    }
}

/// Two ratio items on one line each transfer from the same stretched cross
/// size and keep their own ratios: 2:1 becomes 200 wide, 1:2 becomes 50, and
/// 250 fits in 300 so neither is shrunk.
#[test]
fn two_ratio_items_on_one_line_transfer_independently() {
    let mut tree = TestTree::default();
    let wide = tree.push_measured_leaf(TestStyle::default(), bitmap_leaf);
    let tall = tree.push_measured_leaf(TestStyle::default(), tall_bitmap_leaf);
    let root = flex_container(&mut tree, TestStyle::default(), &[wide, tall]);

    definite_layout(&tree, root, 300.0, 100.0);
    assert_size(tree.layout(wide).size, Size::new(200.0, 100.0));
    assert_size(tree.layout(tall).size, Size::new(50.0, 100.0));
    assert_close(tree.layout(tall).location.x, 200.0);
}

/// A ratio item beside an item whose main size does not follow from its cross
/// one: only the ratio item's base size moves with the stretch, and the line
/// still stretches both.
#[test]
fn a_ratio_item_shares_a_line_with_a_content_sized_item() {
    let mut tree = TestTree::default();
    let image = tree.push_measured_leaf(TestStyle::default(), bitmap_leaf);
    let text = tree.push_leaf(TestStyle::default(), Size::new(30.0, 16.0), None);
    let root = flex_container(&mut tree, TestStyle::default(), &[image, text]);

    definite_layout(&tree, root, 300.0, 100.0);
    assert_size(tree.layout(image).size, Size::new(200.0, 100.0));
    assert_size(tree.layout(text).size, Size::new(30.0, 100.0));
}

/// The definite cross size travels down: the outer row stretches the inner
/// container to 100 tall, the inner row is then a single-line container with a
/// definite cross size of its own, and the leaf ends 200x100 two levels down.
#[test]
fn the_transferred_cross_size_reaches_a_nested_containers_item() {
    let mut tree = TestTree::default();
    let item = tree.push_measured_leaf(TestStyle::default(), bitmap_leaf);
    let inner = flex_container(&mut tree, TestStyle::default(), &[item]);
    let root = flex_container(&mut tree, TestStyle::default(), &[inner]);

    definite_layout(&tree, root, 300.0, 100.0);
    assert_size(tree.layout(item).size, Size::new(200.0, 100.0));
    assert_size(tree.layout(inner).size, Size::new(200.0, 100.0));
}

/// §9.8's definite cross size is a single-line rule in both directions: a
/// wrapping row leaves the item 40 wide and stretches it to the line's 100
/// afterwards, the transpose of the column case above.
#[test]
fn a_wrapping_row_container_stretches_after_measuring_rather_than_before() {
    let mut tree = TestTree::default();
    let item = tree.push_measured_leaf(TestStyle::default(), bitmap_leaf);
    let root = flex_container(
        &mut tree,
        TestStyle {
            flex_wrap: flex_wrap::T::WRAP,
            ..TestStyle::default()
        },
        &[item],
    );

    definite_layout(&tree, root, 300.0, 100.0);
    assert_size(tree.layout(item).size, Size::new(40.0, 100.0));
}

#[test]
fn nowrap_auto_cross_size_clamped_by_min_stretches_its_line() {
    let mut tree = TestTree::default();
    let item = tree.push_leaf(TestStyle::default(), Size::new(20.0, 20.0), None);
    let root = flex_container(
        &mut tree,
        TestStyle {
            min_size: Size::new(size_auto(), size_px(100.0)),
            ..TestStyle::default()
        },
        &[item],
    );

    let output = perform_layout(
        &tree,
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
            size: Size::new(size_auto(), size_px(80.0)),
            aspect_ratio: ratio(2.0),
            ..TestStyle::default()
        },
        Size::new(10.0, 80.0),
        Size::new(10.0, 80.0),
    );
    let root = flex_container(&mut tree, TestStyle::default(), &[item]);

    definite_layout(&tree, root, 100.0, 80.0);
    assert_close(tree.layout(item).size.width, 160.0);
}

#[test]
fn intrinsic_main_size_keywords_use_content_contributions() {
    let mut tree = TestTree::default();
    let item = tree.push_intrinsic_leaf(
        TestStyle {
            size: Size::new(size_min_content(), size_px(10.0)),
            flex_basis: basis_auto(),
            min_size: Size::new(size_min_content(), size_auto()),
            overflow: Point::new(Overflow::Hidden, Overflow::Hidden),
            ..TestStyle::default()
        },
        Size::new(50.0, 10.0),
        Size::new(200.0, 10.0),
    );
    let root = flex_container(&mut tree, TestStyle::default(), &[item]);

    definite_layout(&tree, root, 300.0, 10.0);
    assert_close(tree.layout(item).size.width, 50.0);
}

#[test]
fn multiline_column_min_content_cross_size_uses_largest_column() {
    let mut tree = TestTree::default();
    let mut child_style = fixed_leaf_style(30.0, 30.0);
    child_style.flex_basis = basis_px(30.0);
    let first = tree.push_leaf(child_style.clone(), Size::new(30.0, 30.0), None);
    let second = tree.push_leaf(child_style, Size::new(30.0, 30.0), None);
    let root = flex_container(
        &mut tree,
        TestStyle {
            flex_direction: flex_direction::T::Column,
            flex_wrap: flex_wrap::T::WRAP,
            ..TestStyle::default()
        },
        &[first, second],
    );

    let output = perform_layout(
        &tree,
        root,
        Size::new(None, Some(50.0)),
        Size::new(AvailableSpace::MinContent, AvailableSpace::Definite(50.0)),
    );
    assert_size(output.size, Size::new(30.0, 50.0));
}

#[test]
fn start_and_flex_start_remain_distinct_under_reversal() {
    for (alignment, expected) in [
        (AlignFlags::START, Point::new(0.0, 0.0)),
        (AlignFlags::FLEX_START, Point::new(80.0, 40.0)),
    ] {
        let mut tree = TestTree::default();
        let item = fixed_leaf(&mut tree, 20.0, 10.0);
        let root = flex_container(
            &mut tree,
            TestStyle {
                flex_direction: flex_direction::T::RowReverse,
                flex_wrap: flex_wrap::T::WRAP_REVERSE,
                justify_content: content(alignment),
                align_items: items(alignment),
                ..TestStyle::default()
            },
            &[item],
        );
        definite_layout(&tree, root, 100.0, 50.0);
        assert_point(tree.layout(item).location, expected);
    }
}

#[test]
fn negative_margin_affects_line_breaking_without_being_clamped() {
    let mut tree = TestTree::default();
    let first = fixed_leaf(&mut tree, 80.0, 10.0);
    let mut second_style = fixed_leaf_style(40.0, 10.0);
    second_style.margin.left = margin_px(-20.0);
    let second = tree.push_leaf(second_style, Size::new(40.0, 10.0), None);
    let third = fixed_leaf(&mut tree, 1.0, 10.0);
    let root = flex_container(
        &mut tree,
        TestStyle {
            flex_wrap: flex_wrap::T::WRAP,
            align_items: items(AlignFlags::FLEX_START),
            ..TestStyle::default()
        },
        &[first, second, third],
    );

    definite_layout(&tree, root, 100.0, 20.0);
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
    absolute_style.position = PositionProperty::Absolute;
    let absolute = tree.push_leaf(absolute_style, Size::new(20.0, 10.0), None);
    let root = flex_container(&mut tree, TestStyle::default(), &[inflow, absolute]);

    definite_layout(&tree, root, 100.0, 20.0);
    assert_eq!(tree.layout(absolute).order, 0);
    assert_eq!(tree.layout(inflow).order, 1);
}

#[test]
fn natural_sized_leaf_has_no_content_baseline() {
    let style = fixed_leaf_style(100.0, 20.0);
    let output = compute_leaf_layout(
        LayoutInput::commit(
            Size::NONE,
            Size::NONE,
            Size::MAX_CONTENT,
            Size::new(false, false),
        ),
        &style,
        NaturalSize::from_size(Size::new(100.0, 20.0)),
    );
    assert_size(output.size, Size::new(100.0, 20.0));
    assert_size(output.content_size, Size::new(100.0, 20.0));
    assert_eq!(output.first_baselines, Point::NONE);
}

#[test]
fn leaf_max_width_constrains_measurement_and_preserves_overflow_extent() {
    let style = TestStyle {
        max_size: Size::new(max_px(100.0), max_none()),
        padding: Edges::uniform(npx(10.0)),
        ..TestStyle::default()
    };
    let output = compute_leaf_layout(
        LayoutInput::commit(
            Size::NONE,
            Size::new(Some(500.0), Some(500.0)),
            Size::new(
                AvailableSpace::Definite(500.0),
                AvailableSpace::Definite(500.0),
            ),
            Size::new(false, false),
        ),
        &style,
        NaturalSize::from_size(Size::new(200.0, 30.0)),
    );
    assert_size(output.size, Size::new(120.0, 50.0));
    assert_size(output.content_size, Size::new(220.0, 50.0));
    assert_eq!(output.first_baselines, Point::NONE);
}

#[test]
fn absolute_aspect_ratio_uses_vertical_inset_stretch_when_horizontal_is_auto() {
    let mut tree = TestTree::default();
    let child = tree.push_leaf(
        TestStyle {
            position: PositionProperty::Absolute,
            inset: Edges {
                left: inset_auto(),
                right: inset_auto(),
                top: inset_px(10.0),
                bottom: inset_px(10.0),
            },
            aspect_ratio: ratio(2.0),
            ..TestStyle::default()
        },
        Size::ZERO,
        None,
    );

    let layout = tree.with_layout_state(true, |tree, state| {
        compute_absolute_layout(
            tree,
            state,
            tree.node(child),
            Size::new(100.0, 100.0),
            Point::ZERO,
        )
    });
    assert_size(layout.size, Size::new(160.0, 80.0));
    assert_point(layout.location, Point::new(0.0, 10.0));
}

/// One `width` × `height` item that neither grows nor shrinks.
fn rigid_item(tree: &mut TestTree, width: f32, height: f32) -> TestId {
    tree.push_leaf(
        TestStyle {
            flex_shrink: nn(0.0),
            ..fixed_leaf_style(width, height)
        },
        Size::new(width, height),
        None,
    )
}

/// A paragraph whose longest word is 500 wide and whose unbroken line is
/// 1000: it fills any width between the two, on one 100-tall line at 1000
/// or more and on two below that.
fn paragraph_leaf(input: LeafMeasureInput) -> LeafMetrics {
    let width = input
        .known_dimensions
        .width
        .unwrap_or(match input.available_space.width {
            AvailableSpace::MinContent => 500.0,
            AvailableSpace::MaxContent => 1000.0,
            AvailableSpace::Definite(limit) => limit.clamp(500.0, 1000.0),
        });
    let height =
        input
            .known_dimensions
            .height
            .unwrap_or(if width >= 1000.0 { 100.0 } else { 200.0 });
    LeafMetrics::new(Size::new(width, height))
}

fn paragraph(tree: &mut TestTree) -> TestId {
    tree.push_measured_leaf(TestStyle::default(), paragraph_leaf)
}

fn inset_zero(style: TestStyle) -> TestStyle {
    TestStyle {
        position: PositionProperty::Absolute,
        inset: Edges::uniform(inset_px(0.0)),
        ..style
    }
}

/// A flex container placed with `inset: 0` in an 800×600 containing block
/// around the one item `content` pushes.
fn inset_zero_box(style: TestStyle, content: impl FnOnce(&mut TestTree) -> TestId) -> Layout {
    let mut tree = TestTree::default();
    let child = content(&mut tree);
    let target = tree.push_flex(inset_zero(style), vec![child]);
    place_in_800_by_600(&tree, target)
}

/// [`paragraph_leaf`] itself placed with `inset: 0` in an 800×600
/// containing block.
fn inset_zero_paragraph(style: TestStyle) -> Layout {
    let mut tree = TestTree::default();
    let target = tree.push_measured_leaf(inset_zero(style), paragraph_leaf);
    place_in_800_by_600(&tree, target)
}

fn place_in_800_by_600(tree: &TestTree, target: TestId) -> Layout {
    tree.with_layout_state(true, |tree, state| {
        compute_absolute_layout(
            tree,
            state,
            tree.node(target),
            Size::new(800.0, 600.0),
            Point::ZERO,
        )
    })
}

fn sized_auto_margin(width: StyleSize, height: StyleSize) -> TestStyle {
    TestStyle {
        size: Size::new(width, height),
        margin: Edges::uniform(Margin::Auto),
        ..TestStyle::default()
    }
}

/// css-position-3 §4.1 stretch-fits only an automatic (`auto`) size between
/// two non-`auto` insets; `fit-content` is css-sizing-3 §3.2's
/// `min(max-content, max(min-content, stretch-fit))`, and the `auto` margins
/// then centre the box (HTML's `dialog:modal`).
#[test]
fn absolute_fit_content_between_insets_shrinks_to_fit_and_centres() {
    let small = |tree: &mut TestTree| rigid_item(tree, 100.0, 100.0);
    let fit = sized_auto_margin(StyleSize::FitContent, StyleSize::FitContent);
    let layout = inset_zero_box(fit, small);
    assert_size(layout.size, Size::new(100.0, 100.0));
    assert_point(layout.location, Point::new(350.0, 250.0));
    assert_eq!(
        layout.margin,
        Edges {
            left: 350.0,
            right: 350.0,
            top: 250.0,
            bottom: 250.0,
        }
    );

    let auto_width = sized_auto_margin(size_auto(), StyleSize::FitContent);
    let layout = inset_zero_box(auto_width, small);
    assert_size(layout.size, Size::new(800.0, 100.0));
    assert_point(layout.location, Point::new(0.0, 250.0));

    let auto_both = sized_auto_margin(size_auto(), size_auto());
    let layout = inset_zero_box(auto_both, small);
    assert_size(layout.size, Size::new(800.0, 600.0));
    assert_point(layout.location, Point::ZERO);
}

/// The fit-content clamp's other two arms: content wider than the
/// stretch-fit size is held to it (and wraps), content whose min-content
/// width alone exceeds it keeps that width, and the stretch-fit size is what
/// remains of the inset-modified containing block after the non-`auto`
/// margins, border and padding.
#[test]
fn absolute_fit_content_between_insets_clamps_to_the_stretch_fit_size() {
    let fit = || sized_auto_margin(StyleSize::FitContent, StyleSize::FitContent);
    let wrapped = inset_zero_paragraph(fit());
    assert_size(wrapped.size, Size::new(800.0, 200.0));
    assert_point(wrapped.location, Point::new(0.0, 200.0));

    let too_wide = inset_zero_box(fit(), |tree| rigid_item(tree, 1000.0, 100.0));
    assert_size(too_wide.size, Size::new(1000.0, 100.0));
    assert_point(too_wide.location, Point::new(0.0, 250.0));

    let edged = TestStyle {
        margin: Edges {
            left: margin_px(50.0),
            right: Margin::Auto,
            top: margin_px(20.0),
            bottom: Margin::Auto,
        },
        padding: Edges::uniform(npx(10.0)),
        border: Edges::uniform(border_px(5.0)),
        ..fit()
    };
    let edged = inset_zero_paragraph(edged);
    // 800 - 50 (margin) - 30 (padding and border) leaves 720 for the
    // paragraph; the auto end margins take the rest.
    assert_size(edged.size, Size::new(750.0, 230.0));
    assert_point(edged.location, Point::new(50.0, 20.0));
}

/// `min-content` and `max-content` between two insets keep sizing from
/// their own constraint, and the `auto` margins centre what fits.
#[test]
fn absolute_intrinsic_keywords_between_insets_ignore_the_stretch_fit_size() {
    let min = inset_zero_box(
        sized_auto_margin(size_min_content(), StyleSize::FitContent),
        paragraph,
    );
    assert_size(min.size, Size::new(500.0, 200.0));
    assert_point(min.location, Point::new(150.0, 200.0));

    let max = inset_zero_box(
        sized_auto_margin(size_max_content(), StyleSize::FitContent),
        paragraph,
    );
    assert_size(max.size, Size::new(1000.0, 100.0));
    assert_point(max.location, Point::new(0.0, 250.0));
}

/// A flex container with `style` placed with `inset: 0` in an 800×600
/// containing block around the items `content` pushes; returns the tree for
/// reading the items back.
fn inset_zero_flex(
    style: TestStyle,
    content: impl FnOnce(&mut TestTree) -> Vec<TestId>,
) -> (TestTree, Vec<TestId>, Layout) {
    let mut tree = TestTree::default();
    let children = content(&mut tree);
    let target = tree.push_flex(inset_zero(style), children.clone());
    let layout = place_in_800_by_600(&tree, target);
    (tree, children, layout)
}

fn wrapping(style: TestStyle) -> TestStyle {
    TestStyle {
        flex_wrap: flex_wrap::T::WRAP,
        ..style
    }
}

fn two_rigid_items(width: f32) -> impl FnOnce(&mut TestTree) -> Vec<TestId> {
    move |tree| {
        vec![
            rigid_item(tree, width, 100.0),
            rigid_item(tree, width, 100.0),
        ]
    }
}

/// A `fit-content` flex container's width is css-sizing-3 §3.2's
/// `min(max-content, max(min-content, stretch-fit))` over css-flexbox-1
/// §9.9.1's intrinsic main sizes, so content wider than the inset-modified
/// containing block is held to it. `nowrap`: min-content 500 (the
/// paragraph's longest word), max-content 1000 (its unbroken line), 800 in
/// between, where the paragraph wraps onto two lines. Multi-line: two
/// 500-wide items make max-content 1000 (one line) and min-content 500 (the
/// largest item), so the container is 800 wide and the items still break
/// onto two lines.
#[test]
fn absolute_fit_content_between_insets_limits_a_flex_container_to_the_stretch_fit_size() {
    let fit = || sized_auto_margin(StyleSize::FitContent, StyleSize::FitContent);

    let (tree, items, layout) = inset_zero_flex(fit(), |tree| vec![paragraph(tree)]);
    assert_size(layout.size, Size::new(800.0, 200.0));
    assert_point(layout.location, Point::new(0.0, 200.0));
    assert_size(tree.layout(items[0]).size, Size::new(800.0, 200.0));

    let (tree, items, layout) = inset_zero_flex(wrapping(fit()), two_rigid_items(500.0));
    assert_size(layout.size, Size::new(800.0, 200.0));
    assert_point(layout.location, Point::new(0.0, 200.0));
    assert_point(tree.layout(items[0]).location, Point::ZERO);
    assert_point(tree.layout(items[1]).location, Point::new(0.0, 100.0));
}

/// Content narrower than the stretch-fit size keeps its max-content width
/// (one line, centred by the `auto` margins), where `width: auto` still
/// stretches to the inset-modified containing block (css-position-3 §4.1).
#[test]
fn absolute_fit_content_between_insets_keeps_a_narrower_flex_container_at_its_content() {
    let fit = || sized_auto_margin(StyleSize::FitContent, StyleSize::FitContent);
    let (tree, items, layout) = inset_zero_flex(wrapping(fit()), two_rigid_items(300.0));
    assert_size(layout.size, Size::new(600.0, 100.0));
    assert_point(layout.location, Point::new(100.0, 250.0));
    assert_point(tree.layout(items[1]).location, Point::new(300.0, 0.0));

    let auto_width = sized_auto_margin(size_auto(), StyleSize::FitContent);
    let (_, _, layout) = inset_zero_flex(wrapping(auto_width), two_rigid_items(300.0));
    assert_size(layout.size, Size::new(800.0, 100.0));
    assert_point(layout.location, Point::new(0.0, 250.0));
}

/// A flex item around `content` px of 10px-tall content (its min- and
/// max-content width alike).
fn content_item(tree: &mut TestTree, style: TestStyle, content: f32) -> TestId {
    tree.push_leaf(style, Size::new(content, 10.0), None)
}

fn flex_item_style(grow: f32, shrink: f32, basis: FlexBasis) -> TestStyle {
    TestStyle {
        flex_grow: nn(grow),
        flex_shrink: nn(shrink),
        flex_basis: basis,
        ..TestStyle::default()
    }
}

/// A fit-content flex container around one item: the container's width and
/// the item's.
fn fit_content_around(style: TestStyle, content: f32) -> (f32, f32) {
    let fit = sized_auto_margin(StyleSize::FitContent, StyleSize::FitContent);
    let (tree, items, layout) =
        inset_zero_flex(fit, |tree| vec![content_item(tree, style, content)]);
    (layout.size.width, tree.layout(items[0]).size.width)
}

/// css-flexbox-1 §9.9.3: a definite flex basis caps a non-growable item's
/// contribution and floors a non-shrinkable one's — and is no lower bound
/// otherwise. Three `flex: 0 1 400px` items around 50px of content
/// contribute 50 each, so the fit-content container is 150, not the 800 its
/// 1200px of flex bases were held to. Every number here is Chrome's.
#[test]
fn absolute_fit_content_between_insets_caps_contributions_at_a_definite_flex_basis() {
    let fit = sized_auto_margin(StyleSize::FitContent, StyleSize::FitContent);
    let (tree, items, layout) = inset_zero_flex(fit, |tree| {
        (0..3)
            .map(|_| content_item(tree, flex_item_style(0.0, 1.0, basis_px(400.0)), 50.0))
            .collect()
    });
    assert_size(layout.size, Size::new(150.0, 10.0));
    assert_point(layout.location, Point::new(325.0, 295.0));
    for (item, left) in items.iter().zip([0.0, 50.0, 100.0]) {
        assert_size(tree.layout(*item).size, Size::new(50.0, 10.0));
        assert_point(tree.layout(*item).location, Point::new(left, 0.0));
    }

    let no_min = |style: TestStyle| TestStyle {
        min_size: Size::new(size_px(0.0), size_auto()),
        ..style
    };
    for (name, style, content, expected) in [
        (
            "flex: 1 1 400px, growable: neither cap nor floor",
            flex_item_style(1.0, 1.0, basis_px(400.0)),
            50.0,
            (50.0, 50.0),
        ),
        (
            "flex: 1 0 200px, not shrinkable: floored",
            flex_item_style(1.0, 0.0, basis_px(200.0)),
            50.0,
            (200.0, 200.0),
        ),
        (
            "flex: 0 1 50px; min-width: 0, not growable: capped",
            no_min(flex_item_style(0.0, 1.0, basis_px(50.0))),
            300.0,
            (50.0, 50.0),
        ),
        (
            "flex: 0 1 50px, capped, then held by the automatic minimum",
            flex_item_style(0.0, 1.0, basis_px(50.0)),
            300.0,
            (300.0, 300.0),
        ),
    ] {
        assert_eq!(fit_content_around(style, content), expected, "{name}");
    }
}

/// css-sizing-3 §5.2: the contribution of a box with a definite preferred
/// size is that size, clamped by its min/max — for a flex item too, where
/// css-flexbox-1 §9.9.3 would take the larger of it and the content (Blink's
/// web-compatible algorithm, so Chrome, takes the box's own). `flex: 1 1
/// auto; width: 100px` around 300px of content contributes 100; a definite
/// flex basis then still caps and floors it. Chrome's numbers.
#[test]
fn absolute_fit_content_between_insets_takes_a_definite_preferred_size_as_the_contribution() {
    let sized = |style: TestStyle| TestStyle {
        size: Size::new(size_px(100.0), size_auto()),
        ..style
    };
    for (name, style, content, expected) in [
        (
            "flex: 1 1 auto",
            flex_item_style(1.0, 1.0, basis_auto()),
            300.0,
            (100.0, 100.0),
        ),
        (
            "flex: 1 1 200px",
            flex_item_style(1.0, 1.0, basis_px(200.0)),
            50.0,
            (100.0, 100.0),
        ),
        (
            "flex: 0 0 200px",
            flex_item_style(0.0, 0.0, basis_px(200.0)),
            50.0,
            (200.0, 200.0),
        ),
        (
            "flex: 1 0 200px",
            flex_item_style(1.0, 0.0, basis_px(200.0)),
            50.0,
            (200.0, 200.0),
        ),
    ] {
        assert_eq!(
            fit_content_around(sized(style), content),
            expected,
            "{name}"
        );
    }
}

/// `flex-basis: content` sizes the item from its content (css-flexbox-1
/// §9.2.3 step E, `content` as `max-content`), never from its `width`: around
/// 50px of content a `width: 100px` item is 50 wide, and around 300px of
/// content an item that cannot shrink is 300 wide. Its contribution is still
/// its `width` (css-sizing-3 §5.2), which a flex base size from the content
/// neither caps nor floors, so the fit-content container is 100 each time.
/// Chrome's numbers.
#[test]
fn absolute_fit_content_between_insets_sizes_a_content_flex_basis_from_the_content() {
    let sized = |shrink: f32| TestStyle {
        size: Size::new(size_px(100.0), size_auto()),
        ..flex_item_style(0.0, shrink, basis_content())
    };
    for (name, style, content, expected) in [
        ("content 300", sized(1.0), 300.0, (100.0, 100.0)),
        ("content 50", sized(1.0), 50.0, (100.0, 50.0)),
        (
            "content 300, flex-shrink: 0",
            sized(0.0),
            300.0,
            (100.0, 300.0),
        ),
    ] {
        assert_eq!(fit_content_around(style, content), expected, "{name}");
    }
}

fn column_wrap(style: TestStyle) -> TestStyle {
    TestStyle {
        flex_direction: flex_direction::T::Column,
        flex_wrap: flex_wrap::T::WRAP,
        ..style
    }
}

fn two_tall_items(tree: &mut TestTree) -> Vec<TestId> {
    (0..2)
        .map(|_| {
            tree.push_leaf(
                TestStyle {
                    size: Size::new(size_px(100.0), size_px(400.0)),
                    ..TestStyle::default()
                },
                Size::new(100.0, 400.0),
                None,
            )
        })
        .collect()
}

/// A vertical main axis is the block axis, where css-sizing-3 §2.1 makes the
/// min-content size the max-content size, the content's height after layout:
/// a column `wrap` container with an automatic height takes its max-content
/// height however little is available, and its items share one line. Two
/// 100×400 items in 600px of available height make it 100×800 — fit-content
/// between insets, or `flex-start` in a row container. In a column container
/// it is an item whose §4.5 automatic minimum is that same 800, so it does
/// not shrink to the container's 600 unless `min-height: 0` lets it, when its
/// items break into two columns. Chrome's numbers.
#[test]
fn absolute_fit_content_between_insets_sizes_a_column_flex_container_at_its_max_content_height() {
    let one_column = |tree: &TestTree, items: &[TestId]| {
        assert_point(tree.layout(items[0]).location, Point::ZERO);
        assert_point(tree.layout(items[1]).location, Point::new(0.0, 400.0));
    };
    let fit = sized_auto_margin(StyleSize::FitContent, StyleSize::FitContent);
    let (tree, items, layout) = inset_zero_flex(column_wrap(fit), two_tall_items);
    assert_size(layout.size, Size::new(100.0, 800.0));
    assert_point(layout.location, Point::new(350.0, -100.0));
    one_column(&tree, &items);

    let mut tree = TestTree::default();
    let items = two_tall_items(&mut tree);
    let column = flex_container(&mut tree, column_wrap(TestStyle::default()), &items);
    let row = flex_container(
        &mut tree,
        TestStyle {
            align_items: self::items(AlignFlags::FLEX_START),
            ..TestStyle::default()
        },
        &[column],
    );
    definite_layout(&tree, row, 800.0, 600.0);
    assert_size(tree.layout(column).size, Size::new(100.0, 800.0));
    one_column(&tree, &items);

    for (min_height, height, second) in [
        (size_auto(), 800.0, Point::new(0.0, 400.0)),
        (size_px(0.0), 600.0, Point::new(400.0, 0.0)),
    ] {
        let mut tree = TestTree::default();
        let items = two_tall_items(&mut tree);
        let column = flex_container(
            &mut tree,
            column_wrap(TestStyle {
                min_size: Size::new(size_auto(), min_height),
                ..TestStyle::default()
            }),
            &items,
        );
        let outer = flex_container(
            &mut tree,
            TestStyle {
                flex_direction: flex_direction::T::Column,
                ..TestStyle::default()
            },
            &[column],
        );
        definite_layout(&tree, outer, 800.0, 600.0);
        assert_size(tree.layout(column).size, Size::new(800.0, height));
        assert_point(tree.layout(items[1]).location, second);
    }
}

/// A fit-content flex container whose content fits takes its max-content
/// main size, so it measures no item for anything else. The container runs
/// twice, its height unknown and then known, and each run asks the rigid
/// item for its min-content width (its automatic minimum) and nothing more:
/// its contributions are its own 300 whatever its content, since it neither
/// grows nor shrinks. The content-sized item with `min-width: 0` shares its
/// max-content probe with its flex base size, one per run, and is never
/// asked for its min-content size. Overflowing the stretch-fit size, the
/// first run, the one that decides the width, reads it after all.
#[test]
fn fit_content_flex_container_probes_only_the_sizes_it_reads() {
    let probes = |tree: &TestTree, id: TestId, available: AvailableSpace| {
        tree.measure_inputs(id)
            .iter()
            .filter(|input| input.available_space.width == available)
            .count()
    };
    for (content_width, container_width, min_content_probes) in
        [(200.0, 500.0, 0), (600.0, 800.0, 1)]
    {
        let fit = sized_auto_margin(StyleSize::FitContent, StyleSize::FitContent);
        let (tree, items, layout) = inset_zero_flex(fit, |tree| {
            let content = tree.push_intrinsic_leaf(
                TestStyle {
                    min_size: Size::new(size_px(0.0), size_auto()),
                    ..TestStyle::default()
                },
                Size::new(100.0, 100.0),
                Size::new(content_width, 100.0),
            );
            vec![rigid_item(tree, 300.0, 100.0), content]
        });
        assert_close(layout.size.width, container_width);
        let runs = probes(&tree, items[0], AvailableSpace::MinContent);
        assert_eq!(runs, 2);
        assert_eq!(probes(&tree, items[0], AvailableSpace::MaxContent), 0);
        assert_eq!(probes(&tree, items[1], AvailableSpace::MaxContent), runs);
        assert_eq!(
            probes(&tree, items[1], AvailableSpace::MinContent),
            min_content_probes,
            "content {content_width}"
        );
    }

    // A definite preferred size is the contribution whatever the content
    // (css-sizing-3 §5.2), so `flex: 1 1 auto; width: 100px` is measured for
    // nothing with `min-width: 0` and only for its automatic minimum without.
    // A non-growable item's definite flex basis caps its content (§9.9.3),
    // which takes one max-content probe, in the run that decides the width,
    // and nothing else: `flex: 0 1 400px` around 50px, beside its automatic
    // minimum's min-content probe in each run.
    let fit = sized_auto_margin(StyleSize::FitContent, StyleSize::FitContent);
    let (tree, items, layout) = inset_zero_flex(fit, |tree| {
        let sized = |min_width: StyleSize| TestStyle {
            size: Size::new(size_px(100.0), size_auto()),
            min_size: Size::new(min_width, size_auto()),
            ..flex_item_style(1.0, 1.0, basis_auto())
        };
        vec![
            rigid_item(tree, 300.0, 100.0),
            content_item(tree, sized(size_px(0.0)), 300.0),
            content_item(tree, sized(size_auto()), 300.0),
            content_item(tree, flex_item_style(0.0, 1.0, basis_px(400.0)), 50.0),
        ]
    });
    assert_close(layout.size.width, 550.0);
    // Chrome's widths: 550 is less than the 900 of hypothetical main sizes.
    for (item, width) in items.iter().zip([300.0, 30.0, 100.0, 120.0]) {
        assert_close(tree.layout(*item).size.width, width);
    }
    let runs = probes(&tree, items[0], AvailableSpace::MinContent);
    assert_eq!(runs, 2);
    for (item, min_content, max_content) in [(1, 0, 0), (2, runs, 0), (3, runs, 1)] {
        assert_eq!(
            [
                probes(&tree, items[item], AvailableSpace::MinContent),
                probes(&tree, items[item], AvailableSpace::MaxContent)
            ],
            [min_content, max_content],
            "item {item}"
        );
    }
}

/// `min-content` and `max-content` on a multi-line container keep their own
/// constraint: the largest item's width on one line each, or every item on
/// one line, whatever the stretch-fit size.
#[test]
fn absolute_intrinsic_keywords_between_insets_size_a_multi_line_flex_container() {
    let (_, _, min) = inset_zero_flex(
        wrapping(sized_auto_margin(size_min_content(), StyleSize::FitContent)),
        two_rigid_items(500.0),
    );
    assert_size(min.size, Size::new(500.0, 200.0));
    assert_point(min.location, Point::new(150.0, 200.0));

    let (_, _, max) = inset_zero_flex(
        wrapping(sized_auto_margin(size_max_content(), StyleSize::FitContent)),
        two_rigid_items(500.0),
    );
    assert_size(max.size, Size::new(1000.0, 100.0));
    assert_point(max.location, Point::new(0.0, 250.0));
}

/// A flex container whose main size its parent already knows is not
/// fit-content sized: stretched by a column parent to 300, it stays 300 and
/// its paragraph overflows at its 500 min-content width. Not stretched
/// (`align-self: flex-start`), the same container is fit-content sized in
/// the 300 available (css-flexbox-1 §9.4 step 7): its min-content 500.
#[test]
fn in_flow_flex_container_fit_content_follows_its_parents_stretch() {
    for (align_self, container_width) in [
        (AlignFlags::STRETCH, 300.0),
        (AlignFlags::FLEX_START, 500.0),
    ] {
        let mut tree = TestTree::default();
        let item = paragraph(&mut tree);
        let row = flex_container(
            &mut tree,
            TestStyle {
                align_self: self_align(align_self),
                ..TestStyle::default()
            },
            &[item],
        );
        let root = flex_container(
            &mut tree,
            TestStyle {
                flex_direction: flex_direction::T::Column,
                ..TestStyle::default()
            },
            &[row],
        );
        definite_layout(&tree, root, 300.0, 400.0);
        assert_size(tree.layout(row).size, Size::new(container_width, 200.0));
        assert_size(tree.layout(item).size, Size::new(500.0, 200.0));
    }
}

#[test]
fn cyclic_percentage_item_margin_resolves_after_intrinsic_container_sizing() {
    let mut tree = TestTree::default();
    let mut child_style = fixed_leaf_style(100.0, 10.0);
    child_style.flex_basis = basis_px(10.0);
    child_style.margin.left = margin_pct(0.1);
    let child = tree.push_leaf(child_style, Size::new(100.0, 10.0), None);
    let root = flex_container(
        &mut tree,
        TestStyle {
            flex_direction: flex_direction::T::Column,
            ..TestStyle::default()
        },
        &[child],
    );

    let output = perform_layout(
        &tree,
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
            size: Size::new(size_px(120.0), size_px(80.0)),
            flex_basis: basis_px(120.0),
            flex_shrink: nn(0.0),
            margin: Edges::uniform(margin_auto()),
            ..TestStyle::default()
        },
        Size::new(120.0, 80.0),
        None,
    );
    let root = flex_container(
        &mut tree,
        TestStyle {
            justify_content: content(AlignFlags::CENTER),
            ..TestStyle::default()
        },
        &[item],
    );

    definite_layout(&tree, root, 100.0, 50.0);
    let layout = tree.layout(item);
    assert_point(layout.location, Point::new(-10.0, 0.0));
    assert_close(layout.margin.left, 0.0);
    assert_close(layout.margin.right, 0.0);
    assert_close(layout.margin.top, 0.0);
    assert_close(layout.margin.bottom, -30.0);
}

#[test]
fn intrinsic_item_keywords_resolve_preferred_min_max_and_fit_content_basis() {
    let mut tree = TestTree::default();
    let constrained = tree.push_intrinsic_leaf(
        TestStyle {
            size: Size::new(size_max_content(), size_px(10.0)),
            min_size: Size::new(size_fit_content_px(30.0), size_auto()),
            max_size: Size::new(max_min_content(), max_none()),
            ..TestStyle::default()
        },
        Size::new(20.0, 10.0),
        Size::new(80.0, 10.0),
    );
    let fit_basis = tree.push_intrinsic_leaf(
        TestStyle {
            min_size: Size::new(size_px(0.0), size_auto()),
            flex_basis: basis_fit_content_px(50.0),
            ..TestStyle::default()
        },
        Size::new(20.0, 10.0),
        Size::new(80.0, 10.0),
    );
    let root = flex_container(&mut tree, TestStyle::default(), &[constrained, fit_basis]);

    definite_layout(&tree, root, 200.0, 20.0);

    assert_close(tree.layout(constrained).size.width, 30.0);
    assert_close(tree.layout(fit_basis).size.width, 50.0);
}

#[test]
fn container_preferred_axes_clamp_with_minimum_precedence_and_content_mode_ignores_them() {
    let mut tree = TestTree::default();
    let root = flex_container(
        &mut tree,
        TestStyle {
            size: Size::new(size_px(300.0), size_px(100.0)),
            min_size: Size::new(size_px(400.0), size_px(120.0)),
            max_size: Size::new(max_px(200.0), max_px(80.0)),
            ..TestStyle::default()
        },
        &[],
    );

    let inherent = perform_layout(
        &tree,
        root,
        Size::NONE,
        Size::new(AvailableSpace::MaxContent, AvailableSpace::MaxContent),
    );
    assert_size(inherent.size, Size::new(400.0, 120.0));

    let mut content_input = LayoutInput::commit(
        Size::NONE,
        Size::NONE,
        Size::MAX_CONTENT,
        Size::new(false, false),
    );
    content_input.sizing_mode = SizingMode::IgnoreSizeStyles;
    let content = tree.compute_layout(root, content_input);
    assert_size(content.size, Size::ZERO);
}

#[test]
fn content_basis_and_intrinsic_width_keywords_set_main_sizes() {
    use stylo::values::computed::Size as StyleSize;

    let width_of = |style: TestStyle| -> f32 {
        let mut tree = TestTree::default();
        let item = tree.push_intrinsic_leaf(style, Size::new(30.0, 10.0), Size::new(90.0, 10.0));
        let root = flex_container(&mut tree, TestStyle::default(), &[item]);
        definite_layout(&tree, root, 300.0, 50.0);
        tree.layout(item).size.width
    };

    assert_close(
        width_of(TestStyle {
            flex_basis: basis_content(),
            ..TestStyle::default()
        }),
        90.0,
    );
    assert_close(
        width_of(TestStyle {
            size: Size::new(size_min_content(), size_auto()),
            ..TestStyle::default()
        }),
        30.0,
    );
    assert_close(
        width_of(TestStyle {
            size: Size::new(size_max_content(), size_auto()),
            ..TestStyle::default()
        }),
        90.0,
    );
    for keyword in [
        StyleSize::FitContent,
        StyleSize::Stretch,
        StyleSize::WebkitFillAvailable,
    ] {
        assert_close(
            width_of(TestStyle {
                size: Size::new(keyword, size_auto()),
                ..TestStyle::default()
            }),
            90.0,
        );
    }
}

#[test]
fn indefinite_bases_fall_back_to_the_probe_extreme() {
    let mut tree = TestTree::default();
    let calc_item = tree.push_intrinsic_leaf(
        TestStyle {
            size: Size::new(size_calc(20.0, 0.1), size_auto()),
            ..TestStyle::default()
        },
        Size::new(30.0, 10.0),
        Size::new(90.0, 10.0),
    );
    let root = flex_container(&mut tree, TestStyle::default(), &[calc_item]);
    let output = measure_layout(
        &tree,
        root,
        Size::NONE,
        Size::new(AvailableSpace::MaxContent, AvailableSpace::MaxContent),
    );
    assert_close(output.size.width, 90.0);

    let mut tree = TestTree::default();
    let auto_item = tree.push_intrinsic_leaf(
        TestStyle::default(),
        Size::new(30.0, 10.0),
        Size::new(90.0, 10.0),
    );
    let root = flex_container(&mut tree, TestStyle::default(), &[auto_item]);
    let output = measure_layout(
        &tree,
        root,
        Size::NONE,
        Size::new(AvailableSpace::MinContent, AvailableSpace::MinContent),
    );
    assert_close(output.size.width, 30.0);
}

#[test]
fn max_width_intrinsic_keywords_clamp_growth() {
    let clamped_width = |max_size: MaxSize| -> f32 {
        let mut tree = TestTree::default();
        let item = tree.push_intrinsic_leaf(
            TestStyle {
                flex_grow: nn(1.0),
                max_size: Size::new(max_size, max_none()),
                ..TestStyle::default()
            },
            Size::new(30.0, 10.0),
            Size::new(90.0, 10.0),
        );
        let root = flex_container(&mut tree, TestStyle::default(), &[item]);
        definite_layout(&tree, root, 300.0, 50.0);
        tree.layout(item).size.width
    };

    assert_close(clamped_width(max_max_content()), 90.0);
    assert_close(clamped_width(max_min_content()), 30.0);
    assert_close(clamped_width(max_fit_content_px(50.0)), 50.0);
}

#[test]
fn main_end_auto_margin_absorbs_free_space() {
    let mut tree = TestTree::default();
    let mut style = fixed_leaf_style(50.0, 20.0);
    style.margin.right = margin_auto();
    let item = tree.push_leaf(style, Size::new(50.0, 20.0), None);
    let root = flex_container(
        &mut tree,
        TestStyle {
            justify_content: content(AlignFlags::CENTER),
            ..TestStyle::default()
        },
        &[item],
    );

    definite_layout(&tree, root, 200.0, 20.0);

    assert_close(tree.layout(item).location.x, 0.0);
    assert_close(tree.layout(item).margin.right, 150.0);
}

#[test]
fn zero_basis_items_freeze_under_gap_overflow() {
    let mut tree = TestTree::default();
    let first = tree.push_leaf(fixed_leaf_style(0.0, 10.0), Size::ZERO, None);
    let second = tree.push_leaf(fixed_leaf_style(0.0, 10.0), Size::ZERO, None);
    let root = flex_container(
        &mut tree,
        TestStyle {
            gap: Size::new(gap_px(50.0), gap_normal()),
            ..TestStyle::default()
        },
        &[first, second],
    );

    definite_layout(&tree, root, 30.0, 20.0);

    assert_close(tree.layout(first).size.width, 0.0);
    assert_close(tree.layout(second).size.width, 0.0);
    assert_close(tree.layout(first).location.x, 0.0);
    assert_close(tree.layout(second).location.x, 50.0);
}

#[test]
fn intrinsic_keyword_flex_bases_use_contributions() {
    let width_of = |basis: FlexBasis| -> f32 {
        let mut tree = TestTree::default();
        let item = tree.push_intrinsic_leaf(
            TestStyle {
                flex_basis: basis,
                ..TestStyle::default()
            },
            Size::new(30.0, 10.0),
            Size::new(90.0, 10.0),
        );
        let root = flex_container(&mut tree, TestStyle::default(), &[item]);
        definite_layout(&tree, root, 300.0, 50.0);
        tree.layout(item).size.width
    };

    assert_close(width_of(FlexBasis::Size(size_min_content())), 30.0);
    assert_close(width_of(FlexBasis::Size(size_max_content())), 90.0);
}

/// css-overflow-3 §3.3 scrollable overflow: a scroll container's area takes
/// in "the margin areas of grid item and flex item boxes for which the box
/// establishes a containing block". Three unshrinkable 60px items in a 100px
/// container, the last one (or the first) carrying `margins`.
fn scrolling_flex_row(
    overflow: Overflow,
    first_margin: Edges<Margin>,
    last_margin: Edges<Margin>,
) -> (TestTree, TestId, [TestId; 3]) {
    let mut tree = TestTree::default();
    let mut item = |margin: Edges<Margin>| {
        let style = TestStyle {
            flex_shrink: nn(0.0),
            margin,
            ..fixed_leaf_style(60.0, 20.0)
        };
        tree.push_leaf(style, Size::new(60.0, 20.0), None)
    };
    let items = [
        item(first_margin),
        item(Edges::uniform(margin_px(0.0))),
        item(last_margin),
    ];
    let root = flex_container(
        &mut tree,
        TestStyle {
            overflow: Point::new(overflow, overflow),
            ..TestStyle::default()
        },
        &items,
    );
    (tree, root, items)
}

fn right_margin(value: f32) -> Edges<Margin> {
    Edges {
        right: margin_px(value),
        ..Edges::uniform(margin_px(0.0))
    }
}

#[test]
fn a_scroll_container_counts_the_last_flex_item_end_margin() {
    let (tree, root, _) = scrolling_flex_row(
        Overflow::Hidden,
        Edges::uniform(margin_px(0.0)),
        right_margin(20.0),
    );
    let output = definite_layout(&tree, root, 100.0, 20.0);
    // Border boxes end at 180; the last margin area at 200.
    assert_size(output.content_size, Size::new(200.0, 20.0));

    let mut tree = TestTree::default();
    let mut item = |bottom: f32| {
        let style = TestStyle {
            flex_shrink: nn(0.0),
            margin: Edges {
                bottom: margin_px(bottom),
                ..Edges::uniform(margin_px(0.0))
            },
            flex_basis: basis_px(60.0),
            ..fixed_leaf_style(20.0, 60.0)
        };
        tree.push_leaf(style, Size::new(20.0, 60.0), None)
    };
    let items = [item(0.0), item(0.0), item(20.0)];
    let root = flex_container(
        &mut tree,
        TestStyle {
            flex_direction: flex_direction::T::Column,
            overflow: Point::new(Overflow::Hidden, Overflow::Hidden),
            ..TestStyle::default()
        },
        &items,
    );
    let output = definite_layout(&tree, root, 20.0, 100.0);
    assert_size(output.content_size, Size::new(20.0, 200.0));
}

#[test]
fn a_first_item_start_margin_reaches_scrollable_overflow_through_its_location() {
    let left = Edges {
        left: margin_px(30.0),
        ..Edges::uniform(margin_px(0.0))
    };
    let (tree, root, items) = scrolling_flex_row(
        Overflow::Hidden,
        left.clone(),
        Edges::uniform(margin_px(0.0)),
    );
    let output = definite_layout(&tree, root, 100.0, 20.0);
    assert_close(tree.layout(items[0]).location.x, 30.0);
    assert_size(output.content_size, Size::new(210.0, 20.0));

    let (tree, root, _) = scrolling_flex_row(Overflow::Hidden, left, right_margin(20.0));
    let output = definite_layout(&tree, root, 100.0, 20.0);
    assert_size(output.content_size, Size::new(230.0, 20.0));
}

#[test]
fn a_negative_end_margin_does_not_shrink_scrollable_overflow() {
    // The area is a union with the border box: a margin area ending at 170
    // leaves the border box's 180.
    let (tree, root, _) = scrolling_flex_row(
        Overflow::Hidden,
        Edges::uniform(margin_px(0.0)),
        right_margin(-10.0),
    );
    let output = definite_layout(&tree, root, 100.0, 20.0);
    assert_size(output.content_size, Size::new(180.0, 20.0));
}

#[test]
fn an_item_margin_area_and_its_visible_overflow_are_a_union() {
    // One 60px item whose own content reaches 100px: the margin area and the
    // content overflow are each a rectangle of the union, not a sum.
    let content_with = |margin: f32| {
        let mut tree = TestTree::default();
        let wide = tree.push_leaf(
            TestStyle {
                flex_shrink: nn(0.0),
                ..fixed_leaf_style(100.0, 20.0)
            },
            Size::new(100.0, 20.0),
            None,
        );
        let item = flex_container(
            &mut tree,
            TestStyle {
                flex_shrink: nn(0.0),
                margin: right_margin(margin),
                ..fixed_leaf_style(60.0, 20.0)
            },
            &[wide],
        );
        let root = flex_container(
            &mut tree,
            TestStyle {
                overflow: Point::new(Overflow::Hidden, Overflow::Hidden),
                ..TestStyle::default()
            },
            &[item],
        );
        definite_layout(&tree, root, 50.0, 20.0).content_size.width
    };
    assert_close(content_with(20.0), 100.0);
    assert_close(content_with(50.0), 110.0);
}

#[test]
fn an_absolutely_positioned_child_counts_its_border_box_only() {
    let mut tree = TestTree::default();
    let absolute = tree.push_leaf(
        TestStyle {
            position: PositionProperty::Absolute,
            inset: Edges {
                left: inset_px(150.0),
                top: inset_px(0.0),
                ..Edges::uniform(inset_auto())
            },
            margin: Edges {
                right: margin_px(30.0),
                bottom: margin_px(30.0),
                ..Edges::uniform(margin_px(0.0))
            },
            ..fixed_leaf_style(20.0, 20.0)
        },
        Size::new(20.0, 20.0),
        None,
    );
    let root = flex_container(
        &mut tree,
        TestStyle {
            overflow: Point::new(Overflow::Hidden, Overflow::Hidden),
            ..TestStyle::default()
        },
        &[absolute],
    );
    let output = definite_layout(&tree, root, 100.0, 100.0);
    assert_size(output.content_size, Size::new(170.0, 100.0));
}

#[test]
fn a_non_scrolling_container_keeps_its_item_margins_to_itself() {
    // css-overflow-3 §3.3 defines the area for any box, so read literally a
    // `visible` flex container would carry the 20px margin up to the scroll
    // container above it (220). No browser does (csswg-drafts#9194): an item
    // margin counts only in its own container's area when that container
    // scrolls, and the engine follows them.
    let (mut tree, inner, _) = scrolling_flex_row(
        Overflow::Visible,
        Edges::uniform(margin_px(0.0)),
        right_margin(20.0),
    );
    let inner_style = TestStyle {
        flex_shrink: nn(0.0),
        margin: right_margin(10.0),
        ..fixed_leaf_style(100.0, 20.0)
    };
    tree.source_node_mut(inner).style = inner_style;
    let root = flex_container(
        &mut tree,
        TestStyle {
            overflow: Point::new(Overflow::Hidden, Overflow::Hidden),
            ..TestStyle::default()
        },
        &[inner],
    );
    let output = definite_layout(&tree, root, 100.0, 20.0);
    assert_size(tree.layout(inner).content_size, Size::new(180.0, 20.0));
    // The inner container's own 10px margin ends at 110, inside its 180.
    assert_size(output.content_size, Size::new(180.0, 20.0));
}

/// `content_size` is the union of the padding box and the items' reach,
/// in border-box coordinates: an empty bordered scroll container's ends at
/// its padding box's far edge, not its border box's; an item that ends
/// exactly at the padding edge adds nothing; one past it by less than the
/// far border still shows, which a border-box floor would have hidden.
#[test]
fn content_size_floors_at_the_padding_box_not_the_border_box() {
    let container_style = || TestStyle {
        padding: Edges::uniform(npx(5.0)),
        border: Edges::uniform(border_px(10.0)),
        overflow: Point::new(Overflow::Hidden, Overflow::Hidden),
        ..TestStyle::default()
    };
    let item_style = |width: f32| TestStyle {
        flex_shrink: nn(0.0),
        ..fixed_leaf_style(width, 60.0)
    };

    let mut empty = TestTree::default();
    let root = flex_container(&mut empty, container_style(), &[]);
    let output = definite_layout(&empty, root, 100.0, 100.0);
    assert_size(output.size, Size::new(100.0, 100.0));
    assert_size(output.content_size, Size::new(90.0, 90.0));

    // Content origin 15: a 75px item ends at 90, the padding edge.
    let mut fitted = TestTree::default();
    let item = fitted.push_leaf(item_style(75.0), Size::new(75.0, 60.0), None);
    let root = flex_container(&mut fitted, container_style(), &[item]);
    let output = definite_layout(&fitted, root, 100.0, 100.0);
    assert_point(fitted.layout(item).location, Point::new(15.0, 15.0));
    assert_size(output.content_size, Size::new(90.0, 90.0));

    // A 78px item ends at 93: 3px past the padding edge, inside the border.
    let mut overflowing = TestTree::default();
    let item = overflowing.push_leaf(item_style(78.0), Size::new(78.0, 60.0), None);
    let root = flex_container(&mut overflowing, container_style(), &[item]);
    let output = definite_layout(&overflowing, root, 100.0, 100.0);
    assert_size(output.content_size, Size::new(93.0, 90.0));
}

/// A leaf's `content_size` ends at its padding edge too: its measured
/// contents plus the end padding, never the far border.
#[test]
fn leaf_content_size_ends_at_its_padding_edge() {
    let style = TestStyle {
        max_size: Size::new(max_px(70.0), max_none()),
        padding: Edges::uniform(npx(5.0)),
        border: Edges::uniform(border_px(10.0)),
        ..TestStyle::default()
    };
    let output = compute_leaf_layout(
        LayoutInput::commit(
            Size::NONE,
            Size::new(Some(500.0), Some(500.0)),
            Size::new(
                AvailableSpace::Definite(500.0),
                AvailableSpace::Definite(500.0),
            ),
            Size::new(false, false),
        ),
        &style,
        NaturalSize::from_size(Size::new(120.0, 20.0)),
    );
    // A 70px content box under 5px padding and a 10px border: 100 wide, and
    // the 20px contents make it 50 tall.
    assert_size(output.size, Size::new(100.0, 50.0));
    // Width: the 120px contents from the content origin at 15 reach 135,
    // plus the end padding, 140 — not the measured border box's 150.
    // Height: nothing overflows, so the padding edge, 40.
    assert_size(output.content_size, Size::new(140.0, 40.0));
}
