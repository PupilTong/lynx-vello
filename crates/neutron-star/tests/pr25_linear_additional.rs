//! PR #25 Linear coverage that lives outside its dedicated Linear suite.
//!
//! Sticky offsets are a host scroll-time post-pass in neutron-star. The two
//! sticky fixtures therefore exercise the PR #25 compatibility host: it keeps
//! Sticky in normal Linear flow and exports resolved `sticky_pos` metadata
//! without adding Sticky to neutron-star's production protocol.

mod pr25_support;
mod support;

use pr25_support::{
    Constraints, Direction, Display, Length, LinearGravity, LinearLayoutGravity, LinearOrientation,
    PositionType, STICKY_AUTO_INSET, SimpleNode, SimpleTree, Style, run_rust_layout,
};

fn assert_close(actual: f32, expected: f32) {
    assert!(
        (actual - expected).abs() <= 0.01,
        "expected {expected}, got {actual}"
    );
}

#[test]
fn absolute_linear_child_without_insets_uses_linear_gravity() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(Style {
        display: Display::Linear,
        linear_orientation: LinearOrientation::Horizontal,
        linear_gravity: LinearGravity::Center,
        width: Length::points(100.0),
        height: Length::points(40.0),
        ..Style::default()
    }));
    let absolute = tree.push(SimpleNode::new(Style {
        position: PositionType::Absolute,
        width: Length::points(20.0),
        height: Length::points(10.0),
        linear_layout_gravity: LinearLayoutGravity::End,
        ..Style::default()
    }));
    tree.append_child(root, absolute);

    run_rust_layout(&mut tree, root, Constraints::definite(100.0, 40.0));

    assert_close(tree.nodes[absolute].layout.offset.x, 40.0);
    assert_close(tree.nodes[absolute].layout.offset.y, 30.0);
}

#[test]
fn absolute_rtl_horizontal_linear_child_without_insets_uses_rtl_main_front() {
    for (gravity, expected_x) in [
        (LinearGravity::None, 80.0),
        (LinearGravity::Left, 0.0),
        (LinearGravity::Right, 80.0),
        (LinearGravity::Center, 40.0),
    ] {
        let mut tree = SimpleTree::default();
        let root = tree.push(SimpleNode::new(Style {
            display: Display::Linear,
            direction: Direction::Rtl,
            linear_orientation: LinearOrientation::Horizontal,
            linear_gravity: gravity,
            width: Length::points(100.0),
            height: Length::points(40.0),
            ..Style::default()
        }));
        let absolute = tree.push(SimpleNode::new(Style {
            position: PositionType::Absolute,
            width: Length::points(20.0),
            height: Length::points(10.0),
            linear_layout_gravity: LinearLayoutGravity::End,
            ..Style::default()
        }));
        tree.append_child(root, absolute);

        run_rust_layout(&mut tree, root, Constraints::definite(100.0, 40.0));

        assert_close(tree.nodes[absolute].layout.offset.x, expected_x);
        assert_close(tree.nodes[absolute].layout.offset.y, 30.0);
    }
}

fn assert_linear_sticky_boundary(start_insets: bool) {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(Style {
        display: Display::Linear,
        linear_orientation: LinearOrientation::Horizontal,
        width: Length::points(100.0),
        height: Length::points(40.0),
        ..Style::default()
    }));
    let sticky = tree.push(SimpleNode::new(Style {
        position: PositionType::Sticky,
        width: Length::points(20.0),
        height: Length::points(10.0),
        left: if start_insets {
            Length::percent(10.0)
        } else {
            Length::Auto
        },
        right: if start_insets {
            Length::Auto
        } else {
            Length::percent(20.0)
        },
        top: if start_insets {
            Length::percent(25.0)
        } else {
            Length::Auto
        },
        bottom: if start_insets {
            Length::Auto
        } else {
            Length::percent(50.0)
        },
        ..Style::default()
    }));
    tree.append_child(root, sticky);

    run_rust_layout(&mut tree, root, Constraints::definite(100.0, 40.0));

    // Insets are metadata for the later sticky pass, never Relative visual
    // offsets. The item therefore stays at Linear main/cross start.
    assert_close(tree.nodes[sticky].layout.offset.x, 0.0);
    assert_close(tree.nodes[sticky].layout.offset.y, 0.0);
    let sticky_pos = tree.nodes[sticky].layout.sticky_pos;
    if start_insets {
        assert_close(sticky_pos.left, 10.0);
        assert_close(sticky_pos.right, STICKY_AUTO_INSET);
        assert_close(sticky_pos.top, 10.0);
        assert_close(sticky_pos.bottom, STICKY_AUTO_INSET);
    } else {
        assert_close(sticky_pos.left, STICKY_AUTO_INSET);
        assert_close(sticky_pos.right, 20.0);
        assert_close(sticky_pos.top, STICKY_AUTO_INSET);
        assert_close(sticky_pos.bottom, 20.0);
    }
}

#[test]
fn linear_sticky_child_percent_insets_resolve_against_container_constraints() {
    assert_linear_sticky_boundary(true);
}

#[test]
fn linear_sticky_child_end_percent_insets_resolve_against_container_constraints() {
    assert_linear_sticky_boundary(false);
}
