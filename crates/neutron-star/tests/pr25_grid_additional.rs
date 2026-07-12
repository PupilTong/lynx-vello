//! Grid-related PR #25 tests that live outside its Grid-specific suites.

mod pr25_support;
mod support;

use pr25_support::*;

fn assert_close(actual: f32, expected: f32) {
    assert!(
        (actual - expected).abs() <= 0.01,
        "expected {expected}, got {actual}"
    );
}

#[test]
fn grid_auto_row_grows_from_child_aspect_ratio() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(Style {
        display: Display::Grid,
        width: Length::points(80.0),
        grid_template_columns: vec![Length::points(80.0)],
        ..Style::default()
    }));
    let child = tree.push(SimpleNode::new(Style {
        width: Length::points(80.0),
        aspect_ratio: Some(2.0),
        ..Style::default()
    }));
    tree.append_child(root, child);
    let size = run_rust_layout(&mut tree, root, Constraints::indefinite());
    assert_close(size.width, 80.0);
    assert_close(size.height, 40.0);
}

fn sticky_grid(start: bool) -> (SimpleTree, usize, usize) {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(Style {
        display: Display::Grid,
        width: Length::points(100.0),
        height: Length::points(40.0),
        grid_template_columns: vec![Length::points(100.0)],
        grid_template_rows: vec![Length::points(40.0)],
        ..Style::default()
    }));
    let child = tree.push(SimpleNode::new(Style {
        position: PositionType::Sticky,
        width: Length::points(20.0),
        height: Length::points(10.0),
        left: if start {
            Length::percent(10.0)
        } else {
            Length::Auto
        },
        right: if start {
            Length::Auto
        } else {
            Length::percent(20.0)
        },
        top: if start {
            Length::percent(25.0)
        } else {
            Length::Auto
        },
        bottom: if start {
            Length::Auto
        } else {
            Length::percent(50.0)
        },
        justify_self: JustifyItems::Start,
        align_self: Some(AlignItems::Start),
        ..Style::default()
    }));
    tree.append_child(root, child);
    (tree, root, child)
}

#[test]
fn grid_sticky_child_percent_insets_resolve_against_container_constraints() {
    let (mut tree, root, child) = sticky_grid(true);
    run_rust_layout(&mut tree, root, Constraints::definite(100.0, 40.0));
    // neutron-star exports in-flow relative geometry; the host's sticky
    // clamp/post-pass consumes the same resolved offsets.
    assert_close(tree.nodes[child].layout.offset.x, 10.0);
    assert_close(tree.nodes[child].layout.offset.y, 10.0);
}

#[test]
fn grid_sticky_child_end_percent_insets_resolve_against_container_constraints() {
    let (mut tree, root, child) = sticky_grid(false);
    run_rust_layout(&mut tree, root, Constraints::definite(100.0, 40.0));
    assert_close(tree.nodes[child].layout.offset.x, -20.0);
    assert_close(tree.nodes[child].layout.offset.y, -20.0);
}

#[test]
fn grid_auto_flow_public_variants_reach_static_dispatch() {
    for flow in [
        GridAutoFlow::Row,
        GridAutoFlow::Column,
        GridAutoFlow::Dense,
        GridAutoFlow::RowDense,
        GridAutoFlow::ColumnDense,
    ] {
        let mut tree = SimpleTree::default();
        let root = tree.push(SimpleNode::new(Style {
            display: Display::Grid,
            grid_auto_flow: flow,
            grid_template_columns: vec![Length::points(10.0); 2],
            grid_template_rows: vec![Length::points(10.0); 2],
            ..Style::default()
        }));
        for _ in 0..3 {
            let child = tree.push(SimpleNode::new(Style::default()));
            tree.append_child(root, child);
        }
        let size = run_rust_layout(&mut tree, root, Constraints::indefinite());
        assert!(size.width.is_finite() && size.height.is_finite());
    }
}
