//! Rust-only port of PR #25's standalone public-API Linear snapshots.
//!
//! This keeps the Rust fixture shapes and matrix order from
//! `standalone_public_api_tests.rs`, including the Linear row of the mixed
//! display matrix and the seven list-gap rows. The source compared the
//! resulting snapshots with C++; this target deliberately exercises only
//! neutron-star.

mod pr25_support;
mod support;

use pr25_support::{
    AlignItems, BaseLength, Constraints, Display, FlexDirection, FlexWrap, GridAutoFlow,
    JustifyContent, JustifyItems, Length, LinearCrossGravity, LinearGravity, LinearLayoutGravity,
    LinearOrientation, Point, Rect, SideConstraint, SimpleNode, SimpleTree, Size, Style,
    run_rust_layout,
};

const ORIENTATIONS: [LinearOrientation; 8] = [
    LinearOrientation::Horizontal,
    LinearOrientation::HorizontalReverse,
    LinearOrientation::Vertical,
    LinearOrientation::VerticalReverse,
    LinearOrientation::Row,
    LinearOrientation::Column,
    LinearOrientation::RowReverse,
    LinearOrientation::ColumnReverse,
];

const MAIN_GRAVITIES: [LinearGravity; 11] = [
    LinearGravity::None,
    LinearGravity::Top,
    LinearGravity::Bottom,
    LinearGravity::Left,
    LinearGravity::Right,
    LinearGravity::CenterVertical,
    LinearGravity::CenterHorizontal,
    LinearGravity::SpaceBetween,
    LinearGravity::Start,
    LinearGravity::End,
    LinearGravity::Center,
];

const CROSS_GRAVITIES: [LinearCrossGravity; 5] = [
    LinearCrossGravity::None,
    LinearCrossGravity::Start,
    LinearCrossGravity::End,
    LinearCrossGravity::Center,
    LinearCrossGravity::Stretch,
];

const ITEM_GRAVITIES: [LinearLayoutGravity; 13] = [
    LinearLayoutGravity::None,
    LinearLayoutGravity::Top,
    LinearLayoutGravity::Bottom,
    LinearLayoutGravity::Left,
    LinearLayoutGravity::Right,
    LinearLayoutGravity::CenterVertical,
    LinearLayoutGravity::CenterHorizontal,
    LinearLayoutGravity::FillVertical,
    LinearLayoutGravity::FillHorizontal,
    LinearLayoutGravity::Center,
    LinearLayoutGravity::Stretch,
    LinearLayoutGravity::Start,
    LinearLayoutGravity::End,
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum HostListComponentType {
    Regular,
    Default,
    Header,
    Footer,
    ListRow,
}

#[derive(Clone, Debug, PartialEq)]
struct HostLinearListMetadata {
    column_count: usize,
    main_axis_gap: Length,
    cross_axis_gap: Length,
    item_roles: Vec<HostListComponentType>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PublicListGapVariant {
    Points,
    Percent,
    Calc,
    ValueAuto,
    ValueFr,
    ValueMaxContent,
    ValueFitContent,
}

const PUBLIC_LIST_GAP_VARIANTS: [PublicListGapVariant; 7] = [
    PublicListGapVariant::Points,
    PublicListGapVariant::Percent,
    PublicListGapVariant::Calc,
    PublicListGapVariant::ValueAuto,
    PublicListGapVariant::ValueFr,
    PublicListGapVariant::ValueMaxContent,
    PublicListGapVariant::ValueFitContent,
];

const SOURCE_PUBLIC_DISPLAY_VARIANTS: [Display; 6] = [
    Display::None,
    Display::Block,
    Display::Flex,
    Display::Linear,
    Display::Relative,
    Display::Grid,
];

fn assert_close(actual: f32, expected: f32) {
    let error = (actual - expected).abs();
    assert!(
        error <= 0.01,
        "expected {expected}, got {actual} (absolute error {error})"
    );
}

fn is_horizontal(orientation: LinearOrientation) -> bool {
    matches!(
        orientation,
        LinearOrientation::Horizontal
            | LinearOrientation::HorizontalReverse
            | LinearOrientation::Row
            | LinearOrientation::RowReverse
    )
}

fn run_fixture(root_style: Style, children: &[Style], constraints: Size) -> SimpleTree {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(root_style));
    for style in children {
        let child = tree.push(SimpleNode::new(style.clone()));
        tree.append_child(root, child);
    }
    run_rust_layout(
        &mut tree,
        root,
        Constraints::definite(constraints.width, constraints.height),
    );
    assert_eq!(tree.nodes[root].layout.size, constraints);
    assert_eq!(tree.nodes.len(), children.len() + 1);
    for node in &tree.nodes {
        for value in [
            node.layout.offset.x,
            node.layout.offset.y,
            node.layout.size.width,
            node.layout.size.height,
        ] {
            assert!(value.is_finite(), "public snapshot geometry must be finite");
        }
        assert!(node.layout.size.width >= 0.0);
        assert!(node.layout.size.height >= 0.0);
    }
    tree
}

fn gravity_snapshot() {
    let tree = run_fixture(
        Style {
            display: Display::Linear,
            linear_orientation: LinearOrientation::Horizontal,
            linear_gravity: LinearGravity::Center,
            linear_cross_gravity: LinearCrossGravity::Center,
            width: Length::points(100.0),
            height: Length::points(40.0),
            ..Style::default()
        },
        &[
            Style {
                display: Display::Block,
                width: Length::points(10.0),
                height: Length::points(8.0),
                ..Style::default()
            },
            Style {
                display: Display::Block,
                width: Length::points(10.0),
                height: Length::points(6.0),
                linear_layout_gravity: LinearLayoutGravity::End,
                ..Style::default()
            },
            Style {
                display: Display::Block,
                width: Length::points(10.0),
                height: Length::points(5.0),
                linear_layout_gravity: LinearLayoutGravity::Stretch,
                ..Style::default()
            },
        ],
        Size::new(100.0, 40.0),
    );

    for (node, x) in tree.nodes[1..].iter().zip([35.0, 45.0, 55.0]) {
        assert_close(node.layout.offset.x, x);
    }
    assert_close(tree.nodes[1].layout.offset.y, 16.0);
    assert_close(tree.nodes[2].layout.offset.y, 34.0);
    assert_close(tree.nodes[3].layout.offset.y, 0.0);
    assert_close(tree.nodes[3].layout.size.height, 40.0);
}

fn horizontal_weight_sum_snapshot() {
    let tree = run_fixture(
        Style {
            display: Display::Linear,
            linear_orientation: LinearOrientation::Horizontal,
            linear_gravity: LinearGravity::End,
            linear_weight_sum: 4.0,
            width: Length::points(100.0),
            height: Length::points(20.0),
            ..Style::default()
        },
        &[
            Style {
                display: Display::Block,
                linear_weight: 1.0,
                height: Length::points(10.0),
                ..Style::default()
            },
            Style {
                display: Display::Block,
                linear_weight: 1.0,
                height: Length::points(10.0),
                ..Style::default()
            },
        ],
        Size::new(100.0, 20.0),
    );
    for (node, (x, width)) in tree.nodes[1..].iter().zip([(50.0, 25.0), (75.0, 25.0)]) {
        assert_close(node.layout.offset.x, x);
        assert_close(node.layout.size.width, width);
    }
}

fn horizontal_weight_ratio_snapshot() {
    let tree = run_fixture(
        Style {
            display: Display::Linear,
            linear_orientation: LinearOrientation::Horizontal,
            width: Length::points(120.0),
            height: Length::points(30.0),
            ..Style::default()
        },
        &[
            Style {
                display: Display::Block,
                width: Length::points(15.0),
                height: Length::points(10.0),
                ..Style::default()
            },
            Style {
                display: Display::Block,
                linear_weight: 1.0,
                height: Length::points(12.0),
                ..Style::default()
            },
            Style {
                display: Display::Block,
                linear_weight: 2.0,
                height: Length::points(14.0),
                ..Style::default()
            },
        ],
        Size::new(120.0, 30.0),
    );
    for (node, (x, width)) in tree.nodes[1..]
        .iter()
        .zip([(0.0, 15.0), (15.0, 35.0), (50.0, 70.0)])
    {
        assert_close(node.layout.offset.x, x);
        assert_close(node.layout.size.width, width);
    }
}

fn vertical_weight_ratio_snapshot() {
    let tree = run_fixture(
        Style {
            display: Display::Linear,
            linear_orientation: LinearOrientation::Vertical,
            width: Length::points(50.0),
            height: Length::points(120.0),
            ..Style::default()
        },
        &[
            Style {
                display: Display::Block,
                width: Length::points(10.0),
                height: Length::points(15.0),
                ..Style::default()
            },
            Style {
                display: Display::Block,
                linear_weight: 1.0,
                width: Length::points(12.0),
                ..Style::default()
            },
            Style {
                display: Display::Block,
                linear_weight: 2.0,
                width: Length::points(14.0),
                ..Style::default()
            },
        ],
        Size::new(50.0, 120.0),
    );
    for (node, (y, height)) in tree.nodes[1..]
        .iter()
        .zip([(0.0, 15.0), (15.0, 35.0), (50.0, 70.0)])
    {
        assert_close(node.layout.offset.y, y);
        assert_close(node.layout.size.height, height);
    }
}

fn vertical_weight_sum_snapshot() {
    let tree = run_fixture(
        Style {
            display: Display::Linear,
            linear_orientation: LinearOrientation::Vertical,
            linear_weight_sum: 4.0,
            width: Length::points(40.0),
            height: Length::points(100.0),
            ..Style::default()
        },
        &[
            Style {
                display: Display::Block,
                width: Length::points(10.0),
                height: Length::points(20.0),
                ..Style::default()
            },
            Style {
                display: Display::Block,
                linear_weight: 1.0,
                width: Length::points(10.0),
                ..Style::default()
            },
            Style {
                display: Display::Block,
                linear_weight: 1.0,
                width: Length::points(12.0),
                ..Style::default()
            },
        ],
        Size::new(40.0, 100.0),
    );
    for (node, (y, height)) in tree.nodes[1..]
        .iter()
        .zip([(0.0, 20.0), (20.0, 20.0), (40.0, 20.0)])
    {
        assert_close(node.layout.offset.y, y);
        assert_close(node.layout.size.height, height);
    }
}

fn partial_weight_snapshot() {
    let tree = run_fixture(
        Style {
            display: Display::Linear,
            linear_orientation: LinearOrientation::Horizontal,
            width: Length::points(100.0),
            height: Length::points(20.0),
            ..Style::default()
        },
        &[Style {
            display: Display::Block,
            linear_weight: 0.5,
            height: Length::points(10.0),
            ..Style::default()
        }],
        Size::new(100.0, 20.0),
    );
    assert_close(tree.nodes[1].layout.size.width, 50.0);
}

fn orientation_snapshot(orientation: LinearOrientation) {
    let tree = run_fixture(
        Style {
            display: Display::Linear,
            linear_orientation: orientation,
            width: Length::points(90.0),
            height: Length::points(70.0),
            ..Style::default()
        },
        &[(10.0, 12.0), (20.0, 16.0), (15.0, 10.0)].map(|(width, height)| Style {
            display: Display::Block,
            width: Length::points(width),
            height: Length::points(height),
            ..Style::default()
        }),
        Size::new(90.0, 70.0),
    );

    let actual = tree.nodes[1..]
        .iter()
        .map(|node| node.layout.offset)
        .collect::<Vec<_>>();
    let expected = match orientation {
        LinearOrientation::Horizontal | LinearOrientation::Row => {
            vec![
                Point::new(0.0, 0.0),
                Point::new(10.0, 0.0),
                Point::new(30.0, 0.0),
            ]
        }
        LinearOrientation::HorizontalReverse | LinearOrientation::RowReverse => {
            vec![
                Point::new(80.0, 0.0),
                Point::new(60.0, 0.0),
                Point::new(45.0, 0.0),
            ]
        }
        LinearOrientation::Vertical | LinearOrientation::Column => {
            vec![
                Point::new(0.0, 0.0),
                Point::new(0.0, 12.0),
                Point::new(0.0, 28.0),
            ]
        }
        LinearOrientation::VerticalReverse | LinearOrientation::ColumnReverse => {
            vec![
                Point::new(0.0, 58.0),
                Point::new(0.0, 42.0),
                Point::new(0.0, 32.0),
            ]
        }
    };
    assert_eq!(actual, expected);
}

fn main_gravity_snapshot(orientation: LinearOrientation, gravity: LinearGravity) {
    let tree = run_fixture(
        Style {
            display: Display::Linear,
            linear_orientation: orientation,
            linear_gravity: gravity,
            width: Length::points(100.0),
            height: Length::points(90.0),
            ..Style::default()
        },
        &[(10.0, 12.0), (20.0, 16.0), (15.0, 10.0)].map(|(width, height)| Style {
            display: Display::Block,
            width: Length::points(width),
            height: Length::points(height),
            ..Style::default()
        }),
        Size::new(100.0, 90.0),
    );
    let main = |node: &SimpleNode| {
        if is_horizontal(orientation) {
            node.layout.offset.x
        } else {
            node.layout.offset.y
        }
    };
    let free = if is_horizontal(orientation) {
        55.0
    } else {
        52.0
    };
    let sizes = if is_horizontal(orientation) {
        [10.0, 20.0, 15.0]
    } else {
        [12.0, 16.0, 10.0]
    };
    let mode = match gravity {
        LinearGravity::Center | LinearGravity::CenterHorizontal | LinearGravity::CenterVertical => {
            1
        }
        LinearGravity::SpaceBetween => 2,
        LinearGravity::End | LinearGravity::Right if is_horizontal(orientation) => 3,
        LinearGravity::End | LinearGravity::Bottom if !is_horizontal(orientation) => 3,
        _ => 0,
    };
    let (leading, gap) = match mode {
        1 => (free / 2.0, 0.0),
        2 => (0.0, free / 2.0),
        3 => (free, 0.0),
        _ => (0.0, 0.0),
    };
    let expected = [
        leading,
        leading + sizes[0] + gap,
        leading + sizes[0] + sizes[1] + gap * 2.0,
    ];
    for (node, expected) in tree.nodes[1..].iter().zip(expected) {
        assert_close(main(node), expected);
    }
}

fn cross_gravity_snapshot(orientation: LinearOrientation, cross_gravity: LinearCrossGravity) {
    let tree = run_fixture(
        Style {
            display: Display::Linear,
            linear_orientation: orientation,
            linear_cross_gravity: cross_gravity,
            width: Length::points(100.0),
            height: Length::points(90.0),
            ..Style::default()
        },
        &[(20.0, 10.0), (30.0, 12.0)].map(|(width, height)| Style {
            display: Display::Block,
            width: Length::points(width),
            height: Length::points(height),
            ..Style::default()
        }),
        Size::new(100.0, 90.0),
    );
    if cross_gravity == LinearCrossGravity::Stretch {
        for node in &tree.nodes[1..] {
            let size = if is_horizontal(orientation) {
                node.layout.size.height
            } else {
                node.layout.size.width
            };
            assert_close(
                size,
                if is_horizontal(orientation) {
                    90.0
                } else {
                    100.0
                },
            );
        }
    } else {
        let available = if is_horizontal(orientation) {
            90.0
        } else {
            100.0
        };
        let sizes = if is_horizontal(orientation) {
            [10.0, 12.0]
        } else {
            [20.0, 30.0]
        };
        for (node, size) in tree.nodes[1..].iter().zip(sizes) {
            let actual = if is_horizontal(orientation) {
                node.layout.offset.y
            } else {
                node.layout.offset.x
            };
            let expected = match cross_gravity {
                LinearCrossGravity::End => available - size,
                LinearCrossGravity::Center => (available - size) / 2.0,
                LinearCrossGravity::None | LinearCrossGravity::Start => 0.0,
                LinearCrossGravity::Stretch => unreachable!(),
            };
            assert_close(actual, expected);
        }
    }
}

fn item_gravity_snapshot(orientation: LinearOrientation, layout_gravity: LinearLayoutGravity) {
    let tree = run_fixture(
        Style {
            display: Display::Linear,
            linear_orientation: orientation,
            linear_cross_gravity: LinearCrossGravity::Start,
            width: Length::points(100.0),
            height: Length::points(90.0),
            ..Style::default()
        },
        &[Style {
            display: Display::Block,
            linear_layout_gravity: layout_gravity,
            width: Length::points(20.0),
            height: Length::points(10.0),
            ..Style::default()
        }],
        Size::new(100.0, 90.0),
    );
    let child = &tree.nodes[1].layout;
    let (cross_offset, cross_size, available) = if is_horizontal(orientation) {
        (child.offset.y, child.size.height, 90.0)
    } else {
        (child.offset.x, child.size.width, 100.0)
    };
    match layout_gravity {
        LinearLayoutGravity::End | LinearLayoutGravity::Right | LinearLayoutGravity::Bottom => {
            assert_close(
                cross_offset,
                available
                    - if is_horizontal(orientation) {
                        10.0
                    } else {
                        20.0
                    },
            );
        }
        LinearLayoutGravity::Center
        | LinearLayoutGravity::CenterHorizontal
        | LinearLayoutGravity::CenterVertical => {
            assert_close(
                cross_offset,
                (available
                    - if is_horizontal(orientation) {
                        10.0
                    } else {
                        20.0
                    })
                    / 2.0,
            );
        }
        LinearLayoutGravity::Stretch
        | LinearLayoutGravity::FillHorizontal
        | LinearLayoutGravity::FillVertical => {
            assert_close(cross_offset, 0.0);
            assert_close(cross_size, available);
        }
        LinearLayoutGravity::None
        | LinearLayoutGravity::Start
        | LinearLayoutGravity::Left
        | LinearLayoutGravity::Top => assert_close(cross_offset, 0.0),
    }
}

#[test]
fn standalone_public_linear_matrix_runs_all_72_source_shaped_snapshots() {
    let mut snapshots = 0usize;

    gravity_snapshot();
    horizontal_weight_sum_snapshot();
    horizontal_weight_ratio_snapshot();
    vertical_weight_ratio_snapshot();
    vertical_weight_sum_snapshot();
    partial_weight_snapshot();
    snapshots += 6;

    for orientation in ORIENTATIONS {
        orientation_snapshot(orientation);
        snapshots += 1;
    }
    for gravity in MAIN_GRAVITIES {
        // Source ordering is vertical then horizontal for every value.
        for orientation in [LinearOrientation::Vertical, LinearOrientation::Horizontal] {
            main_gravity_snapshot(orientation, gravity);
            snapshots += 1;
        }
    }
    for cross_gravity in CROSS_GRAVITIES {
        for orientation in [LinearOrientation::Vertical, LinearOrientation::Horizontal] {
            cross_gravity_snapshot(orientation, cross_gravity);
            snapshots += 1;
        }
    }
    for layout_gravity in ITEM_GRAVITIES {
        for orientation in [LinearOrientation::Vertical, LinearOrientation::Horizontal] {
            item_gravity_snapshot(orientation, layout_gravity);
            snapshots += 1;
        }
    }

    assert_eq!(
        snapshots, 72,
        "the source public API exported 72 Linear snapshots"
    );
}

#[test]
#[allow(clippy::too_many_lines)] // Retains the source public display matrix's Linear row.
fn standalone_public_display_layout_matrix_retains_the_linear_slice() {
    assert_eq!(
        SOURCE_PUBLIC_DISPLAY_VARIANTS,
        [
            Display::None,
            Display::Block,
            Display::Flex,
            Display::Linear,
            Display::Relative,
            Display::Grid,
        ]
    );
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(Style {
        display: Display::Block,
        width: Length::points(180.0),
        height: Length::points(130.0),
        padding: Rect::new(
            Length::points(3.0),
            Length::ZERO,
            Length::points(5.0),
            Length::ZERO,
        ),
        border: Rect::all(1.0),
        ..Style::default()
    }));
    let container = tree.push(SimpleNode::new(Style {
        display: Display::Linear,
        width: Length::points(120.0),
        height: Length::points(72.0),
        margin: Rect::new(
            Length::points(7.0),
            Length::ZERO,
            Length::points(6.0),
            Length::ZERO,
        ),
        padding: Rect::new(
            Length::points(2.0),
            Length::ZERO,
            Length::points(3.0),
            Length::ZERO,
        ),
        border: Rect::all(1.0),
        column_gap: Length::points(4.0),
        row_gap: Length::points(3.0),
        flex_direction: FlexDirection::Row,
        flex_wrap: FlexWrap::NoWrap,
        justify_content: JustifyContent::FlexStart,
        align_items: AlignItems::FlexStart,
        linear_orientation: LinearOrientation::Horizontal,
        grid_auto_flow: GridAutoFlow::Row,
        justify_items: JustifyItems::Start,
        grid_template_columns: vec![Length::points(36.0), Length::points(24.0)],
        grid_template_columns_max: vec![Length::points(36.0), Length::points(24.0)],
        grid_template_rows: vec![Length::points(18.0), Length::points(16.0)],
        grid_template_rows_max: vec![Length::points(18.0), Length::points(16.0)],
        grid_auto_columns: vec![Length::points(24.0)],
        grid_auto_columns_max: vec![Length::points(24.0)],
        grid_auto_rows: vec![Length::points(16.0)],
        grid_auto_rows_max: vec![Length::points(16.0)],
        ..Style::default()
    }));
    let first = tree.push(SimpleNode::new(Style {
        display: Display::Block,
        width: Length::points(30.0),
        height: Length::points(10.0),
        relative_id: 10,
        grid_column_start: Some(1),
        grid_row_start: Some(1),
        ..Style::default()
    }));
    let second = tree.push(SimpleNode::new(Style {
        display: Display::Block,
        width: Length::points(20.0),
        height: Length::points(12.0),
        relative_right_of: 10,
        grid_column_start: Some(2),
        grid_row_start: Some(1),
        ..Style::default()
    }));
    let third = tree.push(SimpleNode::new(Style {
        display: Display::Block,
        width: Length::points(16.0),
        height: Length::points(8.0),
        relative_bottom_of: 10,
        grid_column_start: Some(1),
        grid_row_start: Some(2),
        ..Style::default()
    }));
    tree.append_child(container, first);
    tree.append_child(container, second);
    tree.append_child(container, third);
    tree.append_child(root, container);

    let size = run_rust_layout(&mut tree, root, Constraints::definite(180.0, 130.0));

    assert_eq!(tree.nodes.len(), 5);
    assert_eq!(size, Size::new(180.0, 130.0));
    assert_eq!(tree.nodes[container].style.column_gap, Length::points(4.0));
    assert_eq!(tree.nodes[container].style.row_gap, Length::points(3.0));
    assert_eq!(tree.nodes[container].style.grid_template_columns.len(), 2);
    assert_eq!(tree.nodes[container].style.grid_template_rows.len(), 2);
    assert_close(tree.nodes[container].layout.offset.x, 11.0);
    assert_close(tree.nodes[container].layout.offset.y, 12.0);
    assert_close(tree.nodes[container].layout.size.width, 124.0);
    assert_close(tree.nodes[container].layout.size.height, 77.0);
    for (node, (x, y, width, height)) in [first, second, third].into_iter().zip([
        (3.0, 4.0, 30.0, 10.0),
        (33.0, 4.0, 20.0, 12.0),
        (53.0, 4.0, 16.0, 8.0),
    ]) {
        assert_close(tree.nodes[node].layout.offset.x, x);
        assert_close(tree.nodes[node].layout.offset.y, y);
        assert_close(tree.nodes[node].layout.size.width, width);
        assert_close(tree.nodes[node].layout.size.height, height);
    }
}

fn public_list_gap_metadata(variant: PublicListGapVariant) -> HostLinearListMetadata {
    let (main_axis_gap, cross_axis_gap) = match variant {
        PublicListGapVariant::Points => (Length::points(4.0), Length::points(12.0)),
        PublicListGapVariant::Percent => (Length::percent(5.0), Length::percent(10.0)),
        PublicListGapVariant::Calc => (Length::calc(6.0, 50.0), Length::calc(12.0, 50.0)),
        PublicListGapVariant::ValueAuto => (Length::Auto, Length::Auto),
        PublicListGapVariant::ValueFr => (Length::fr(3.0), Length::fr(12.0)),
        PublicListGapVariant::ValueMaxContent => (Length::max_content(), Length::max_content()),
        PublicListGapVariant::ValueFitContent => (
            Length::fit_content(Some(BaseLength::fixed_and_percent(7.0, 8.0))),
            Length::fit_content(Some(BaseLength::fixed_and_percent(14.0, 16.0))),
        ),
    };
    HostLinearListMetadata {
        column_count: 2,
        main_axis_gap,
        cross_axis_gap,
        item_roles: vec![HostListComponentType::Regular; 4],
    }
}

fn public_list_gap_snapshot(
    variant: PublicListGapVariant,
) -> (SimpleTree, usize, HostLinearListMetadata) {
    let metadata = public_list_gap_metadata(variant);
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(Style {
        display: Display::Linear,
        linear_orientation: LinearOrientation::Vertical,
        width: Length::points(200.0),
        ..Style::default()
    }));
    for height in [10.0, 20.0, 30.0, 40.0] {
        let child = tree.push(SimpleNode::new(Style {
            display: Display::Block,
            width: Length::Auto,
            height: Length::points(height),
            ..Style::default()
        }));
        tree.append_child(root, child);
    }
    run_rust_layout(
        &mut tree,
        root,
        Constraints::new(
            SideConstraint::definite(200.0),
            SideConstraint::indefinite(),
        ),
    );
    (tree, root, metadata)
}

#[test]
fn standalone_public_list_gap_layout_matrix_runs_all_seven_rust_rows() {
    let snapshots = PUBLIC_LIST_GAP_VARIANTS.map(|variant| {
        let (tree, root, metadata) = public_list_gap_snapshot(variant);
        assert_eq!(tree.nodes.len(), 5);
        assert_eq!(metadata, public_list_gap_metadata(variant));
        assert_close(tree.nodes[root].layout.size.width, 200.0);
        assert_close(tree.nodes[root].layout.size.height, 100.0);
        for (node, (y, height)) in
            tree.nodes[1..]
                .iter()
                .zip([(0.0, 10.0), (10.0, 20.0), (30.0, 30.0), (60.0, 40.0)])
        {
            assert_close(node.layout.offset.y, y);
            assert_close(node.layout.size.width, 200.0);
            assert_close(node.layout.size.height, height);
        }
        (variant, metadata)
    });

    assert_eq!(snapshots.len(), 7);
    assert_eq!(
        snapshots.map(|(variant, _)| variant),
        PUBLIC_LIST_GAP_VARIANTS
    );
}

#[test]
fn standalone_public_linear_list_fixture_is_host_owned() {
    // Keep the source's Linear fallback tree. Column count, list gaps, and the
    // Default/Header/Footer/ListRow role tags remain host/widget metadata and
    // intentionally are not generalized into neutron-star's Linear protocol.
    let host_metadata = HostLinearListMetadata {
        column_count: 3,
        main_axis_gap: Length::points(4.0),
        cross_axis_gap: Length::points(12.0),
        item_roles: vec![
            HostListComponentType::Regular,
            HostListComponentType::Regular,
            HostListComponentType::Default,
            HostListComponentType::Header,
            HostListComponentType::Footer,
            HostListComponentType::ListRow,
        ],
    };
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(Style {
        display: Display::Linear,
        linear_orientation: LinearOrientation::Vertical,
        width: Length::points(150.0),
        ..Style::default()
    }));
    for (index, height) in [10.0, 11.0, 12.0, 7.0, 8.0, 9.0].into_iter().enumerate() {
        let child = tree.push(SimpleNode::new(Style {
            display: Display::Block,
            height: Length::points(height),
            margin: if index == 0 {
                Rect::new(
                    Length::points(3.0),
                    Length::points(5.0),
                    Length::ZERO,
                    Length::ZERO,
                )
            } else {
                Rect::all(Length::ZERO)
            },
            ..Style::default()
        }));
        tree.append_child(root, child);
    }
    run_rust_layout(
        &mut tree,
        root,
        Constraints::new(
            SideConstraint::definite(150.0),
            SideConstraint::indefinite(),
        ),
    );

    assert_eq!(tree.nodes.len(), 7);
    assert_eq!(host_metadata.column_count, 3);
    assert_eq!(host_metadata.main_axis_gap, Length::points(4.0));
    assert_eq!(host_metadata.cross_axis_gap, Length::points(12.0));
    assert_eq!(host_metadata.item_roles.len(), 6);
    assert_close(tree.nodes[root].layout.size.width, 150.0);
    assert_close(tree.nodes[root].layout.size.height, 57.0);
    for (node, y) in tree.nodes[1..]
        .iter()
        .zip([0.0, 10.0, 21.0, 33.0, 40.0, 48.0])
    {
        assert_close(node.layout.offset.y, y);
    }
}
