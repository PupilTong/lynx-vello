//! Deterministic engine-native `display: linear` workloads for Divan.

#![allow(dead_code)]
#![allow(clippy::cast_precision_loss)]

use neutron_star::compute::{LeafMeasureInput, LeafMetrics};
use neutron_star::prelude::*;
use neutron_star::style::{
    AlignContent, AlignItems, BoxGenerationMode, Dimension, Direction, LengthPercentage,
    LengthPercentageAuto, LinearCrossGravity, LinearGravity, LinearLayoutGravity,
    LinearOrientation, Position,
};

use crate::support::{TestStyle, TestTree, perform_layout};

#[derive(Clone, Copy)]
pub(super) struct Scenario {
    pub(super) name: &'static str,
    pub(super) build: fn(usize) -> BenchCase,
}

impl std::fmt::Debug for Scenario {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Scenario")
            .field("name", &self.name)
            .finish_non_exhaustive()
    }
}

#[derive(Debug)]
pub(super) struct BenchCase {
    pub(super) tree: TestTree,
    pub(super) root: NodeId,
    pub(super) known_dimensions: Size<Option<f32>>,
    pub(super) available_space: Size<AvailableSpace>,
}

impl BenchCase {
    fn new(
        mut tree: TestTree,
        root: NodeId,
        known_dimensions: Size<Option<f32>>,
        available_space: Size<AvailableSpace>,
    ) -> Self {
        tree.session.record_measure_inputs = false;
        Self {
            tree,
            root,
            known_dimensions,
            available_space,
        }
    }

    pub(super) fn node_count(&self) -> usize {
        self.tree.source.nodes.len()
    }

    pub(super) fn run(&mut self) -> LayoutOutput {
        perform_layout(
            &mut self.tree,
            self.root,
            self.known_dimensions,
            self.available_space,
        )
    }
}

macro_rules! scenario {
    ($function:ident, $build:ident) => {
        Scenario {
            name: stringify!($function),
            build: $build,
        }
    };
}

macro_rules! for_each_linear_scenario {
    ($callback:ident) => {
        $callback! {
            fixed_stack, build_fixed_stack;
            ordered_stack, build_ordered_stack;
            weighted_distribution, build_weighted_distribution;
            weighted_freeze, build_weighted_freeze;
            measured_stretch, build_measured_stretch;
            mixed_hidden_absolute, build_mixed_hidden_absolute;
            intrinsic_pure_length, build_intrinsic_pure_length;
            intrinsic_sparse_percentage, build_intrinsic_sparse_percentage;
            intrinsic_dense_percentage, build_intrinsic_dense_percentage;
            intrinsic_dense_padding_percentage, build_intrinsic_dense_padding_percentage;
            intrinsic_percentage_size_only, build_intrinsic_percentage_size_only;
            intrinsic_percentage_min_max_only, build_intrinsic_percentage_min_max_only;
            intrinsic_relative_inset_only, build_intrinsic_relative_inset_only;
            linear_gravity_matrix, build_linear_gravity_matrix;
            linear_layout_gravity_matrix, build_linear_layout_gravity_matrix;
            linear_cross_gravity_matrix, build_linear_cross_gravity_matrix;
        }
    };
}
#[allow(
    unused_imports,
    reason = "the benchmark registry expands the declaration list twice"
)]
pub(super) use for_each_linear_scenario;

macro_rules! declare_scenarios {
    ($( $function:ident, $build:ident; )*) => {
        pub(super) const SCENARIOS: &[Scenario] = &[
            $(scenario!($function, $build),)*
        ];
    };
}

for_each_linear_scenario!(declare_scenarios);

pub(super) const ORIENTATIONS: [LinearOrientation; 8] = [
    LinearOrientation::Horizontal,
    LinearOrientation::HorizontalReverse,
    LinearOrientation::Vertical,
    LinearOrientation::VerticalReverse,
    LinearOrientation::Row,
    LinearOrientation::RowReverse,
    LinearOrientation::Column,
    LinearOrientation::ColumnReverse,
];

pub(super) const MAIN_GRAVITIES: [LinearGravity; 11] = [
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

pub(super) const LAYOUT_GRAVITIES: [LinearLayoutGravity; 13] = [
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

pub(super) const CROSS_GRAVITIES: [LinearCrossGravity; 5] = [
    LinearCrossGravity::None,
    LinearCrossGravity::Start,
    LinearCrossGravity::End,
    LinearCrossGravity::Center,
    LinearCrossGravity::Stretch,
];

pub(super) fn scenario_named(name: &str) -> Scenario {
    *SCENARIOS
        .iter()
        .find(|scenario| scenario.name == name)
        .unwrap_or_else(|| panic!("unknown Linear benchmark scenario {name}"))
}

fn px(value: f32) -> Dimension {
    Dimension::Length(value)
}

#[allow(clippy::needless_pass_by_value)]
fn fixed_leaf(tree: &mut TestTree, style: TestStyle, width: f32, height: f32) -> NodeId {
    tree.push_leaf(
        TestStyle {
            size: Size::new(px(width), px(height)),
            ..style
        },
        Size::new(width, height),
        None,
    )
}

fn fixed_case(tree: TestTree, root: NodeId, width: f32, height: f32) -> BenchCase {
    BenchCase::new(
        tree,
        root,
        Size::new(Some(width), Some(height)),
        Size::new(
            AvailableSpace::Definite(width),
            AvailableSpace::Definite(height),
        ),
    )
}

fn intrinsic_case(tree: TestTree, root: NodeId) -> BenchCase {
    BenchCase::new(
        tree,
        root,
        Size::new(None, Some(16.0)),
        Size::new(AvailableSpace::MaxContent, AvailableSpace::Definite(16.0)),
    )
}

fn build_fixed_stack(nodes: usize) -> BenchCase {
    let count = nodes.max(1);
    let mut tree = TestTree::default();
    let mut children = Vec::with_capacity(count);
    for index in 0..count {
        children.push(fixed_leaf(
            &mut tree,
            TestStyle::default(),
            10.0 + (index % 5) as f32,
            2.0,
        ));
    }
    let height = count as f32 * 2.0;
    let root = tree.push_linear(TestStyle::default(), children);
    fixed_case(tree, root, 320.0, height)
}

fn build_ordered_stack(nodes: usize) -> BenchCase {
    let count = nodes.max(1);
    let mut tree = TestTree::default();
    let mut children = Vec::with_capacity(count);
    for index in 0..count {
        let order = i32::try_from((index * 37) % 31).unwrap_or_default() - 15;
        children.push(fixed_leaf(
            &mut tree,
            TestStyle {
                order,
                ..TestStyle::default()
            },
            2.0,
            10.0,
        ));
    }
    let width = count as f32 * 2.0;
    let root = tree.push_linear(
        TestStyle {
            linear_orientation: LinearOrientation::Horizontal,
            ..TestStyle::default()
        },
        children,
    );
    fixed_case(tree, root, width, 32.0)
}

fn build_weighted_distribution(nodes: usize) -> BenchCase {
    let count = nodes.max(1);
    let mut tree = TestTree::default();
    let mut children = Vec::with_capacity(count);
    for index in 0..count {
        children.push(tree.push_leaf(
            TestStyle {
                size: Size::new(Dimension::Auto, px(10.0)),
                linear_weight: 1.0 + (index % 4) as f32,
                ..TestStyle::default()
            },
            Size::new(1.0, 10.0),
            None,
        ));
    }
    let width = count as f32 * 8.0;
    let root = tree.push_linear(
        TestStyle {
            linear_orientation: LinearOrientation::Horizontal,
            ..TestStyle::default()
        },
        children,
    );
    fixed_case(tree, root, width, 32.0)
}

fn build_weighted_freeze(nodes: usize) -> BenchCase {
    let count = nodes.max(1);
    let mut tree = TestTree::default();
    let mut children = Vec::with_capacity(count);
    for index in 0..count {
        let (min, max) = match index % 3 {
            0 => (Dimension::Auto, px(2.0 + (index % 7) as f32)),
            1 => (px(9.0 + (index % 5) as f32), Dimension::Auto),
            _ => (Dimension::Auto, Dimension::Auto),
        };
        children.push(tree.push_leaf(
            TestStyle {
                size: Size::new(Dimension::Auto, px(10.0)),
                min_size: Size::new(min, Dimension::Auto),
                max_size: Size::new(max, Dimension::Auto),
                linear_weight: 1.0 + (index % 3) as f32,
                ..TestStyle::default()
            },
            Size::new(1.0, 10.0),
            None,
        ));
    }
    let width = count as f32 * 8.0;
    let root = tree.push_linear(
        TestStyle {
            linear_orientation: LinearOrientation::Horizontal,
            ..TestStyle::default()
        },
        children,
    );
    fixed_case(tree, root, width, 32.0)
}

fn callback_metrics(input: LeafMeasureInput) -> LeafMetrics {
    let width = match input.available_space.width {
        AvailableSpace::Definite(value) => value.min(96.0),
        AvailableSpace::MinContent => 12.0,
        AvailableSpace::MaxContent => 48.0,
    };
    let height = input.known_dimensions.height.unwrap_or(10.0);
    LeafMetrics::new(Size::new(width, height))
        .with_first_baselines(Point::new(None, Some((height - 2.0).max(0.0))))
}

fn build_measured_stretch(nodes: usize) -> BenchCase {
    let count = nodes.max(1);
    let mut tree = TestTree::default();
    let mut children = Vec::with_capacity(count);
    for index in 0..count {
        let width = match index % 4 {
            0 => Dimension::Auto,
            1 => Dimension::Percent(0.5),
            2 => Dimension::FitContent(LengthPercentage::Length(80.0)),
            _ => px(40.0),
        };
        children.push(tree.push_measured_leaf(
            TestStyle {
                size: Size::new(width, Dimension::Auto),
                linear_layout_gravity: if index.is_multiple_of(3) {
                    LinearLayoutGravity::Stretch
                } else {
                    LinearLayoutGravity::None
                },
                ..TestStyle::default()
            },
            callback_metrics,
        ));
    }
    let height = count as f32 * 10.0;
    let root = tree.push_linear(TestStyle::default(), children);
    fixed_case(tree, root, 320.0, height)
}

fn build_mixed_hidden_absolute(nodes: usize) -> BenchCase {
    let count = nodes.max(1);
    let mut tree = TestTree::default();
    let mut children = Vec::with_capacity(count);
    for index in 0..count {
        match index % 6 {
            0 => children.push(fixed_leaf(
                &mut tree,
                TestStyle {
                    order: i32::try_from(index % 7).unwrap_or_default() - 3,
                    ..TestStyle::default()
                },
                8.0,
                10.0,
            )),
            1 => {
                let hidden_leaf = fixed_leaf(&mut tree, TestStyle::default(), 8.0, 10.0);
                children.push(tree.push_linear(
                    TestStyle {
                        box_generation_mode: BoxGenerationMode::None,
                        ..TestStyle::default()
                    },
                    vec![hidden_leaf],
                ));
            }
            2 => children.push(fixed_leaf(
                &mut tree,
                TestStyle {
                    position: Position::Absolute,
                    inset: Edges {
                        left: LengthPercentageAuto::Length((index % 13) as f32),
                        right: LengthPercentageAuto::Auto,
                        top: LengthPercentageAuto::Length((index % 11) as f32),
                        bottom: LengthPercentageAuto::Auto,
                    },
                    ..TestStyle::default()
                },
                8.0,
                10.0,
            )),
            3 => children.push(fixed_leaf(
                &mut tree,
                TestStyle {
                    position: Position::Absolute,
                    linear_layout_gravity: LinearLayoutGravity::Center,
                    ..TestStyle::default()
                },
                8.0,
                10.0,
            )),
            4 => children.push(fixed_leaf(
                &mut tree,
                TestStyle {
                    position: Position::AbsoluteHoisted,
                    linear_layout_gravity: LinearLayoutGravity::End,
                    ..TestStyle::default()
                },
                8.0,
                10.0,
            )),
            _ => children.push(fixed_leaf(&mut tree, TestStyle::default(), 8.0, 10.0)),
        }
    }
    let root = tree.push_linear(
        TestStyle {
            linear_orientation: LinearOrientation::Horizontal,
            linear_gravity: LinearGravity::SpaceBetween,
            ..TestStyle::default()
        },
        children,
    );
    fixed_case(tree, root, count as f32 * 8.0, 64.0)
}

fn build_intrinsic_percentage_stack(nodes: usize, percentage_items: usize) -> BenchCase {
    let count = nodes.max(1);
    let percentage = 1.0 / (count as f32 * 8.0);
    let mut tree = TestTree::default();
    let mut children = Vec::with_capacity(count);
    for index in 0..count {
        let margin = if index >= count.saturating_sub(percentage_items) {
            LengthPercentageAuto::Percent(percentage)
        } else {
            LengthPercentageAuto::ZERO
        };
        children.push(fixed_leaf(
            &mut tree,
            TestStyle {
                margin: Edges {
                    left: margin,
                    right: LengthPercentageAuto::ZERO,
                    top: LengthPercentageAuto::ZERO,
                    bottom: LengthPercentageAuto::ZERO,
                },
                ..TestStyle::default()
            },
            8.0,
            10.0,
        ));
    }
    let root = tree.push_linear(
        TestStyle {
            linear_orientation: LinearOrientation::Horizontal,
            ..TestStyle::default()
        },
        children,
    );
    intrinsic_case(tree, root)
}

fn build_intrinsic_pure_length(nodes: usize) -> BenchCase {
    build_intrinsic_percentage_stack(nodes, 0)
}

fn build_intrinsic_sparse_percentage(nodes: usize) -> BenchCase {
    build_intrinsic_percentage_stack(nodes, 1)
}

fn build_intrinsic_dense_percentage(nodes: usize) -> BenchCase {
    build_intrinsic_percentage_stack(nodes, nodes.max(1))
}

fn build_intrinsic_dense_padding_percentage(nodes: usize) -> BenchCase {
    let count = nodes.max(1);
    let percentage = 1.0 / (count as f32 * 8.0);
    let mut tree = TestTree::default();
    let mut children = Vec::with_capacity(count);
    for _ in 0..count {
        children.push(fixed_leaf(
            &mut tree,
            TestStyle {
                padding: Edges {
                    left: LengthPercentage::Percent(percentage),
                    ..Edges::uniform(LengthPercentage::ZERO)
                },
                ..TestStyle::default()
            },
            8.0,
            10.0,
        ));
    }
    let root = tree.push_linear(
        TestStyle {
            linear_orientation: LinearOrientation::Horizontal,
            ..TestStyle::default()
        },
        children,
    );
    intrinsic_case(tree, root)
}

fn build_intrinsic_percentage_size_only(nodes: usize) -> BenchCase {
    let count = nodes.max(1);
    let mut tree = TestTree::default();
    let mut children = Vec::with_capacity(count);
    for _ in 0..count {
        children.push(tree.push_leaf(
            TestStyle {
                size: Size::new(Dimension::Percent(0.5), px(10.0)),
                ..TestStyle::default()
            },
            Size::new(8.0, 10.0),
            None,
        ));
    }
    let root = tree.push_linear(
        TestStyle {
            linear_orientation: LinearOrientation::Horizontal,
            ..TestStyle::default()
        },
        children,
    );
    intrinsic_case(tree, root)
}

fn build_intrinsic_percentage_min_max_only(nodes: usize) -> BenchCase {
    let count = nodes.max(1);
    let mut tree = TestTree::default();
    let mut children = Vec::with_capacity(count);
    for _ in 0..count {
        children.push(fixed_leaf(
            &mut tree,
            TestStyle {
                min_size: Size::new(Dimension::Percent(0.5), Dimension::Auto),
                ..TestStyle::default()
            },
            8.0,
            10.0,
        ));
    }
    let root = tree.push_linear(
        TestStyle {
            linear_orientation: LinearOrientation::Horizontal,
            ..TestStyle::default()
        },
        children,
    );
    intrinsic_case(tree, root)
}

fn build_intrinsic_relative_inset_only(nodes: usize) -> BenchCase {
    let count = nodes.max(1);
    let percentage = 1.0 / (count as f32 * 8.0);
    let mut tree = TestTree::default();
    let mut children = Vec::with_capacity(count);
    for _ in 0..count {
        children.push(fixed_leaf(
            &mut tree,
            TestStyle {
                inset: Edges {
                    left: LengthPercentageAuto::Percent(percentage),
                    right: LengthPercentageAuto::Auto,
                    top: LengthPercentageAuto::Auto,
                    bottom: LengthPercentageAuto::Auto,
                },
                ..TestStyle::default()
            },
            8.0,
            10.0,
        ));
    }
    let root = tree.push_linear(
        TestStyle {
            linear_orientation: LinearOrientation::Horizontal,
            ..TestStyle::default()
        },
        children,
    );
    intrinsic_case(tree, root)
}

fn direction(index: usize) -> Direction {
    if index.is_multiple_of(2) {
        Direction::Ltr
    } else {
        Direction::Rtl
    }
}

fn matrix_root(tree: &mut TestTree, children: Vec<NodeId>, count: usize) -> NodeId {
    tree.push_linear(
        TestStyle {
            size: Size::new(px(340.0), px(count as f32 * 100.0)),
            ..TestStyle::default()
        },
        children,
    )
}

fn matrix_child_style(index: usize, child_index: usize) -> TestStyle {
    TestStyle {
        margin: Edges {
            left: LengthPercentageAuto::Length((child_index % 2) as f32),
            right: LengthPercentageAuto::Length(((index + child_index) % 3) as f32),
            top: LengthPercentageAuto::Length(((child_index + 1) % 2) as f32),
            bottom: LengthPercentageAuto::ZERO,
        },
        ..TestStyle::default()
    }
}

fn push_matrix_children(tree: &mut TestTree, index: usize) -> Vec<NodeId> {
    (0..3)
        .map(|child_index| {
            fixed_leaf(
                tree,
                matrix_child_style(index, child_index),
                12.0 + ((index + child_index) % 5) as f32,
                8.0 + ((index + child_index * 2) % 4) as f32,
            )
        })
        .collect()
}

fn matrix_container_style(index: usize, orientation: LinearOrientation) -> TestStyle {
    let is_row = orientation.is_horizontal();
    TestStyle {
        size: Size::new(
            px(if is_row { 118.0 } else { 54.0 }),
            px(if is_row { 42.0 } else { 96.0 }),
        ),
        direction: direction(index),
        linear_orientation: orientation,
        linear_layout_gravity: LinearLayoutGravity::Start,
        padding: Edges {
            left: LengthPercentage::Length(2.0),
            right: LengthPercentage::Length(3.0),
            top: LengthPercentage::Length(4.0),
            bottom: LengthPercentage::Length(5.0),
        },
        margin: Edges {
            left: LengthPercentageAuto::ZERO,
            right: LengthPercentageAuto::ZERO,
            top: LengthPercentageAuto::Length(1.0),
            bottom: LengthPercentageAuto::ZERO,
        },
        ..TestStyle::default()
    }
}

fn build_linear_gravity_matrix(nodes: usize) -> BenchCase {
    let count = nodes.max(1);
    let mut tree = TestTree::default();
    let mut containers = Vec::with_capacity(count);
    for index in 0..count {
        let orientation = ORIENTATIONS[index % ORIENTATIONS.len()];
        let children = push_matrix_children(&mut tree, index);
        containers.push(tree.push_linear(
            TestStyle {
                linear_gravity: MAIN_GRAVITIES[index % MAIN_GRAVITIES.len()],
                justify_content: Some(AlignContent::FlexEnd),
                align_items: Some(AlignItems::FlexStart),
                ..matrix_container_style(index, orientation)
            },
            children,
        ));
    }
    let root = matrix_root(&mut tree, containers, count);
    fixed_case(tree, root, 340.0, count as f32 * 100.0)
}

fn build_linear_layout_gravity_matrix(nodes: usize) -> BenchCase {
    let count = nodes.max(1);
    let mut tree = TestTree::default();
    let mut containers = Vec::with_capacity(count);
    for index in 0..count {
        let orientation = ORIENTATIONS[index % ORIENTATIONS.len()];
        let mut children = push_matrix_children(&mut tree, index);
        tree.source_node_mut(children[1])
            .style
            .linear_layout_gravity = LAYOUT_GRAVITIES[index % LAYOUT_GRAVITIES.len()];
        containers.push(tree.push_linear(
            TestStyle {
                align_items: Some(AlignItems::Stretch),
                ..matrix_container_style(index, orientation)
            },
            std::mem::take(&mut children),
        ));
    }
    let root = matrix_root(&mut tree, containers, count);
    fixed_case(tree, root, 340.0, count as f32 * 100.0)
}

fn build_linear_cross_gravity_matrix(nodes: usize) -> BenchCase {
    let count = nodes.max(1);
    let mut tree = TestTree::default();
    let mut containers = Vec::with_capacity(count);
    for index in 0..count {
        let orientation = ORIENTATIONS[index % ORIENTATIONS.len()];
        let children = push_matrix_children(&mut tree, index);
        containers.push(tree.push_linear(
            TestStyle {
                align_items: Some(AlignItems::FlexStart),
                linear_cross_gravity: CROSS_GRAVITIES[index % CROSS_GRAVITIES.len()],
                ..matrix_container_style(index, orientation)
            },
            children,
        ));
    }
    let root = matrix_root(&mut tree, containers, count);
    fixed_case(tree, root, 340.0, count as f32 * 100.0)
}
