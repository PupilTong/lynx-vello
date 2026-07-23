//! Starlight relative-layout conformance tests over a plain `Vec` host.

mod support;

use neutron_star::prelude::*;
use stylo::computed_values::{
    box_sizing, direction, relative_center, relative_layout_once, visibility,
};
use stylo::values::computed::lynx_layout::RelativeReference;
use stylo::values::computed::{Display, PositionProperty};
use support::*;

fn width_bounded_by_available(
    input: neutron_star::compute::LeafMeasureInput,
) -> neutron_star::compute::LeafMetrics {
    let width = match input.available_space.width {
        AvailableSpace::Definite(width) => width.min(200.0),
        AvailableSpace::MinContent | AvailableSpace::MaxContent => 200.0,
    };
    neutron_star::compute::LeafMetrics::new(Size::new(width, 10.0))
}

fn intrinsic_width_bounded_by_available(
    input: neutron_star::compute::LeafMeasureInput,
) -> neutron_star::compute::LeafMetrics {
    let width = match input.available_space.width {
        AvailableSpace::Definite(width) => width.min(200.0),
        AvailableSpace::MinContent => 20.0,
        AvailableSpace::MaxContent => 200.0,
    };
    neutron_star::compute::LeafMetrics::new(Size::new(width, 10.0))
}

fn id(value: i32) -> RelativeReference {
    value
}

fn relative_leaf_style(width: f32, height: f32, relative_id: i32) -> TestStyle {
    TestStyle {
        size: Size::new(size_px(width), size_px(height)),
        relative_id: id(relative_id),
        ..TestStyle::default()
    }
}

fn relative_leaf(tree: &mut TestTree, width: f32, height: f32, relative_id: i32) -> TestId {
    tree.push_leaf(
        relative_leaf_style(width, height, relative_id),
        Size::new(width, height),
        None,
    )
}

fn dependency_cycle_fixture(
    layout_once: relative_layout_once::T,
) -> (TestTree, TestId, TestId, TestId) {
    let mut tree = TestTree::default();
    let mut a_style = relative_leaf_style(10.0, 10.0, 1);
    a_style.relative_adjacent.right = id(2);
    let a = tree.push_leaf(a_style, Size::new(10.0, 10.0), None);
    let mut b_style = relative_leaf_style(10.0, 10.0, 2);
    b_style.relative_adjacent.bottom = id(1);
    let b = tree.push_leaf(b_style, Size::new(10.0, 10.0), None);
    let root = relative_container(
        &mut tree,
        TestStyle {
            relative_layout_once: layout_once,
            ..TestStyle::default()
        },
        &[a, b],
    );
    (tree, root, a, b)
}

#[test]
fn parent_alignment_and_sibling_adjacency_use_physical_margin_edges() {
    let mut tree = TestTree::default();
    let mut anchor_style = relative_leaf_style(10.0, 10.0, 1);
    anchor_style.relative_align.left = RELATIVE_PARENT;
    anchor_style.relative_align.top = RELATIVE_PARENT;
    let anchor = tree.push_leaf(anchor_style, Size::new(10.0, 10.0), None);

    let mut follower_style = relative_leaf_style(15.0, 20.0, 2);
    follower_style.relative_adjacent.right = id(1);
    follower_style.relative_adjacent.bottom = id(1);
    let follower = tree.push_leaf(follower_style, Size::new(15.0, 20.0), None);

    let mut trailing_style = relative_leaf_style(10.0, 10.0, 3);
    trailing_style.relative_align.right = RELATIVE_PARENT;
    trailing_style.relative_align.bottom = RELATIVE_PARENT;
    let trailing = tree.push_leaf(trailing_style, Size::new(10.0, 10.0), None);
    let root = relative_container(
        &mut tree,
        TestStyle::default(),
        &[anchor, follower, trailing],
    );

    definite_layout(&tree, root, 100.0, 80.0);

    assert_point(tree.layout(anchor).location, Point::new(0.0, 0.0));
    assert_point(tree.layout(follower).location, Point::new(10.0, 10.0));
    assert_point(tree.layout(trailing).location, Point::new(90.0, 70.0));
}

#[test]
fn alignment_precedes_adjacency_for_the_same_side() {
    let mut tree = TestTree::default();
    let mut anchor_style = relative_leaf_style(10.0, 10.0, 1);
    anchor_style.relative_align.right = RELATIVE_PARENT;
    let anchor = tree.push_leaf(anchor_style, Size::new(10.0, 10.0), None);

    let mut child_style = relative_leaf_style(10.0, 10.0, 2);
    child_style.relative_align.left = RELATIVE_PARENT;
    child_style.relative_adjacent.right = id(1);
    let child = tree.push_leaf(child_style, Size::new(10.0, 10.0), None);
    let root = relative_container(&mut tree, TestStyle::default(), &[anchor, child]);

    definite_layout(&tree, root, 100.0, 20.0);
    assert_close(tree.layout(child).location.x, 0.0);
}

#[test]
fn both_sides_refine_child_size_after_dependencies_are_positioned() {
    let mut tree = TestTree::default();
    let mut left_style = relative_leaf_style(10.0, 10.0, 1);
    left_style.relative_align.left = RELATIVE_PARENT;
    let left = tree.push_leaf(left_style, Size::new(10.0, 10.0), None);

    let mut right_style = relative_leaf_style(10.0, 10.0, 2);
    right_style.relative_align.right = RELATIVE_PARENT;
    let right = tree.push_leaf(right_style, Size::new(10.0, 10.0), None);

    let mut middle_style = TestStyle {
        relative_id: id(3),
        ..TestStyle::default()
    };
    middle_style.relative_adjacent.right = id(1);
    middle_style.relative_adjacent.left = id(2);
    let middle = tree.push_leaf(middle_style, Size::new(200.0, 10.0), None);
    let root = relative_container(&mut tree, TestStyle::default(), &[middle, right, left]);

    definite_layout(&tree, root, 100.0, 20.0);

    assert_close(tree.layout(middle).location.x, 10.0);
    assert_close(tree.layout(middle).size.width, 80.0);
}

#[test]
fn parent_double_alignment_subtracts_used_margins() {
    let mut tree = TestTree::default();
    let mut style = TestStyle {
        margin: Edges {
            left: margin_px(5.0),
            right: margin_px(7.0),
            top: margin_px(0.0),
            bottom: margin_px(0.0),
        },
        ..TestStyle::default()
    };
    style.relative_align.left = RELATIVE_PARENT;
    style.relative_align.right = RELATIVE_PARENT;
    let child = tree.push_leaf(style, Size::new(10.0, 10.0), None);
    let root = relative_container(&mut tree, TestStyle::default(), &[child]);

    definite_layout(&tree, root, 100.0, 20.0);

    assert_close(tree.layout(child).location.x, 5.0);
    assert_close(tree.layout(child).size.width, 88.0);
}

#[test]
fn duplicate_ids_resolve_to_the_last_ordered_relative_item() {
    let mut tree = TestTree::default();
    let first = relative_leaf(&mut tree, 10.0, 10.0, 7);
    let mut last_style = relative_leaf_style(10.0, 10.0, 7);
    last_style.relative_align.right = RELATIVE_PARENT;
    let last = tree.push_leaf(last_style, Size::new(10.0, 10.0), None);
    let mut follower_style = relative_leaf_style(10.0, 10.0, 8);
    follower_style.relative_adjacent.right = id(7);
    let follower = tree.push_leaf(follower_style, Size::new(10.0, 10.0), None);
    let root = relative_container(&mut tree, TestStyle::default(), &[first, last, follower]);

    definite_layout(&tree, root, 100.0, 20.0);

    assert_close(tree.layout(first).location.x, 0.0);
    assert_close(tree.layout(last).location.x, 90.0);
    assert_close(tree.layout(follower).location.x, 100.0);
}

#[test]
fn order_sorting_is_stable_and_controls_id_lookup() {
    let mut tree = TestTree::default();
    let mut later_style = relative_leaf_style(10.0, 10.0, 7);
    later_style.order = 2;
    let later = tree.push_leaf(later_style, Size::new(10.0, 10.0), None);
    let mut earlier_style = relative_leaf_style(10.0, 10.0, 7);
    earlier_style.order = 1;
    earlier_style.relative_align.right = RELATIVE_PARENT;
    let earlier = tree.push_leaf(earlier_style, Size::new(10.0, 10.0), None);
    let mut follower_style = relative_leaf_style(10.0, 10.0, 8);
    follower_style.order = 3;
    follower_style.relative_adjacent.right = id(7);
    let follower = tree.push_leaf(follower_style, Size::new(10.0, 10.0), None);
    let root = relative_container(&mut tree, TestStyle::default(), &[later, earlier, follower]);

    definite_layout(&tree, root, 100.0, 20.0);

    assert_eq!(tree.layout(earlier).order, 0);
    assert_eq!(tree.layout(later).order, 1);
    assert_eq!(tree.layout(follower).order, 2);
    assert_close(tree.layout(follower).location.x, 10.0);
}

#[test]
fn parent_id_zero_is_reserved_and_never_identifies_an_item() {
    let mut tree = TestTree::default();
    let mut zero_style = relative_leaf_style(10.0, 10.0, 0);
    zero_style.relative_align.left = RELATIVE_PARENT;
    let zero = tree.push_leaf(zero_style, Size::new(10.0, 10.0), None);
    let mut follower_style = relative_leaf_style(10.0, 10.0, 2);
    follower_style.relative_adjacent.right = RELATIVE_PARENT;
    let follower = tree.push_leaf(follower_style, Size::new(10.0, 10.0), None);
    let root = relative_container(&mut tree, TestStyle::default(), &[zero, follower]);

    definite_layout(&tree, root, 100.0, 20.0);

    assert_close(tree.layout(zero).location.x, 0.0);
    assert_close(tree.layout(follower).location.x, 100.0);
}

#[test]
fn missing_reference_falls_back_to_the_other_property_or_default_bounds() {
    let mut tree = TestTree::default();
    let mut style = relative_leaf_style(10.0, 10.0, 1);
    style.relative_align.left = id(999);
    style.relative_adjacent.right = RELATIVE_PARENT;
    let fallback = tree.push_leaf(style, Size::new(10.0, 10.0), None);
    let root = relative_container(&mut tree, TestStyle::default(), &[fallback]);

    definite_layout(&tree, root, 100.0, 20.0);
    assert_close(tree.layout(fallback).location.x, 100.0);
}

#[test]
fn unconstrained_centering_is_axis_selective() {
    let mut tree = TestTree::default();
    let horizontal = tree.push_leaf(
        TestStyle {
            size: Size::new(size_px(20.0), size_px(10.0)),
            relative_center: relative_center::T::Horizontal,
            ..TestStyle::default()
        },
        Size::new(20.0, 10.0),
        None,
    );
    let both = tree.push_leaf(
        TestStyle {
            size: Size::new(size_px(10.0), size_px(20.0)),
            relative_center: relative_center::T::Both,
            ..TestStyle::default()
        },
        Size::new(10.0, 20.0),
        None,
    );
    let root = relative_container(&mut tree, TestStyle::default(), &[horizontal, both]);

    definite_layout(&tree, root, 100.0, 80.0);

    assert_point(tree.layout(horizontal).location, Point::new(40.0, 0.0));
    assert_point(tree.layout(both).location, Point::new(45.0, 30.0));
}

mod dependency_order {
    use super::*;

    #[test]
    fn one_pass_cycle_fallback_uses_combined_dependency_order() {
        let (once, root, a, b) = dependency_cycle_fixture(relative_layout_once::T::True);
        definite_layout(&once, root, 100.0, 100.0);
        assert_point(once.layout(a).location, Point::new(0.0, 0.0));
        assert_point(once.layout(b).location, Point::new(0.0, 10.0));
    }

    #[test]
    fn two_pass_cycle_fallback_orders_each_axis_independently() {
        let (twice, root, a, b) = dependency_cycle_fixture(relative_layout_once::T::False);
        definite_layout(&twice, root, 100.0, 100.0);
        assert_point(twice.layout(a).location, Point::new(10.0, 0.0));
        assert_point(twice.layout(b).location, Point::new(0.0, 10.0));
    }

    #[test]
    fn one_pass_processes_all_initial_roots_before_newly_ready_dependents() {
        let mut tree = TestTree::default();
        let mut dependent_style = relative_leaf_style(10.0, 10.0, 1);
        dependent_style.relative_adjacent.right = id(2);
        let dependent = tree.push_leaf(dependent_style, Size::new(10.0, 10.0), None);
        let centered_root = tree.push_leaf(
            TestStyle {
                size: Size::new(size_px(10.0), size_px(10.0)),
                relative_center: relative_center::T::Horizontal,
                ..TestStyle::default()
            },
            Size::new(10.0, 10.0),
            None,
        );
        let anchor = relative_leaf(&mut tree, 20.0, 10.0, 2);
        let root = relative_container(
            &mut tree,
            TestStyle {
                relative_layout_once: relative_layout_once::T::True,
                ..TestStyle::default()
            },
            &[dependent, centered_root, anchor],
        );

        let output = perform_layout(
            &tree,
            root,
            Size::NONE,
            Size::new(AvailableSpace::MaxContent, AvailableSpace::MaxContent),
        );

        assert_size(output.size, Size::new(30.0, 10.0));
        assert_point(tree.layout(centered_root).location, Point::new(-5.0, 0.0));
        assert_point(tree.layout(anchor).location, Point::new(-5.0, 0.0));
        assert_point(tree.layout(dependent).location, Point::new(15.0, 0.0));
    }
}

#[test]
fn self_cycles_and_duplicate_dependency_fields_terminate_deterministically() {
    let mut tree = TestTree::default();
    let mut style = relative_leaf_style(10.0, 10.0, 1);
    style.relative_align.left = id(1);
    style.relative_adjacent.right = id(1);
    style.relative_align.top = id(1);
    style.relative_adjacent.bottom = id(1);
    let child = tree.push_leaf(style, Size::new(10.0, 10.0), None);
    let root = relative_container(&mut tree, TestStyle::default(), &[child]);

    definite_layout(&tree, root, 100.0, 100.0);
    assert_point(tree.layout(child).location, Point::ZERO);
}

#[test]
fn wrap_content_uses_dependency_extent_and_container_surrounds() {
    let mut tree = TestTree::default();
    let anchor = relative_leaf(&mut tree, 10.0, 12.0, 1);
    let mut follower_style = relative_leaf_style(15.0, 8.0, 2);
    follower_style.relative_adjacent.right = id(1);
    let follower = tree.push_leaf(follower_style, Size::new(15.0, 8.0), None);
    let root = relative_container(
        &mut tree,
        TestStyle {
            padding: Edges::uniform(npx(5.0)),
            border: Edges::uniform(border_px(2.0)),
            ..TestStyle::default()
        },
        &[follower, anchor],
    );

    let output = perform_layout(
        &tree,
        root,
        Size::NONE,
        Size::new(AvailableSpace::MaxContent, AvailableSpace::MaxContent),
    );

    assert_size(output.size, Size::new(39.0, 26.0));
    assert_point(tree.layout(anchor).location, Point::new(7.0, 7.0));
    assert_point(tree.layout(follower).location, Point::new(17.0, 7.0));
}

#[test]
fn padding_and_border_translate_every_relative_position_from_the_content_origin() {
    let mut tree = TestTree::default();
    let anchor = relative_leaf(&mut tree, 20.0, 10.0, 10);

    let mut parent_end_style = relative_leaf_style(10.0, 8.0, 11);
    parent_end_style.relative_align.right = RELATIVE_PARENT;
    parent_end_style.relative_align.bottom = RELATIVE_PARENT;
    let parent_end = tree.push_leaf(parent_end_style, Size::new(10.0, 8.0), None);

    let centered = tree.push_leaf(
        TestStyle {
            size: Size::new(size_px(20.0), size_px(10.0)),
            relative_center: relative_center::T::Both,
            ..TestStyle::default()
        },
        Size::new(20.0, 10.0),
        None,
    );

    let mut sibling_after_style = relative_leaf_style(6.0, 4.0, 12);
    sibling_after_style.relative_adjacent.right = id(10);
    sibling_after_style.relative_adjacent.bottom = id(10);
    let sibling_after = tree.push_leaf(sibling_after_style, Size::new(6.0, 4.0), None);

    let root = relative_container(
        &mut tree,
        TestStyle {
            size: Size::new(size_px(100.0), size_px(80.0)),
            padding: Edges {
                left: npx(3.0),
                right: npx(7.0),
                top: npx(5.0),
                bottom: npx(11.0),
            },
            border: Edges {
                left: border_px(2.0),
                right: border_px(1.0),
                top: border_px(4.0),
                bottom: border_px(6.0),
            },
            ..TestStyle::default()
        },
        &[anchor, parent_end, centered, sibling_after],
    );

    let output = perform_layout(
        &tree,
        root,
        Size::NONE,
        Size::new(AvailableSpace::MaxContent, AvailableSpace::MaxContent),
    );

    assert_size(output.size, Size::new(113.0, 106.0));
    assert_point(tree.layout(anchor).location, Point::new(5.0, 9.0));
    assert_point(tree.layout(parent_end).location, Point::new(95.0, 81.0));
    assert_point(tree.layout(centered).location, Point::new(45.0, 44.0));
    assert_point(tree.layout(sibling_after).location, Point::new(25.0, 19.0));
}

#[test]
fn wrap_width_refresh_reuses_basis_independent_fixed_measurement() {
    let mut tree = TestTree::default();
    let child = fixed_leaf(&mut tree, 12.0, 10.0);
    let root = relative_container(
        &mut tree,
        TestStyle {
            relative_layout_once: relative_layout_once::T::False,
            ..TestStyle::default()
        },
        &[child],
    );

    let output = perform_layout(
        &tree,
        root,
        Size::NONE,
        Size::new(AvailableSpace::Definite(200.0), AvailableSpace::MaxContent),
    );

    assert_size(output.size, Size::new(12.0, 10.0));
    assert_eq!(tree.child_layout_calls.get(), 3);
}

#[test]
fn wrap_width_refresh_remeasures_fixed_item_when_double_anchors_tighten() {
    let mut tree = TestTree::default();
    let mut child_style = relative_leaf_style(12.0, 10.0, 1);
    child_style.relative_align.left = RELATIVE_PARENT;
    child_style.relative_align.right = RELATIVE_PARENT;
    let child = tree.push_leaf(child_style, Size::new(12.0, 10.0), None);
    let root = relative_container(
        &mut tree,
        TestStyle {
            min_size: Size::new(size_px(100.0), size_px(10.0)),
            relative_layout_once: relative_layout_once::T::False,
            ..TestStyle::default()
        },
        &[child],
    );

    let output = perform_layout(
        &tree,
        root,
        Size::NONE,
        Size::new(AvailableSpace::MaxContent, AvailableSpace::MaxContent),
    );

    assert_size(output.size, Size::new(100.0, 10.0));
    assert_close(tree.layout(child).size.width, 100.0);
}

#[test]
fn fixed_nested_item_reuse_preserves_grandchild_percentage_basis() {
    let mut tree = TestTree::default();
    let grandchild = tree.push_leaf(
        TestStyle {
            size: Size::new(size_pct(0.5), size_px(10.0)),
            ..TestStyle::default()
        },
        Size::new(8.0, 10.0),
        None,
    );
    let inner = relative_container(
        &mut tree,
        TestStyle {
            size: Size::new(size_px(40.0), size_px(20.0)),
            ..TestStyle::default()
        },
        &[grandchild],
    );
    let root = relative_container(&mut tree, TestStyle::default(), &[inner]);

    let output = perform_layout(
        &tree,
        root,
        Size::NONE,
        Size::new(AvailableSpace::MaxContent, AvailableSpace::MaxContent),
    );

    assert_size(output.size, Size::new(40.0, 20.0));
    assert_size(tree.layout(inner).size, Size::new(40.0, 20.0));
    assert_close(tree.layout(grandchild).size.width, 20.0);
}

#[test]
fn final_min_size_reanchors_parent_edges_in_two_pass_mode() {
    let mut tree = TestTree::default();
    let mut child_style = relative_leaf_style(10.0, 10.0, 1);
    child_style.relative_align.right = RELATIVE_PARENT;
    let child = tree.push_leaf(child_style, Size::new(10.0, 10.0), None);
    let root = relative_container(
        &mut tree,
        TestStyle {
            min_size: Size::new(size_px(100.0), size_px(20.0)),
            relative_layout_once: relative_layout_once::T::False,
            ..TestStyle::default()
        },
        &[child],
    );

    let output = perform_layout(
        &tree,
        root,
        Size::NONE,
        Size::new(AvailableSpace::MaxContent, AvailableSpace::MaxContent),
    );

    assert_size(output.size, Size::new(100.0, 20.0));
    assert_close(tree.layout(child).location.x, 90.0);
}

#[test]
fn contradictory_double_anchors_collapse_the_item_at_start() {
    let mut tree = TestTree::default();
    let mut left_style = relative_leaf_style(10.0, 10.0, 1);
    left_style.relative_align.left = RELATIVE_PARENT;
    let left = tree.push_leaf(left_style, Size::new(10.0, 10.0), None);
    let mut right_style = relative_leaf_style(10.0, 10.0, 2);
    right_style.relative_align.right = RELATIVE_PARENT;
    let right = tree.push_leaf(right_style, Size::new(10.0, 10.0), None);
    let mut child_style = relative_leaf_style(20.0, 10.0, 3);
    child_style.relative_adjacent.right = id(2);
    child_style.relative_adjacent.left = id(1);
    let child = tree.push_leaf(child_style, Size::new(20.0, 10.0), None);
    let root = relative_container(&mut tree, TestStyle::default(), &[child, left, right]);

    definite_layout(&tree, root, 100.0, 20.0);

    assert_close(tree.layout(child).location.x, 100.0);
    assert_close(tree.layout(child).size.width, 0.0);
}

#[test]
fn relative_position_insets_are_visual_only_for_sibling_dependencies() {
    let mut tree = TestTree::default();
    let mut anchor_style = relative_leaf_style(10.0, 10.0, 1);
    anchor_style.inset.left = inset_px(20.0);
    let anchor = tree.push_leaf(anchor_style, Size::new(10.0, 10.0), None);
    let mut follower_style = relative_leaf_style(10.0, 10.0, 2);
    follower_style.relative_adjacent.right = id(1);
    let follower = tree.push_leaf(follower_style, Size::new(10.0, 10.0), None);
    let root = relative_container(&mut tree, TestStyle::default(), &[follower, anchor]);

    definite_layout(&tree, root, 100.0, 20.0);

    assert_close(tree.layout(anchor).location.x, 20.0);
    assert_close(tree.layout(follower).location.x, 10.0);
}

#[test]
fn hidden_visibility_items_remain_in_the_constraint_graph() {
    let mut tree = TestTree::default();
    let mut hidden_style = relative_leaf_style(10.0, 10.0, 1);
    hidden_style.visibility = visibility::T::Hidden;
    let hidden = tree.push_leaf(hidden_style, Size::new(10.0, 10.0), None);
    let mut follower_style = relative_leaf_style(10.0, 10.0, 2);
    follower_style.visibility = visibility::T::Hidden;
    follower_style.relative_adjacent.right = id(1);
    let follower = tree.push_leaf(follower_style, Size::new(10.0, 10.0), None);
    let root = relative_container(&mut tree, TestStyle::default(), &[follower, hidden]);

    definite_layout(&tree, root, 100.0, 20.0);

    assert_close(tree.layout(hidden).location.x, 0.0);
    assert_close(tree.layout(follower).location.x, 10.0);
    assert_size(tree.layout(follower).size, Size::new(10.0, 10.0));
}

#[test]
fn display_none_is_zeroed_and_excluded_from_relative_ids() {
    let mut tree = TestTree::default();
    let mut hidden_style = relative_leaf_style(80.0, 50.0, 1);
    hidden_style.display = Display::None;
    let hidden = tree.push_leaf(hidden_style, Size::new(80.0, 50.0), None);
    let hidden_slots = tree.session_node(hidden);
    let mut hidden_layout = hidden_slots.layout.borrow().clone();
    hidden_layout.size = Size::new(80.0, 50.0);
    *hidden_slots.layout.borrow_mut() = hidden_layout;
    let mut child_style = relative_leaf_style(10.0, 10.0, 2);
    child_style.relative_adjacent.right = id(1);
    let child = tree.push_leaf(child_style, Size::new(10.0, 10.0), None);
    let root = relative_container(&mut tree, TestStyle::default(), &[hidden, child]);

    let output = perform_layout(
        &tree,
        root,
        Size::NONE,
        Size::new(AvailableSpace::MaxContent, AvailableSpace::MaxContent),
    );

    assert_size(output.size, Size::new(10.0, 10.0));
    assert_eq!(tree.layout(hidden), Layout::with_order(0));
    assert_point(tree.layout(child).location, Point::ZERO);
}

#[test]
fn absolute_children_use_padding_box_and_do_not_affect_wrap_content() {
    let mut tree = TestTree::default();
    let mut absolute_style = relative_leaf_style(30.0, 20.0, 1);
    absolute_style.position = PositionProperty::Absolute;
    let absolute = tree.push_leaf(absolute_style, Size::new(30.0, 20.0), None);
    let root = relative_container(
        &mut tree,
        TestStyle {
            padding: Edges::uniform(npx(5.0)),
            border: Edges::uniform(border_px(2.0)),
            ..TestStyle::default()
        },
        &[absolute],
    );

    let output = perform_layout(
        &tree,
        root,
        Size::NONE,
        Size::new(AvailableSpace::MaxContent, AvailableSpace::MaxContent),
    );

    assert_size(output.size, Size::new(14.0, 14.0));
    assert_point(tree.layout(absolute).location, Point::new(2.0, 2.0));
}

#[test]
fn absolute_and_in_flow_children_share_contiguous_paint_order() {
    let mut tree = TestTree::default();
    let mut absolute_style = fixed_leaf_style(10.0, 10.0);
    absolute_style.position = PositionProperty::Absolute;
    absolute_style.order = 10;
    let absolute = tree.push_leaf(absolute_style, Size::new(10.0, 10.0), None);
    let in_flow = fixed_leaf(&mut tree, 10.0, 10.0);
    let root = relative_container(
        &mut tree,
        TestStyle {
            size: Size::new(size_px(20.0), size_px(10.0)),
            ..TestStyle::default()
        },
        &[absolute, in_flow],
    );

    definite_layout(&tree, root, 20.0, 10.0);

    assert_eq!(tree.layout(absolute).order, 0);
    assert_eq!(tree.layout(in_flow).order, 1);
}

#[test]
fn hoisted_children_record_padding_box_static_position_only() {
    let mut tree = TestTree::default();
    let mut fixed_style = relative_leaf_style(10.0, 10.0, 1);
    fixed_style.position = PositionProperty::Fixed;
    let fixed = tree.push_leaf(fixed_style, Size::new(10.0, 10.0), None);
    let root = relative_container(
        &mut tree,
        TestStyle {
            border: Edges::uniform(border_px(3.0)),
            ..TestStyle::default()
        },
        &[fixed],
    );

    definite_layout(&tree, root, 100.0, 80.0);
    assert_eq!(tree.static_position(fixed), Some(Point::new(3.0, 3.0)));
    assert_eq!(tree.layout(fixed), Layout::default());
}

#[test]
fn measure_goal_has_no_durable_geometry_side_effects_or_baseline() {
    let mut tree = TestTree::default();
    let child = relative_leaf(&mut tree, 10.0, 10.0, 1);
    let root = relative_container(&mut tree, TestStyle::default(), &[child]);
    let output = tree.compute_layout(
        root,
        LayoutInput::measure(
            Size::NONE,
            Size::NONE,
            Size::new(AvailableSpace::MaxContent, AvailableSpace::MaxContent),
            RequestedAxis::Both,
        ),
    );

    assert_size(output.size, Size::new(10.0, 10.0));
    assert_eq!(output.first_baselines, Point::NONE);
    assert_eq!(tree.layout_writes.get(), 0);
    assert_eq!(tree.layout(child), Layout::default());
}

#[test]
fn nested_relative_containers_propagate_intrinsic_child_size() {
    let mut tree = TestTree::default();
    let leaf = relative_leaf(&mut tree, 12.0, 8.0, 1);
    let inner = relative_container(&mut tree, TestStyle::default(), &[leaf]);
    let outer = relative_container(&mut tree, TestStyle::default(), &[inner]);

    let output = perform_layout(
        &tree,
        outer,
        Size::NONE,
        Size::new(AvailableSpace::MaxContent, AvailableSpace::MaxContent),
    );

    assert_size(output.size, Size::new(12.0, 8.0));
    assert_size(tree.layout(inner).size, Size::new(12.0, 8.0));
    assert_size(tree.layout(leaf).size, Size::new(12.0, 8.0));
}

#[test]
fn sibling_same_side_alignment_and_before_adjacency_use_the_referenced_edges() {
    let mut tree = TestTree::default();
    let mut anchor_style = relative_leaf_style(10.0, 10.0, 1);
    anchor_style.relative_align.right = RELATIVE_PARENT;
    anchor_style.relative_align.bottom = RELATIVE_PARENT;
    let anchor = tree.push_leaf(anchor_style, Size::new(10.0, 10.0), None);

    let mut aligned_style = relative_leaf_style(10.0, 10.0, 2);
    aligned_style.relative_align.left = id(1);
    aligned_style.relative_align.top = id(1);
    let aligned = tree.push_leaf(aligned_style, Size::new(10.0, 10.0), None);

    let mut before_style = relative_leaf_style(10.0, 10.0, 3);
    before_style.relative_adjacent.left = id(1);
    before_style.relative_adjacent.top = id(1);
    let before = tree.push_leaf(before_style, Size::new(10.0, 10.0), None);
    let root = relative_container(&mut tree, TestStyle::default(), &[aligned, before, anchor]);

    definite_layout(&tree, root, 100.0, 100.0);

    assert_point(tree.layout(aligned).location, Point::new(90.0, 90.0));
    assert_point(tree.layout(before).location, Point::new(80.0, 80.0));
}

#[test]
fn sibling_edges_stretch_an_auto_sized_item_after_subtracting_margins() {
    let mut tree = TestTree::default();
    let mut left_style = relative_leaf_style(20.0, 10.0, 40);
    left_style.relative_align.left = RELATIVE_PARENT;
    let left = tree.push_leaf(left_style, Size::new(20.0, 10.0), None);
    let mut right_style = relative_leaf_style(20.0, 10.0, 41);
    right_style.relative_align.right = RELATIVE_PARENT;
    let right = tree.push_leaf(right_style, Size::new(20.0, 10.0), None);
    let mut middle_style = TestStyle {
        margin: Edges {
            left: margin_px(5.0),
            right: margin_px(5.0),
            top: margin_px(0.0),
            bottom: margin_px(0.0),
        },
        ..TestStyle::default()
    };
    middle_style.relative_adjacent.right = id(40);
    middle_style.relative_adjacent.left = id(41);
    let middle = tree.push_leaf(middle_style, Size::new(200.0, 10.0), None);
    let root = relative_container(&mut tree, TestStyle::default(), &[middle, right, left]);

    definite_layout(&tree, root, 100.0, 40.0);

    assert_point(tree.layout(left).location, Point::new(0.0, 0.0));
    assert_point(tree.layout(right).location, Point::new(80.0, 0.0));
    assert_point(tree.layout(middle).location, Point::new(25.0, 0.0));
    assert_size(tree.layout(middle).size, Size::new(50.0, 10.0));
    assert_close(tree.layout(middle).margin.left, 5.0);
    assert_close(tree.layout(middle).margin.right, 5.0);
}

#[test]
fn a_single_start_constraint_reduces_leaf_available_space() {
    let mut tree = TestTree::default();
    let anchor = relative_leaf(&mut tree, 20.0, 10.0, 10);
    let mut follower_style = TestStyle::default();
    follower_style.relative_adjacent.right = id(10);
    let follower = tree.push_measured_leaf(follower_style, width_bounded_by_available);
    let root = relative_container(&mut tree, TestStyle::default(), &[follower, anchor]);

    definite_layout(&tree, root, 100.0, 20.0);

    assert_point(tree.layout(anchor).location, Point::new(0.0, 0.0));
    assert_point(tree.layout(follower).location, Point::new(20.0, 0.0));
    assert_size(tree.layout(follower).size, Size::new(80.0, 10.0));
    assert!(tree.measure_inputs(follower).iter().any(|input| {
        input.known_dimensions.width.is_none()
            && input.available_space.width == AvailableSpace::Definite(80.0)
    }));
}

#[test]
fn a_single_end_constraint_preserves_margins_in_leaf_available_space() {
    let mut tree = TestTree::default();
    let mut anchor_style = relative_leaf_style(20.0, 10.0, 20);
    anchor_style.relative_align.right = RELATIVE_PARENT;
    let anchor = tree.push_leaf(anchor_style, Size::new(20.0, 10.0), None);
    let mut follower_style = TestStyle {
        margin: Edges {
            left: margin_px(3.0),
            right: margin_px(3.0),
            top: margin_px(0.0),
            bottom: margin_px(0.0),
        },
        ..TestStyle::default()
    };
    follower_style.relative_adjacent.left = id(20);
    let follower = tree.push_measured_leaf(follower_style, width_bounded_by_available);
    let root = relative_container(&mut tree, TestStyle::default(), &[follower, anchor]);

    definite_layout(&tree, root, 100.0, 20.0);

    assert_point(tree.layout(anchor).location, Point::new(80.0, 0.0));
    assert_point(tree.layout(follower).location, Point::new(-3.0, 0.0));
    assert_size(tree.layout(follower).size, Size::new(80.0, 10.0));
    assert_close(tree.layout(follower).margin.left, 3.0);
    assert_close(tree.layout(follower).margin.right, 3.0);
    assert!(tree.measure_inputs(follower).iter().any(|input| {
        input.known_dimensions.width.is_none()
            && input.available_space.width == AvailableSpace::Definite(80.0)
    }));
}

#[test]
fn an_unanchored_child_removes_its_margins_from_available_space_once() {
    let mut tree = TestTree::default();
    let child = tree.push_measured_leaf(
        TestStyle {
            margin: Edges {
                left: margin_px(7.0),
                right: margin_px(3.0),
                top: margin_px(0.0),
                bottom: margin_px(0.0),
            },
            ..TestStyle::default()
        },
        width_bounded_by_available,
    );
    let root = relative_container(&mut tree, TestStyle::default(), &[child]);

    definite_layout(&tree, root, 100.0, 20.0);

    assert_point(tree.layout(child).location, Point::new(7.0, 0.0));
    assert_size(tree.layout(child).size, Size::new(90.0, 10.0));
    assert_close(tree.layout(child).margin.left, 7.0);
    assert_close(tree.layout(child).margin.right, 3.0);
    assert!(tree.measure_inputs(child).iter().any(|input| {
        input.known_dimensions.width.is_none()
            && input.available_space.width == AvailableSpace::Definite(90.0)
    }));
}

#[test]
fn a_start_constraint_reduces_the_fit_content_limit_before_measurement() {
    let mut tree = TestTree::default();
    let anchor = relative_leaf(&mut tree, 20.0, 10.0, 30);
    let mut follower_style = TestStyle {
        size: Size::new(size_fit_content_px(50.0), size_px(10.0)),
        ..TestStyle::default()
    };
    follower_style.relative_adjacent.right = id(30);
    let follower = tree.push_measured_leaf(follower_style, intrinsic_width_bounded_by_available);
    let root = relative_container(&mut tree, TestStyle::default(), &[follower, anchor]);

    definite_layout(&tree, root, 100.0, 20.0);

    assert_point(tree.layout(anchor).location, Point::new(0.0, 0.0));
    assert_point(tree.layout(follower).location, Point::new(20.0, 0.0));
    assert_size(tree.layout(follower).size, Size::new(30.0, 10.0));
    assert!(tree.measure_inputs(follower).iter().any(|input| {
        input.known_dimensions.width.is_none()
            && input.available_space.width == AvailableSpace::Definite(30.0)
    }));
}

#[test]
fn an_end_constraint_overrides_the_fit_content_limit_before_measurement() {
    let mut tree = TestTree::default();
    let mut anchor_style = relative_leaf_style(20.0, 10.0, 40);
    anchor_style.relative_align.right = RELATIVE_PARENT;
    let anchor = tree.push_leaf(anchor_style, Size::new(20.0, 10.0), None);
    let mut follower_style = TestStyle {
        size: Size::new(size_fit_content_px(50.0), size_px(10.0)),
        ..TestStyle::default()
    };
    follower_style.relative_adjacent.left = id(40);
    let follower = tree.push_measured_leaf(follower_style, intrinsic_width_bounded_by_available);
    let root = relative_container(&mut tree, TestStyle::default(), &[follower, anchor]);

    definite_layout(&tree, root, 100.0, 20.0);

    assert_point(tree.layout(anchor).location, Point::new(80.0, 0.0));
    assert_point(tree.layout(follower).location, Point::new(0.0, 0.0));
    assert_size(tree.layout(follower).size, Size::new(80.0, 10.0));
    assert!(tree.measure_inputs(follower).iter().any(|input| {
        input.known_dimensions.width.is_none()
            && input.available_space.width == AvailableSpace::Definite(80.0)
    }));
}

#[test]
fn intrinsic_keywords_and_fit_content_use_the_owner_constraint() {
    let mut tree = TestTree::default();
    let fit = tree.push_intrinsic_leaf(
        TestStyle {
            size: Size::new(size_fit_content_pct(0.5), size_px(10.0)),
            ..TestStyle::default()
        },
        Size::new(20.0, 10.0),
        Size::new(80.0, 10.0),
    );
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
    let root = relative_container(&mut tree, TestStyle::default(), &[fit, constrained]);

    definite_layout(&tree, root, 100.0, 20.0);

    assert_close(tree.layout(fit).size.width, 50.0);
    assert_close(tree.layout(constrained).size.width, 30.0);
}

#[test]
fn edges_use_available_width_while_child_percent_sizes_require_definiteness() {
    let mut tree = TestTree::default();
    let child = tree.push_leaf(
        TestStyle {
            size: Size::new(size_pct(0.5), size_px(10.0)),
            margin: Edges {
                left: margin_pct(0.1),
                right: margin_pct(0.1),
                top: margin_px(0.0),
                bottom: margin_px(0.0),
            },
            ..TestStyle::default()
        },
        Size::new(12.0, 10.0),
        None,
    );
    let root = relative_container(
        &mut tree,
        TestStyle {
            relative_layout_once: relative_layout_once::T::False,
            ..TestStyle::default()
        },
        &[child],
    );

    let output = perform_layout(
        &tree,
        root,
        Size::NONE,
        Size::new(AvailableSpace::Definite(200.0), AvailableSpace::MaxContent),
    );

    assert_close(output.size.width, 52.0);
    assert_close(tree.layout(child).margin.left, 5.2);
    assert_close(tree.layout(child).size.width, 26.0);
    assert_eq!(tree.child_layout_calls.get(), 4);
}

#[test]
fn aspect_ratio_and_box_sizing_are_shared_with_other_layout_algorithms() {
    let mut tree = TestTree::default();
    let child = tree.push_leaf(
        TestStyle {
            size: Size::new(size_px(40.0), size_auto()),
            aspect_ratio: ratio(2.0),
            padding: Edges::uniform(npx(2.0)),
            ..TestStyle::default()
        },
        Size::ZERO,
        None,
    );
    let root = relative_container(&mut tree, TestStyle::default(), &[child]);

    definite_layout(&tree, root, 100.0, 100.0);
    assert_size(tree.layout(child).size, Size::new(44.0, 24.0));
}

#[test]
fn one_pass_keeps_wrap_fallback_positions_after_a_minimum_expands_the_parent() {
    let mut tree = TestTree::default();
    let mut child_style = relative_leaf_style(10.0, 10.0, 1);
    child_style.relative_align.right = RELATIVE_PARENT;
    let child = tree.push_leaf(child_style, Size::new(10.0, 10.0), None);
    let root = relative_container(
        &mut tree,
        TestStyle {
            min_size: Size::new(size_px(100.0), size_px(20.0)),
            relative_layout_once: relative_layout_once::T::True,
            ..TestStyle::default()
        },
        &[child],
    );

    let output = perform_layout(
        &tree,
        root,
        Size::NONE,
        Size::new(AvailableSpace::MaxContent, AvailableSpace::MaxContent),
    );

    assert_size(output.size, Size::new(100.0, 20.0));
    assert_close(tree.layout(child).location.x, -10.0);
}

#[test]
fn intrinsic_preferred_width_does_not_define_descendant_percentage_basis() {
    let mut tree = TestTree::default();
    let percent_child = tree.push_leaf(
        TestStyle {
            size: Size::new(size_pct(0.5), size_px(10.0)),
            ..TestStyle::default()
        },
        Size::new(20.0, 10.0),
        None,
    );
    let intrinsic_parent = relative_container(
        &mut tree,
        TestStyle {
            size: Size::new(size_max_content(), size_px(10.0)),
            relative_layout_once: relative_layout_once::T::True,
            ..TestStyle::default()
        },
        &[percent_child],
    );
    let root = relative_container(
        &mut tree,
        TestStyle {
            relative_layout_once: relative_layout_once::T::True,
            ..TestStyle::default()
        },
        &[intrinsic_parent],
    );

    definite_layout(&tree, root, 100.0, 20.0);

    assert_close(tree.layout(intrinsic_parent).size.width, 20.0);
    assert_close(tree.layout(percent_child).size.width, 20.0);
}

#[test]
fn double_anchor_proposal_applies_child_max_size_before_measurement() {
    let mut tree = TestTree::default();
    let mut child_style = TestStyle {
        max_size: Size::new(max_px(50.0), max_none()),
        ..TestStyle::default()
    };
    child_style.relative_align.left = RELATIVE_PARENT;
    child_style.relative_align.right = RELATIVE_PARENT;
    let child = tree.push_leaf(child_style, Size::new(200.0, 10.0), None);
    let root = relative_container(&mut tree, TestStyle::default(), &[child]);

    definite_layout(&tree, root, 100.0, 20.0);

    assert_close(tree.layout(child).location.x, 0.0);
    assert_close(tree.layout(child).size.width, 50.0);
}

fn width_sensitive_intrinsic_max(
    input: neutron_star::compute::LeafMeasureInput,
) -> neutron_star::compute::LeafMetrics {
    let width = input.known_dimensions.width.unwrap_or_else(|| {
        if input.available_space.width == AvailableSpace::MinContent {
            20.0
        } else {
            100.0
        }
    });
    let height = if width <= 20.0 { 50.0 } else { 10.0 };
    neutron_star::compute::LeafMetrics::new(Size::new(width, height))
}

#[test]
fn intrinsic_max_width_remeasures_width_sensitive_height() {
    let mut tree = TestTree::default();
    let child = tree.push_measured_leaf(
        TestStyle {
            max_size: Size::new(max_min_content(), max_none()),
            ..TestStyle::default()
        },
        width_sensitive_intrinsic_max,
    );
    let root = relative_container(&mut tree, TestStyle::default(), &[child]);

    definite_layout(&tree, root, 100.0, 100.0);

    assert_size(tree.layout(child).size, Size::new(20.0, 50.0));
}

#[test]
fn intrinsic_keyword_widths_resolve_on_relative_children() {
    use stylo::values::computed::{MaxSize, Size as StyleSize};

    let width_of = |style: TestStyle| -> f32 {
        let mut tree = TestTree::default();
        let item = tree.push_measured_leaf(style, intrinsic_width_bounded_by_available);
        let root = relative_container(&mut tree, TestStyle::default(), &[item]);
        definite_layout(&tree, root, 300.0, 100.0);
        tree.layout(item).size.width
    };

    assert_close(
        width_of(TestStyle {
            size: Size::new(size_min_content(), size_auto()),
            ..TestStyle::default()
        }),
        20.0,
    );
    assert_close(
        width_of(TestStyle {
            size: Size::new(size_max_content(), size_auto()),
            ..TestStyle::default()
        }),
        200.0,
    );
    assert_close(
        width_of(TestStyle {
            size: Size::new(size_fit_content_px(150.0), size_auto()),
            ..TestStyle::default()
        }),
        150.0,
    );
    assert_close(
        width_of(TestStyle {
            size: Size::new(size_fit_content_pct(0.5), size_auto()),
            ..TestStyle::default()
        }),
        150.0,
    );
    assert_close(
        width_of(TestStyle {
            size: Size::new(size_fit_content_px(150.0), size_auto()),
            box_sizing: box_sizing::T::BorderBox,
            padding: Edges::uniform(npx(10.0)),
            ..TestStyle::default()
        }),
        150.0,
    );
    for keyword in [StyleSize::FitContent, StyleSize::Stretch] {
        assert_close(
            width_of(TestStyle {
                size: Size::new(keyword, size_auto()),
                ..TestStyle::default()
            }),
            200.0,
        );
    }

    assert_close(
        width_of(TestStyle {
            max_size: Size::new(max_max_content(), max_none()),
            ..TestStyle::default()
        }),
        200.0,
    );
    assert_close(
        width_of(TestStyle {
            max_size: Size::new(max_min_content(), max_none()),
            ..TestStyle::default()
        }),
        20.0,
    );
    assert_close(
        width_of(TestStyle {
            max_size: Size::new(max_fit_content_px(150.0), max_none()),
            ..TestStyle::default()
        }),
        150.0,
    );
    assert_close(
        width_of(TestStyle {
            size: Size::new(size_px(250.0), size_auto()),
            max_size: Size::new(MaxSize::Stretch, max_none()),
            ..TestStyle::default()
        }),
        250.0,
    );
}

#[test]
fn intrinsic_minimums_and_heights_probe_their_axes() {
    let mut tree = TestTree::default();
    let floored = tree.push_measured_leaf(
        TestStyle {
            min_size: Size::new(size_max_content(), size_auto()),
            ..TestStyle::default()
        },
        intrinsic_width_bounded_by_available,
    );
    let root = relative_container(&mut tree, TestStyle::default(), &[floored]);
    definite_layout(&tree, root, 100.0, 50.0);
    assert_close(tree.layout(floored).size.width, 200.0);

    let mut tree = TestTree::default();
    let item = tree.push_intrinsic_leaf(
        TestStyle {
            size: Size::new(size_px(40.0), size_max_content()),
            ..TestStyle::default()
        },
        Size::new(30.0, 12.0),
        Size::new(90.0, 48.0),
    );
    let root = relative_container(&mut tree, TestStyle::default(), &[item]);
    definite_layout(&tree, root, 300.0, 100.0);
    assert_close(tree.layout(item).size.height, 48.0);
}

#[test]
fn definite_preferred_sizes_clamp_by_resolved_min_max_bounds() {
    let width_of = |style: TestStyle| -> f32 {
        let mut tree = TestTree::default();
        let item = tree.push_measured_leaf(style, intrinsic_width_bounded_by_available);
        let root = relative_container(&mut tree, TestStyle::default(), &[item]);
        definite_layout(&tree, root, 300.0, 100.0);
        tree.layout(item).size.width
    };

    assert_close(
        width_of(TestStyle {
            size: Size::new(size_px(250.0), size_auto()),
            max_size: Size::new(max_max_content(), max_none()),
            ..TestStyle::default()
        }),
        200.0,
    );
    assert_close(
        width_of(TestStyle {
            size: Size::new(size_px(250.0), size_auto()),
            max_size: Size::new(max_px(180.0), max_none()),
            ..TestStyle::default()
        }),
        180.0,
    );
    assert_close(
        width_of(TestStyle {
            size: Size::new(size_px(50.0), size_auto()),
            min_size: Size::new(size_max_content(), size_auto()),
            ..TestStyle::default()
        }),
        200.0,
    );

    let mut tree = TestTree::default();
    let wrapped = tree.push_measured_leaf(
        TestStyle {
            size: Size::new(size_px(250.0), size_auto()),
            max_size: Size::new(max_min_content(), max_none()),
            ..TestStyle::default()
        },
        width_sensitive_intrinsic_max,
    );
    let root = relative_container(&mut tree, TestStyle::default(), &[wrapped]);
    definite_layout(&tree, root, 300.0, 100.0);
    assert_size(tree.layout(wrapped).size, Size::new(20.0, 50.0));

    let mut tree = TestTree::default();
    let short = tree.push_intrinsic_leaf(
        TestStyle {
            size: Size::new(size_px(30.0), size_px(40.0)),
            max_size: Size::new(max_none(), max_max_content()),
            ..TestStyle::default()
        },
        Size::new(20.0, 10.0),
        Size::new(200.0, 10.0),
    );
    let root = relative_container(&mut tree, TestStyle::default(), &[short]);
    definite_layout(&tree, root, 300.0, 100.0);
    assert_close(tree.layout(short).size.height, 10.0);
}

#[test]
fn container_measurement_resolves_child_fit_content_limits() {
    let measured_width = |size: stylo::values::computed::Size| -> f32 {
        let mut tree = TestTree::default();
        let item = tree.push_measured_leaf(
            TestStyle {
                size: Size::new(size, size_auto()),
                ..TestStyle::default()
            },
            intrinsic_width_bounded_by_available,
        );
        let root = relative_container(&mut tree, TestStyle::default(), &[item]);
        measure_layout(
            &tree,
            root,
            Size::NONE,
            Size::new(AvailableSpace::MaxContent, AvailableSpace::MaxContent),
        )
        .size
        .width
    };

    assert_close(measured_width(size_fit_content_px(150.0)), 150.0);
    assert_close(measured_width(size_fit_content_pct(0.5)), 200.0);
}

#[test]
fn relative_insets_follow_direction_and_skip_static_children() {
    let run = |style: TestStyle| -> Point<f32> {
        let mut tree = TestTree::default();
        let item = tree.push_leaf(style, Size::new(20.0, 10.0), None);
        let root = relative_container(&mut tree, TestStyle::default(), &[item]);
        definite_layout(&tree, root, 100.0, 50.0);
        tree.layout(item).location
    };

    let both_edges = |direction: direction::T| TestStyle {
        direction,
        inset: Edges {
            left: inset_px(12.0),
            right: inset_px(30.0),
            top: inset_auto(),
            bottom: inset_auto(),
        },
        size: Size::new(size_px(20.0), size_px(10.0)),
        ..TestStyle::default()
    };
    assert_point(run(both_edges(direction::T::Ltr)), Point::new(12.0, 0.0));
    assert_point(run(both_edges(direction::T::Rtl)), Point::new(-30.0, 0.0));

    assert_point(
        run(TestStyle {
            inset: Edges {
                left: inset_auto(),
                right: inset_px(8.0),
                top: inset_auto(),
                bottom: inset_auto(),
            },
            size: Size::new(size_px(20.0), size_px(10.0)),
            ..TestStyle::default()
        }),
        Point::new(-8.0, 0.0),
    );

    assert_point(
        run(TestStyle {
            position: PositionProperty::Static,
            inset: Edges {
                left: inset_px(15.0),
                right: inset_auto(),
                top: inset_px(5.0),
                bottom: inset_auto(),
            },
            size: Size::new(size_px(20.0), size_px(10.0)),
            ..TestStyle::default()
        }),
        Point::new(0.0, 0.0),
    );
}

#[test]
fn same_axis_adjacency_cycles_fall_back_to_document_order() {
    let mut tree = TestTree::default();
    let mut a_style = relative_leaf_style(10.0, 10.0, 1);
    a_style.relative_adjacent.right = id(2);
    let a = tree.push_leaf(a_style, Size::new(10.0, 10.0), None);
    let mut b_style = relative_leaf_style(10.0, 10.0, 2);
    b_style.relative_adjacent.right = id(1);
    let b = tree.push_leaf(b_style, Size::new(10.0, 10.0), None);
    let mut c_style = relative_leaf_style(10.0, 10.0, 3);
    c_style.relative_adjacent.right = id(4);
    let c = tree.push_leaf(c_style, Size::new(10.0, 10.0), None);
    let mut d_style = relative_leaf_style(10.0, 10.0, 4);
    d_style.relative_adjacent.right = id(3);
    let d = tree.push_leaf(d_style, Size::new(10.0, 10.0), None);
    let root = relative_container(&mut tree, TestStyle::default(), &[a, b, c, d]);

    definite_layout(&tree, root, 100.0, 50.0);

    assert_close(tree.layout(a).location.x, 0.0);
    assert_close(tree.layout(b).location.x, 10.0);
    assert_close(tree.layout(c).location.x, 0.0);
    assert_close(tree.layout(d).location.x, 10.0);
}

#[test]
fn parent_sentinel_in_adjacency_channel_uses_the_parent_edge() {
    let mut tree = TestTree::default();
    let mut style = relative_leaf_style(10.0, 10.0, 1);
    style.relative_adjacent.right = RELATIVE_PARENT;
    let item = tree.push_leaf(style, Size::new(10.0, 10.0), None);
    let root = relative_container(&mut tree, TestStyle::default(), &[item]);

    definite_layout(&tree, root, 100.0, 50.0);

    assert_point(tree.layout(item).location, Point::new(100.0, 0.0));
}

#[test]
fn quantitative_values_survive_intrinsic_resolution() {
    use stylo::values::computed::Size as StyleSize;

    let mut tree = TestTree::default();
    let fixed_width = tree.push_measured_leaf(
        TestStyle {
            size: Size::new(size_px(120.0), size_auto()),
            min_size: Size::new(size_min_content(), size_auto()),
            ..TestStyle::default()
        },
        intrinsic_width_bounded_by_available,
    );
    let fit_keyword = tree.push_measured_leaf(
        TestStyle {
            size: Size::new(StyleSize::FitContent, size_auto()),
            min_size: Size::new(size_min_content(), size_auto()),
            ..TestStyle::default()
        },
        intrinsic_width_bounded_by_available,
    );
    let stretch_keyword = tree.push_measured_leaf(
        TestStyle {
            size: Size::new(StyleSize::Stretch, size_auto()),
            min_size: Size::new(size_min_content(), size_auto()),
            max_size: Size::new(max_px(150.0), max_none()),
            ..TestStyle::default()
        },
        intrinsic_width_bounded_by_available,
    );
    let root = relative_container(
        &mut tree,
        TestStyle::default(),
        &[fixed_width, fit_keyword, stretch_keyword],
    );

    definite_layout(&tree, root, 300.0, 100.0);

    assert_close(tree.layout(fixed_width).size.width, 120.0);
    assert_close(tree.layout(fit_keyword).size.width, 200.0);
    assert_close(tree.layout(stretch_keyword).size.width, 150.0);
}
