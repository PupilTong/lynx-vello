//! Engine-native Flex workload builders.
//!
//! Each scenario exercises only the Flex behavior named by the registry; no
//! external runner or foreign layout mode participates in the timed path.

#![allow(dead_code)]
// Scenario sizes are deliberately capped by the benchmark's 1,000-node input,
// and all index-derived values use small modulo periods.
#![allow(clippy::cast_precision_loss)]

use neutron_star::compute::{LeafMeasureInput, LeafMetrics};
use neutron_star::prelude::*;
use neutron_star::style::{
    AlignContent, AlignItems, BoxGenerationMode, BoxSizing, Dimension, Direction, FlexDirection,
    FlexWrap, LengthPercentage, LengthPercentageAuto, Position,
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
        tree: TestTree,
        root: NodeId,
        known_dimensions: Size<Option<f32>>,
        available_space: Size<AvailableSpace>,
    ) -> Self {
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
    ($name:literal, $build:ident) => {
        Scenario {
            name: $name,
            build: $build,
        }
    };
}

pub(super) const SCENARIOS: &[Scenario] = &[
    scenario!("flex_grow_row", build_flex_grow_row),
    scenario!("flex_wrap_gaps", build_flex_wrap_gaps),
    scenario!("flex_at_most_root", build_flex_at_most_root),
    scenario!("at_most_owner_matrix", build_at_most_owner_matrix),
    scenario!(
        "owner_direction_inheritance",
        build_owner_direction_inheritance
    ),
    scenario!(
        "flex_axis_alignment_matrix",
        build_flex_axis_alignment_matrix
    ),
    scenario!("flex_distribution_matrix", build_flex_distribution_matrix),
    scenario!(
        "flex_wrap_alignment_matrix",
        build_flex_wrap_alignment_matrix
    ),
    scenario!("flex_baseline_measured", build_flex_baseline_measured),
    scenario!(
        "baseline_propagation_matrix",
        build_baseline_propagation_matrix
    ),
    scenario!("measured_callback_matrix", build_measured_callback_matrix),
    scenario!("absolute_children", build_absolute_children),
    scenario!("nested_column_flex", build_nested_column_flex),
    scenario!("in_flow_order_matrix", build_in_flow_order_matrix),
    scenario!("full_value_spacing_matrix", build_full_value_spacing_matrix),
    scenario!("box_sizing_matrix", build_box_sizing_matrix),
    scenario!("fit_content_subtrees", build_fit_content_subtrees),
    scenario!("mixed_display_none", build_mixed_display_none),
];

pub(super) fn scenario_named(name: &str) -> Scenario {
    *SCENARIOS
        .iter()
        .find(|scenario| scenario.name == name)
        .unwrap_or_else(|| panic!("unknown Flex benchmark scenario {name}"))
}

fn edges<T>(left: T, right: T, top: T, bottom: T) -> Edges<T> {
    Edges {
        left,
        right,
        top,
        bottom,
    }
}

fn px(value: f32) -> Dimension {
    Dimension::Length(value)
}

fn lp(value: f32) -> LengthPercentage {
    LengthPercentage::Length(value)
}

fn margin_px(left: f32, right: f32, top: f32, bottom: f32) -> Edges<LengthPercentageAuto> {
    edges(
        LengthPercentageAuto::Length(left),
        LengthPercentageAuto::Length(right),
        LengthPercentageAuto::Length(top),
        LengthPercentageAuto::Length(bottom),
    )
}

fn direction(index: usize) -> Direction {
    if index.is_multiple_of(2) {
        Direction::Ltr
    } else {
        Direction::Rtl
    }
}

fn flex_direction(index: usize) -> FlexDirection {
    match index % 4 {
        0 => FlexDirection::Row,
        1 => FlexDirection::RowReverse,
        2 => FlexDirection::Column,
        _ => FlexDirection::ColumnReverse,
    }
}

fn justify_content(index: usize) -> AlignContent {
    match index % 9 {
        0 => AlignContent::Stretch,
        1 => AlignContent::FlexStart,
        2 => AlignContent::Start,
        3 => AlignContent::Center,
        4 => AlignContent::FlexEnd,
        5 => AlignContent::End,
        6 => AlignContent::SpaceBetween,
        7 => AlignContent::SpaceAround,
        _ => AlignContent::SpaceEvenly,
    }
}

fn align_items(index: usize) -> AlignItems {
    match (index / 9) % 7 {
        0 => AlignItems::Stretch,
        1 => AlignItems::FlexStart,
        2 => AlignItems::Start,
        3 => AlignItems::Center,
        4 => AlignItems::FlexEnd,
        5 => AlignItems::End,
        _ => AlignItems::Baseline,
    }
}

fn flex_wrap(index: usize) -> FlexWrap {
    match index % 3 {
        0 => FlexWrap::NoWrap,
        1 => FlexWrap::Wrap,
        _ => FlexWrap::WrapReverse,
    }
}

fn align_content(index: usize) -> AlignContent {
    match index % 9 {
        0 => AlignContent::FlexStart,
        1 => AlignContent::Start,
        2 => AlignContent::Center,
        3 => AlignContent::FlexEnd,
        4 => AlignContent::End,
        5 => AlignContent::SpaceBetween,
        6 => AlignContent::SpaceAround,
        7 => AlignContent::SpaceEvenly,
        _ => AlignContent::Stretch,
    }
}

fn known_width(width: f32) -> (Size<Option<f32>>, Size<AvailableSpace>) {
    (
        Size::new(Some(width), None),
        Size::new(AvailableSpace::Definite(width), AvailableSpace::MaxContent),
    )
}

fn at_most(width: f32, height: Option<f32>) -> (Size<Option<f32>>, Size<AvailableSpace>) {
    (
        Size::NONE,
        Size::new(
            AvailableSpace::Definite(width),
            height.map_or(AvailableSpace::MaxContent, AvailableSpace::Definite),
        ),
    )
}

fn definite(width: f32, height: f32) -> (Size<Option<f32>>, Size<AvailableSpace>) {
    (
        Size::new(Some(width), Some(height)),
        Size::new(
            AvailableSpace::Definite(width),
            AvailableSpace::Definite(height),
        ),
    )
}

fn finish(
    tree: TestTree,
    root: NodeId,
    constraints: (Size<Option<f32>>, Size<AvailableSpace>),
) -> BenchCase {
    BenchCase::new(tree, root, constraints.0, constraints.1)
}

fn fixed_leaf(tree: &mut TestTree, style: TestStyle, width: f32, height: f32) -> NodeId {
    tree.push_leaf(style, Size::new(width, height), None)
}

fn column_wrapper(tree: &mut TestTree, children: Vec<NodeId>, width: f32, height: f32) -> NodeId {
    tree.push_flex(
        TestStyle {
            size: Size::new(px(width), px(height)),
            flex_direction: FlexDirection::Column,
            align_items: Some(AlignItems::FlexStart),
            ..TestStyle::default()
        },
        children,
    )
}

fn build_flex_grow_row(nodes: usize) -> BenchCase {
    let count = nodes.max(1);
    let mut tree = TestTree::default();
    let mut children = Vec::with_capacity(count);
    for index in 0..count {
        let basis = 1.0;
        children.push(fixed_leaf(
            &mut tree,
            TestStyle {
                size: Size::new(px(basis), px(10.0)),
                flex_basis: px(basis),
                flex_grow: 1.0 + (index % 3) as f32,
                ..TestStyle::default()
            },
            basis,
            10.0,
        ));
    }
    let root = tree.push_flex(
        TestStyle {
            size: Size::new(Dimension::Auto, px(10.0)),
            align_items: Some(AlignItems::Stretch),
            justify_content: Some(AlignContent::FlexStart),
            ..TestStyle::default()
        },
        children,
    );
    finish(tree, root, known_width(count as f32))
}

fn build_flex_wrap_gaps(nodes: usize) -> BenchCase {
    let count = nodes.max(1);
    let mut tree = TestTree::default();
    let mut children = Vec::with_capacity(count);
    for index in 0..count {
        let basis = 16.0 + (index % 5) as f32;
        let height = 6.0 + (index % 3) as f32;
        children.push(fixed_leaf(
            &mut tree,
            TestStyle {
                size: Size::new(px(basis), px(height)),
                flex_basis: px(basis),
                ..TestStyle::default()
            },
            basis,
            height,
        ));
    }
    let root = tree.push_flex(
        TestStyle {
            size: Size::new(px(320.0), Dimension::Auto),
            flex_wrap: FlexWrap::Wrap,
            gap: Size::new(lp(1.0), lp(1.0)),
            align_items: Some(AlignItems::FlexStart),
            justify_content: Some(AlignContent::FlexStart),
            ..TestStyle::default()
        },
        children,
    );
    finish(tree, root, known_width(320.0))
}

fn build_flex_at_most_root(nodes: usize) -> BenchCase {
    let count = nodes.max(1);
    let mut tree = TestTree::default();
    let mut children = Vec::with_capacity(count);
    for index in 0..count {
        let basis = 1.0 + (index % 4) as f32;
        let height = 4.0 + (index % 2) as f32;
        children.push(fixed_leaf(
            &mut tree,
            TestStyle {
                size: Size::new(Dimension::Auto, px(height)),
                flex_basis: px(basis),
                ..TestStyle::default()
            },
            basis,
            height,
        ));
    }
    let root = tree.push_flex(
        TestStyle {
            align_items: Some(AlignItems::FlexStart),
            justify_content: Some(AlignContent::FlexStart),
            ..TestStyle::default()
        },
        children,
    );
    finish(tree, root, at_most(count as f32 * 4.0, None))
}

#[allow(clippy::too_many_lines)]
fn build_at_most_owner_matrix(nodes: usize) -> BenchCase {
    let count = nodes.max(1);
    let mut tree = TestTree::default();
    let mut containers = Vec::with_capacity(count);
    for index in 0..count {
        let case_index = index;
        let max_width = tree.push_calc(24.0, 0.60);
        let max_height = tree.push_calc(12.0, 0.70);
        let width = if case_index.is_multiple_of(2) {
            Dimension::Percent(0.42 + (case_index % 5) as f32 / 100.0)
        } else {
            Dimension::FitContent(LengthPercentage::Calc(
                tree.push_calc(18.0 + (case_index % 4) as f32, 0.45),
            ))
        };
        let height = if case_index.is_multiple_of(3) {
            Dimension::Auto
        } else {
            Dimension::FitContent(LengthPercentage::Length(34.0 + (case_index % 6) as f32))
        };

        let mut children = Vec::with_capacity(3);
        for child_index in 0..3 {
            let child_max_width = tree.push_calc(18.0 + child_index as f32 * 3.0, 0.45);
            let child_max_height = tree.push_calc(10.0 + child_index as f32 * 2.0, 0.55);
            let intrinsic = Size::new(
                20.0 + (case_index % 7) as f32 + child_index as f32 * 4.0,
                10.0 + (case_index % 5) as f32 + child_index as f32 * 3.0,
            );
            let fit_content_width = tree.push_calc(6.0, 0.40);
            children.push(tree.push_leaf(
                TestStyle {
                    size: Size::new(
                        match child_index {
                            0 => Dimension::Auto,
                            1 => Dimension::FitContent(LengthPercentage::Calc(fit_content_width)),
                            _ => Dimension::Percent(0.35),
                        },
                        match child_index {
                            0 => Dimension::FitContent(LengthPercentage::Length(18.0)),
                            1 => Dimension::Auto,
                            _ => Dimension::Percent(0.30),
                        },
                    ),
                    min_size: Size::new(
                        px(12.0 + child_index as f32 * 2.0),
                        px(8.0 + child_index as f32),
                    ),
                    max_size: Size::new(
                        Dimension::Calc(child_max_width),
                        Dimension::Calc(child_max_height),
                    ),
                    margin: margin_px(
                        (child_index % 2) as f32,
                        (child_index % 3) as f32 * 0.5,
                        0.0,
                        0.0,
                    ),
                    ..TestStyle::default()
                },
                intrinsic,
                None,
            ));
        }

        containers.push(tree.push_flex(
            TestStyle {
                size: Size::new(width, height),
                min_size: Size::new(px(36.0), px(18.0)),
                max_size: Size::new(Dimension::Calc(max_width), Dimension::Calc(max_height)),
                flex_wrap: FlexWrap::Wrap,
                align_items: Some(AlignItems::FlexStart),
                align_content: Some(AlignContent::FlexStart),
                justify_content: Some(AlignContent::FlexStart),
                gap: Size::new(lp(1.0), lp(1.0)),
                padding: Edges::uniform(lp(1.0)),
                margin: margin_px(1.0, 0.0, 1.0, 0.0),
                ..TestStyle::default()
            },
            children,
        ));
    }

    let root_width = tree.push_calc(12.0, 0.80);
    let root_height = tree.push_calc(8.0, 0.70);
    let root_max_width = tree.push_calc(36.0, 0.80);
    let root_max_height = tree.push_calc(20.0, 0.85);
    // A column Flex wrapper supplies the owner constraints for this workload.
    let root = tree.push_flex(
        TestStyle {
            size: Size::new(
                Dimension::FitContent(LengthPercentage::Calc(root_width)),
                Dimension::FitContent(LengthPercentage::Calc(root_height)),
            ),
            min_size: Size::new(Dimension::Percent(0.35), px(48.0)),
            max_size: Size::new(
                Dimension::Calc(root_max_width),
                Dimension::Calc(root_max_height),
            ),
            flex_direction: FlexDirection::Column,
            align_items: Some(AlignItems::FlexStart),
            padding: Edges::uniform(lp(1.0)),
            ..TestStyle::default()
        },
        containers,
    );
    finish(tree, root, at_most(320.0, Some(220.0)))
}

fn build_owner_direction_inheritance(nodes: usize) -> BenchCase {
    let count = nodes.max(1);
    let mut tree = TestTree::default();
    let mut rows = Vec::with_capacity(count * 2);
    for index in 0..count {
        for row_direction in [Direction::Rtl, Direction::Ltr] {
            let width = 10.0 + (index % 3) as f32;
            let height = 5.0 + (index % 2) as f32;
            let leaf = fixed_leaf(
                &mut tree,
                TestStyle {
                    size: Size::new(px(width), px(height)),
                    flex_basis: px(width),
                    direction: row_direction,
                    ..TestStyle::default()
                },
                width,
                height,
            );
            rows.push(tree.push_flex(
                TestStyle {
                    size: Size::new(px(30.0), px(10.0 + (index % 2) as f32)),
                    flex_basis: px(10.0),
                    direction: row_direction,
                    justify_content: Some(AlignContent::FlexStart),
                    align_items: Some(AlignItems::FlexStart),
                    ..TestStyle::default()
                },
                vec![leaf],
            ));
        }
    }
    let root = tree.push_flex(
        TestStyle {
            size: Size::new(px(30.0), px(count as f32 * 20.0)),
            direction: Direction::Rtl,
            flex_direction: FlexDirection::Column,
            align_items: Some(AlignItems::FlexStart),
            ..TestStyle::default()
        },
        rows,
    );
    finish(tree, root, definite(30.0, count as f32 * 20.0))
}

fn build_flex_axis_alignment_matrix(nodes: usize) -> BenchCase {
    let count = nodes.max(1);
    let mut tree = TestTree::default();
    let mut containers = Vec::with_capacity(count);
    for index in 0..count {
        let axis = flex_direction(index);
        let mut children = Vec::with_capacity(3);
        for child_index in 0..3 {
            let basis = [18.0, 24.0, 30.0][child_index];
            let cross = [8.0, 12.0, 16.0][child_index];
            let auto_cross = child_index == 1;
            let size = if axis.is_row() {
                Size::new(
                    px(basis),
                    if auto_cross {
                        Dimension::Auto
                    } else {
                        px(cross)
                    },
                )
            } else {
                Size::new(
                    if auto_cross {
                        Dimension::Auto
                    } else {
                        px(cross)
                    },
                    px(basis),
                )
            };
            children.push(tree.push_leaf(
                TestStyle {
                    size,
                    flex_basis: px(basis),
                    margin: margin_px(
                        child_index as f32,
                        (2 - child_index) as f32,
                        0.0,
                        (child_index % 2) as f32,
                    ),
                    ..TestStyle::default()
                },
                Size::new(basis, cross),
                None,
            ));
        }
        containers.push(tree.push_flex(
            TestStyle {
                size: Size::new(px(120.0), px(80.0)),
                flex_basis: px(80.0),
                direction: direction(index),
                flex_direction: axis,
                justify_content: Some(justify_content(index)),
                align_items: Some(align_items(index)),
                margin: margin_px(1.0, 0.0, 1.0, 0.0),
                ..TestStyle::default()
            },
            children,
        ));
    }
    let height = count as f32 * 88.0 + 8.0;
    let root = column_wrapper(&mut tree, containers, 320.0, height);
    finish(tree, root, known_width(320.0))
}

fn build_flex_distribution_matrix(nodes: usize) -> BenchCase {
    let count = nodes.max(1);
    let mut tree = TestTree::default();
    let mut containers = Vec::with_capacity(count);
    for index in 0..count {
        let axis = flex_direction(index);
        let grow_case = index.is_multiple_of(2);
        let main = if grow_case { 178.0 } else { 94.0 };
        let cross = 58.0;
        let container_size = if axis.is_row() {
            Size::new(px(main), px(cross))
        } else {
            Size::new(px(cross), px(main))
        };
        let mut children = Vec::with_capacity(5);
        for child_index in 0..5 {
            let basis = [
                px(18.0),
                px(28.0),
                px(36.0),
                px(22.0),
                Dimension::Percent(0.18),
            ][child_index];
            let grow = [0.0, 1.0, 2.0, 1.5, 0.5][child_index];
            let shrink = [1.0, 2.0, 0.5, 1.5, 0.0][child_index];
            let order = [-1, 2, 0, 3, -2][child_index] + [0, 1][index % 2];
            let cross_size = 12.0 + child_index as f32 * 2.0;
            let (min_main, max_main) = match child_index {
                0 => (px(24.0), Dimension::Auto),
                1 => (Dimension::Auto, px(32.0)),
                2 => (Dimension::Percent(0.22), Dimension::Auto),
                3 => (Dimension::Auto, Dimension::Percent(0.24)),
                _ => (Dimension::Auto, Dimension::Auto),
            };
            let (size, min_size, max_size) = if axis.is_row() {
                (
                    Size::new(Dimension::Auto, px(cross_size)),
                    Size::new(min_main, Dimension::Auto),
                    Size::new(max_main, Dimension::Auto),
                )
            } else {
                (
                    Size::new(px(cross_size), Dimension::Auto),
                    Size::new(Dimension::Auto, min_main),
                    Size::new(Dimension::Auto, max_main),
                )
            };
            children.push(tree.push_leaf(
                TestStyle {
                    size,
                    min_size,
                    max_size,
                    flex_basis: basis,
                    flex_grow: grow,
                    flex_shrink: shrink,
                    order,
                    margin: margin_px(
                        (child_index % 2) as f32,
                        (child_index % 3) as f32 * 0.5,
                        0.0,
                        0.0,
                    ),
                    ..TestStyle::default()
                },
                Size::new(24.0 + child_index as f32, cross_size),
                None,
            ));
        }
        containers.push(tree.push_flex(
            TestStyle {
                size: container_size,
                flex_basis: px(84.0),
                direction: direction(index),
                flex_direction: axis,
                flex_wrap: FlexWrap::NoWrap,
                justify_content: Some(AlignContent::FlexStart),
                align_items: Some(AlignItems::FlexStart),
                gap: Size::new(lp(1.0), lp(1.0)),
                padding: edges(lp(2.0), lp(2.0), lp(1.0), lp(1.0)),
                margin: margin_px(1.0, 0.0, 1.0, 0.0),
                ..TestStyle::default()
            },
            children,
        ));
    }
    let height = count as f32 * 86.0 + 8.0;
    let root = column_wrapper(&mut tree, containers, 320.0, height);
    finish(tree, root, known_width(320.0))
}

fn build_flex_wrap_alignment_matrix(nodes: usize) -> BenchCase {
    let count = nodes.max(1);
    let mut tree = TestTree::default();
    let mut containers = Vec::with_capacity(count);
    for index in 0..count {
        let axis = flex_direction(index);
        let dimensions = [(28.0, 16.0), (34.0, 12.0), (20.0, 18.0), (25.0, 14.0)];
        let mut children = Vec::with_capacity(dimensions.len());
        for (child_index, (width, height)) in dimensions.into_iter().enumerate() {
            children.push(fixed_leaf(
                &mut tree,
                TestStyle {
                    size: Size::new(px(width), px(height)),
                    flex_basis: px(if axis.is_row() { width } else { height }),
                    margin: margin_px(
                        (child_index % 2) as f32,
                        (child_index % 3) as f32,
                        (child_index % 2) as f32,
                        0.0,
                    ),
                    ..TestStyle::default()
                },
                width,
                height,
            ));
        }
        containers.push(tree.push_flex(
            TestStyle {
                size: Size::new(px(76.0), px(64.0)),
                flex_basis: px(70.0),
                direction: direction(index),
                flex_direction: axis,
                flex_wrap: flex_wrap(index),
                justify_content: Some(AlignContent::FlexStart),
                align_content: Some(align_content(index)),
                align_items: Some(align_items(index)),
                gap: Size::new(lp(2.0), lp(3.0)),
                padding: edges(lp(3.0), lp(5.0), lp(2.0), lp(4.0)),
                margin: margin_px(1.0, 0.0, 1.0, 0.0),
                ..TestStyle::default()
            },
            children,
        ));
    }
    let height = count as f32 * 72.0 + 8.0;
    let root = column_wrapper(&mut tree, containers, 320.0, height);
    finish(tree, root, known_width(320.0))
}

fn build_flex_baseline_measured(nodes: usize) -> BenchCase {
    let count = nodes.max(1);
    let mut tree = TestTree::default();
    let mut children = Vec::with_capacity(count);
    for index in 0..count {
        let width = 8.0 + (index % 7) as f32;
        let height = 6.0 + (index % 11) as f32;
        let baseline = (2.0 + (index % 5) as f32).min(height);
        children.push(tree.push_leaf(
            TestStyle {
                align_self: index.is_multiple_of(3).then_some(AlignItems::Baseline),
                margin: margin_px(
                    (index % 2) as f32,
                    (index % 3) as f32,
                    (index % 4) as f32 * 0.5,
                    (index % 5) as f32 * 0.25,
                ),
                ..TestStyle::default()
            },
            Size::new(width, height),
            Some(baseline),
        ));
    }
    let root = tree.push_flex(
        TestStyle {
            size: Size::new(px(320.0), Dimension::Auto),
            align_items: Some(AlignItems::Baseline),
            justify_content: Some(AlignContent::FlexStart),
            ..TestStyle::default()
        },
        children,
    );
    finish(tree, root, known_width(320.0))
}

fn baseline_leaf(tree: &mut TestTree, width: f32, height: f32, baseline: f32) -> NodeId {
    tree.push_leaf(
        TestStyle {
            size: Size::new(px(width), px(height)),
            flex_basis: px(width),
            ..TestStyle::default()
        },
        Size::new(width, height),
        Some(baseline.min(height)),
    )
}

fn nested_baseline_flex(tree: &mut TestTree, column: bool, trigger: bool) -> NodeId {
    let first = baseline_leaf(tree, 10.0, 18.0, 7.0);
    let second = baseline_leaf(tree, 12.0, 24.0, 18.0);
    tree.push_flex(
        TestStyle {
            size: Size::new(px(28.0), px(if column { 40.0 } else { 26.0 })),
            flex_basis: px(28.0),
            flex_direction: if column {
                FlexDirection::Column
            } else {
                FlexDirection::Row
            },
            align_items: Some(if column {
                AlignItems::FlexStart
            } else {
                AlignItems::Baseline
            }),
            justify_content: Some(if column {
                AlignContent::Center
            } else {
                AlignContent::FlexStart
            }),
            align_self: trigger.then_some(AlignItems::Baseline),
            margin: margin_px(1.0, 1.0, 2.0, 1.0),
            ..TestStyle::default()
        },
        vec![first, second],
    )
}

fn build_baseline_propagation_matrix(nodes: usize) -> BenchCase {
    let count = nodes.max(1);
    let mut tree = TestTree::default();
    let mut rows = Vec::with_capacity(count);
    for index in 0..count {
        let reference = tree.push_leaf(
            TestStyle {
                size: Size::new(px(12.0), px(32.0)),
                flex_basis: px(12.0),
                margin: margin_px(1.0, 1.0, 2.0, 1.0),
                ..TestStyle::default()
            },
            Size::new(12.0, 32.0),
            Some(26.0),
        );
        let trigger = !index.is_multiple_of(2);
        let candidate = match index % 3 {
            0 => tree.push_leaf(
                TestStyle {
                    size: Size::new(px(18.0), px(22.0)),
                    flex_basis: px(18.0),
                    align_self: trigger.then_some(AlignItems::Baseline),
                    margin: margin_px(1.0, 2.0, 1.0, 2.0),
                    ..TestStyle::default()
                },
                Size::new(18.0, 22.0),
                Some(16.0),
            ),
            1 => nested_baseline_flex(&mut tree, false, trigger),
            _ => nested_baseline_flex(&mut tree, true, trigger),
        };
        let trailing = tree.push_leaf(
            TestStyle {
                size: Size::new(px(10.0), px(12.0)),
                flex_basis: px(10.0),
                margin: margin_px(0.0, 1.0, 0.0, 0.0),
                ..TestStyle::default()
            },
            Size::new(10.0, 12.0),
            None,
        );
        rows.push(tree.push_flex(
            TestStyle {
                size: Size::new(px(116.0), px(48.0)),
                flex_basis: px(52.0),
                align_items: Some(if index.is_multiple_of(2) {
                    AlignItems::Baseline
                } else {
                    AlignItems::FlexStart
                }),
                justify_content: Some(AlignContent::FlexStart),
                padding: edges(lp(2.0), lp(2.0), lp(1.0), lp(1.0)),
                margin: margin_px(1.0, 0.0, 1.0, 0.0),
                ..TestStyle::default()
            },
            vec![reference, candidate, trailing],
        ));
    }
    let height = count as f32 * 54.0 + 8.0;
    let root = column_wrapper(&mut tree, rows, 320.0, height);
    finish(tree, root, known_width(320.0))
}

fn callback_metrics(input: LeafMeasureInput) -> LeafMetrics {
    let width = match input.available_space.width {
        AvailableSpace::Definite(value) => (value - 3.0).max(1.0),
        AvailableSpace::MinContent => 12.0,
        AvailableSpace::MaxContent => 24.0,
    };
    let height = match input.available_space.height {
        AvailableSpace::Definite(value) => (value - 2.0).max(1.0),
        AvailableSpace::MinContent => 8.0,
        AvailableSpace::MaxContent => 12.0,
    };
    LeafMetrics::new(Size::new(width, height))
        .with_first_baselines(Point::new(None, Some((height - 3.0).max(0.0))))
}

fn callback_metrics_without_baseline(input: LeafMeasureInput) -> LeafMetrics {
    let mut metrics = callback_metrics(input);
    metrics.first_baselines = Point::NONE;
    metrics
}

#[allow(clippy::too_many_lines)]
fn build_measured_callback_matrix(nodes: usize) -> BenchCase {
    let count = nodes.max(1);
    let mut tree = TestTree::default();
    let mut containers = Vec::with_capacity(count);
    for index in 0..count {
        let case_index = index;
        let mut children = Vec::with_capacity(4);
        for child_index in 0_usize..4 {
            let intrinsic = Size::new(
                18.0 + (case_index % 7) as f32 + child_index as f32 * 3.0,
                9.0 + (case_index % 5) as f32 + child_index as f32 * 2.0,
            );
            let style = TestStyle {
                size: Size::new(
                    if child_index == 0 {
                        Dimension::FitContent(lp(36.0))
                    } else {
                        Dimension::Auto
                    },
                    if child_index == 1 {
                        Dimension::FitContent(lp(18.0))
                    } else {
                        Dimension::Auto
                    },
                ),
                min_size: Size::new(
                    if child_index == 2 {
                        px(20.0)
                    } else {
                        Dimension::Auto
                    },
                    if child_index == 1 {
                        px(10.0)
                    } else {
                        Dimension::Auto
                    },
                ),
                max_size: Size::new(
                    if child_index == 3 {
                        px(54.0)
                    } else {
                        Dimension::Auto
                    },
                    if child_index == 2 {
                        px(32.0)
                    } else {
                        Dimension::Auto
                    },
                ),
                align_self: child_index
                    .is_multiple_of(2)
                    .then_some(AlignItems::Baseline),
                margin: margin_px(
                    (child_index % 2) as f32,
                    (child_index % 3) as f32 * 0.5,
                    (case_index % 2) as f32 * 0.5,
                    0.0,
                ),
                ..TestStyle::default()
            };
            let child = match child_index {
                0 => tree.push_measured_leaf(style, callback_metrics),
                1 => tree.push_measured_leaf(style, callback_metrics_without_baseline),
                2 => tree.push_leaf(
                    style,
                    intrinsic,
                    Some((4.0 + child_index as f32 * 2.0).min(intrinsic.height)),
                ),
                _ => tree.push_leaf(style, intrinsic, None),
            };
            children.push(child);
        }
        containers.push(tree.push_flex(
            TestStyle {
                size: Size::new(
                    if case_index.is_multiple_of(3) {
                        Dimension::FitContent(lp(126.0))
                    } else {
                        px(136.0)
                    },
                    if case_index.is_multiple_of(4) {
                        Dimension::FitContent(lp(44.0))
                    } else {
                        px(58.0)
                    },
                ),
                min_size: Size::new(px(72.0), px(28.0)),
                max_size: Size::new(px(180.0), px(92.0)),
                flex_basis: px(68.0),
                flex_wrap: FlexWrap::Wrap,
                align_items: Some(AlignItems::Baseline),
                align_content: Some(AlignContent::FlexStart),
                justify_content: Some(AlignContent::FlexStart),
                gap: Size::new(lp(1.0), lp(1.0)),
                padding: Edges::uniform(lp(1.0)),
                border: Edges::uniform(lp((case_index % 2) as f32 * 0.5)),
                margin: margin_px(1.0, 0.0, 1.0, 0.0),
                ..TestStyle::default()
            },
            children,
        ));
    }
    let height = count as f32 * 70.0 + 8.0;
    let root = column_wrapper(&mut tree, containers, 320.0, height);
    finish(tree, root, known_width(320.0))
}

fn build_absolute_children(nodes: usize) -> BenchCase {
    let count = nodes.max(1);
    let rows = count.div_ceil(64);
    let height = rows as f32 * 4.0 + 4.0;
    let mut tree = TestTree::default();
    let mut children = Vec::with_capacity(count);
    for index in 0..count {
        children.push(fixed_leaf(
            &mut tree,
            TestStyle {
                position: Position::Absolute,
                inset: edges(
                    LengthPercentageAuto::Length((index % 64) as f32 * 5.0),
                    LengthPercentageAuto::Auto,
                    LengthPercentageAuto::Length((index / 64) as f32 * 4.0),
                    LengthPercentageAuto::Auto,
                ),
                size: Size::new(px(4.0), px(3.0)),
                flex_basis: px(4.0),
                ..TestStyle::default()
            },
            4.0,
            3.0,
        ));
    }
    let root = tree.push_flex(
        TestStyle {
            size: Size::new(px(320.0), px(height)),
            ..TestStyle::default()
        },
        children,
    );
    finish(tree, root, definite(320.0, height))
}

fn integer_sqrt_ceil(value: usize) -> usize {
    let mut candidate = 1;
    while candidate * candidate < value {
        candidate += 1;
    }
    candidate
}

fn build_nested_column_flex(nodes: usize) -> BenchCase {
    let count = nodes.max(1);
    let branch_count = integer_sqrt_ceil(count);
    let leaves_per_branch = count.div_ceil(branch_count);
    let mut tree = TestTree::default();
    let mut branches = Vec::with_capacity(branch_count);
    let mut emitted = 0;
    for branch_index in 0..branch_count {
        let mut leaves = Vec::with_capacity(leaves_per_branch);
        for leaf_index in 0..leaves_per_branch {
            if emitted == count {
                break;
            }
            emitted += 1;
            let width = 4.0 + (leaf_index % 3) as f32;
            leaves.push(fixed_leaf(
                &mut tree,
                TestStyle {
                    size: Size::new(px(width), px(2.0)),
                    flex_basis: px(2.0),
                    ..TestStyle::default()
                },
                width,
                2.0,
            ));
        }
        branches.push(tree.push_flex(
            TestStyle {
                flex_direction: FlexDirection::Column,
                flex_basis: px(8.0 + (branch_index % 4) as f32),
                gap: Size::new(lp(0.0), lp(0.5)),
                align_items: Some(AlignItems::FlexStart),
                ..TestStyle::default()
            },
            leaves,
        ));
    }
    let root = tree.push_flex(
        TestStyle {
            align_items: Some(AlignItems::FlexStart),
            justify_content: Some(AlignContent::FlexStart),
            ..TestStyle::default()
        },
        branches,
    );
    finish(tree, root, known_width(count as f32))
}

fn build_in_flow_order_matrix(nodes: usize) -> BenchCase {
    let count = nodes.max(1);
    let mut tree = TestTree::default();
    let mut containers = Vec::with_capacity(count);
    for index in 0..count {
        let case_index = index;
        let mut children = Vec::with_capacity(5);
        for child_index in 0..5 {
            let width = 14.0 + child_index as f32 * 2.0;
            let height = 8.0 + (child_index % 3) as f32;
            children.push(fixed_leaf(
                &mut tree,
                TestStyle {
                    size: Size::new(px(width), px(height)),
                    flex_basis: px(width),
                    order: [-2, 3, 0, 1, -1][child_index] + [-1, 0, 1][case_index % 3],
                    margin: margin_px(
                        (child_index % 2) as f32 * 0.5,
                        0.0,
                        0.0,
                        (child_index % 3) as f32 * 0.5,
                    ),
                    ..TestStyle::default()
                },
                width,
                height,
            ));
        }
        containers.push(tree.push_flex(
            TestStyle {
                size: Size::new(px(122.0), px(52.0)),
                flex_basis: px(58.0),
                direction: direction(case_index),
                flex_direction: flex_direction(case_index),
                justify_content: Some(AlignContent::FlexStart),
                align_items: Some(AlignItems::FlexStart),
                gap: Size::new(lp(1.0), lp(0.0)),
                padding: Edges::uniform(lp(1.0)),
                margin: margin_px(1.0, 0.0, 1.0, 0.0),
                ..TestStyle::default()
            },
            children,
        ));
    }
    let height = count as f32 * 60.0 + 8.0;
    let root = column_wrapper(&mut tree, containers, 320.0, height);
    finish(tree, root, known_width(320.0))
}

fn spacing(tree: &mut TestTree, index: usize) -> LengthPercentage {
    match index % 9 {
        0 => lp(2.0 + (index % 5) as f32),
        1 => LengthPercentage::Percent(0.04 + (index % 7) as f32 / 100.0),
        2 => LengthPercentage::Calc(
            tree.push_calc(1.0 + (index % 3) as f32, 0.03 + (index % 5) as f32 / 100.0),
        ),
        // auto/fr/intrinsic values are not valid for CSS padding/gap. Keep
        // their phase in the nine-value period while mapping unsupported raw
        // values to the host's initial zero.
        3..=8 => LengthPercentage::ZERO,
        _ => unreachable!(),
    }
}

fn spacing_auto(tree: &mut TestTree, index: usize) -> LengthPercentageAuto {
    match index % 9 {
        0 => LengthPercentageAuto::Length(2.0 + (index % 5) as f32),
        1 => LengthPercentageAuto::Percent(0.04 + (index % 7) as f32 / 100.0),
        2 => LengthPercentageAuto::Calc(
            tree.push_calc(1.0 + (index % 3) as f32, 0.03 + (index % 5) as f32 / 100.0),
        ),
        3 => LengthPercentageAuto::Auto,
        // fr/intrinsic/fit-content are not valid CSS margin/inset values.
        4..=8 => LengthPercentageAuto::ZERO,
        _ => unreachable!(),
    }
}

fn build_full_value_spacing_matrix(nodes: usize) -> BenchCase {
    let count = nodes.max(1);
    let mut tree = TestTree::default();
    let mut containers = Vec::with_capacity(count);
    for index in 0..count {
        let case_index = index;
        let padding = edges(
            spacing(&mut tree, case_index),
            spacing(&mut tree, case_index + 1),
            spacing(&mut tree, case_index + 2),
            spacing(&mut tree, case_index + 3),
        );
        let gap = Size::new(
            spacing(&mut tree, case_index + 5),
            spacing(&mut tree, case_index + 4),
        );
        let mut children = Vec::with_capacity(4);
        for child_index in 0..4 {
            let base = case_index + child_index * 3;
            let inset = edges(
                spacing_auto(&mut tree, base),
                LengthPercentageAuto::Auto,
                spacing_auto(&mut tree, base + 1),
                LengthPercentageAuto::Auto,
            );
            let margin = edges(
                spacing_auto(&mut tree, base + 2),
                spacing_auto(&mut tree, base + 3),
                spacing_auto(&mut tree, base + 4),
                spacing_auto(&mut tree, base + 5),
            );
            let child_padding = edges(
                spacing(&mut tree, base + 6),
                spacing(&mut tree, base + 7),
                spacing(&mut tree, base + 8),
                spacing(&mut tree, base + 9),
            );
            let width = 18.0 + child_index as f32 * 3.0;
            let height = 8.0 + child_index as f32 * 2.0;
            children.push(fixed_leaf(
                &mut tree,
                TestStyle {
                    position: Position::Relative,
                    inset,
                    size: Size::new(px(width), px(height)),
                    flex_basis: px(width),
                    margin,
                    padding: child_padding,
                    border: edges(
                        lp(child_index as f32 * 0.5),
                        lp(0.5 + (child_index % 2) as f32),
                        lp((child_index % 3) as f32 * 0.25),
                        lp(1.0),
                    ),
                    ..TestStyle::default()
                },
                width,
                height,
            ));
        }
        containers.push(tree.push_flex(
            TestStyle {
                size: Size::new(px(128.0), px(64.0)),
                flex_basis: px(70.0),
                direction: direction(case_index),
                flex_direction: flex_direction(case_index),
                flex_wrap: FlexWrap::Wrap,
                justify_content: Some(AlignContent::FlexStart),
                align_items: Some(AlignItems::FlexStart),
                align_content: Some(AlignContent::FlexStart),
                padding,
                border: edges(
                    lp(1.0 + (case_index % 2) as f32),
                    lp((case_index % 3) as f32 * 0.5),
                    lp(0.5 + (case_index % 2) as f32),
                    lp((case_index % 4) as f32 * 0.25),
                ),
                gap,
                margin: margin_px(1.0, 0.0, 1.0, 0.0),
                ..TestStyle::default()
            },
            children,
        ));
    }
    let height = count as f32 * 72.0 + 8.0;
    let root = column_wrapper(&mut tree, containers, 320.0, height);
    finish(tree, root, known_width(320.0))
}

#[allow(clippy::too_many_lines)]
fn build_box_sizing_matrix(nodes: usize) -> BenchCase {
    let count = nodes.max(1);
    let mut tree = TestTree::default();
    let mut containers = Vec::with_capacity(count);
    for index in 0..count {
        let case_index = index;
        let width = match case_index % 3 {
            0 => px(42.0 + (case_index % 11) as f32),
            1 => Dimension::Percent(0.26 + (case_index % 7) as f32 / 100.0),
            _ => Dimension::Calc(tree.push_calc(
                8.0 + (case_index % 5) as f32,
                0.18 + (case_index % 4) as f32 / 100.0,
            )),
        };
        let max_width = Dimension::Calc(tree.push_calc(40.0 + (case_index % 9) as f32, 0.32));
        let max_height = Dimension::Calc(tree.push_calc(24.0 + (case_index % 6) as f32, 0.45));
        let content_width = 18.0 + (case_index % 9) as f32;
        let content_height = 8.0 + (case_index % 5) as f32;
        let content = fixed_leaf(
            &mut tree,
            TestStyle {
                size: Size::new(px(content_width), px(content_height)),
                flex_basis: px(content_width),
                margin: margin_px(
                    (case_index % 2) as f32,
                    0.0,
                    (case_index % 3) as f32 * 0.5,
                    (case_index % 2) as f32,
                ),
                padding: Edges::uniform(lp((case_index % 3) as f32 * 0.5)),
                border: Edges::uniform(lp((case_index % 2) as f32)),
                ..TestStyle::default()
            },
            content_width,
            content_height,
        );
        containers.push(
            tree.push_flex(
                TestStyle {
                    box_sizing: if case_index.is_multiple_of(2) {
                        BoxSizing::ContentBox
                    } else {
                        BoxSizing::BorderBox
                    },
                    size: Size::new(
                        width,
                        if case_index.is_multiple_of(4) {
                            Dimension::Auto
                        } else {
                            px(20.0 + (case_index % 9) as f32)
                        },
                    ),
                    min_size: Size::new(
                        px(24.0 + (case_index % 5) as f32),
                        px(12.0 + (case_index % 4) as f32),
                    ),
                    max_size: Size::new(max_width, max_height),
                    aspect_ratio: case_index
                        .is_multiple_of(4)
                        .then_some(1.15 + (case_index % 5) as f32 * 0.12),
                    flex_direction: if case_index.is_multiple_of(2) {
                        FlexDirection::Row
                    } else {
                        FlexDirection::Column
                    },
                    align_items: Some(AlignItems::Center),
                    justify_content: Some(AlignContent::Center),
                    margin: margin_px(
                        (case_index % 3) as f32,
                        (case_index % 4) as f32 * 0.5,
                        (case_index % 2) as f32,
                        0.0,
                    ),
                    padding: edges(
                        lp(1.0 + (case_index % 2) as f32),
                        lp(2.0 + (case_index % 3) as f32),
                        lp(1.0 + (case_index % 4) as f32 * 0.5),
                        lp(1.0),
                    ),
                    border: edges(
                        lp(1.0 + (case_index % 2) as f32),
                        lp(0.5 + (case_index % 3) as f32 * 0.5),
                        lp(1.0),
                        lp(0.5 + (case_index % 2) as f32),
                    ),
                    ..TestStyle::default()
                },
                vec![content],
            ),
        );
    }
    let root = tree.push_flex(
        TestStyle {
            size: Size::new(px(360.0), Dimension::Auto),
            flex_direction: FlexDirection::Column,
            align_items: Some(AlignItems::FlexStart),
            padding: Edges::uniform(lp(2.0)),
            border: Edges::uniform(lp(1.0)),
            ..TestStyle::default()
        },
        containers,
    );
    finish(tree, root, known_width(count as f32))
}

fn build_fit_content_subtrees(nodes: usize) -> BenchCase {
    let count = nodes.max(1);
    let mut tree = TestTree::default();
    let mut containers = Vec::with_capacity(count);
    for index in 0..count {
        let case_index = index;
        let content_width = 20.0 + (case_index % 17) as f32;
        let content_height = 8.0 + (case_index % 7) as f32;
        let content = fixed_leaf(
            &mut tree,
            TestStyle {
                size: Size::new(px(content_width), px(content_height)),
                flex_basis: px(content_width),
                padding: Edges::uniform(lp((case_index % 2) as f32)),
                border: Edges::uniform(lp((case_index % 3) as f32 * 0.5)),
                ..TestStyle::default()
            },
            content_width,
            content_height,
        );
        let width = tree.push_calc(
            4.0 + (case_index % 3) as f32,
            0.40 + (case_index % 5) as f32 * 0.03,
        );
        let height = tree.push_calc(
            2.0 + (case_index % 2) as f32,
            0.25 + (case_index % 4) as f32 * 0.04,
        );
        containers.push(tree.push_flex(
            TestStyle {
                size: Size::new(
                    Dimension::FitContent(LengthPercentage::Calc(width)),
                    Dimension::FitContent(LengthPercentage::Calc(height)),
                ),
                flex_direction: if case_index.is_multiple_of(2) {
                    FlexDirection::Row
                } else {
                    FlexDirection::Column
                },
                align_items: Some(AlignItems::FlexStart),
                justify_content: Some(AlignContent::FlexStart),
                margin: margin_px(
                    (case_index % 2) as f32,
                    (case_index % 3) as f32,
                    (case_index % 4) as f32 * 0.5,
                    0.0,
                ),
                padding: Edges::uniform(lp((case_index % 3) as f32 * 0.5)),
                border: Edges::uniform(lp((case_index % 2) as f32)),
                ..TestStyle::default()
            },
            vec![content],
        ));
    }
    let root = tree.push_flex(
        TestStyle {
            size: Size::new(px(320.0), Dimension::Auto),
            flex_direction: FlexDirection::Column,
            align_items: Some(AlignItems::FlexStart),
            padding: Edges::uniform(lp(2.0)),
            ..TestStyle::default()
        },
        containers,
    );
    finish(tree, root, known_width(320.0))
}

fn build_mixed_display_none(nodes: usize) -> BenchCase {
    let count = nodes.max(1);
    let mut tree = TestTree::default();
    let mut rows = Vec::with_capacity(count);
    for index in 0..count {
        let first_width = 10.0 + (index % 5) as f32;
        let first = fixed_leaf(
            &mut tree,
            TestStyle {
                size: Size::new(px(first_width), px(10.0)),
                flex_basis: px(first_width),
                ..TestStyle::default()
            },
            first_width,
            10.0,
        );
        let hidden = fixed_leaf(
            &mut tree,
            TestStyle {
                box_generation_mode: BoxGenerationMode::None,
                size: Size::new(px(80.0), px(20.0)),
                flex_basis: px(80.0),
                ..TestStyle::default()
            },
            80.0,
            20.0,
        );
        let second_width = 12.0 + (index % 3) as f32;
        let second = fixed_leaf(
            &mut tree,
            TestStyle {
                size: Size::new(px(second_width), px(10.0)),
                flex_basis: px(second_width),
                ..TestStyle::default()
            },
            second_width,
            10.0,
        );
        rows.push(tree.push_flex(
            TestStyle {
                size: Size::new(px(320.0), px(12.0)),
                flex_basis: px(12.0),
                align_items: Some(AlignItems::FlexStart),
                ..TestStyle::default()
            },
            vec![first, hidden, second],
        ));
    }
    let height = count as f32 * 12.0;
    let root = column_wrapper(&mut tree, rows, 320.0, height);
    finish(tree, root, known_width(320.0))
}
