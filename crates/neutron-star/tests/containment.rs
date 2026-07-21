//! CSS containment (`css-contain-2`) conformance over the shared mock host:
//! size containment across all four algorithms and the leaf, layout-containment
//! baseline suppression, and `content-visibility` skipped-contents behavior.
//!
//! Damage-driven, containment-bounded cache invalidation
//! (`is_relayout_boundary`/`invalidate_for_relayout`) lives in
//! `tests/protocol.rs`, which owns the cache-tracking mock.

mod support;

use neutron_star::prelude::*;
use neutron_star::style::{Contain, Overflow};
use stylo::computed_values::relative_layout_once;
use support::*;

/// A leaf that neither grows nor shrinks, so it keeps its intrinsic size and
/// overflows a size-contained container rather than being flexed to fit.
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

/// Adds `contain: size` plus a `contain-intrinsic-size` substitute to a style.
fn size_contained(mut style: TestStyle, width: f32, height: f32) -> TestStyle {
    style.containment = Contain::SIZE;
    style.contain_intrinsic_width = contain_intrinsic_px(width);
    style.contain_intrinsic_height = contain_intrinsic_px(height);
    style
}

fn measure(tree: &TestTree, node: TestId, available: Size<AvailableSpace>) -> LayoutOutput {
    measure_layout(tree, node, Size::NONE, available)
}

// ---------------------------------------------------------------------------
// Size containment: the container's own size ignores its contents.
// ---------------------------------------------------------------------------

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

    // The container is sized purely from contain-intrinsic-size, ignoring the
    // 200x100 child.
    assert_size(output.size, Size::new(50.0, 30.0));
    // The child is still laid out (rigid, so it overflows).
    assert_size(tree.layout(child).size, Size::new(200.0, 100.0));
    // Scrollable overflow still reflects the laid-out child.
    assert!(output.content_size.width >= 200.0);
}

#[test]
fn flex_min_content_probe_of_a_contained_child_reports_intrinsic() {
    // A parent flex row auto-sizes from its children's contributions. A
    // size-contained flex child must contribute its contain-intrinsic-size
    // (as if empty) — this is the §4.5 automatic-minimum mechanism too.
    let mut tree = TestTree::default();
    let inner = rigid_leaf(&mut tree, 300.0, 120.0);
    let contained_child = flex_container(
        &mut tree,
        size_contained(TestStyle::default(), 40.0, 24.0),
        &[inner],
    );

    // Directly probe min- and max-content: both are the contained size.
    let min = measure(&tree, contained_child, Size::MIN_CONTENT);
    let max = measure(&tree, contained_child, Size::MAX_CONTENT);
    assert_size(min.size, Size::new(40.0, 24.0));
    assert_size(max.size, Size::new(40.0, 24.0));

    // And through a parent that auto-sizes to the child.
    let parent = flex_container(&mut tree, TestStyle::default(), &[contained_child]);
    let output = perform_layout(&tree, parent, Size::NONE, Size::MAX_CONTENT);
    assert_close(output.size.width, 40.0);
    // The deeply-nested rigid leaf is still laid out despite containment.
    assert_size(tree.layout(inner).size, Size::new(300.0, 120.0));
}

#[test]
fn flex_automatic_minimum_uses_the_contained_min_content() {
    // A shrinkable item with a definite flex-basis larger than the parent is
    // floored by its automatic minimum size (§4.5). Size containment lowers
    // that floor to the contain-intrinsic min-content, letting the item shrink.
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

    // Uncontained: automatic minimum = 300, so the item cannot shrink below it.
    let mut tree = TestTree::default();
    let uncontained_item = item(&mut tree, false);
    let uncontained_parent = flex_container(&mut tree, TestStyle::default(), &[uncontained_item]);
    definite_layout(&tree, uncontained_parent, 100.0, 200.0);
    assert_close(tree.layout(uncontained_item).size.width, 300.0);

    // Contained: automatic minimum drops to 20, so the item shrinks to 100.
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
    // The child is still placed and laid out into the (now definite) grid.
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
    // A child edge that cyclically depends on the container's inline size
    // (`padding-left: 50%`) must resolve against the container's *contained*
    // inline size (`contain-intrinsic-width`), not the items' uncontained
    // natural extent. Linear must agree with flexbox here.
    fn build(
        make: fn(&mut TestTree, TestStyle, &[TestId]) -> TestId,
    ) -> (TestTree, TestId, TestId) {
        let mut tree = TestTree::default();
        // The child's own natural width (20) is far from the container's
        // contained inline size (100), so the two candidate percentage bases
        // resolve `padding-left: 50%` to visibly different values (10 vs 50).
        let mut child_style = TestStyle {
            size: Size::new(size_px(20.0), size_px(20.0)),
            flex_basis: basis_px(20.0),
            flex_grow: nn(0.0),
            flex_shrink: nn(0.0),
            ..TestStyle::default()
        };
        child_style.padding.left = npct(0.5);
        let child = tree.push_leaf(child_style, Size::new(20.0, 20.0), None);
        // `contain: size`, width auto (intrinsic) => contained inline size 100.
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
    // Sized purely from contain-intrinsic-size.
    assert_close(linear_output.size.width, 100.0);
    // `padding-left: 50%` resolves against the contained inline size (=> 50),
    // never the ~20 natural extent (which would give 10).
    assert_close(linear_tree.layout(linear_child).padding.left, 50.0);

    // Flexbox resolves the identical tree the same way (contained basis).
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
    // relative-layout-once = false exercises the two-pass sizing path.
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
    // relative-layout-once = true exercises the single-pass sizing path.
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

    // The measurer's 999x999 is never consulted; the size is contain-intrinsic.
    assert_size(output.size, Size::new(50.0, 30.0));
    assert_eq!(output.first_baselines, Point::NONE);
    assert_eq!(tree.leaf_measure_calls.get(), 0);
}

#[test]
fn leaf_without_containment_still_calls_its_measurer() {
    // Control: the same leaf without size containment measures normally.
    let mut tree = TestTree::default();
    let leaf = tree.push_measured_leaf(TestStyle::default(), |_input| {
        LeafMetrics::new(Size::new(999.0, 999.0))
    });

    let output = perform_layout(&tree, leaf, Size::NONE, Size::MAX_CONTENT);

    assert_size(output.size, Size::new(999.0, 999.0));
    assert_eq!(tree.leaf_measure_calls.get(), 1);
}

// ---------------------------------------------------------------------------
// Layout containment: the exported container baseline is suppressed.
// ---------------------------------------------------------------------------

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
    // Without containment the flex container exports its first item's baseline.
    let mut tree = TestTree::default();
    let child = baseline_child(&mut tree);
    let container = flex_container(&mut tree, TestStyle::default(), &[child]);
    let normal = perform_layout(&tree, container, Size::NONE, Size::MAX_CONTENT);
    assert!(normal.first_baselines.y.is_some());

    // Layout containment nulls it (both on measure and commit).
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

// ---------------------------------------------------------------------------
// Scrollable-overflow trapping: css-overflow-3 §3.3 (scroll containers) and
// css-contain-2 §3.3 (layout containment collapses to ink overflow).
// ---------------------------------------------------------------------------

#[test]
fn scroll_container_child_contributes_only_its_border_box() {
    // css-overflow-3 §3.3: a scroll-container child's interior scrollable
    // overflow is trapped — it keeps its own content_size as its scroll range
    // but contributes only its border box to the parent's scrollable overflow.
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

    // The scroll child keeps its full interior union as its own scroll range...
    assert!(tree.layout(scroll).content_size.width >= 200.0);
    assert!(tree.layout(scroll).content_size.height >= 100.0);
    // ...but the parent's scrollable overflow sees only its 50x30 border box.
    assert_size(output.content_size, Size::new(50.0, 30.0));
}

#[test]
fn layout_contained_visible_box_excludes_descendant_overflow() {
    // css-contain-2 §3.3 (item 3): with overflow: visible a layout-contained
    // box treats descendant overflow as ink overflow, so its scrollable overflow
    // is just its border box — even though the child is laid out and overflows.
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

    // The child is laid out and overflows the 50x30 box...
    assert_size(tree.layout(child).size, Size::new(200.0, 100.0));
    // ...but scrollable overflow equals the border box (descendant is ink-only).
    assert_size(output.content_size, Size::new(50.0, 30.0));
}

#[test]
fn layout_contained_scroll_container_keeps_its_interior_scroll_range() {
    // A layout-contained box that is ALSO a scroll container reports its full
    // interior union as its own scroll range (css-overflow-3): §3.3's ink-only
    // treatment applies only with overflow: visible. Trapping happens toward the
    // ancestor, not within the box itself.
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
    // Scroll container => own content_size is the interior union, not the border box.
    assert!(output.content_size.width >= 200.0);
    assert!(output.content_size.height >= 100.0);
}

// ---------------------------------------------------------------------------
// content-visibility skipped contents + transitions.
// ---------------------------------------------------------------------------

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

    // Sized from contain-intrinsic-size (auto <len> == <len> in v1), no baseline.
    assert_size(output.size, Size::new(60.0, 40.0));
    assert_eq!(output.first_baselines, Point::NONE);
    // No overflowing content: content_size equals the border box.
    assert_size(output.content_size, Size::new(60.0, 40.0));
    // The child subtree is hidden (zeroed), not laid out.
    assert_eq!(tree.layout(child).size, Size::ZERO);
}

#[test]
fn skipped_contents_measure_probe_does_not_hide_children() {
    let mut tree = TestTree::default();
    let child = rigid_leaf(&mut tree, 200.0, 100.0);
    let container = flex_container(&mut tree, skipped_style(60.0, 40.0), &[child]);
    // Prime stale child geometry.
    let mut stale = tree.layout(child);
    stale.size = Size::new(7.0, 7.0);
    *tree.session_node(child).layout.borrow_mut() = stale;

    // A measure probe must stay side-effect free (no hiding).
    let output = measure(&tree, container, Size::MAX_CONTENT);
    assert_size(output.size, Size::new(60.0, 40.0));
    assert_size(tree.layout(child).size, Size::new(7.0, 7.0));
}

#[test]
fn normal_to_skipped_transition_cleans_stale_child_layout() {
    let mut tree = TestTree::default();
    let child = rigid_leaf(&mut tree, 30.0, 20.0);
    let container = flex_container(&mut tree, TestStyle::default(), &[child]);

    // First lay out normally: the child gets real geometry.
    perform_layout(&tree, container, Size::NONE, Size::MAX_CONTENT);
    assert_size(tree.layout(child).size, Size::new(30.0, 20.0));

    // Flip the container to content-visibility skipping and re-lay out.
    tree.source_node_mut(container).style = skipped_style(40.0, 24.0);
    let output = perform_layout(&tree, container, Size::NONE, Size::MAX_CONTENT);
    assert_size(output.size, Size::new(40.0, 24.0));
    // Stale child geometry from the earlier pass is cleaned.
    assert_eq!(tree.layout(child).size, Size::ZERO);
}

#[test]
fn skipped_to_normal_transition_lays_children_out_again() {
    let mut tree = TestTree::default();
    let child = rigid_leaf(&mut tree, 30.0, 20.0);
    let container = flex_container(&mut tree, skipped_style(40.0, 24.0), &[child]);

    // Skipped first: the child is hidden.
    perform_layout(&tree, container, Size::NONE, Size::MAX_CONTENT);
    assert_eq!(tree.layout(child).size, Size::ZERO);

    // Flip back to a normal flex container: the child is laid out again.
    tree.source_node_mut(container).style = TestStyle::default();
    perform_layout(&tree, container, Size::NONE, Size::MAX_CONTENT);
    assert_size(tree.layout(child).size, Size::new(30.0, 20.0));
}
