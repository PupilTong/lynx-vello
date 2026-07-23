//! CSS containment (`css-contain-2`) conformance over the shared mock host:
//! size containment across all four algorithms and the leaf, layout-containment
//! baseline suppression, and `content-visibility` skipped-contents behavior.

mod support;

use neutron_star::prelude::*;
use neutron_star::style::{Contain, Overflow};
use stylo::computed_values::relative_layout_once;
use support::*;

fn rigid_leaf(tree: &mut TestTree, width: f32, height: f32) -> TestId {
    let style = TestStyle {
        size: Size::new(size_px(width), size_px(height)),
        flex_basis: basis_px(width),
        flex_grow: nn(0.0),
        flex_shrink: nn(0.0),
        ..TestStyle::default()
    };
    tree.push_leaf(style, Size::new(width, height), None)
}

fn size_contained(mut style: TestStyle, width: f32, height: f32) -> TestStyle {
    style.containment = Contain::SIZE;
    style.contain_intrinsic_width = contain_intrinsic_px(width);
    style.contain_intrinsic_height = contain_intrinsic_px(height);
    style
}

fn measure(tree: &TestTree, node: TestId, available: Size<AvailableSpace>) -> LayoutOutput {
    measure_layout(tree, node, Size::NONE, available)
}

#[test]
fn flex_size_containment_substitutes_intrinsic_and_still_lays_out_children() {
    let mut tree = TestTree::default();
    let child = rigid_leaf(&mut tree, 200.0, 100.0);
    let container = flex_container(
        &mut tree,
        size_contained(TestStyle::default(), 50.0, 30.0),
        &[child],
    );

    let output = perform_layout(&tree, container, Size::NONE, Size::MAX_CONTENT);

    assert_size(output.size, Size::new(50.0, 30.0));
    assert_size(tree.layout(child).size, Size::new(200.0, 100.0));
    assert!(output.content_size.width >= 200.0);
}

#[test]
fn flex_min_content_probe_of_a_contained_child_reports_intrinsic() {
    let mut tree = TestTree::default();
    let inner = rigid_leaf(&mut tree, 300.0, 120.0);
    let contained_child = flex_container(
        &mut tree,
        size_contained(TestStyle::default(), 40.0, 24.0),
        &[inner],
    );

    let min = measure(&tree, contained_child, Size::MIN_CONTENT);
    let max = measure(&tree, contained_child, Size::MAX_CONTENT);
    assert_size(min.size, Size::new(40.0, 24.0));
    assert_size(max.size, Size::new(40.0, 24.0));

    let parent = flex_container(&mut tree, TestStyle::default(), &[contained_child]);
    let output = perform_layout(&tree, parent, Size::NONE, Size::MAX_CONTENT);
    assert_close(output.size.width, 40.0);
    assert_size(tree.layout(inner).size, Size::new(300.0, 120.0));
}

#[test]
fn flex_automatic_minimum_uses_the_contained_min_content() {
    fn item(tree: &mut TestTree, contained: bool) -> TestId {
        let inner = rigid_leaf(tree, 300.0, 50.0);
        let mut style = TestStyle {
            flex_basis: basis_px(300.0),
            flex_grow: nn(0.0),
            flex_shrink: nn(1.0),
            ..TestStyle::default()
        };
        if contained {
            style = size_contained(style, 20.0, 20.0);
        }
        flex_container(tree, style, &[inner])
    }

    let mut tree = TestTree::default();
    let uncontained_item = item(&mut tree, false);
    let uncontained_parent = flex_container(&mut tree, TestStyle::default(), &[uncontained_item]);
    definite_layout(&tree, uncontained_parent, 100.0, 200.0);
    assert_close(tree.layout(uncontained_item).size.width, 300.0);

    let mut tree = TestTree::default();
    let contained_item = item(&mut tree, true);
    let contained_parent = flex_container(&mut tree, TestStyle::default(), &[contained_item]);
    definite_layout(&tree, contained_parent, 100.0, 200.0);
    assert_close(tree.layout(contained_item).size.width, 100.0);
}

#[test]
fn grid_size_containment_substitutes_intrinsic_and_still_lays_out_children() {
    let mut tree = TestTree::default();
    let child = rigid_leaf(&mut tree, 60.0, 40.0);
    let container = tree.push_grid(
        size_contained(TestStyle::default(), 50.0, 30.0),
        vec![child],
    );

    let output = perform_layout(&tree, container, Size::NONE, Size::MAX_CONTENT);

    assert_size(output.size, Size::new(50.0, 30.0));
    assert!(tree.layout(child).size.width > 0.0);
    assert!(tree.layout(child).size.height > 0.0);
}

#[test]
fn linear_size_containment_substitutes_intrinsic_and_still_lays_out_children() {
    let mut tree = TestTree::default();
    let child = rigid_leaf(&mut tree, 70.0, 45.0);
    let container = linear_container(
        &mut tree,
        size_contained(TestStyle::default(), 50.0, 30.0),
        &[child],
    );

    let output = perform_layout(&tree, container, Size::NONE, Size::MAX_CONTENT);

    assert_size(output.size, Size::new(50.0, 30.0));
    assert_size(tree.layout(child).size, Size::new(70.0, 45.0));
}

#[test]
fn relative_size_containment_substitutes_intrinsic_and_still_lays_out_children() {
    let mut tree = TestTree::default();
    let child = rigid_leaf(&mut tree, 80.0, 55.0);
    let container = relative_container(
        &mut tree,
        size_contained(TestStyle::default(), 50.0, 30.0),
        &[child],
    );

    let output = perform_layout(&tree, container, Size::NONE, Size::MAX_CONTENT);

    assert_size(output.size, Size::new(50.0, 30.0));
    assert_size(tree.layout(child).size, Size::new(80.0, 55.0));
}

#[test]
fn linear_size_containment_resolves_child_edges_against_the_contained_inline_size() {
    fn build(
        make: fn(&mut TestTree, TestStyle, &[TestId]) -> TestId,
    ) -> (TestTree, TestId, TestId) {
        let mut tree = TestTree::default();
        let mut child_style = TestStyle {
            size: Size::new(size_px(20.0), size_px(20.0)),
            flex_basis: basis_px(20.0),
            flex_grow: nn(0.0),
            flex_shrink: nn(0.0),
            ..TestStyle::default()
        };
        child_style.padding.left = npct(0.5);
        let child = tree.push_leaf(child_style, Size::new(20.0, 20.0), None);
        let container = make(
            &mut tree,
            size_contained(TestStyle::default(), 100.0, 40.0),
            &[child],
        );
        (tree, container, child)
    }

    let (linear_tree, linear_container, linear_child) = build(linear_container);
    let linear_output = perform_layout(
        &linear_tree,
        linear_container,
        Size::NONE,
        Size::MAX_CONTENT,
    );
    assert_close(linear_output.size.width, 100.0);
    assert_close(linear_tree.layout(linear_child).padding.left, 50.0);

    let (flex_tree, flex_container_id, flex_child) = build(flex_container);
    let flex_output = perform_layout(&flex_tree, flex_container_id, Size::NONE, Size::MAX_CONTENT);
    assert_close(flex_output.size.width, 100.0);
    assert_close(
        flex_tree.layout(flex_child).padding.left,
        linear_tree.layout(linear_child).padding.left,
    );
}

#[test]
fn relative_two_pass_size_containment_lays_children_against_the_contained_size() {
    let mut tree = TestTree::default();
    let child = rigid_leaf(&mut tree, 90.0, 60.0);
    let mut style = size_contained(TestStyle::default(), 50.0, 30.0);
    style.relative_layout_once = relative_layout_once::T::False;
    let container = relative_container(&mut tree, style, &[child]);

    let output = perform_layout(&tree, container, Size::NONE, Size::MAX_CONTENT);
    assert_size(output.size, Size::new(50.0, 30.0));
    assert_size(tree.layout(child).size, Size::new(90.0, 60.0));
}

#[test]
fn relative_one_pass_size_containment_substitutes_intrinsic() {
    let mut tree = TestTree::default();
    let child = rigid_leaf(&mut tree, 90.0, 60.0);
    let mut style = size_contained(TestStyle::default(), 50.0, 30.0);
    style.relative_layout_once = relative_layout_once::T::True;
    let container = relative_container(&mut tree, style, &[child]);

    let output = perform_layout(&tree, container, Size::NONE, Size::MAX_CONTENT);
    assert_size(output.size, Size::new(50.0, 30.0));
    assert_size(tree.layout(child).size, Size::new(90.0, 60.0));
}

#[test]
fn leaf_size_containment_skips_the_measurer() {
    let mut tree = TestTree::default();
    let leaf = tree
        .push_measured_leaf(size_contained(TestStyle::default(), 50.0, 30.0), |_input| {
            LeafMetrics::new(Size::new(999.0, 999.0))
        });

    let output = perform_layout(&tree, leaf, Size::NONE, Size::MAX_CONTENT);

    assert_size(output.size, Size::new(50.0, 30.0));
    assert_eq!(output.first_baselines, Point::NONE);
    assert_eq!(tree.leaf_measure_calls.get(), 0);
}

#[test]
fn leaf_without_containment_still_calls_its_measurer() {
    let mut tree = TestTree::default();
    let leaf = tree.push_measured_leaf(TestStyle::default(), |_input| {
        LeafMetrics::new(Size::new(999.0, 999.0))
    });

    let output = perform_layout(&tree, leaf, Size::NONE, Size::MAX_CONTENT);

    assert_size(output.size, Size::new(999.0, 999.0));
    assert_eq!(tree.leaf_measure_calls.get(), 1);
}

fn baseline_child(tree: &mut TestTree) -> TestId {
    let style = TestStyle {
        size: Size::new(size_px(40.0), size_px(20.0)),
        flex_basis: basis_px(40.0),
        flex_grow: nn(0.0),
        flex_shrink: nn(0.0),
        ..TestStyle::default()
    };
    tree.push_leaf(style, Size::new(40.0, 20.0), Some(12.0))
}

fn layout_contained(style: TestStyle) -> TestStyle {
    TestStyle {
        containment: Contain::LAYOUT,
        ..style
    }
}

#[test]
fn flex_layout_containment_suppresses_the_exported_baseline() {
    let mut tree = TestTree::default();
    let child = baseline_child(&mut tree);
    let container = flex_container(&mut tree, TestStyle::default(), &[child]);
    let normal = perform_layout(&tree, container, Size::NONE, Size::MAX_CONTENT);
    assert!(normal.first_baselines.y.is_some());

    let mut tree = TestTree::default();
    let child = baseline_child(&mut tree);
    let container = flex_container(&mut tree, layout_contained(TestStyle::default()), &[child]);
    let committed = perform_layout(&tree, container, Size::NONE, Size::MAX_CONTENT);
    assert_eq!(committed.first_baselines.y, None);
    let measured = measure(&tree, container, Size::MAX_CONTENT);
    assert_eq!(measured.first_baselines.y, None);
}

#[test]
fn grid_layout_containment_suppresses_the_exported_baseline() {
    let mut tree = TestTree::default();
    let child = baseline_child(&mut tree);
    let container = tree.push_grid(TestStyle::default(), vec![child]);
    let normal = perform_layout(&tree, container, Size::NONE, Size::MAX_CONTENT);
    assert!(normal.first_baselines.y.is_some());

    let mut tree = TestTree::default();
    let child = baseline_child(&mut tree);
    let container = tree.push_grid(layout_contained(TestStyle::default()), vec![child]);
    let committed = perform_layout(&tree, container, Size::NONE, Size::MAX_CONTENT);
    assert_eq!(committed.first_baselines.y, None);
}

#[test]
fn linear_layout_containment_suppresses_the_exported_baseline() {
    let mut tree = TestTree::default();
    let child = baseline_child(&mut tree);
    let container = linear_container(&mut tree, TestStyle::default(), &[child]);
    let normal = perform_layout(&tree, container, Size::NONE, Size::MAX_CONTENT);
    assert!(normal.first_baselines.y.is_some());

    let mut tree = TestTree::default();
    let child = baseline_child(&mut tree);
    let container = linear_container(&mut tree, layout_contained(TestStyle::default()), &[child]);
    let committed = perform_layout(&tree, container, Size::NONE, Size::MAX_CONTENT);
    assert_eq!(committed.first_baselines.y, None);
    let measured = measure(&tree, container, Size::MAX_CONTENT);
    assert_eq!(measured.first_baselines.y, None);
}

#[test]
fn scroll_container_child_contributes_only_its_border_box() {
    let mut tree = TestTree::default();
    let inner = rigid_leaf(&mut tree, 200.0, 100.0);
    let scroll = flex_container(
        &mut tree,
        TestStyle {
            size: Size::new(size_px(50.0), size_px(30.0)),
            overflow: Point::new(Overflow::Hidden, Overflow::Hidden),
            ..TestStyle::default()
        },
        &[inner],
    );
    let parent = flex_container(&mut tree, TestStyle::default(), &[scroll]);

    let output = perform_layout(&tree, parent, Size::NONE, Size::MAX_CONTENT);

    assert!(tree.layout(scroll).content_size.width >= 200.0);
    assert!(tree.layout(scroll).content_size.height >= 100.0);
    assert_size(output.content_size, Size::new(50.0, 30.0));
}

#[test]
fn layout_contained_visible_box_excludes_descendant_overflow() {
    let mut tree = TestTree::default();
    let child = rigid_leaf(&mut tree, 200.0, 100.0);
    let container = flex_container(
        &mut tree,
        layout_contained(TestStyle {
            size: Size::new(size_px(50.0), size_px(30.0)),
            ..TestStyle::default()
        }),
        &[child],
    );

    let output = perform_layout(&tree, container, Size::NONE, Size::MAX_CONTENT);

    assert_size(tree.layout(child).size, Size::new(200.0, 100.0));
    assert_size(output.content_size, Size::new(50.0, 30.0));
}

#[test]
fn layout_contained_scroll_container_keeps_its_interior_scroll_range() {
    let mut tree = TestTree::default();
    let child = rigid_leaf(&mut tree, 200.0, 100.0);
    let mut style = layout_contained(TestStyle {
        size: Size::new(size_px(50.0), size_px(30.0)),
        ..TestStyle::default()
    });
    style.overflow = Point::new(Overflow::Hidden, Overflow::Hidden);
    let container = flex_container(&mut tree, style, &[child]);

    let output = perform_layout(&tree, container, Size::NONE, Size::MAX_CONTENT);

    assert_size(tree.layout(child).size, Size::new(200.0, 100.0));
    assert!(output.content_size.width >= 200.0);
    assert!(output.content_size.height >= 100.0);
}

fn skipped_style(width: f32, height: f32) -> TestStyle {
    TestStyle {
        skips_contents: true,
        containment: Contain::STRICT,
        contain_intrinsic_width: contain_intrinsic_px(width),
        contain_intrinsic_height: contain_intrinsic_auto_px(height),
        ..TestStyle::default()
    }
}

#[test]
fn skipped_contents_size_from_intrinsic_and_hidden_children() {
    let mut tree = TestTree::default();
    let child = rigid_leaf(&mut tree, 200.0, 100.0);
    let container = flex_container(&mut tree, skipped_style(60.0, 40.0), &[child]);

    let output = perform_layout(&tree, container, Size::NONE, Size::MAX_CONTENT);

    assert_size(output.size, Size::new(60.0, 40.0));
    assert_eq!(output.first_baselines, Point::NONE);
    assert_size(output.content_size, Size::new(60.0, 40.0));
    assert_eq!(tree.layout(child).size, Size::ZERO);
}

#[test]
fn skipped_contents_measure_probe_does_not_hide_children() {
    let mut tree = TestTree::default();
    let child = rigid_leaf(&mut tree, 200.0, 100.0);
    let container = flex_container(&mut tree, skipped_style(60.0, 40.0), &[child]);
    let mut stale = tree.layout(child);
    stale.size = Size::new(7.0, 7.0);
    *tree.session_node(child).layout.borrow_mut() = stale;

    let output = measure(&tree, container, Size::MAX_CONTENT);
    assert_size(output.size, Size::new(60.0, 40.0));
    assert_size(tree.layout(child).size, Size::new(7.0, 7.0));
}

#[test]
fn normal_to_skipped_transition_cleans_stale_child_layout() {
    let mut tree = TestTree::default();
    let child = rigid_leaf(&mut tree, 30.0, 20.0);
    let container = flex_container(&mut tree, TestStyle::default(), &[child]);

    perform_layout(&tree, container, Size::NONE, Size::MAX_CONTENT);
    assert_size(tree.layout(child).size, Size::new(30.0, 20.0));

    tree.source_node_mut(container).style = skipped_style(40.0, 24.0);
    let output = perform_layout(&tree, container, Size::NONE, Size::MAX_CONTENT);
    assert_size(output.size, Size::new(40.0, 24.0));
    assert_eq!(tree.layout(child).size, Size::ZERO);
}

#[test]
fn skipped_to_normal_transition_lays_children_out_again() {
    let mut tree = TestTree::default();
    let child = rigid_leaf(&mut tree, 30.0, 20.0);
    let container = flex_container(&mut tree, skipped_style(40.0, 24.0), &[child]);

    perform_layout(&tree, container, Size::NONE, Size::MAX_CONTENT);
    assert_eq!(tree.layout(child).size, Size::ZERO);

    tree.source_node_mut(container).style = TestStyle::default();
    perform_layout(&tree, container, Size::NONE, Size::MAX_CONTENT);
    assert_size(tree.layout(child).size, Size::new(30.0, 20.0));
}
