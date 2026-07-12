//! Starlight relative-layout conformance tests over a plain `Vec` host.

mod support;

use neutron_star::prelude::*;
use neutron_star::style::{
    BoxGenerationMode, Dimension, LengthPercentage, LengthPercentageAuto, Position, RelativeCenter,
    RelativeReference, Visibility,
};
use support::*;

fn id(value: i32) -> RelativeReference {
    RelativeReference::new(value)
}

fn relative_leaf_style(width: f32, height: f32, relative_id: i32) -> TestStyle {
    TestStyle {
        size: Size::new(Dimension::Length(width), Dimension::Length(height)),
        relative_id: id(relative_id),
        ..TestStyle::default()
    }
}

fn relative_leaf(tree: &mut TestTree, width: f32, height: f32, relative_id: i32) -> NodeId {
    tree.push_leaf(
        relative_leaf_style(width, height, relative_id),
        Size::new(width, height),
        None,
    )
}

#[test]
fn parent_alignment_and_sibling_adjacency_use_physical_margin_edges() {
    let mut tree = TestTree::default();
    let mut anchor_style = relative_leaf_style(10.0, 10.0, 1);
    anchor_style.relative_align.left = RelativeReference::PARENT;
    anchor_style.relative_align.top = RelativeReference::PARENT;
    let anchor = tree.push_leaf(anchor_style, Size::new(10.0, 10.0), None);

    let mut follower_style = relative_leaf_style(15.0, 20.0, 2);
    follower_style.relative_adjacent.right = id(1);
    follower_style.relative_adjacent.bottom = id(1);
    let follower = tree.push_leaf(follower_style, Size::new(15.0, 20.0), None);

    let mut trailing_style = relative_leaf_style(10.0, 10.0, 3);
    trailing_style.relative_align.right = RelativeReference::PARENT;
    trailing_style.relative_align.bottom = RelativeReference::PARENT;
    let trailing = tree.push_leaf(trailing_style, Size::new(10.0, 10.0), None);
    let root = relative_container(
        &mut tree,
        TestStyle::default(),
        &[anchor, follower, trailing],
    );

    definite_layout(&mut tree, root, 100.0, 80.0);

    assert_point(tree.layout(anchor).location, Point::new(0.0, 0.0));
    assert_point(tree.layout(follower).location, Point::new(10.0, 10.0));
    assert_point(tree.layout(trailing).location, Point::new(90.0, 70.0));
}

#[test]
fn alignment_precedes_adjacency_for_the_same_side() {
    let mut tree = TestTree::default();
    let mut anchor_style = relative_leaf_style(10.0, 10.0, 1);
    anchor_style.relative_align.right = RelativeReference::PARENT;
    let anchor = tree.push_leaf(anchor_style, Size::new(10.0, 10.0), None);

    let mut child_style = relative_leaf_style(10.0, 10.0, 2);
    child_style.relative_align.left = RelativeReference::PARENT;
    child_style.relative_adjacent.right = id(1);
    let child = tree.push_leaf(child_style, Size::new(10.0, 10.0), None);
    let root = relative_container(&mut tree, TestStyle::default(), &[anchor, child]);

    definite_layout(&mut tree, root, 100.0, 20.0);
    assert_close(tree.layout(child).location.x, 0.0);
}

#[test]
fn both_sides_refine_child_size_after_dependencies_are_positioned() {
    let mut tree = TestTree::default();
    let mut left_style = relative_leaf_style(10.0, 10.0, 1);
    left_style.relative_align.left = RelativeReference::PARENT;
    let left = tree.push_leaf(left_style, Size::new(10.0, 10.0), None);

    let mut right_style = relative_leaf_style(10.0, 10.0, 2);
    right_style.relative_align.right = RelativeReference::PARENT;
    let right = tree.push_leaf(right_style, Size::new(10.0, 10.0), None);

    let mut middle_style = TestStyle {
        relative_id: id(3),
        ..TestStyle::default()
    };
    middle_style.relative_adjacent.right = id(1);
    middle_style.relative_adjacent.left = id(2);
    let middle = tree.push_leaf(middle_style, Size::new(200.0, 10.0), None);
    let root = relative_container(&mut tree, TestStyle::default(), &[middle, right, left]);

    definite_layout(&mut tree, root, 100.0, 20.0);

    assert_close(tree.layout(middle).location.x, 10.0);
    assert_close(tree.layout(middle).size.width, 80.0);
}

#[test]
fn parent_double_alignment_subtracts_used_margins() {
    let mut tree = TestTree::default();
    let mut style = TestStyle {
        margin: Edges {
            left: LengthPercentageAuto::Length(5.0),
            right: LengthPercentageAuto::Length(7.0),
            top: LengthPercentageAuto::ZERO,
            bottom: LengthPercentageAuto::ZERO,
        },
        ..TestStyle::default()
    };
    style.relative_align.left = RelativeReference::PARENT;
    style.relative_align.right = RelativeReference::PARENT;
    let child = tree.push_leaf(style, Size::new(10.0, 10.0), None);
    let root = relative_container(&mut tree, TestStyle::default(), &[child]);

    definite_layout(&mut tree, root, 100.0, 20.0);

    assert_close(tree.layout(child).location.x, 5.0);
    assert_close(tree.layout(child).size.width, 88.0);
}

#[test]
fn duplicate_ids_resolve_to_the_last_ordered_relative_item() {
    let mut tree = TestTree::default();
    let first = relative_leaf(&mut tree, 10.0, 10.0, 7);
    let mut last_style = relative_leaf_style(10.0, 10.0, 7);
    last_style.relative_align.right = RelativeReference::PARENT;
    let last = tree.push_leaf(last_style, Size::new(10.0, 10.0), None);
    let mut follower_style = relative_leaf_style(10.0, 10.0, 8);
    follower_style.relative_adjacent.right = id(7);
    let follower = tree.push_leaf(follower_style, Size::new(10.0, 10.0), None);
    let root = relative_container(&mut tree, TestStyle::default(), &[first, last, follower]);

    definite_layout(&mut tree, root, 100.0, 20.0);

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
    earlier_style.relative_align.right = RelativeReference::PARENT;
    let earlier = tree.push_leaf(earlier_style, Size::new(10.0, 10.0), None);
    let mut follower_style = relative_leaf_style(10.0, 10.0, 8);
    follower_style.order = 3;
    follower_style.relative_adjacent.right = id(7);
    let follower = tree.push_leaf(follower_style, Size::new(10.0, 10.0), None);
    let root = relative_container(&mut tree, TestStyle::default(), &[later, earlier, follower]);

    definite_layout(&mut tree, root, 100.0, 20.0);

    assert_eq!(tree.layout(earlier).order, 0);
    assert_eq!(tree.layout(later).order, 1);
    assert_eq!(tree.layout(follower).order, 2);
    // `later` is the last id 7 after order sorting, despite appearing first
    // in source order.
    assert_close(tree.layout(follower).location.x, 10.0);
}

#[test]
fn parent_id_zero_is_reserved_and_never_identifies_an_item() {
    let mut tree = TestTree::default();
    let mut zero_style = relative_leaf_style(10.0, 10.0, 0);
    zero_style.relative_align.left = RelativeReference::PARENT;
    let zero = tree.push_leaf(zero_style, Size::new(10.0, 10.0), None);
    let mut follower_style = relative_leaf_style(10.0, 10.0, 2);
    follower_style.relative_adjacent.right = RelativeReference::PARENT;
    let follower = tree.push_leaf(follower_style, Size::new(10.0, 10.0), None);
    let root = relative_container(&mut tree, TestStyle::default(), &[zero, follower]);

    definite_layout(&mut tree, root, 100.0, 20.0);

    assert_close(tree.layout(zero).location.x, 0.0);
    assert_close(tree.layout(follower).location.x, 100.0);
}

#[test]
fn missing_reference_falls_back_to_the_other_property_or_default_bounds() {
    let mut tree = TestTree::default();
    let mut style = relative_leaf_style(10.0, 10.0, 1);
    style.relative_align.left = id(999);
    style.relative_adjacent.right = RelativeReference::PARENT;
    let fallback = tree.push_leaf(style, Size::new(10.0, 10.0), None);
    let root = relative_container(&mut tree, TestStyle::default(), &[fallback]);

    definite_layout(&mut tree, root, 100.0, 20.0);
    assert_close(tree.layout(fallback).location.x, 100.0);
}

#[test]
fn unconstrained_centering_is_axis_selective() {
    let mut tree = TestTree::default();
    let horizontal = tree.push_leaf(
        TestStyle {
            size: Size::new(Dimension::Length(20.0), Dimension::Length(10.0)),
            relative_center: RelativeCenter::Horizontal,
            ..TestStyle::default()
        },
        Size::new(20.0, 10.0),
        None,
    );
    let both = tree.push_leaf(
        TestStyle {
            size: Size::new(Dimension::Length(10.0), Dimension::Length(20.0)),
            relative_center: RelativeCenter::Both,
            ..TestStyle::default()
        },
        Size::new(10.0, 20.0),
        None,
    );
    let root = relative_container(&mut tree, TestStyle::default(), &[horizontal, both]);

    definite_layout(&mut tree, root, 100.0, 80.0);

    assert_point(tree.layout(horizontal).location, Point::new(40.0, 0.0));
    assert_point(tree.layout(both).location, Point::new(45.0, 30.0));
}

#[test]
fn combined_and_separate_dependency_orders_have_defined_cycle_behavior() {
    fn fixture(layout_once: bool) -> (TestTree, NodeId, NodeId, NodeId) {
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

    let (mut once, root, a, b) = fixture(true);
    definite_layout(&mut once, root, 100.0, 100.0);
    assert_point(once.layout(a).location, Point::new(0.0, 0.0));
    assert_point(once.layout(b).location, Point::new(0.0, 10.0));

    let (mut twice, root, a, b) = fixture(false);
    definite_layout(&mut twice, root, 100.0, 100.0);
    assert_point(twice.layout(a).location, Point::new(10.0, 0.0));
    assert_point(twice.layout(b).location, Point::new(0.0, 10.0));
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

    definite_layout(&mut tree, root, 100.0, 100.0);
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
            padding: Edges::uniform(LengthPercentage::length(5.0)),
            border: Edges::uniform(LengthPercentage::length(2.0)),
            ..TestStyle::default()
        },
        &[follower, anchor],
    );

    let output = perform_layout(
        &mut tree,
        root,
        Size::NONE,
        Size::new(AvailableSpace::MaxContent, AvailableSpace::MaxContent),
    );

    assert_size(output.size, Size::new(39.0, 26.0));
    assert_point(tree.layout(anchor).location, Point::new(7.0, 7.0));
    assert_point(tree.layout(follower).location, Point::new(17.0, 7.0));
}

#[test]
fn wrap_width_refresh_reuses_basis_independent_fixed_measurement() {
    let mut tree = TestTree::default();
    let child = fixed_leaf(&mut tree, 12.0, 10.0);
    let root = relative_container(&mut tree, TestStyle::default(), &[child]);

    let output = perform_layout(
        &mut tree,
        root,
        Size::NONE,
        Size::new(AvailableSpace::Definite(200.0), AvailableSpace::MaxContent),
    );

    assert_size(output.size, Size::new(12.0, 10.0));
    // Root dispatch, one child measurement, and one final child commit. The
    // wrap-width refinement must reuse the measurement rather than dispatch a
    // fourth layout call under a changed parent percentage basis.
    assert_eq!(tree.session.child_layout_calls, 3);
}

#[test]
fn wrap_width_refresh_remeasures_fixed_item_when_double_anchors_tighten() {
    let mut tree = TestTree::default();
    let mut child_style = relative_leaf_style(12.0, 10.0, 1);
    child_style.relative_align.left = RelativeReference::PARENT;
    child_style.relative_align.right = RelativeReference::PARENT;
    let child = tree.push_leaf(child_style, Size::new(12.0, 10.0), None);
    let root = relative_container(
        &mut tree,
        TestStyle {
            min_size: Size::new(Dimension::Length(100.0), Dimension::Length(10.0)),
            ..TestStyle::default()
        },
        &[child],
    );

    let output = perform_layout(
        &mut tree,
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
            size: Size::new(Dimension::Percent(0.5), Dimension::Length(10.0)),
            ..TestStyle::default()
        },
        Size::new(8.0, 10.0),
        None,
    );
    let inner = relative_container(
        &mut tree,
        TestStyle {
            size: Size::new(Dimension::Length(40.0), Dimension::Length(20.0)),
            ..TestStyle::default()
        },
        &[grandchild],
    );
    let root = relative_container(&mut tree, TestStyle::default(), &[inner]);

    let output = perform_layout(
        &mut tree,
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
    child_style.relative_align.right = RelativeReference::PARENT;
    let child = tree.push_leaf(child_style, Size::new(10.0, 10.0), None);
    let root = relative_container(
        &mut tree,
        TestStyle {
            min_size: Size::new(Dimension::Length(100.0), Dimension::Length(20.0)),
            ..TestStyle::default()
        },
        &[child],
    );

    let output = perform_layout(
        &mut tree,
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
    left_style.relative_align.left = RelativeReference::PARENT;
    let left = tree.push_leaf(left_style, Size::new(10.0, 10.0), None);
    let mut right_style = relative_leaf_style(10.0, 10.0, 2);
    right_style.relative_align.right = RelativeReference::PARENT;
    let right = tree.push_leaf(right_style, Size::new(10.0, 10.0), None);
    let mut child_style = relative_leaf_style(20.0, 10.0, 3);
    child_style.relative_adjacent.right = id(2);
    child_style.relative_adjacent.left = id(1);
    let child = tree.push_leaf(child_style, Size::new(20.0, 10.0), None);
    let root = relative_container(&mut tree, TestStyle::default(), &[child, left, right]);

    definite_layout(&mut tree, root, 100.0, 20.0);

    assert_close(tree.layout(child).location.x, 100.0);
    assert_close(tree.layout(child).size.width, 0.0);
}

#[test]
fn relative_position_insets_are_visual_only_for_sibling_dependencies() {
    let mut tree = TestTree::default();
    let mut anchor_style = relative_leaf_style(10.0, 10.0, 1);
    anchor_style.inset.left = LengthPercentageAuto::Length(20.0);
    let anchor = tree.push_leaf(anchor_style, Size::new(10.0, 10.0), None);
    let mut follower_style = relative_leaf_style(10.0, 10.0, 2);
    follower_style.relative_adjacent.right = id(1);
    let follower = tree.push_leaf(follower_style, Size::new(10.0, 10.0), None);
    let root = relative_container(&mut tree, TestStyle::default(), &[follower, anchor]);

    definite_layout(&mut tree, root, 100.0, 20.0);

    assert_close(tree.layout(anchor).location.x, 20.0);
    assert_close(tree.layout(follower).location.x, 10.0);
}

#[test]
fn hidden_and_collapsed_visibility_items_remain_in_the_constraint_graph() {
    let mut tree = TestTree::default();
    let mut hidden_style = relative_leaf_style(10.0, 10.0, 1);
    hidden_style.visibility = Visibility::Hidden;
    let hidden = tree.push_leaf(hidden_style, Size::new(10.0, 10.0), None);
    let mut collapsed_style = relative_leaf_style(10.0, 10.0, 2);
    collapsed_style.visibility = Visibility::Collapse;
    collapsed_style.relative_adjacent.right = id(1);
    let collapsed = tree.push_leaf(collapsed_style, Size::new(10.0, 10.0), None);
    let root = relative_container(&mut tree, TestStyle::default(), &[collapsed, hidden]);

    definite_layout(&mut tree, root, 100.0, 20.0);

    assert_close(tree.layout(hidden).location.x, 0.0);
    assert_close(tree.layout(collapsed).location.x, 10.0);
    assert_size(tree.layout(collapsed).size, Size::new(10.0, 10.0));
}

#[test]
fn display_none_is_zeroed_and_excluded_from_relative_ids() {
    let mut tree = TestTree::default();
    let mut hidden_style = relative_leaf_style(80.0, 50.0, 1);
    hidden_style.box_generation_mode = BoxGenerationMode::None;
    let hidden = tree.push_leaf(hidden_style, Size::new(80.0, 50.0), None);
    tree.session_node_mut(hidden).layout.size = Size::new(80.0, 50.0);
    let mut child_style = relative_leaf_style(10.0, 10.0, 2);
    child_style.relative_adjacent.right = id(1);
    let child = tree.push_leaf(child_style, Size::new(10.0, 10.0), None);
    let root = relative_container(&mut tree, TestStyle::default(), &[hidden, child]);

    let output = perform_layout(
        &mut tree,
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
    absolute_style.position = Position::Absolute;
    let absolute = tree.push_leaf(absolute_style, Size::new(30.0, 20.0), None);
    let root = relative_container(
        &mut tree,
        TestStyle {
            padding: Edges::uniform(LengthPercentage::length(5.0)),
            border: Edges::uniform(LengthPercentage::length(2.0)),
            ..TestStyle::default()
        },
        &[absolute],
    );

    let output = perform_layout(
        &mut tree,
        root,
        Size::NONE,
        Size::new(AvailableSpace::MaxContent, AvailableSpace::MaxContent),
    );

    assert_size(output.size, Size::new(14.0, 14.0));
    assert_point(tree.layout(absolute).location, Point::new(2.0, 2.0));
}

#[test]
fn hoisted_children_record_padding_box_static_position_only() {
    let mut tree = TestTree::default();
    let mut fixed_style = relative_leaf_style(10.0, 10.0, 1);
    fixed_style.position = Position::AbsoluteHoisted;
    let fixed = tree.push_leaf(fixed_style, Size::new(10.0, 10.0), None);
    let root = relative_container(
        &mut tree,
        TestStyle {
            border: Edges::uniform(LengthPercentage::length(3.0)),
            ..TestStyle::default()
        },
        &[fixed],
    );

    definite_layout(&mut tree, root, 100.0, 80.0);
    assert_eq!(tree.static_position(fixed), Some(Point::new(3.0, 3.0)));
    assert_eq!(tree.layout(fixed), Layout::default());
}

#[test]
fn measure_goal_has_no_durable_geometry_side_effects_or_baseline() {
    let mut tree = TestTree::default();
    let child = relative_leaf(&mut tree, 10.0, 10.0, 1);
    let root = relative_container(&mut tree, TestStyle::default(), &[child]);
    let output = tree.session.compute_child_layout(
        &tree.source,
        root,
        LayoutInput::compute_size(
            Size::NONE,
            Size::NONE,
            Size::new(AvailableSpace::MaxContent, AvailableSpace::MaxContent),
            RequestedAxis::Both,
        ),
    );

    assert_size(output.size, Size::new(10.0, 10.0));
    assert_eq!(output.first_baselines, Point::NONE);
    assert_eq!(tree.session.layout_writes, 0);
    assert_eq!(tree.layout(child), Layout::default());
}

#[test]
fn nested_relative_containers_dispatch_through_the_host() {
    let mut tree = TestTree::default();
    let leaf = relative_leaf(&mut tree, 12.0, 8.0, 1);
    let inner = relative_container(&mut tree, TestStyle::default(), &[leaf]);
    let outer = relative_container(&mut tree, TestStyle::default(), &[inner]);

    let output = perform_layout(
        &mut tree,
        outer,
        Size::NONE,
        Size::new(AvailableSpace::MaxContent, AvailableSpace::MaxContent),
    );

    assert_size(output.size, Size::new(12.0, 8.0));
    assert_size(tree.layout(inner).size, Size::new(12.0, 8.0));
    assert_size(tree.layout(leaf).size, Size::new(12.0, 8.0));
}

#[test]
fn sibling_same_side_alignment_and_before_adjacency_cover_remaining_properties() {
    let mut tree = TestTree::default();
    let mut anchor_style = relative_leaf_style(10.0, 10.0, 1);
    anchor_style.relative_align.right = RelativeReference::PARENT;
    anchor_style.relative_align.bottom = RelativeReference::PARENT;
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

    definite_layout(&mut tree, root, 100.0, 100.0);

    assert_point(tree.layout(aligned).location, Point::new(90.0, 90.0));
    assert_point(tree.layout(before).location, Point::new(80.0, 80.0));
}

#[test]
fn intrinsic_keywords_and_fit_content_use_the_owner_constraint() {
    let mut tree = TestTree::default();
    let fit = tree.push_intrinsic_leaf(
        TestStyle {
            size: Size::new(
                Dimension::FitContent(LengthPercentage::Percent(0.5)),
                Dimension::Length(10.0),
            ),
            ..TestStyle::default()
        },
        Size::new(20.0, 10.0),
        Size::new(80.0, 10.0),
    );
    let constrained = tree.push_intrinsic_leaf(
        TestStyle {
            size: Size::new(Dimension::MaxContent, Dimension::Length(10.0)),
            min_size: Size::new(
                Dimension::FitContent(LengthPercentage::Length(30.0)),
                Dimension::Auto,
            ),
            max_size: Size::new(Dimension::MinContent, Dimension::Auto),
            ..TestStyle::default()
        },
        Size::new(20.0, 10.0),
        Size::new(80.0, 10.0),
    );
    let root = relative_container(&mut tree, TestStyle::default(), &[fit, constrained]);

    definite_layout(&mut tree, root, 100.0, 20.0);

    assert_close(tree.layout(fit).size.width, 50.0);
    // Minimum precedence applies when the intrinsic maximum is smaller.
    assert_close(tree.layout(constrained).size.width, 30.0);
}

#[test]
fn edges_use_available_width_while_child_percent_sizes_require_definiteness() {
    let mut tree = TestTree::default();
    let child = tree.push_leaf(
        TestStyle {
            size: Size::new(Dimension::Percent(0.5), Dimension::Length(10.0)),
            margin: Edges {
                left: LengthPercentageAuto::Percent(0.1),
                right: LengthPercentageAuto::Percent(0.1),
                top: LengthPercentageAuto::ZERO,
                bottom: LengthPercentageAuto::ZERO,
            },
            ..TestStyle::default()
        },
        Size::new(12.0, 10.0),
        None,
    );
    let root = relative_container(&mut tree, TestStyle::default(), &[child]);

    let output = perform_layout(
        &mut tree,
        root,
        Size::NONE,
        Size::new(AvailableSpace::Definite(200.0), AvailableSpace::MaxContent),
    );

    // Initial sizing uses 20px margins from the 200px available width and the
    // child's 12px intrinsic width, producing 52px. The two-pass percentage
    // override then resolves used values against that content-sized width.
    assert_close(output.size.width, 52.0);
    assert_close(tree.layout(child).margin.left, 5.2);
    assert_close(tree.layout(child).size.width, 26.0);
    assert_eq!(tree.session.child_layout_calls, 4);
}

#[test]
fn aspect_ratio_and_box_sizing_are_shared_with_other_layout_algorithms() {
    let mut tree = TestTree::default();
    let child = tree.push_leaf(
        TestStyle {
            size: Size::new(Dimension::Length(40.0), Dimension::Auto),
            aspect_ratio: Some(2.0),
            padding: Edges::uniform(LengthPercentage::Length(2.0)),
            ..TestStyle::default()
        },
        Size::ZERO,
        None,
    );
    let root = relative_container(&mut tree, TestStyle::default(), &[child]);

    definite_layout(&mut tree, root, 100.0, 100.0);
    assert_size(tree.layout(child).size, Size::new(44.0, 24.0));
}

#[test]
fn one_pass_keeps_wrap_fallback_positions_after_a_minimum_expands_the_parent() {
    let mut tree = TestTree::default();
    let mut child_style = relative_leaf_style(10.0, 10.0, 1);
    child_style.relative_align.right = RelativeReference::PARENT;
    let child = tree.push_leaf(child_style, Size::new(10.0, 10.0), None);
    let root = relative_container(
        &mut tree,
        TestStyle {
            min_size: Size::new(Dimension::Length(100.0), Dimension::Length(20.0)),
            relative_layout_once: true,
            ..TestStyle::default()
        },
        &[child],
    );

    let output = perform_layout(
        &mut tree,
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
            size: Size::new(Dimension::Percent(0.5), Dimension::Length(10.0)),
            ..TestStyle::default()
        },
        Size::new(20.0, 10.0),
        None,
    );
    let intrinsic_parent = relative_container(
        &mut tree,
        TestStyle {
            size: Size::new(Dimension::MaxContent, Dimension::Length(10.0)),
            relative_layout_once: true,
            ..TestStyle::default()
        },
        &[percent_child],
    );
    let root = relative_container(
        &mut tree,
        TestStyle {
            relative_layout_once: true,
            ..TestStyle::default()
        },
        &[intrinsic_parent],
    );

    definite_layout(&mut tree, root, 100.0, 20.0);

    assert_close(tree.layout(intrinsic_parent).size.width, 20.0);
    assert_close(tree.layout(percent_child).size.width, 20.0);
}

#[test]
fn double_anchor_proposal_applies_child_max_size_before_measurement() {
    let mut tree = TestTree::default();
    let mut child_style = TestStyle {
        max_size: Size::new(Dimension::Length(50.0), Dimension::Auto),
        ..TestStyle::default()
    };
    child_style.relative_align.left = RelativeReference::PARENT;
    child_style.relative_align.right = RelativeReference::PARENT;
    let child = tree.push_leaf(child_style, Size::new(200.0, 10.0), None);
    let root = relative_container(&mut tree, TestStyle::default(), &[child]);

    definite_layout(&mut tree, root, 100.0, 20.0);

    // The dependency edges remain 0..100 for downstream references, while
    // the child's used border box honors max-width.
    assert_close(tree.layout(child).location.x, 0.0);
    assert_close(tree.layout(child).size.width, 50.0);
}

fn width_sensitive_intrinsic_max(input: LeafMeasureInput) -> LeafMetrics {
    let width = input.known_dimensions.width.unwrap_or_else(|| {
        if input.available_space.width == AvailableSpace::MinContent {
            20.0
        } else {
            100.0
        }
    });
    let height = if width <= 20.0 { 50.0 } else { 10.0 };
    LeafMetrics::new(Size::new(width, height))
}

#[test]
fn intrinsic_max_width_remeasures_width_sensitive_height() {
    let mut tree = TestTree::default();
    let child = tree.push_measured_leaf(
        TestStyle {
            max_size: Size::new(Dimension::MinContent, Dimension::Auto),
            ..TestStyle::default()
        },
        width_sensitive_intrinsic_max,
    );
    let root = relative_container(&mut tree, TestStyle::default(), &[child]);

    definite_layout(&mut tree, root, 100.0, 100.0);

    assert_size(tree.layout(child).size, Size::new(20.0, 50.0));
}
