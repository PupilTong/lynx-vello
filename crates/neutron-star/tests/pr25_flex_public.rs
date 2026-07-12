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

#[derive(Clone, Copy, Debug)]
struct ExpectedNode {
    offset: Point,
    size: Size,
    baseline: f32,
}

const fn expected_node(x: f32, y: f32, width: f32, height: f32, baseline: f32) -> ExpectedNode {
    ExpectedNode {
        offset: Point::new(x, y),
        size: Size::new(width, height),
        baseline,
    }
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

fn assert_snapshot(tree: &SimpleTree, expected: &[ExpectedNode]) {
    assert_eq!(
        tree.nodes.len(),
        expected.len(),
        "snapshot node cardinality changed"
    );
    assert_valid_snapshot(tree, 0, expected[0].size);

    for (index, (node, expected)) in tree.nodes.iter().zip(expected).enumerate() {
        for (actual, expected, field) in [
            (node.layout.offset.x, expected.offset.x, "offset.x"),
            (node.layout.offset.y, expected.offset.y, "offset.y"),
            (node.layout.size.width, expected.size.width, "size.width"),
            (node.layout.size.height, expected.size.height, "size.height"),
        ] {
            let error = (actual - expected).abs();
            assert!(
                error <= 0.01,
                "node {index} {field}: expected {expected}, got {actual} (absolute error {error})"
            );
        }
        let actual_baseline = node
            .layout
            .baseline
            .expect("public snapshot has a baseline");
        let baseline_error = (actual_baseline - expected.baseline).abs();
        assert!(
            baseline_error <= 0.01,
            "node {index} baseline: expected {}, got {actual_baseline} (absolute error {baseline_error})",
            expected.baseline,
        );

        for edge in [
            node.layout.margin.left,
            node.layout.margin.right,
            node.layout.margin.top,
            node.layout.margin.bottom,
            node.layout.padding.left,
            node.layout.padding.right,
            node.layout.padding.top,
            node.layout.padding.bottom,
            node.layout.border.left,
            node.layout.border.right,
            node.layout.border.top,
            node.layout.border.bottom,
        ] {
            assert_close(edge, 0.0);
        }
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
        flex_direction: FlexDirection::Row,
        flex_wrap: FlexWrap::NoWrap,
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

    // Visual order is second, third, first. The 67px of free main-axis space is split into
    // two 33.5px gaps; no integer layout-unit rounding is applied.
    assert_snapshot(
        &tree,
        &[
            expected_node(0.0, 0.0, 180.0, 60.0, 40.0),
            expected_node(150.0, 50.0, 30.0, 10.0, 10.0),
            expected_node(0.0, 20.0, 45.0, 20.0, 20.0),
            expected_node(78.5, 15.0, 38.0, 30.0, 30.0),
        ],
    );
}

fn run_grow_snapshot() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(Style {
        display: Display::Flex,
        width: Length::points(180.0),
        height: Length::points(50.0),
        flex_direction: FlexDirection::Row,
        flex_wrap: FlexWrap::NoWrap,
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

    // The 130px free space is distributed 1:2. Keeping the thirds here is deliberate: Flex
    // sizing remains in fractional CSS pixels until an optional device-pixel snapping pass.
    assert_snapshot(
        &tree,
        &[
            expected_node(0.0, 0.0, 180.0, 50.0, 10.0),
            expected_node(0.0, 0.0, 30.0, 10.0, 10.0),
            expected_node(30.0, 0.0, 190.0 / 3.0, 20.0, 20.0),
            expected_node(280.0 / 3.0, 0.0, 260.0 / 3.0, 30.0, 30.0),
        ],
    );
}

fn run_shrink_snapshot() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(Style {
        display: Display::Flex,
        width: Length::points(90.0),
        height: Length::points(50.0),
        flex_direction: FlexDirection::Row,
        flex_wrap: FlexWrap::NoWrap,
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

    // Scaled shrink factors are 60 and 116. They absorb 18.75px and 36.25px respectively;
    // the zero-shrink 30% item remains 27px wide.
    assert_snapshot(
        &tree,
        &[
            expected_node(0.0, 0.0, 90.0, 50.0, 10.0),
            expected_node(0.0, 0.0, 41.25, 10.0, 10.0),
            expected_node(41.25, 0.0, 21.75, 20.0, 20.0),
            expected_node(63.0, 0.0, 27.0, 30.0, 30.0),
        ],
    );
}

fn run_wrap_snapshot(flex_wrap: FlexWrap) {
    let tree = run_three_item_case(
        Style {
            display: Display::Flex,
            width: Length::points(55.0),
            height: Length::points(80.0),
            flex_direction: FlexDirection::Row,
            flex_wrap,
            justify_content: JustifyContent::FlexStart,
            align_content: AlignContent::FlexStart,
            align_items: AlignItems::FlexStart,
            row_gap: Length::points(5.0),
            ..Style::default()
        },
        [(30.0, 10.0), (30.0, 20.0), (30.0, 15.0)],
    );
    let expected = match flex_wrap {
        FlexWrap::NoWrap => {
            let item_width = 55.0 / 3.0;
            [
                expected_node(0.0, 0.0, 55.0, 80.0, 10.0),
                expected_node(0.0, 0.0, item_width, 10.0, 10.0),
                expected_node(item_width, 0.0, item_width, 20.0, 20.0),
                expected_node(item_width * 2.0, 0.0, item_width, 15.0, 15.0),
            ]
        }
        FlexWrap::Wrap => [
            expected_node(0.0, 0.0, 55.0, 80.0, 10.0),
            expected_node(0.0, 0.0, 30.0, 10.0, 10.0),
            expected_node(0.0, 15.0, 30.0, 20.0, 20.0),
            expected_node(0.0, 40.0, 30.0, 15.0, 15.0),
        ],
        FlexWrap::WrapReverse => [
            expected_node(0.0, 0.0, 55.0, 80.0, 80.0),
            expected_node(0.0, 70.0, 30.0, 10.0, 10.0),
            expected_node(0.0, 45.0, 30.0, 20.0, 20.0),
            expected_node(0.0, 25.0, 30.0, 15.0, 15.0),
        ],
    };
    assert_snapshot(&tree, &expected);
}

fn build_align_content_snapshot(align_content: AlignContent, height: f32) -> SimpleTree {
    run_three_item_case(
        Style {
            display: Display::Flex,
            width: Length::points(55.0),
            height: Length::points(height),
            flex_direction: FlexDirection::Row,
            flex_wrap: FlexWrap::Wrap,
            justify_content: JustifyContent::FlexStart,
            align_content,
            align_items: AlignItems::FlexStart,
            ..Style::default()
        },
        [(30.0, 10.0), (30.0, 20.0), (30.0, 15.0)],
    )
}

fn run_dedicated_align_content_snapshot() {
    // This is the PR's dedicated 55x95 space-between builder, distinct from the 55x105
    // align-content variant matrix below. Its 50px free cross space becomes two 25px gaps.
    let tree = build_align_content_snapshot(AlignContent::SpaceBetween, 95.0);
    assert_snapshot(
        &tree,
        &[
            expected_node(0.0, 0.0, 55.0, 95.0, 10.0),
            expected_node(0.0, 0.0, 30.0, 10.0, 10.0),
            expected_node(0.0, 35.0, 30.0, 20.0, 20.0),
            expected_node(0.0, 80.0, 30.0, 15.0, 15.0),
        ],
    );
}

fn run_align_content_snapshot(align_content: AlignContent) {
    let tree = build_align_content_snapshot(align_content, 105.0);
    let line_offsets = match align_content {
        AlignContent::FlexStart | AlignContent::Start => [0.0, 10.0, 30.0],
        AlignContent::FlexEnd | AlignContent::End => [60.0, 70.0, 90.0],
        AlignContent::Center => [30.0, 40.0, 60.0],
        AlignContent::Stretch => [0.0, 30.0, 70.0],
        AlignContent::SpaceBetween => [0.0, 40.0, 90.0],
        AlignContent::SpaceAround => [10.0, 40.0, 80.0],
        AlignContent::SpaceEvenly => [15.0, 40.0, 75.0],
    };
    assert_snapshot(
        &tree,
        &[
            expected_node(0.0, 0.0, 55.0, 105.0, line_offsets[0] + 10.0),
            expected_node(0.0, line_offsets[0], 30.0, 10.0, 10.0),
            expected_node(0.0, line_offsets[1], 30.0, 20.0, 20.0),
            expected_node(0.0, line_offsets[2], 30.0, 15.0, 15.0),
        ],
    );
}

fn run_direction_snapshot(flex_direction: FlexDirection) {
    let tree = run_three_item_case(
        Style {
            display: Display::Flex,
            width: Length::points(100.0),
            height: Length::points(90.0),
            flex_direction,
            flex_wrap: FlexWrap::NoWrap,
            justify_content: JustifyContent::FlexStart,
            align_items: AlignItems::FlexStart,
            ..Style::default()
        },
        [(10.0, 15.0), (20.0, 25.0), (15.0, 10.0)],
    );
    let item_offsets = match flex_direction {
        FlexDirection::Row => [
            Point::new(0.0, 0.0),
            Point::new(10.0, 0.0),
            Point::new(30.0, 0.0),
        ],
        FlexDirection::RowReverse => [
            Point::new(90.0, 0.0),
            Point::new(70.0, 0.0),
            Point::new(55.0, 0.0),
        ],
        FlexDirection::Column => [
            Point::new(0.0, 0.0),
            Point::new(0.0, 15.0),
            Point::new(0.0, 40.0),
        ],
        FlexDirection::ColumnReverse => [
            Point::new(0.0, 75.0),
            Point::new(0.0, 50.0),
            Point::new(0.0, 40.0),
        ],
    };
    assert_snapshot(
        &tree,
        &[
            expected_node(0.0, 0.0, 100.0, 90.0, item_offsets[0].y + 15.0),
            expected_node(item_offsets[0].x, item_offsets[0].y, 10.0, 15.0, 15.0),
            expected_node(item_offsets[1].x, item_offsets[1].y, 20.0, 25.0, 25.0),
            expected_node(item_offsets[2].x, item_offsets[2].y, 15.0, 10.0, 10.0),
        ],
    );
}

fn run_justify_snapshot(justify_content: JustifyContent) {
    let tree = run_three_item_case(
        Style {
            display: Display::Flex,
            width: Length::points(105.0),
            height: Length::points(30.0),
            flex_direction: FlexDirection::Row,
            flex_wrap: FlexWrap::NoWrap,
            justify_content,
            align_items: AlignItems::FlexStart,
            ..Style::default()
        },
        [(10.0, 10.0), (20.0, 10.0), (15.0, 10.0)],
    );
    let item_offsets = match justify_content {
        JustifyContent::FlexStart | JustifyContent::Stretch | JustifyContent::Start => {
            [0.0, 10.0, 30.0]
        }
        JustifyContent::Center => [30.0, 40.0, 60.0],
        JustifyContent::FlexEnd | JustifyContent::End => [60.0, 70.0, 90.0],
        JustifyContent::SpaceBetween => [0.0, 40.0, 90.0],
        JustifyContent::SpaceAround => [10.0, 40.0, 80.0],
        JustifyContent::SpaceEvenly => [15.0, 40.0, 75.0],
    };
    assert_snapshot(
        &tree,
        &[
            expected_node(0.0, 0.0, 105.0, 30.0, 10.0),
            expected_node(item_offsets[0], 0.0, 10.0, 10.0, 10.0),
            expected_node(item_offsets[1], 0.0, 20.0, 10.0, 10.0),
            expected_node(item_offsets[2], 0.0, 15.0, 10.0, 10.0),
        ],
    );
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
        flex_direction: FlexDirection::Row,
        flex_wrap: FlexWrap::NoWrap,
        justify_content: JustifyContent::FlexStart,
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
    let (item_offsets, item_heights) = match align_items {
        AlignItems::Stretch => ([0.0, 0.0, 0.0], [60.0, 60.0, 60.0]),
        AlignItems::FlexStart | AlignItems::Start => ([0.0, 0.0, 0.0], [10.0, 20.0, 15.0]),
        AlignItems::FlexEnd | AlignItems::End => ([50.0, 40.0, 45.0], [10.0, 20.0, 15.0]),
        AlignItems::Center => ([25.0, 20.0, 22.5], [10.0, 20.0, 15.0]),
        AlignItems::Baseline => ([10.0, 0.0, 5.0], [10.0, 20.0, 15.0]),
    };
    assert_snapshot(
        &tree,
        &[
            expected_node(0.0, 0.0, 100.0, 60.0, item_offsets[0] + item_heights[0]),
            expected_node(0.0, item_offsets[0], 10.0, item_heights[0], item_heights[0]),
            expected_node(
                10.0,
                item_offsets[1],
                20.0,
                item_heights[1],
                item_heights[1],
            ),
            expected_node(
                30.0,
                item_offsets[2],
                15.0,
                item_heights[2],
                item_heights[2],
            ),
        ],
    );

    if align_items == AlignItems::Baseline {
        let physical_baseline =
            tree.nodes[1].layout.offset.y + tree.nodes[1].layout.baseline.expect("first baseline");
        for child in 2..=3 {
            assert_close(
                tree.nodes[child].layout.offset.y
                    + tree.nodes[child].layout.baseline.expect("peer baseline"),
                physical_baseline,
            );
        }
    }
}

fn run_align_self_base_snapshot() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(Style {
        display: Display::Flex,
        width: Length::points(120.0),
        height: Length::points(60.0),
        flex_direction: FlexDirection::Row,
        flex_wrap: FlexWrap::NoWrap,
        justify_content: JustifyContent::FlexStart,
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
    assert_snapshot(
        &tree,
        &[
            expected_node(0.0, 0.0, 120.0, 60.0, 35.0),
            expected_node(0.0, 25.0, 10.0, 10.0, 10.0),
            expected_node(10.0, 0.0, 10.0, 20.0, 20.0),
            expected_node(20.0, 45.0, 10.0, 15.0, 15.0),
            expected_node(30.0, 0.0, 10.0, 60.0, 60.0),
        ],
    );
}

fn run_align_self_variant_snapshot(align_self: Option<AlignItems>) {
    let first_has_height = align_self != Some(AlignItems::Stretch);
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(Style {
        display: Display::Flex,
        width: Length::points(100.0),
        height: Length::points(50.0),
        flex_direction: FlexDirection::Row,
        flex_wrap: FlexWrap::NoWrap,
        justify_content: JustifyContent::FlexStart,
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
    let (first_y, first_height) = match align_self {
        None | Some(AlignItems::FlexEnd | AlignItems::End) => (42.0, 8.0),
        Some(AlignItems::Stretch) => (0.0, 50.0),
        Some(AlignItems::FlexStart | AlignItems::Baseline | AlignItems::Start) => (0.0, 8.0),
        Some(AlignItems::Center) => (21.0, 8.0),
    };
    assert_snapshot(
        &tree,
        &[
            expected_node(0.0, 0.0, 100.0, 50.0, first_y + first_height),
            expected_node(0.0, first_y, 12.0, first_height, first_height),
            expected_node(12.0, 40.0, 14.0, 10.0, 10.0),
        ],
    );
}

fn run_baseline_snapshot(use_align_self: bool) {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(Style {
        display: Display::Flex,
        width: Length::points(120.0),
        height: Length::points(60.0),
        flex_direction: FlexDirection::Row,
        flex_wrap: FlexWrap::NoWrap,
        justify_content: JustifyContent::FlexStart,
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
    let third_y = if use_align_self { 0.0 } else { 7.0 };
    assert_snapshot(
        &tree,
        &[
            expected_node(0.0, 0.0, 120.0, 60.0, 15.0),
            expected_node(0.0, 0.0, 30.0, 20.0, 15.0),
            expected_node(30.0, 11.0, 20.0, 10.0, 4.0),
            expected_node(50.0, third_y, 25.0, 16.0, 8.0),
        ],
    );

    let shared_baseline = tree.nodes[first].layout.offset.y
        + tree.nodes[first]
            .layout
            .baseline
            .expect("first measured baseline");
    assert_close(
        tree.nodes[second].layout.offset.y
            + tree.nodes[second]
                .layout
                .baseline
                .expect("second measured baseline"),
        shared_baseline,
    );
    if !use_align_self {
        assert_close(
            tree.nodes[third].layout.offset.y
                + tree.nodes[third]
                    .layout
                    .baseline
                    .expect("third measured baseline"),
            shared_baseline,
        );
    }
}

#[test]
fn standalone_public_flex_layout_matrix_runs_all_47_rust_snapshots() {
    let mut snapshots = 0usize;

    run_alignment_order_snapshot();
    run_grow_snapshot();
    run_shrink_snapshot();
    run_dedicated_align_content_snapshot();
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
