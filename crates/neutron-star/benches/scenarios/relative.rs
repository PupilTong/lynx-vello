//! Rust-only lowerings of the nine `display: relative` benchmark workloads
//! from `PupilTong/lynx#25`.
//!
//! Seven source scenarios cycle through several display algorithms. Those
//! builders retain the exact Relative branch's index sequence and workload,
//! while [`Lowering::RelativeSlice`] records that the unrelated display
//! branches were deliberately omitted. The two dedicated Relative scenarios
//! are copied directly.

#![allow(dead_code, clippy::cast_precision_loss, clippy::too_many_lines)]

use neutron_star::compute::{LeafMeasureInput, LeafMetrics};
use neutron_star::prelude::*;
use neutron_star::style::{
    AlignContent, AlignItems, BoxGenerationMode, BoxSizing, Dimension, FlexDirection,
    LengthPercentage, LengthPercentageAuto, Position, RelativeCenter, RelativeReference,
};

use crate::support::{TestStyle, TestTree, perform_layout};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Lowering {
    Direct,
    RelativeSlice,
}

#[derive(Clone, Copy)]
pub(super) struct Scenario {
    pub(super) name: &'static str,
    pub(super) lowering: Lowering,
    pub(super) build: fn(usize) -> BenchCase,
}

#[derive(Debug)]
pub(super) struct BenchCase {
    pub(super) tree: TestTree,
    pub(super) root: NodeId,
    pub(super) known_dimensions: Size<Option<f32>>,
    pub(super) available_space: Size<AvailableSpace>,
}

impl BenchCase {
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
    ($name:literal, $lowering:ident, $build:ident) => {
        Scenario {
            name: $name,
            lowering: Lowering::$lowering,
            build: $build,
        }
    };
}

pub(super) const SCENARIOS: &[Scenario] = &[
    scenario!(
        "at_most_owner_matrix",
        RelativeSlice,
        build_at_most_owner_matrix
    ),
    scenario!(
        "baseline_propagation_matrix",
        RelativeSlice,
        build_baseline_propagation_matrix
    ),
    scenario!(
        "measured_callback_matrix",
        RelativeSlice,
        build_measured_callback_matrix
    ),
    scenario!("box_sizing_matrix", RelativeSlice, build_box_sizing_matrix),
    scenario!(
        "fit_content_subtrees",
        RelativeSlice,
        build_fit_content_subtrees
    ),
    scenario!(
        "relative_dependency_graph",
        Direct,
        build_relative_dependency_graph
    ),
    scenario!(
        "relative_center_matrix",
        Direct,
        build_relative_center_matrix
    ),
    scenario!(
        "sticky_percent_insets",
        RelativeSlice,
        build_sticky_percent_insets
    ),
    scenario!(
        "mixed_display_none",
        RelativeSlice,
        build_mixed_display_none
    ),
];

pub(super) fn scenario_named(name: &str) -> Scenario {
    *SCENARIOS
        .iter()
        .find(|scenario| scenario.name == name)
        .unwrap_or_else(|| panic!("unknown Relative benchmark scenario {name}"))
}

fn px(value: f32) -> Dimension {
    Dimension::Length(value)
}

fn lp(value: f32) -> LengthPercentage {
    LengthPercentage::Length(value)
}

fn margin(left: f32, right: f32, top: f32, bottom: f32) -> Edges<LengthPercentageAuto> {
    Edges {
        left: LengthPercentageAuto::Length(left),
        right: LengthPercentageAuto::Length(right),
        top: LengthPercentageAuto::Length(top),
        bottom: LengthPercentageAuto::Length(bottom),
    }
}

fn padding(left: f32, right: f32, top: f32, bottom: f32) -> Edges<LengthPercentage> {
    Edges {
        left: lp(left),
        right: lp(right),
        top: lp(top),
        bottom: lp(bottom),
    }
}

fn border(left: f32, right: f32, top: f32, bottom: f32) -> Edges<LengthPercentage> {
    padding(left, right, top, bottom)
}

fn reference(value: i32) -> RelativeReference {
    RelativeReference::new(value)
}

const LEFT: u8 = 1 << 0;
const RIGHT: u8 = 1 << 1;
const TOP: u8 = 1 << 2;
const BOTTOM: u8 = 1 << 3;

fn parent_edges(sides: u8) -> Edges<RelativeReference> {
    Edges {
        left: if sides & LEFT != 0 {
            RelativeReference::PARENT
        } else {
            RelativeReference::NONE
        },
        right: if sides & RIGHT != 0 {
            RelativeReference::PARENT
        } else {
            RelativeReference::NONE
        },
        top: if sides & TOP != 0 {
            RelativeReference::PARENT
        } else {
            RelativeReference::NONE
        },
        bottom: if sides & BOTTOM != 0 {
            RelativeReference::PARENT
        } else {
            RelativeReference::NONE
        },
    }
}

fn source_index(index: usize, period: usize, residue: usize) -> usize {
    index
        .checked_mul(period)
        .and_then(|index| index.checked_add(residue))
        .expect("the capped benchmark input has a representable source index")
}

fn finish(tree: TestTree, root: NodeId, available_space: Size<AvailableSpace>) -> BenchCase {
    BenchCase {
        tree,
        root,
        // PR #25 invokes `layout_with_owner_constraints`, so even definite
        // owner constraints are availability rather than known root sizes.
        known_dimensions: Size::NONE,
        available_space,
    }
}

fn wrap_space() -> Size<AvailableSpace> {
    Size::new(AvailableSpace::Definite(320.0), AvailableSpace::MaxContent)
}

fn column_wrapper(tree: &mut TestTree, style: TestStyle, children: Vec<NodeId>) -> NodeId {
    // neutron-star intentionally has no Block algorithm yet. A column Flex
    // adapter preserves the source Block root's child sequence while the
    // Relative containers themselves still use the production Relative path.
    tree.push_flex(
        TestStyle {
            flex_direction: FlexDirection::Column,
            align_items: Some(AlignItems::FlexStart),
            ..style
        },
        children,
    )
}

fn fixed_leaf(tree: &mut TestTree, style: TestStyle, width: f32, height: f32) -> NodeId {
    tree.push_leaf(style, Size::new(width, height), None)
}

fn build_at_most_owner_matrix(nodes: usize) -> BenchCase {
    let count = nodes.max(1);
    let mut tree = TestTree::default();
    let mut containers = Vec::with_capacity(count);

    for index in 0..count {
        // Relative is branch 4 of the source's five-display cycle.
        let source_index = source_index(index, 5, 4);
        let mut children = Vec::with_capacity(3);
        for child_index in 0_usize..3 {
            let width = match child_index {
                0 => Dimension::Auto,
                1 => Dimension::FitContent(LengthPercentage::Calc(tree.push_calc(6.0, 0.40))),
                _ => Dimension::Percent(0.35),
            };
            let height = match child_index {
                0 => Dimension::FitContent(lp(18.0)),
                1 => Dimension::Auto,
                _ => Dimension::Percent(0.30),
            };
            let intrinsic = Size::new(
                20.0 + (source_index % 7) as f32 + child_index as f32 * 4.0,
                10.0 + (source_index % 5) as f32 + child_index as f32 * 3.0,
            );
            let max_width = tree.push_calc(18.0 + child_index as f32 * 3.0, 0.45);
            let max_height = tree.push_calc(10.0 + child_index as f32 * 2.0, 0.55);
            children.push(tree.push_leaf(
                TestStyle {
                    size: Size::new(width, height),
                    min_size: Size::new(
                        px(12.0 + child_index as f32 * 2.0),
                        px(8.0 + child_index as f32),
                    ),
                    max_size: Size::new(Dimension::Calc(max_width), Dimension::Calc(max_height)),
                    margin: margin(
                        (child_index % 2) as f32,
                        (child_index % 3) as f32 * 0.5,
                        0.0,
                        0.0,
                    ),
                    relative_align: parent_edges(LEFT | TOP),
                    relative_center: match child_index {
                        0 => RelativeCenter::Horizontal,
                        1 => RelativeCenter::Vertical,
                        _ => RelativeCenter::Both,
                    },
                    ..TestStyle::default()
                },
                intrinsic,
                None,
            ));
        }

        let width = if source_index.is_multiple_of(2) {
            Dimension::Percent(0.42 + (source_index % 5) as f32 / 100.0)
        } else {
            Dimension::FitContent(LengthPercentage::Calc(
                tree.push_calc(18.0 + (source_index % 4) as f32, 0.45),
            ))
        };
        let height = if source_index.is_multiple_of(3) {
            Dimension::Auto
        } else {
            Dimension::FitContent(lp(34.0 + (source_index % 6) as f32))
        };
        let max_width = tree.push_calc(24.0, 0.60);
        let max_height = tree.push_calc(12.0, 0.70);
        containers.push(tree.push_relative(
            TestStyle {
                size: Size::new(width, height),
                min_size: Size::new(px(36.0), px(18.0)),
                max_size: Size::new(Dimension::Calc(max_width), Dimension::Calc(max_height)),
                margin: margin(1.0, 0.0, 1.0, 0.0),
                padding: padding(1.0, 2.0, 1.0, 2.0),
                ..TestStyle::default()
            },
            children,
        ));
    }

    let root_width = tree.push_calc(12.0, 0.80);
    let root_height = tree.push_calc(8.0, 0.70);
    let root_max_width = tree.push_calc(36.0, 0.80);
    let root_max_height = tree.push_calc(20.0, 0.85);
    let root = column_wrapper(
        &mut tree,
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
            padding: Edges::uniform(lp(1.0)),
            ..TestStyle::default()
        },
        containers,
    );
    finish(
        tree,
        root,
        Size::new(
            AvailableSpace::Definite(320.0),
            AvailableSpace::Definite(220.0),
        ),
    )
}

fn build_baseline_propagation_matrix(nodes: usize) -> BenchCase {
    let count = nodes.max(1);
    let mut tree = TestTree::default();
    let mut rows = Vec::with_capacity(count);

    for index in 0..count {
        // Relative is source branch 5 of the six-source baseline cycle.
        let source_index = source_index(index, 6, 5);
        let reference = tree.push_leaf(
            TestStyle {
                size: Size::new(px(12.0), px(32.0)),
                flex_basis: px(12.0),
                margin: margin(1.0, 1.0, 2.0, 1.0),
                ..TestStyle::default()
            },
            Size::new(12.0, 32.0),
            Some(26.0),
        );
        let relative_child = tree.push_leaf(
            TestStyle {
                relative_align: parent_edges(LEFT | TOP),
                ..TestStyle::default()
            },
            Size::new(12.0, 9.0),
            None,
        );
        let candidate = tree.push_relative(
            TestStyle {
                size: Size::new(px(24.0), px(18.0)),
                flex_basis: px(24.0),
                align_self: (!source_index.is_multiple_of(2)).then_some(AlignItems::Baseline),
                margin: margin(2.0, 1.0, 2.0, 1.0),
                ..TestStyle::default()
            },
            vec![relative_child],
        );
        let trailing = tree.push_leaf(
            TestStyle {
                size: Size::new(px(10.0), px(12.0)),
                flex_basis: px(10.0),
                margin: margin(0.0, 1.0, 0.0, 0.0),
                ..TestStyle::default()
            },
            Size::new(10.0, 12.0),
            None,
        );
        rows.push(tree.push_flex(
            TestStyle {
                size: Size::new(px(116.0), px(48.0)),
                flex_direction: FlexDirection::Row,
                align_items: Some(if source_index.is_multiple_of(2) {
                    AlignItems::Baseline
                } else {
                    AlignItems::FlexStart
                }),
                justify_content: Some(AlignContent::FlexStart),
                padding: padding(1.0, 2.0, 1.0, 2.0),
                margin: margin(1.0, 0.0, 1.0, 0.0),
                ..TestStyle::default()
            },
            vec![reference, candidate, trailing],
        ));
    }

    let root = column_wrapper(
        &mut tree,
        TestStyle {
            size: Size::new(px(320.0), px(count as f32 * 54.0 + 8.0)),
            ..TestStyle::default()
        },
        rows,
    );
    finish(tree, root, wrap_space())
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

fn build_measured_callback_matrix(nodes: usize) -> BenchCase {
    let count = nodes.max(1);
    let mut tree = TestTree::default();
    let mut containers = Vec::with_capacity(count);

    for index in 0..count {
        let source_index = source_index(index, 5, 4);
        let mut children = Vec::with_capacity(4);
        for child_index in 0_usize..4 {
            let intrinsic = Size::new(
                18.0 + (source_index % 7) as f32 + child_index as f32 * 3.0,
                9.0 + (source_index % 5) as f32 + child_index as f32 * 2.0,
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
                margin: margin(
                    (child_index % 2) as f32,
                    (child_index % 3) as f32 * 0.5,
                    (source_index % 2) as f32 * 0.5,
                    0.0,
                ),
                relative_align: parent_edges(LEFT | TOP),
                relative_center: match child_index {
                    0 => RelativeCenter::None,
                    1 => RelativeCenter::Horizontal,
                    2 => RelativeCenter::Vertical,
                    _ => RelativeCenter::Both,
                },
                ..TestStyle::default()
            };
            children.push(match child_index {
                0 => tree.push_measured_leaf(style, callback_metrics),
                1 => tree.push_measured_leaf(style, callback_metrics_without_baseline),
                2 => tree.push_leaf(
                    style,
                    intrinsic,
                    Some((4.0 + child_index as f32 * 2.0).min(intrinsic.height)),
                ),
                _ => tree.push_leaf(style, intrinsic, None),
            });
        }

        containers.push(tree.push_relative(
            TestStyle {
                size: Size::new(
                    if source_index.is_multiple_of(3) {
                        Dimension::FitContent(lp(126.0))
                    } else {
                        px(136.0)
                    },
                    if source_index.is_multiple_of(4) {
                        Dimension::FitContent(lp(44.0))
                    } else {
                        px(58.0)
                    },
                ),
                min_size: Size::new(px(72.0), px(28.0)),
                max_size: Size::new(px(180.0), px(92.0)),
                margin: margin(1.0, 0.0, 1.0, 0.0),
                padding: Edges::uniform(lp(1.0)),
                border: Edges::uniform(lp((source_index % 2) as f32 * 0.5)),
                ..TestStyle::default()
            },
            children,
        ));
    }

    let root = column_wrapper(
        &mut tree,
        TestStyle {
            size: Size::new(px(320.0), px(count as f32 * 70.0 + 8.0)),
            ..TestStyle::default()
        },
        containers,
    );
    finish(tree, root, wrap_space())
}

fn build_box_sizing_matrix(nodes: usize) -> BenchCase {
    let count = nodes.max(1);
    let mut tree = TestTree::default();
    let mut containers = Vec::with_capacity(count);

    for index in 0..count {
        let source_index = source_index(index, 5, 3);
        let content_width = 18.0 + (source_index % 9) as f32;
        let content_height = 8.0 + (source_index % 5) as f32;
        let content = fixed_leaf(
            &mut tree,
            TestStyle {
                size: Size::new(px(content_width), px(content_height)),
                margin: margin(
                    (source_index % 2) as f32,
                    0.0,
                    (source_index % 3) as f32 * 0.5,
                    (source_index % 2) as f32,
                ),
                padding: Edges::uniform(lp((source_index % 3) as f32 * 0.5)),
                border: Edges::uniform(lp((source_index % 2) as f32)),
                ..TestStyle::default()
            },
            content_width,
            content_height,
        );
        let width = match source_index % 3 {
            0 => px(42.0 + (source_index % 11) as f32),
            1 => Dimension::Percent(0.26 + (source_index % 7) as f32 / 100.0),
            _ => Dimension::Calc(tree.push_calc(
                8.0 + (source_index % 5) as f32,
                0.18 + (source_index % 4) as f32 / 100.0,
            )),
        };
        let max_width = tree.push_calc(40.0 + (source_index % 9) as f32, 0.32);
        let max_height = tree.push_calc(24.0 + (source_index % 6) as f32, 0.45);
        containers.push(
            tree.push_relative(
                TestStyle {
                    box_sizing: if source_index.is_multiple_of(2) {
                        BoxSizing::ContentBox
                    } else {
                        BoxSizing::BorderBox
                    },
                    size: Size::new(
                        width,
                        if source_index.is_multiple_of(4) {
                            Dimension::Auto
                        } else {
                            px(20.0 + (source_index % 9) as f32)
                        },
                    ),
                    min_size: Size::new(
                        px(24.0 + (source_index % 5) as f32),
                        px(12.0 + (source_index % 4) as f32),
                    ),
                    max_size: Size::new(Dimension::Calc(max_width), Dimension::Calc(max_height)),
                    aspect_ratio: source_index
                        .is_multiple_of(4)
                        .then_some(1.15 + (source_index % 5) as f32 * 0.12),
                    margin: margin(
                        (source_index % 3) as f32,
                        (source_index % 4) as f32 * 0.5,
                        (source_index % 2) as f32,
                        0.0,
                    ),
                    padding: padding(
                        1.0 + (source_index % 2) as f32,
                        2.0 + (source_index % 3) as f32,
                        1.0 + (source_index % 4) as f32 * 0.5,
                        1.0,
                    ),
                    border: border(
                        1.0 + (source_index % 2) as f32,
                        0.5 + (source_index % 3) as f32 * 0.5,
                        1.0,
                        0.5 + (source_index % 2) as f32,
                    ),
                    align_items: Some(AlignItems::Center),
                    justify_content: Some(AlignContent::Center),
                    ..TestStyle::default()
                },
                vec![content],
            ),
        );
    }

    let root = column_wrapper(
        &mut tree,
        TestStyle {
            size: Size::new(px(360.0), Dimension::Auto),
            padding: Edges::uniform(lp(2.0)),
            border: Edges::uniform(lp(1.0)),
            ..TestStyle::default()
        },
        containers,
    );
    finish(
        tree,
        root,
        Size::new(
            AvailableSpace::Definite(count as f32),
            AvailableSpace::MaxContent,
        ),
    )
}

fn build_fit_content_subtrees(nodes: usize) -> BenchCase {
    let count = nodes.max(1);
    let mut tree = TestTree::default();
    let mut containers = Vec::with_capacity(count);

    for index in 0..count {
        let source_index = source_index(index, 5, 3);
        let content_width = 20.0 + (source_index % 17) as f32;
        let content_height = 8.0 + (source_index % 7) as f32;
        let content = fixed_leaf(
            &mut tree,
            TestStyle {
                size: Size::new(px(content_width), px(content_height)),
                padding: Edges::uniform(lp((source_index % 2) as f32)),
                border: Edges::uniform(lp((source_index % 3) as f32 * 0.5)),
                ..TestStyle::default()
            },
            content_width,
            content_height,
        );
        let width = tree.push_calc(
            4.0 + (source_index % 3) as f32,
            0.40 + (source_index % 5) as f32 * 0.03,
        );
        let height = tree.push_calc(
            2.0 + (source_index % 2) as f32,
            0.25 + (source_index % 4) as f32 * 0.04,
        );
        containers.push(tree.push_relative(
            TestStyle {
                size: Size::new(
                    Dimension::FitContent(LengthPercentage::Calc(width)),
                    Dimension::FitContent(LengthPercentage::Calc(height)),
                ),
                margin: margin(
                    (source_index % 2) as f32,
                    (source_index % 3) as f32,
                    (source_index % 4) as f32 * 0.5,
                    0.0,
                ),
                padding: Edges::uniform(lp((source_index % 3) as f32 * 0.5)),
                border: Edges::uniform(lp((source_index % 2) as f32)),
                align_items: Some(AlignItems::FlexStart),
                justify_content: Some(AlignContent::FlexStart),
                ..TestStyle::default()
            },
            vec![content],
        ));
    }

    let root = column_wrapper(
        &mut tree,
        TestStyle {
            size: Size::new(px(320.0), Dimension::Auto),
            padding: Edges::uniform(lp(2.0)),
            ..TestStyle::default()
        },
        containers,
    );
    finish(tree, root, wrap_space())
}

fn build_relative_dependency_graph(nodes: usize) -> BenchCase {
    let count = nodes.max(1);
    let mut tree = TestTree::default();
    let mut children = Vec::with_capacity(count);

    for index in 0..count {
        let relative_id = i32::try_from(index / 4 + 1).expect("benchmark id fits i32");
        let style = match index % 4 {
            0 => TestStyle {
                size: Size::new(px(18.0 + (index % 7) as f32), px(8.0 + (index % 5) as f32)),
                relative_id: reference(relative_id),
                relative_align: parent_edges(RIGHT | BOTTOM),
                ..TestStyle::default()
            },
            1 => TestStyle {
                size: Size::new(px(5.0 + (index % 3) as f32), px(4.0 + (index % 4) as f32)),
                relative_adjacent: Edges {
                    left: RelativeReference::NONE,
                    right: reference(relative_id),
                    top: RelativeReference::NONE,
                    bottom: reference(relative_id),
                },
                ..TestStyle::default()
            },
            2 => TestStyle {
                size: Size::new(px(12.0 + (index % 5) as f32), px(6.0 + (index % 3) as f32)),
                relative_id: reference(relative_id),
                ..TestStyle::default()
            },
            _ => TestStyle {
                size: Size::new(px(4.0 + (index % 4) as f32), px(3.0 + (index % 5) as f32)),
                relative_align: Edges {
                    left: reference(relative_id),
                    right: RelativeReference::NONE,
                    top: RelativeReference::NONE,
                    bottom: reference(relative_id),
                },
                ..TestStyle::default()
            },
        };
        let intrinsic = Size::new(
            match style.size.width {
                Dimension::Length(value) => value,
                _ => 0.0,
            },
            match style.size.height {
                Dimension::Length(value) => value,
                _ => 0.0,
            },
        );
        children.push(tree.push_leaf(style, intrinsic, None));
    }

    let root = tree.push_relative(
        TestStyle {
            size: Size::new(px(320.0), px(160.0)),
            ..TestStyle::default()
        },
        children,
    );
    finish(
        tree,
        root,
        Size::new(
            AvailableSpace::Definite(320.0),
            AvailableSpace::Definite(160.0),
        ),
    )
}

fn build_relative_center_matrix(nodes: usize) -> BenchCase {
    let count = nodes.max(1);
    let mut tree = TestTree::default();
    let mut children = Vec::with_capacity(count);

    for index in 0..count {
        let mut style = TestStyle {
            relative_center: match index % 4 {
                0 => RelativeCenter::None,
                1 => RelativeCenter::Horizontal,
                2 => RelativeCenter::Vertical,
                _ => RelativeCenter::Both,
            },
            margin: margin(
                (index % 3) as f32,
                (index % 2) as f32,
                (index % 4) as f32 * 0.5,
                (index % 5) as f32 * 0.5,
            ),
            padding: Edges::uniform(lp((index % 2) as f32)),
            border: Edges::uniform(lp((index % 3) as f32 * 0.5)),
            ..TestStyle::default()
        };
        style.relative_align = match index % 4 {
            0 => parent_edges(LEFT | TOP),
            1 => parent_edges(RIGHT),
            2 => parent_edges(BOTTOM),
            _ => parent_edges(LEFT | RIGHT | TOP | BOTTOM),
        };
        let intrinsic = Size::new(18.0 + (index % 7) as f32, 10.0 + (index % 5) as f32);
        children.push(tree.push_leaf(style, intrinsic, None));
    }

    let root = tree.push_relative(
        TestStyle {
            size: Size::new(px(320.0), px(220.0)),
            padding: padding(3.0, 5.0, 7.0, 11.0),
            border: border(1.0, 2.0, 1.0, 2.0),
            ..TestStyle::default()
        },
        children,
    );
    finish(
        tree,
        root,
        Size::new(AvailableSpace::MaxContent, AvailableSpace::MaxContent),
    )
}

fn build_sticky_percent_insets(nodes: usize) -> BenchCase {
    let count = nodes.max(1);
    let mut tree = TestTree::default();
    let mut containers = Vec::with_capacity(count);

    for index in 0..count {
        let source_index = source_index(index, 4, 3);
        let sticky_width = 20.0 + (source_index % 5) as f32;
        let sticky_height = 10.0 + (source_index % 3) as f32;
        let sticky = fixed_leaf(
            &mut tree,
            TestStyle {
                // Sticky is a host post-pass in neutron-star. Preserve the
                // source inset-resolution workload as an in-flow relative
                // visual offset inside the Relative container.
                position: Position::Relative,
                inset: Edges {
                    left: LengthPercentageAuto::Percent(0.10),
                    right: if source_index.is_multiple_of(3) {
                        LengthPercentageAuto::Percent(0.05)
                    } else {
                        LengthPercentageAuto::Auto
                    },
                    top: LengthPercentageAuto::Percent(0.25),
                    bottom: if source_index.is_multiple_of(5) {
                        LengthPercentageAuto::Percent(0.10)
                    } else {
                        LengthPercentageAuto::Auto
                    },
                },
                size: Size::new(px(sticky_width), px(sticky_height)),
                ..TestStyle::default()
            },
            sticky_width,
            sticky_height,
        );
        let normal_width = 8.0 + (source_index % 7) as f32;
        let normal_height = 6.0 + (source_index % 5) as f32;
        let normal = fixed_leaf(
            &mut tree,
            TestStyle {
                size: Size::new(px(normal_width), px(normal_height)),
                ..TestStyle::default()
            },
            normal_width,
            normal_height,
        );
        containers.push(tree.push_relative(
            TestStyle {
                size: Size::new(px(320.0), px(40.0)),
                ..TestStyle::default()
            },
            vec![sticky, normal],
        ));
    }

    let root = column_wrapper(
        &mut tree,
        TestStyle {
            size: Size::new(px(320.0), px(count as f32 * 44.0 + 8.0)),
            ..TestStyle::default()
        },
        containers,
    );
    finish(
        tree,
        root,
        Size::new(
            AvailableSpace::Definite(320.0),
            AvailableSpace::Definite(240.0),
        ),
    )
}

fn build_mixed_display_none(nodes: usize) -> BenchCase {
    let count = nodes.max(1);
    let mut tree = TestTree::default();
    let mut containers = Vec::with_capacity(count);

    for index in 0..count {
        let source_index = source_index(index, 4, 3);
        let relative_id = i32::try_from(source_index + 1).expect("benchmark relative id fits i32");
        let anchor_width = 20.0 + (source_index % 5) as f32;
        let anchor_height = 8.0 + (source_index % 3) as f32;
        let visible_anchor = fixed_leaf(
            &mut tree,
            TestStyle {
                size: Size::new(px(anchor_width), px(anchor_height)),
                relative_id: reference(relative_id),
                ..TestStyle::default()
            },
            anchor_width,
            anchor_height,
        );
        let follower_width = 5.0 + (source_index % 4) as f32;
        let follower_height = 4.0 + (source_index % 2) as f32;
        let follower = fixed_leaf(
            &mut tree,
            TestStyle {
                size: Size::new(px(follower_width), px(follower_height)),
                relative_adjacent: Edges {
                    left: RelativeReference::NONE,
                    right: reference(relative_id),
                    top: RelativeReference::NONE,
                    bottom: reference(relative_id),
                },
                ..TestStyle::default()
            },
            follower_width,
            follower_height,
        );
        let hidden_anchor = fixed_leaf(
            &mut tree,
            TestStyle {
                box_generation_mode: BoxGenerationMode::None,
                size: Size::new(px(80.0), px(30.0)),
                relative_id: reference(relative_id),
                ..TestStyle::default()
            },
            80.0,
            30.0,
        );
        containers.push(tree.push_relative(
            TestStyle {
                size: Size::new(px(320.0), px(24.0)),
                ..TestStyle::default()
            },
            vec![visible_anchor, follower, hidden_anchor],
        ));
    }

    let root = column_wrapper(
        &mut tree,
        TestStyle {
            size: Size::new(px(320.0), Dimension::Auto),
            ..TestStyle::default()
        },
        containers,
    );
    finish(tree, root, wrap_space())
}
