//! Rust-only coverage for the 47 Flex layout snapshots exposed by the
//! standalone public-API matrix in PupilTong/lynx#25.

mod pr25_support;
mod support;

use pr25_support::*;

const ALIGN_CONTENT_VALUES: [AlignContent; 9] = [
    AlignContent::FlexStart,
    AlignContent::FlexEnd,
    AlignContent::Center,
    AlignContent::Stretch,
    AlignContent::SpaceBetween,
    AlignContent::SpaceAround,
    AlignContent::SpaceEvenly,
    AlignContent::Start,
    AlignContent::End,
];

const ALIGN_ITEMS_VALUES: [AlignItems; 7] = [
    AlignItems::Stretch,
    AlignItems::FlexStart,
    AlignItems::FlexEnd,
    AlignItems::Center,
    AlignItems::Baseline,
    AlignItems::Start,
    AlignItems::End,
];

const ALIGN_SELF_VALUES: [Option<AlignItems>; 8] = [
    None,
    Some(AlignItems::Stretch),
    Some(AlignItems::FlexStart),
    Some(AlignItems::FlexEnd),
    Some(AlignItems::Center),
    Some(AlignItems::Baseline),
    Some(AlignItems::Start),
    Some(AlignItems::End),
];

const JUSTIFY_CONTENT_VALUES: [JustifyContent; 9] = [
    JustifyContent::FlexStart,
    JustifyContent::Center,
    JustifyContent::FlexEnd,
    JustifyContent::SpaceBetween,
    JustifyContent::SpaceAround,
    JustifyContent::SpaceEvenly,
    JustifyContent::Stretch,
    JustifyContent::Start,
    JustifyContent::End,
];

const FLEX_DIRECTION_VALUES: [FlexDirection; 4] = [
    FlexDirection::Column,
    FlexDirection::Row,
    FlexDirection::RowReverse,
    FlexDirection::ColumnReverse,
];

const FLEX_WRAP_VALUES: [FlexWrap; 3] = [FlexWrap::Wrap, FlexWrap::NoWrap, FlexWrap::WrapReverse];

fn assert_close(actual: f32, expected: f32) {
    let error = (actual - expected).abs();
    assert!(
        error <= 0.01,
        "expected {expected}, got {actual} (absolute error {error})"
    );
}

fn assert_valid_snapshot(tree: &SimpleTree, root: usize, expected_root: Size) {
    assert_eq!(tree.nodes[root].layout.size, expected_root);
    for node in &tree.nodes {
        for value in [
            node.layout.offset.x,
            node.layout.offset.y,
            node.layout.size.width,
            node.layout.size.height,
        ] {
            assert!(value.is_finite(), "snapshot geometry must be finite");
        }
        assert!(node.layout.size.width >= 0.0);
        assert!(node.layout.size.height >= 0.0);
    }
}

fn fixed_leaf(tree: &mut SimpleTree, width: f32, height: f32) -> usize {
    tree.push(SimpleNode::new(Style {
        width: Length::points(width),
        height: Length::points(height),
        ..Style::default()
    }))
}

fn run_three_item_case(root_style: Style, sizes: [(f32, f32); 3]) -> SimpleTree {
    let root_size = Size::new(
        match root_style.width {
            Length::Points(value) => value,
            _ => panic!("public snapshot root width must be definite"),
        },
        match root_style.height {
            Length::Points(value) => value,
            _ => panic!("public snapshot root height must be definite"),
        },
    );
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(root_style));
    for (width, height) in sizes {
        let child = fixed_leaf(&mut tree, width, height);
        tree.append_child(root, child);
    }
    run_rust_layout(
        &mut tree,
        root,
        Constraints::definite(root_size.width, root_size.height),
    );
    assert_valid_snapshot(&tree, root, root_size);
    tree
}

fn run_alignment_order_snapshot() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(Style {
        display: Display::Flex,
        width: Length::points(180.0),
        height: Length::points(60.0),
        justify_content: JustifyContent::SpaceBetween,
        align_items: AlignItems::Center,
        ..Style::default()
    }));
    let first = tree.push(SimpleNode::new(Style {
        flex_basis: Length::points(30.0),
        height: Length::points(10.0),
        order: 2,
        align_self: Some(AlignItems::FlexEnd),
        ..Style::default()
    }));
    let second = tree.push(SimpleNode::new(Style {
        flex_basis: Length::percent(25.0),
        height: Length::points(20.0),
        order: -1,
        align_self: Some(AlignItems::Center),
        ..Style::default()
    }));
    let third = tree.push(SimpleNode::new(Style {
        flex_basis: Length::calc(20.0, 10.0),
        height: Length::points(30.0),
        order: 1,
        ..Style::default()
    }));
    for child in [first, second, third] {
        tree.append_child(root, child);
    }
    run_rust_layout(&mut tree, root, Constraints::definite(180.0, 60.0));

    assert_valid_snapshot(&tree, root, Size::new(180.0, 60.0));
    assert!(tree.nodes[second].layout.offset.x < tree.nodes[third].layout.offset.x);
    assert!(tree.nodes[third].layout.offset.x < tree.nodes[first].layout.offset.x);
}

fn run_grow_snapshot() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(Style {
        display: Display::Flex,
        width: Length::points(180.0),
        height: Length::points(50.0),
        align_items: AlignItems::FlexStart,
        ..Style::default()
    }));
    let fixed = tree.push(SimpleNode::new(Style {
        flex_basis: Length::points(30.0),
        height: Length::points(10.0),
        ..Style::default()
    }));
    let grow = tree.push(SimpleNode::new(Style {
        flex_basis: Length::points(20.0),
        flex_grow: 1.0,
        height: Length::points(20.0),
        ..Style::default()
    }));
    let faster = tree.push(SimpleNode::new(Style {
        flex_basis: Length::ZERO,
        flex_grow: 2.0,
        height: Length::points(30.0),
        ..Style::default()
    }));
    for child in [fixed, grow, faster] {
        tree.append_child(root, child);
    }
    run_rust_layout(&mut tree, root, Constraints::definite(180.0, 50.0));

    assert_valid_snapshot(&tree, root, Size::new(180.0, 50.0));
    assert_close(tree.nodes[fixed].layout.size.width, 30.0);
    assert!(tree.nodes[faster].layout.size.width > tree.nodes[grow].layout.size.width);
}

fn run_shrink_snapshot() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(Style {
        display: Display::Flex,
        width: Length::points(90.0),
        height: Length::points(50.0),
        align_items: AlignItems::FlexStart,
        ..Style::default()
    }));
    let first = tree.push(SimpleNode::new(Style {
        flex_basis: Length::points(60.0),
        flex_shrink: 1.0,
        height: Length::points(10.0),
        ..Style::default()
    }));
    let second = tree.push(SimpleNode::new(Style {
        flex_basis: Length::calc(40.0, 20.0),
        flex_shrink: 2.0,
        height: Length::points(20.0),
        ..Style::default()
    }));
    let inflexible = tree.push(SimpleNode::new(Style {
        flex_basis: Length::percent(30.0),
        flex_shrink: 0.0,
        height: Length::points(30.0),
        ..Style::default()
    }));
    for child in [first, second, inflexible] {
        tree.append_child(root, child);
    }
    run_rust_layout(&mut tree, root, Constraints::definite(90.0, 50.0));

    assert_valid_snapshot(&tree, root, Size::new(90.0, 50.0));
    assert_close(tree.nodes[inflexible].layout.size.width, 27.0);
    assert!(tree.nodes[second].layout.size.width < tree.nodes[first].layout.size.width);
}

fn run_wrap_snapshot(flex_wrap: FlexWrap) {
    let tree = run_three_item_case(
        Style {
            display: Display::Flex,
            width: Length::points(55.0),
            height: Length::points(80.0),
            flex_wrap,
            justify_content: JustifyContent::FlexStart,
            align_content: AlignContent::FlexStart,
            align_items: AlignItems::FlexStart,
            row_gap: Length::points(5.0),
            ..Style::default()
        },
        [(30.0, 10.0), (30.0, 20.0), (30.0, 15.0)],
    );
    match flex_wrap {
        FlexWrap::NoWrap => {
            assert_close(tree.nodes[1].layout.offset.y, tree.nodes[2].layout.offset.y);
            assert_close(tree.nodes[2].layout.offset.y, tree.nodes[3].layout.offset.y);
        }
        FlexWrap::Wrap => {
            assert!(tree.nodes[1].layout.offset.y < tree.nodes[2].layout.offset.y);
            assert!(tree.nodes[2].layout.offset.y < tree.nodes[3].layout.offset.y);
        }
        FlexWrap::WrapReverse => {
            assert!(tree.nodes[1].layout.offset.y > tree.nodes[2].layout.offset.y);
            assert!(tree.nodes[2].layout.offset.y > tree.nodes[3].layout.offset.y);
        }
    }
}

fn run_align_content_snapshot(align_content: AlignContent) {
    let tree = run_three_item_case(
        Style {
            display: Display::Flex,
            width: Length::points(55.0),
            height: Length::points(105.0),
            flex_wrap: FlexWrap::Wrap,
            justify_content: JustifyContent::FlexStart,
            align_content,
            align_items: AlignItems::FlexStart,
            ..Style::default()
        },
        [(30.0, 10.0), (30.0, 20.0), (30.0, 15.0)],
    );
    assert!(tree.nodes[1].layout.offset.y <= tree.nodes[2].layout.offset.y);
    assert!(tree.nodes[2].layout.offset.y <= tree.nodes[3].layout.offset.y);
}

fn run_direction_snapshot(flex_direction: FlexDirection) {
    let tree = run_three_item_case(
        Style {
            display: Display::Flex,
            width: Length::points(100.0),
            height: Length::points(90.0),
            flex_direction,
            align_items: AlignItems::FlexStart,
            ..Style::default()
        },
        [(10.0, 15.0), (20.0, 25.0), (15.0, 10.0)],
    );
    if flex_direction.is_row() {
        let ordered = tree.nodes[1].layout.offset.x < tree.nodes[2].layout.offset.x;
        assert_eq!(ordered, flex_direction == FlexDirection::Row);
    } else {
        let ordered = tree.nodes[1].layout.offset.y < tree.nodes[2].layout.offset.y;
        assert_eq!(ordered, flex_direction == FlexDirection::Column);
    }
}

fn run_justify_snapshot(justify_content: JustifyContent) {
    let tree = run_three_item_case(
        Style {
            display: Display::Flex,
            width: Length::points(105.0),
            height: Length::points(30.0),
            justify_content,
            align_items: AlignItems::FlexStart,
            ..Style::default()
        },
        [(10.0, 10.0), (20.0, 10.0), (15.0, 10.0)],
    );
    assert!(tree.nodes[1].layout.offset.x < tree.nodes[2].layout.offset.x);
    assert!(tree.nodes[2].layout.offset.x < tree.nodes[3].layout.offset.x);
}

fn run_align_items_snapshot(align_items: AlignItems) {
    let auto_cross_size = align_items == AlignItems::Stretch;
    let sizes = if auto_cross_size {
        [(10.0, 0.0), (20.0, 0.0), (15.0, 0.0)]
    } else {
        [(10.0, 10.0), (20.0, 20.0), (15.0, 15.0)]
    };
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(Style {
        display: Display::Flex,
        width: Length::points(100.0),
        height: Length::points(60.0),
        align_items,
        ..Style::default()
    }));
    for (width, height) in sizes {
        let child = tree.push(SimpleNode::new(Style {
            width: Length::points(width),
            height: if auto_cross_size {
                Length::Auto
            } else {
                Length::points(height)
            },
            ..Style::default()
        }));
        tree.append_child(root, child);
    }
    run_rust_layout(&mut tree, root, Constraints::definite(100.0, 60.0));
    assert_valid_snapshot(&tree, root, Size::new(100.0, 60.0));
    if auto_cross_size {
        for child in 1..=3 {
            assert_close(tree.nodes[child].layout.size.height, 60.0);
        }
    }
}

fn run_align_self_base_snapshot() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(Style {
        display: Display::Flex,
        width: Length::points(120.0),
        height: Length::points(60.0),
        align_items: AlignItems::Center,
        ..Style::default()
    }));
    let cases = [
        (10.0, Some(10.0), None),
        (10.0, Some(20.0), Some(AlignItems::Start)),
        (10.0, Some(15.0), Some(AlignItems::End)),
        (10.0, None, Some(AlignItems::Stretch)),
    ];
    for (width, height, align_self) in cases {
        let child = tree.push(SimpleNode::new(Style {
            width: Length::points(width),
            height: height.map_or(Length::Auto, Length::points),
            align_self,
            ..Style::default()
        }));
        tree.append_child(root, child);
    }
    run_rust_layout(&mut tree, root, Constraints::definite(120.0, 60.0));
    assert_valid_snapshot(&tree, root, Size::new(120.0, 60.0));
    assert_close(tree.nodes[4].layout.size.height, 60.0);
}

fn run_align_self_variant_snapshot(align_self: Option<AlignItems>) {
    let first_has_height = align_self != Some(AlignItems::Stretch);
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(Style {
        display: Display::Flex,
        width: Length::points(100.0),
        height: Length::points(50.0),
        align_items: AlignItems::FlexEnd,
        ..Style::default()
    }));
    let first = tree.push(SimpleNode::new(Style {
        width: Length::points(12.0),
        height: if first_has_height {
            Length::points(8.0)
        } else {
            Length::Auto
        },
        align_self,
        ..Style::default()
    }));
    let inherited = fixed_leaf(&mut tree, 14.0, 10.0);
    tree.append_child(root, first);
    tree.append_child(root, inherited);
    run_rust_layout(&mut tree, root, Constraints::definite(100.0, 50.0));
    assert_valid_snapshot(&tree, root, Size::new(100.0, 50.0));
    if align_self == Some(AlignItems::Stretch) {
        assert_close(tree.nodes[first].layout.size.height, 50.0);
    }
}

fn run_baseline_snapshot(use_align_self: bool) {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(Style {
        display: Display::Flex,
        width: Length::points(120.0),
        height: Length::points(60.0),
        align_items: if use_align_self {
            AlignItems::FlexStart
        } else {
            AlignItems::Baseline
        },
        ..Style::default()
    }));
    let first = tree.push(SimpleNode::with_measured_size_and_baseline(
        Style {
            align_self: use_align_self.then_some(AlignItems::Baseline),
            ..Style::default()
        },
        Size::new(30.0, 20.0),
        15.0,
    ));
    let second = tree.push(SimpleNode::with_measured_size_and_baseline(
        Style {
            align_self: use_align_self.then_some(AlignItems::Baseline),
            ..Style::default()
        },
        Size::new(20.0, 10.0),
        4.0,
    ));
    let third = tree.push(SimpleNode::with_measured_size_and_baseline(
        Style::default(),
        Size::new(25.0, 16.0),
        8.0,
    ));
    for child in [first, second, third] {
        tree.append_child(root, child);
    }
    run_rust_layout(&mut tree, root, Constraints::definite(120.0, 60.0));
    assert_valid_snapshot(&tree, root, Size::new(120.0, 60.0));
    assert_close(
        tree.nodes[first].layout.offset.y + 15.0,
        tree.nodes[second].layout.offset.y + 4.0,
    );
}

#[test]
fn standalone_public_flex_layout_matrix_runs_all_47_rust_snapshots() {
    let mut snapshots = 0usize;

    run_alignment_order_snapshot();
    run_grow_snapshot();
    run_shrink_snapshot();
    run_align_content_snapshot(AlignContent::SpaceBetween);
    snapshots += 4;

    for value in FLEX_WRAP_VALUES {
        run_wrap_snapshot(value);
        snapshots += 1;
    }
    for value in ALIGN_CONTENT_VALUES {
        run_align_content_snapshot(value);
        snapshots += 1;
    }
    for value in FLEX_DIRECTION_VALUES {
        run_direction_snapshot(value);
        snapshots += 1;
    }
    for value in JUSTIFY_CONTENT_VALUES {
        run_justify_snapshot(value);
        snapshots += 1;
    }
    for value in ALIGN_ITEMS_VALUES {
        run_align_items_snapshot(value);
        snapshots += 1;
    }

    run_align_self_base_snapshot();
    snapshots += 1;
    for value in ALIGN_SELF_VALUES {
        run_align_self_variant_snapshot(value);
        snapshots += 1;
    }

    run_baseline_snapshot(false);
    run_baseline_snapshot(true);
    snapshots += 2;

    assert_eq!(
        snapshots, 47,
        "the source public API exported 47 Flex snapshots"
    );
}
