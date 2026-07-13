//! Rust-only Relative slices from PR #25's standalone public-API snapshots.
//!
//! The source target called a standalone wrapper and compared the resulting
//! snapshots with Lynx C++. neutron-star intentionally has no such wrapper;
//! these tests retain the two dedicated Relative trees and the Relative branch
//! of the mixed display matrix at the crate's generic host protocol boundary.

mod support;

use neutron_star::prelude::*;
use neutron_star::style::{
    BoxSizing, Dimension, LengthPercentage, RelativeCenter, RelativeReference,
};
use support::{TestStyle, TestTree, definite_layout, perform_layout, relative_container};

fn assert_layout(tree: &TestTree, node: NodeId, location: Point<f32>, size: Size<f32>) {
    let layout = tree.layout(node);
    assert_eq!(layout.location, location);
    assert_eq!(layout.size, size);
}

fn fixed_child(tree: &mut TestTree, width: f32, height: f32, style: TestStyle) -> NodeId {
    tree.push_leaf(
        TestStyle {
            size: Size::new(Dimension::Length(width), Dimension::Length(height)),
            ..style
        },
        Size::new(width, height),
        None,
    )
}

#[allow(clippy::too_many_lines)] // One source snapshot intentionally stays together.
fn public_relative_definite_snapshot() -> Vec<Layout> {
    let mut tree = TestTree::default();
    let center_none = fixed_child(&mut tree, 20.0, 10.0, TestStyle::default());
    let center_horizontal = fixed_child(
        &mut tree,
        20.0,
        10.0,
        TestStyle {
            relative_center: RelativeCenter::Horizontal,
            ..TestStyle::default()
        },
    );
    let center_vertical = fixed_child(
        &mut tree,
        20.0,
        10.0,
        TestStyle {
            relative_center: RelativeCenter::Vertical,
            ..TestStyle::default()
        },
    );
    let center_both = fixed_child(
        &mut tree,
        20.0,
        10.0,
        TestStyle {
            relative_center: RelativeCenter::Both,
            ..TestStyle::default()
        },
    );
    let parent_end = fixed_child(
        &mut tree,
        18.0,
        8.0,
        TestStyle {
            relative_align: Edges {
                right: RelativeReference::PARENT,
                bottom: RelativeReference::PARENT,
                ..Edges::uniform(RelativeReference::NONE)
            },
            ..TestStyle::default()
        },
    );
    let anchor = fixed_child(
        &mut tree,
        20.0,
        20.0,
        TestStyle {
            relative_id: RelativeReference::new(20),
            relative_align: Edges {
                right: RelativeReference::PARENT,
                bottom: RelativeReference::PARENT,
                ..Edges::uniform(RelativeReference::NONE)
            },
            ..TestStyle::default()
        },
    );
    let before = fixed_child(
        &mut tree,
        10.0,
        10.0,
        TestStyle {
            relative_adjacent: Edges {
                left: RelativeReference::new(20),
                top: RelativeReference::new(20),
                ..Edges::uniform(RelativeReference::NONE)
            },
            ..TestStyle::default()
        },
    );
    let aligned = fixed_child(
        &mut tree,
        5.0,
        7.0,
        TestStyle {
            relative_align: Edges {
                left: RelativeReference::new(20),
                bottom: RelativeReference::new(20),
                ..Edges::uniform(RelativeReference::NONE)
            },
            ..TestStyle::default()
        },
    );
    let children = [
        center_none,
        center_horizontal,
        center_vertical,
        center_both,
        parent_end,
        before,
        aligned,
        anchor,
    ];
    let root = relative_container(
        &mut tree,
        TestStyle {
            size: Size::new(Dimension::Length(100.0), Dimension::Length(80.0)),
            ..TestStyle::default()
        },
        &children,
    );

    let output = definite_layout(&mut tree, root, 100.0, 80.0);
    assert_eq!(output.size, Size::new(100.0, 80.0));
    assert_layout(
        &tree,
        center_none,
        Point::new(0.0, 0.0),
        Size::new(20.0, 10.0),
    );
    assert_layout(
        &tree,
        center_horizontal,
        Point::new(40.0, 0.0),
        Size::new(20.0, 10.0),
    );
    assert_layout(
        &tree,
        center_vertical,
        Point::new(0.0, 35.0),
        Size::new(20.0, 10.0),
    );
    assert_layout(
        &tree,
        center_both,
        Point::new(40.0, 35.0),
        Size::new(20.0, 10.0),
    );
    assert_layout(
        &tree,
        parent_end,
        Point::new(82.0, 72.0),
        Size::new(18.0, 8.0),
    );
    assert_layout(&tree, anchor, Point::new(80.0, 60.0), Size::new(20.0, 20.0));
    assert_layout(&tree, before, Point::new(70.0, 50.0), Size::new(10.0, 10.0));
    assert_layout(&tree, aligned, Point::new(80.0, 73.0), Size::new(5.0, 7.0));

    std::iter::once(root)
        .chain(children)
        .map(|node| tree.layout(node))
        .collect()
}

fn public_relative_layout_once_snapshot() -> Vec<Layout> {
    let mut tree = TestTree::default();
    let first = fixed_child(
        &mut tree,
        10.0,
        10.0,
        TestStyle {
            relative_id: RelativeReference::new(1),
            relative_adjacent: Edges {
                bottom: RelativeReference::new(2),
                ..Edges::uniform(RelativeReference::NONE)
            },
            ..TestStyle::default()
        },
    );
    let second = fixed_child(
        &mut tree,
        5.0,
        7.0,
        TestStyle {
            relative_id: RelativeReference::new(2),
            relative_adjacent: Edges {
                right: RelativeReference::new(1),
                ..Edges::uniform(RelativeReference::NONE)
            },
            ..TestStyle::default()
        },
    );
    let root = relative_container(
        &mut tree,
        TestStyle {
            relative_layout_once: true,
            ..TestStyle::default()
        },
        &[first, second],
    );

    let output = perform_layout(
        &mut tree,
        root,
        Size::NONE,
        Size::new(AvailableSpace::MaxContent, AvailableSpace::MaxContent),
    );
    assert_eq!(output.size, Size::new(15.0, 10.0));
    assert_layout(&tree, first, Point::ZERO, Size::new(10.0, 10.0));
    assert_layout(&tree, second, Point::new(10.0, 0.0), Size::new(5.0, 7.0));

    [root, first, second]
        .into_iter()
        .map(|node| tree.layout(node))
        .collect()
}

#[test]
fn standalone_public_relative_layout_matrix_runs_both_rust_snapshots() {
    let snapshots = [
        public_relative_definite_snapshot(),
        public_relative_layout_once_snapshot(),
    ];
    assert_eq!(snapshots.len(), 2);
    assert_eq!(snapshots[0].len(), 9);
    assert_eq!(snapshots[1].len(), 3);
}

#[test]
fn standalone_public_display_layout_matrix_retains_the_relative_slice() {
    let mut tree = TestTree::default();
    let first = fixed_child(
        &mut tree,
        30.0,
        10.0,
        TestStyle {
            relative_id: RelativeReference::new(10),
            ..TestStyle::default()
        },
    );
    let second = fixed_child(
        &mut tree,
        20.0,
        12.0,
        TestStyle {
            relative_adjacent: Edges {
                right: RelativeReference::new(10),
                ..Edges::uniform(RelativeReference::NONE)
            },
            ..TestStyle::default()
        },
    );
    let third = fixed_child(
        &mut tree,
        16.0,
        8.0,
        TestStyle {
            relative_adjacent: Edges {
                bottom: RelativeReference::new(10),
                ..Edges::uniform(RelativeReference::NONE)
            },
            ..TestStyle::default()
        },
    );
    let root = relative_container(
        &mut tree,
        TestStyle {
            size: Size::new(Dimension::Length(120.0), Dimension::Length(72.0)),
            padding: Edges {
                left: LengthPercentage::Length(2.0),
                top: LengthPercentage::Length(3.0),
                ..Edges::uniform(LengthPercentage::ZERO)
            },
            border: Edges::uniform(LengthPercentage::Length(1.0)),
            box_sizing: BoxSizing::BorderBox,
            ..TestStyle::default()
        },
        &[first, second, third],
    );

    let output = definite_layout(&mut tree, root, 120.0, 72.0);
    assert_eq!(output.size, Size::new(120.0, 72.0));
    assert_layout(&tree, first, Point::new(3.0, 4.0), Size::new(30.0, 10.0));
    assert_layout(&tree, second, Point::new(33.0, 4.0), Size::new(20.0, 12.0));
    assert_layout(&tree, third, Point::new(3.0, 14.0), Size::new(16.0, 8.0));
}

#[test]
fn standalone_public_relative_center_enum_slice_covers_every_value() {
    assert_eq!(
        [
            RelativeCenter::None,
            RelativeCenter::Horizontal,
            RelativeCenter::Vertical,
            RelativeCenter::Both,
        ]
        .len(),
        4
    );
    // The definite public snapshot exercises every value as observable
    // geometry, including `None` rather than treating it as an omitted case.
    assert_eq!(public_relative_definite_snapshot().len(), 9);
}
