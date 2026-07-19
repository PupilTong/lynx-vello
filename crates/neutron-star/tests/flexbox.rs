//! Spec-focused flexbox integration tests over a plain `Vec`-backed host.
//!
//! The host deliberately has no styling engine. `TestStyle` is already the
//! computed style view — stylo computed values served straight from struct
//! fields — and leaf measurement is a deterministic intrinsic size stored
//! alongside each node.

mod support;

use neutron_star::compute::{
    FnLeafMeasurer, LeafMetrics, compute_absolute_layout, compute_leaf_layout,
};
use neutron_star::prelude::*;
use stylo::computed_values::{box_sizing, direction, flex_direction, flex_wrap};
use stylo::values::computed::{Display, Overflow, PositionProperty};
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
        flex_wrap: flex_wrap::T::Wrap,
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
                flex_wrap: flex_wrap::T::Wrap,
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
                flex_wrap: flex_wrap::T::Wrap,
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
                flex_wrap: flex_wrap::T::Wrap,
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
                flex_wrap: flex_wrap::T::Wrap,
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
fn measure_goal_does_not_write_durable_layouts() {
    let mut tree = TestTree::default();
    let first = fixed_leaf(&mut tree, 40.0, 10.0);
    let second = fixed_leaf(&mut tree, 40.0, 20.0);
    let root = flex_container(&mut tree, TestStyle::default(), &[first, second]);
    let mut sentinel = Layout::default();
    sentinel.location = Point::new(123.0, 456.0);
    sentinel.size = Size::new(7.0, 8.0);
    tree.session_node(first).layout.set(sentinel);
    tree.session_node(second).layout.set(sentinel);
    tree.session_node(root).layout.set(sentinel);

    let output = tree.compute_child_layout(
        root,
        LayoutInput::compute_size(
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

    let measured = tree.compute_child_layout(
        leaf,
        LayoutInput::compute_size(known, parent, available, RequestedAxis::Horizontal),
    );
    assert_size(measured.size, Size::new(40.0, 20.0));
    assert_eq!(tree.leaf_measure_calls.get(), 0);

    let _ = tree.compute_child_layout(
        leaf,
        LayoutInput::compute_size(known, parent, available, RequestedAxis::Both),
    );
    assert_eq!(tree.leaf_measure_calls.get(), 1);

    let _ = tree.compute_child_layout(leaf, LayoutInput::perform_layout(known, parent, available));
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
            flex_wrap::T::Wrap,
            [Point::new(50.0, 0.0), Point::new(40.0, 0.0)],
        ),
        (
            flex_wrap::T::WrapReverse,
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

#[test]
fn indefinite_percentage_flex_basis_falls_back_to_content_not_width() {
    let mut tree = TestTree::default();
    let item = tree.push_leaf(
        TestStyle {
            size: Size::new(size_px(50.0), size_px(10.0)),
            flex_basis: basis_pct(0.5),
            overflow: Point::new(Overflow::Hidden, Overflow::Hidden),
            ..TestStyle::default()
        },
        Size::new(80.0, 10.0),
        None,
    );
    let root = flex_container(&mut tree, TestStyle::default(), &[item]);

    let output = perform_layout(&tree, root, Size::NONE, Size::MAX_CONTENT);
    assert_close(output.size.width, 80.0);
    assert_close(tree.layout(item).size.width, 80.0);
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
            flex_wrap: flex_wrap::T::Wrap,
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
                flex_wrap: flex_wrap::T::WrapReverse,
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
            flex_wrap: flex_wrap::T::Wrap,
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
fn leaf_measurement_reports_baselines_for_a_fully_sized_box() {
    let style = fixed_leaf_style(100.0, 20.0);
    let mut measurer = FnLeafMeasurer::new(|_input| {
        LeafMetrics::new(Size::new(100.0, 20.0)).with_first_baselines(Point::new(None, Some(15.0)))
    });
    let output = compute_leaf_layout(
        LayoutInput::perform_layout(Size::NONE, Size::NONE, Size::MAX_CONTENT),
        &style,
        &mut measurer,
    );
    assert_size(output.size, Size::new(100.0, 20.0));
    assert_size(output.content_size, Size::new(100.0, 20.0));
    assert_eq!(output.first_baselines.y, Some(15.0));
}

#[test]
fn leaf_max_width_constrains_measurement_and_preserves_overflow_extent() {
    let style = TestStyle {
        max_size: Size::new(max_px(100.0), max_none()),
        padding: Edges::uniform(npx(10.0)),
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

    let layout = compute_absolute_layout(tree.node(child), Size::new(100.0, 100.0), Point::ZERO);
    assert_size(layout.size, Size::new(160.0, 80.0));
    assert_point(layout.location, Point::new(0.0, 10.0));
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

    // CSS minimums take precedence over a smaller maximum.
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

    let mut content_input = LayoutInput::perform_layout(Size::NONE, Size::NONE, Size::MAX_CONTENT);
    content_input.sizing_mode = SizingMode::ContentSize;
    let content = tree.compute_child_layout(root, content_input);
    assert_size(content.size, Size::ZERO);
}
