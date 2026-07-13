//! Rust-host ports of PR #25's Linear-relevant external-callback fixtures.
//!
//! The source tests reached the Rust engine through the Starlight C callback
//! table.  These ports preserve the Rust tree shapes and geometry assertions,
//! but exercise neutron-star through its native host traits and the shared
//! PR #25 compatibility facade.

mod pr25_support;
mod support;

use neutron_star::cache::Cache;
use neutron_star::geometry::Size as CoreSize;
use neutron_star::tree::{AvailableSpace, LayoutInput, LayoutOutput, RequestedAxis};
use pr25_support::{
    AlignItems, Constraints, Display, Length, LinearCrossGravity, LinearLayoutGravity,
    LinearOrientation, Point, PositionType, Rect, STICKY_AUTO_INSET, SideConstraint, SimpleNode,
    SimpleTree, Size, Style, run_rust_layout,
};

fn assert_close(actual: f32, expected: f32) {
    assert!(
        (actual - expected).abs() < 0.0001,
        "expected {expected}, got {actual}"
    );
}

#[test]
fn external_callback_display_none_child_is_zero_and_skipped_by_linear_stack() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(Style {
        display: Display::Linear,
        width: Length::points(100.0),
        ..Style::default()
    }));
    let first = tree.push(SimpleNode::new(Style {
        height: Length::points(10.0),
        ..Style::default()
    }));
    let hidden = tree.push(SimpleNode::new(Style {
        display: Display::None,
        width: Length::points(100.0),
        height: Length::points(50.0),
        ..Style::default()
    }));
    let second = tree.push(SimpleNode::new(Style {
        height: Length::points(20.0),
        ..Style::default()
    }));
    tree.append_child(root, first);
    tree.append_child(root, hidden);
    tree.append_child(root, second);

    let size = run_rust_layout(
        &mut tree,
        root,
        Constraints::new(
            SideConstraint::definite(100.0),
            SideConstraint::indefinite(),
        ),
    );

    assert_close(size.width, 100.0);
    assert_close(size.height, 30.0);
    assert_close(tree.nodes[first].layout.offset.y, 0.0);
    assert_close(tree.nodes[first].layout.size.width, 100.0);
    assert_close(tree.nodes[first].layout.size.height, 10.0);
    assert_eq!(tree.nodes[hidden].layout.offset, Point::ZERO);
    assert_eq!(tree.nodes[hidden].layout.size, Size::ZERO);
    assert_close(tree.nodes[second].layout.offset.y, 10.0);
    assert_close(tree.nodes[second].layout.size.width, 100.0);
    assert_close(tree.nodes[second].layout.size.height, 20.0);
}

#[test]
fn external_callback_sticky_percent_insets_are_exported_for_container_children() {
    fn container_style(display: Display) -> Style {
        Style {
            display,
            width: Length::points(100.0),
            height: Length::points(40.0),
            align_items: AlignItems::FlexStart,
            ..Style::default()
        }
    }

    let mut tree = SimpleTree::default();
    // The source root uses the default Block display.  The PR facade keeps
    // its child-containing Block-as-Linear dispatch rather than changing the
    // fixture into an explicitly Linear root.
    let root = tree.push(SimpleNode::new(Style {
        width: Length::points(100.0),
        ..Style::default()
    }));
    let flex = tree.push(SimpleNode::new(container_style(Display::Flex)));
    let linear = tree.push(SimpleNode::new(Style {
        linear_orientation: LinearOrientation::Horizontal,
        ..container_style(Display::Linear)
    }));
    let grid = tree.push(SimpleNode::new(Style {
        grid_template_columns: vec![Length::points(100.0)],
        grid_template_rows: vec![Length::points(40.0)],
        ..container_style(Display::Grid)
    }));
    let relative = tree.push(SimpleNode::new(container_style(Display::Relative)));

    let mut sticky_children = Vec::new();
    for container in [flex, linear, grid, relative] {
        let sticky = tree.push(SimpleNode::new(Style {
            position: PositionType::Sticky,
            width: Length::points(20.0),
            height: Length::points(10.0),
            left: Length::percent(10.0),
            top: Length::percent(25.0),
            ..Style::default()
        }));
        tree.append_child(container, sticky);
        tree.append_child(root, container);
        sticky_children.push(sticky);
    }

    let size = run_rust_layout(
        &mut tree,
        root,
        Constraints::new(
            SideConstraint::definite(100.0),
            SideConstraint::indefinite(),
        ),
    );

    assert_close(size.width, 100.0);
    assert_close(size.height, 160.0);
    // Sticky stays in normal flow in neutron-star.  The test host exports the
    // authored, containing-block-resolved metadata consumed by the later
    // scroll clamp, which is the observable part of the source callback test.
    for sticky in sticky_children {
        let layout = tree.nodes[sticky].layout;
        assert_close(layout.sticky_pos.left, 10.0);
        assert_close(layout.sticky_pos.top, 10.0);
        assert_close(layout.sticky_pos.right, STICKY_AUTO_INSET);
        assert_close(layout.sticky_pos.bottom, STICKY_AUTO_INSET);
    }
}

#[test]
fn external_callback_flex_uses_nested_linear_container_baseline() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(Style {
        display: Display::Flex,
        align_items: AlignItems::Baseline,
        ..Style::default()
    }));
    let reference = tree.push(SimpleNode::with_measured_size_and_baseline(
        Style::default(),
        Size::new(10.0, 40.0),
        35.0,
    ));
    let nested = tree.push(SimpleNode::new(Style {
        display: Display::Linear,
        linear_orientation: LinearOrientation::Horizontal,
        ..Style::default()
    }));
    let nested_early = tree.push(SimpleNode::with_measured_size_and_baseline(
        Style::default(),
        Size::new(10.0, 20.0),
        5.0,
    ));
    let nested_late = tree.push(SimpleNode::with_measured_size_and_baseline(
        Style::default(),
        Size::new(10.0, 30.0),
        25.0,
    ));
    tree.append_child(nested, nested_early);
    tree.append_child(nested, nested_late);
    tree.append_child(root, reference);
    tree.append_child(root, nested);

    let size = run_rust_layout(&mut tree, root, Constraints::indefinite());

    assert_close(size.width, 30.0);
    assert_close(size.height, 40.0);
    assert_close(tree.nodes[root].layout.baseline.unwrap(), 35.0);
    assert_close(tree.nodes[nested].layout.baseline.unwrap(), 25.0);
    assert_close(tree.nodes[reference].layout.offset.y, 0.0);
    assert_close(tree.nodes[nested].layout.offset.y, 10.0);
}

#[test]
fn external_callback_linear_baseline_keeps_unresolved_start_auto_margin() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(Style {
        display: Display::Linear,
        linear_orientation: LinearOrientation::Horizontal,
        align_items: AlignItems::FlexStart,
        width: Length::points(100.0),
        height: Length::points(100.0),
        ..Style::default()
    }));
    let child = tree.push(SimpleNode::with_measured_size_and_baseline(
        Style {
            margin: Rect::new(Length::ZERO, Length::ZERO, Length::Auto, Length::ZERO),
            ..Style::default()
        },
        Size::new(20.0, 10.0),
        4.0,
    ));
    tree.append_child(root, child);

    run_rust_layout(&mut tree, root, Constraints::definite(100.0, 100.0));

    assert_close(tree.nodes[child].layout.offset.y, 90.0);
    assert_close(tree.nodes[child].layout.margin.top, 90.0);
    assert_close(tree.nodes[child].layout.margin.bottom, 0.0);
    // Unlike the source engine's pre-alignment callback export (4), current
    // Linear L1 computes the baseline after resolving the start auto margin.
    assert_close(tree.nodes[root].layout.baseline.unwrap(), 94.0);
}

#[test]
fn external_callback_linear_baseline_uses_gravity_before_paired_auto_margins_resolve() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(Style {
        display: Display::Linear,
        linear_orientation: LinearOrientation::Horizontal,
        linear_cross_gravity: LinearCrossGravity::End,
        width: Length::points(100.0),
        height: Length::points(100.0),
        ..Style::default()
    }));
    let child = tree.push(SimpleNode::with_measured_size_and_baseline(
        Style {
            margin: Rect::new(Length::ZERO, Length::ZERO, Length::Auto, Length::Auto),
            ..Style::default()
        },
        Size::new(20.0, 10.0),
        4.0,
    ));
    tree.append_child(root, child);

    run_rust_layout(&mut tree, root, Constraints::definite(100.0, 100.0));

    assert_close(tree.nodes[child].layout.offset.y, 45.0);
    assert_close(tree.nodes[child].layout.margin.top, 45.0);
    assert_close(tree.nodes[child].layout.margin.bottom, 45.0);
    // Current Linear L1 exports the post-auto-margin baseline, not the source
    // engine's pre-auto-margin End-gravity value (94).
    assert_close(tree.nodes[root].layout.baseline.unwrap(), 49.0);
}

fn linear_child_cross_size(child_style: Style, parent_height: f32) -> f32 {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(Style {
        display: Display::Linear,
        linear_orientation: LinearOrientation::Horizontal,
        width: Length::points(100.0),
        height: Length::points(parent_height),
        ..Style::default()
    }));
    let child = tree.push(SimpleNode::new(child_style));
    tree.append_child(root, child);
    run_rust_layout(&mut tree, root, Constraints::definite(100.0, parent_height));
    tree.nodes[child].layout.size.height
}

#[test]
fn linear_cross_axis_cache_guard_covers_ignored_stretch_and_auto_children() {
    // PR #25 needed an engine-private tree scan before reusing a result under
    // a newly definite Linear cross constraint.  Neutron-star's public cache
    // instead keys the complete LayoutInput, so a max-content measurement
    // cannot answer either definite-cross request even when the old result's
    // numeric size happens to match.
    let intrinsic_input = LayoutInput::compute_size(
        CoreSize::NONE,
        CoreSize::new(Some(100.0), None),
        CoreSize::new(AvailableSpace::Definite(100.0), AvailableSpace::MaxContent),
        RequestedAxis::Both,
    );
    let definite_20_input = LayoutInput::compute_size(
        CoreSize::NONE,
        CoreSize::new(Some(100.0), Some(20.0)),
        CoreSize::new(
            AvailableSpace::Definite(100.0),
            AvailableSpace::Definite(20.0),
        ),
        RequestedAxis::Both,
    );
    let definite_40_input = LayoutInput::compute_size(
        CoreSize::NONE,
        CoreSize::new(Some(100.0), Some(40.0)),
        CoreSize::new(
            AvailableSpace::Definite(100.0),
            AvailableSpace::Definite(40.0),
        ),
        RequestedAxis::Both,
    );
    let intrinsic_output = LayoutOutput::new(CoreSize::new(20.0, 10.0), CoreSize::new(20.0, 10.0));
    let definite_20_output =
        LayoutOutput::new(CoreSize::new(20.0, 20.0), CoreSize::new(20.0, 20.0));
    let mut cache = Cache::new();
    cache.store(intrinsic_input, intrinsic_output);
    assert_eq!(cache.get(intrinsic_input), Some(intrinsic_output));
    assert_eq!(cache.get(definite_20_input), None);
    cache.store(definite_20_input, definite_20_output);
    assert_eq!(cache.get(definite_20_input), Some(definite_20_output));
    assert_eq!(cache.get(definite_40_input), None);

    // Preserve all three source tree classifications with executable public
    // layout checks. None/absolute children do not affect Linear natural
    // cross size, while explicit Stretch and implicit auto-cross children do.
    let mut ignored_tree = SimpleTree::default();
    let ignored_parent = ignored_tree.push(SimpleNode::new(Style {
        display: Display::Linear,
        linear_orientation: LinearOrientation::Horizontal,
        ..Style::default()
    }));
    let display_none_child = ignored_tree.push(SimpleNode::new(Style {
        display: Display::None,
        ..Style::default()
    }));
    let out_of_flow_child = ignored_tree.push(SimpleNode::new(Style {
        position: PositionType::Absolute,
        ..Style::default()
    }));
    ignored_tree.append_child(ignored_parent, display_none_child);
    ignored_tree.append_child(ignored_parent, out_of_flow_child);
    let ignored_size = run_rust_layout(
        &mut ignored_tree,
        ignored_parent,
        Constraints::new(
            SideConstraint::definite(100.0),
            SideConstraint::indefinite(),
        ),
    );
    assert_close(ignored_size.height, 0.0);
    assert_eq!(
        ignored_tree.nodes[display_none_child].layout.size,
        Size::ZERO
    );

    let stretch_style = Style {
        linear_layout_gravity: LinearLayoutGravity::Stretch,
        width: Length::points(20.0),
        height: Length::points(10.0),
        ..Style::default()
    };
    assert_close(linear_child_cross_size(stretch_style.clone(), 20.0), 20.0);
    assert_close(linear_child_cross_size(stretch_style, 40.0), 40.0);
    assert_close(linear_child_cross_size(Style::default(), 20.0), 20.0);
    assert_close(linear_child_cross_size(Style::default(), 40.0), 40.0);
}
