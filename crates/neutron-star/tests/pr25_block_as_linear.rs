// Copyright 2026 The Lynx Authors. All rights reserved.
// Licensed under the Apache License Version 2.0 that can be found in the
// LICENSE file in the root directory of this source tree.

//! PR #25's cross-file coverage for Starlight's Block-as-Linear dispatch.
//!
//! These fixtures come from `box_edges_layout_tests.rs` and
//! `position_layout_tests.rs`. They say `display: block`, but the PR's Rust
//! engine converts that value to `display: linear` immediately before layout.

mod pr25_support;
mod support;

use pr25_support::{
    BaseLength, Constraints, Display, LayoutEngine, Length, PositionType, SimpleNode, SimpleTree,
    Size, Style, run_rust_layout,
};

fn assert_close(actual: f32, expected: f32) {
    assert!(
        (actual - expected).abs() <= 0.01,
        "expected {expected}, got {actual}"
    );
}

fn fixed_block(width: f32, height: f32) -> SimpleNode {
    SimpleNode::new(Style {
        display: Display::Block,
        width: Length::points(width),
        height: Length::points(height),
        ..Style::default()
    })
}

#[test]
fn root_block_fit_content_percent_argument_uses_linear_natural_size() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(Style {
        display: Display::Block,
        width: Length::fit_content(Some(BaseLength::fixed_and_percent(0.0, 50.0))),
        height: Length::fit_content(Some(BaseLength::fixed_and_percent(0.0, 25.0))),
        ..Style::default()
    }));
    let child = tree.push(fixed_block(120.0, 30.0));
    tree.append_child(root, child);

    let size = LayoutEngine::new().layout_with_owner_constraints(
        &mut tree,
        root,
        Constraints::definite(200.0, 100.0),
    );

    assert_close(size.width, 120.0);
    assert_close(size.height, 30.0);
    assert_close(tree.nodes[root].layout.size.width, 120.0);
    assert_close(tree.nodes[root].layout.size.height, 30.0);
}

#[test]
fn root_block_fit_content_calc_argument_uses_linear_natural_size() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(Style {
        display: Display::Block,
        width: Length::fit_content(Some(BaseLength::fixed_and_percent(10.0, 50.0))),
        height: Length::fit_content(Some(BaseLength::fixed_and_percent(5.0, 25.0))),
        ..Style::default()
    }));
    let child = tree.push(fixed_block(120.0, 30.0));
    tree.append_child(root, child);

    let size = LayoutEngine::new().layout_with_owner_constraints(
        &mut tree,
        root,
        Constraints::definite(200.0, 100.0),
    );

    assert_close(size.width, 120.0);
    assert_close(size.height, 30.0);
    assert_close(tree.nodes[root].layout.size.width, 120.0);
    assert_close(tree.nodes[root].layout.size.height, 30.0);
}

fn assert_child_block_fit_content(base_width: BaseLength, base_height: BaseLength) {
    let mut tree = SimpleTree::default();
    let root = tree.push(fixed_block(200.0, 100.0));
    let child = tree.push(SimpleNode::new(Style {
        display: Display::Block,
        width: Length::fit_content(Some(base_width)),
        height: Length::fit_content(Some(base_height)),
        ..Style::default()
    }));
    let grandchild = tree.push(fixed_block(120.0, 30.0));
    tree.append_child(root, child);
    tree.append_child(child, grandchild);

    LayoutEngine::new().layout_with_owner_constraints(
        &mut tree,
        root,
        Constraints::definite(200.0, 100.0),
    );

    assert_close(tree.nodes[child].layout.size.width, 120.0);
    assert_close(tree.nodes[child].layout.size.height, 30.0);
}

#[test]
fn child_block_fit_content_percent_argument_uses_linear_natural_size() {
    assert_child_block_fit_content(
        BaseLength::fixed_and_percent(0.0, 50.0),
        BaseLength::fixed_and_percent(0.0, 25.0),
    );
}

#[test]
fn child_block_fit_content_calc_argument_uses_linear_natural_size() {
    assert_child_block_fit_content(
        BaseLength::fixed_and_percent(10.0, 50.0),
        BaseLength::fixed_and_percent(5.0, 25.0),
    );
}

fn positioned_block_tree(
    position: PositionType,
    width: Length,
    height: Length,
) -> (SimpleTree, usize, usize) {
    let mut tree = SimpleTree::default();
    let root = tree.push(fixed_block(200.0, 100.0));
    let parent = if position == PositionType::Fixed {
        let nested = tree.push(fixed_block(20.0, 20.0));
        tree.append_child(root, nested);
        nested
    } else {
        root
    };
    let positioned = tree.push(SimpleNode::new(Style {
        display: Display::Block,
        position,
        width,
        height,
        left: Length::points(7.0),
        top: Length::points(9.0),
        ..Style::default()
    }));
    tree.append_child(parent, positioned);
    (tree, root, positioned)
}

#[test]
fn absolute_block_fit_content_argument_uses_latest_linear_natural_size() {
    let (mut tree, root, absolute) = positioned_block_tree(
        PositionType::Absolute,
        Length::fit_content(Some(BaseLength::fixed(80.0))),
        Length::fit_content(Some(BaseLength::fixed(20.0))),
    );
    let grandchild = tree.push(fixed_block(120.0, 30.0));
    tree.append_child(absolute, grandchild);

    run_rust_layout(&mut tree, root, Constraints::definite(200.0, 100.0));

    assert_close(tree.nodes[absolute].layout.size.width, 120.0);
    assert_close(tree.nodes[absolute].layout.size.height, 30.0);
    assert_close(tree.nodes[absolute].layout.offset.x, 7.0);
    assert_close(tree.nodes[absolute].layout.offset.y, 9.0);
}

#[test]
fn absolute_subtree_fit_content_percent_argument_uses_latest_linear_natural_size() {
    let mut tree = SimpleTree::default();
    let root = tree.push(fixed_block(122.0, 89.0));
    let absolute = tree.push(SimpleNode::new(Style {
        display: Display::Block,
        position: PositionType::Absolute,
        width: Length::fit_content(Some(BaseLength::fixed_and_percent(5.0, 50.0))),
        height: Length::fit_content(Some(BaseLength::fixed_and_percent(3.0, 25.0))),
        left: Length::points(9.0),
        top: Length::points(6.0),
        ..Style::default()
    }));
    let grandchild = tree.push(SimpleNode::with_measured_size(
        Style {
            display: Display::Block,
            width: Length::points(74.0),
            height: Length::points(29.0),
            ..Style::default()
        },
        Size::new(74.0, 29.0),
    ));
    tree.append_child(root, absolute);
    tree.append_child(absolute, grandchild);

    run_rust_layout(&mut tree, root, Constraints::definite(180.0, 130.0));

    assert_close(tree.nodes[absolute].layout.size.width, 74.0);
    assert_close(tree.nodes[absolute].layout.size.height, 29.0);
    assert_close(tree.nodes[absolute].layout.offset.x, 9.0);
    assert_close(tree.nodes[absolute].layout.offset.y, 6.0);
}

fn assert_positioned_max_content(position: PositionType) {
    let (mut tree, root, positioned) =
        positioned_block_tree(position, Length::MaxContent, Length::MaxContent);
    let grandchild = tree.push(fixed_block(250.0, 130.0));
    tree.append_child(positioned, grandchild);

    run_rust_layout(&mut tree, root, Constraints::definite(200.0, 100.0));

    assert_close(tree.nodes[positioned].layout.size.width, 250.0);
    assert_close(tree.nodes[positioned].layout.size.height, 130.0);
    assert_close(tree.nodes[positioned].layout.offset.x, 7.0);
    assert_close(tree.nodes[positioned].layout.offset.y, 9.0);
}

#[test]
fn absolute_block_max_content_uses_latest_linear_natural_size() {
    assert_positioned_max_content(PositionType::Absolute);
}

#[test]
fn fixed_descendant_block_fit_content_argument_uses_latest_linear_natural_size() {
    let (mut tree, root, fixed) = positioned_block_tree(
        PositionType::Fixed,
        Length::fit_content(Some(BaseLength::fixed(80.0))),
        Length::fit_content(Some(BaseLength::fixed(20.0))),
    );
    let grandchild = tree.push(fixed_block(120.0, 30.0));
    tree.append_child(fixed, grandchild);

    run_rust_layout(&mut tree, root, Constraints::definite(200.0, 100.0));

    assert_close(tree.nodes[fixed].layout.size.width, 120.0);
    assert_close(tree.nodes[fixed].layout.size.height, 30.0);
    assert_close(tree.nodes[fixed].layout.offset.x, 7.0);
    assert_close(tree.nodes[fixed].layout.offset.y, 9.0);
}

#[test]
fn fixed_descendant_block_max_content_uses_latest_linear_natural_size() {
    assert_positioned_max_content(PositionType::Fixed);
}
