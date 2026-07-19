//! Deterministic engine-native `display: linear` workloads for Divan.
//!
//! The retired Lynx gravity longhands are expressed through their CSS
//! equivalents: main-axis gravity through `justify-content`, cross-axis
//! gravity through `align-items`, and per-item layout gravity through
//! `align-self` (physical values via the `left`/`right` alignment flags,
//! `fill-*` via `stretch`). The matrix periods match the pre-swap fixtures
//! so the workload phase relationships stay identical.

#![allow(dead_code)]
#![allow(clippy::cast_precision_loss)]

use neutron_star::compute::{LeafMeasureInput, LeafMetrics};
use neutron_star::prelude::*;
use stylo::Zero;
use stylo::computed_values::{direction, linear_direction};
use stylo::values::computed::{
    ContentDistribution, Display, Inset, ItemPlacement, Length, LengthPercentage, Margin, MaxSize,
    NonNegativeLengthPercentage, Percentage, PositionProperty, SelfAlignment, Size as StyleSize,
};
use stylo::values::generics::NonNegative;
use stylo::values::specified::align::AlignFlags;

use crate::support::{TestId, TestStyle, TestTree, perform_layout};

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
    pub(super) root: TestId,
    pub(super) known_dimensions: Size<Option<f32>>,
    pub(super) available_space: Size<AvailableSpace>,
}

impl BenchCase {
    fn new(
        tree: TestTree,
        root: TestId,
        known_dimensions: Size<Option<f32>>,
        available_space: Size<AvailableSpace>,
    ) -> Self {
        tree.record_measure_inputs.set(false);
        Self {
            tree,
            root,
            known_dimensions,
            available_space,
        }
    }

    pub(super) fn node_count(&self) -> usize {
        self.tree.nodes.len()
    }

    pub(super) fn run(&mut self) -> LayoutOutput {
        perform_layout(
            &self.tree,
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

/// The pre-swap fixtures cycled eight orientations (four physical, four
/// logical). The fork grammar only has the four logical directions, so the
/// physical half maps onto the same values, preserving the period.
pub(super) const ORIENTATIONS: [linear_direction::T; 8] = [
    linear_direction::T::Row,
    linear_direction::T::RowReverse,
    linear_direction::T::Column,
    linear_direction::T::ColumnReverse,
    linear_direction::T::Row,
    linear_direction::T::RowReverse,
    linear_direction::T::Column,
    linear_direction::T::ColumnReverse,
];

/// Main-axis gravity as `justify-content` flags. The old physical
/// `top`/`bottom` values map to logical `start`/`end`; `left`/`right` stay
/// physical through the dedicated alignment flags. The old `none` phase
/// deferred to the fixture's `justify-content: flex-end`, so the merged
/// channel folds that fallback in directly.
pub(super) const MAIN_GRAVITIES: [AlignFlags; 11] = [
    AlignFlags::FLEX_END,
    AlignFlags::START,
    AlignFlags::END,
    AlignFlags::LEFT,
    AlignFlags::RIGHT,
    AlignFlags::CENTER,
    AlignFlags::CENTER,
    AlignFlags::SPACE_BETWEEN,
    AlignFlags::START,
    AlignFlags::END,
    AlignFlags::CENTER,
];

/// Per-item layout gravity as `align-self` flags (`fill-*` becomes
/// `stretch`).
pub(super) const LAYOUT_GRAVITIES: [AlignFlags; 13] = [
    AlignFlags::AUTO,
    AlignFlags::START,
    AlignFlags::END,
    AlignFlags::LEFT,
    AlignFlags::RIGHT,
    AlignFlags::CENTER,
    AlignFlags::CENTER,
    AlignFlags::STRETCH,
    AlignFlags::STRETCH,
    AlignFlags::CENTER,
    AlignFlags::STRETCH,
    AlignFlags::START,
    AlignFlags::END,
];

/// Cross-axis gravity as `align-items` flags. The old `none` phase deferred
/// to the fixture's `align-items: flex-start`, folded in directly now that
/// cross gravity and `align-items` are one channel.
pub(super) const CROSS_GRAVITIES: [AlignFlags; 5] = [
    AlignFlags::FLEX_START,
    AlignFlags::START,
    AlignFlags::END,
    AlignFlags::CENTER,
    AlignFlags::STRETCH,
];

pub(super) fn scenario_named(name: &str) -> Scenario {
    *SCENARIOS
        .iter()
        .find(|scenario| scenario.name == name)
        .unwrap_or_else(|| panic!("unknown Linear benchmark scenario {name}"))
}

fn lp(value: f32) -> LengthPercentage {
    LengthPercentage::new_length(Length::new(value))
}

fn pct_lp(fraction: f32) -> LengthPercentage {
    LengthPercentage::new_percent(Percentage(fraction))
}

fn px(value: f32) -> StyleSize {
    StyleSize::LengthPercentage(NonNegative(lp(value)))
}

fn pct(fraction: f32) -> StyleSize {
    StyleSize::LengthPercentage(NonNegative(pct_lp(fraction)))
}

fn is_row(direction: linear_direction::T) -> bool {
    matches!(
        direction,
        linear_direction::T::Row | linear_direction::T::RowReverse
    )
}

#[allow(clippy::needless_pass_by_value)]
fn fixed_leaf(tree: &mut TestTree, style: TestStyle, width: f32, height: f32) -> TestId {
    tree.push_leaf(
        TestStyle {
            size: Size::new(px(width), px(height)),
            ..style
        },
        Size::new(width, height),
        None,
    )
}

fn fixed_case(tree: TestTree, root: TestId, width: f32, height: f32) -> BenchCase {
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

fn intrinsic_case(tree: TestTree, root: TestId) -> BenchCase {
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
            linear_direction: linear_direction::T::Row,
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
                size: Size::new(StyleSize::Auto, px(10.0)),
                linear_weight: (1.0 + (index % 4) as f32).into(),
                ..TestStyle::default()
            },
            Size::new(1.0, 10.0),
            None,
        ));
    }
    let width = count as f32 * 8.0;
    let root = tree.push_linear(
        TestStyle {
            linear_direction: linear_direction::T::Row,
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
            0 => (
                StyleSize::Auto,
                MaxSize::LengthPercentage(NonNegative(lp(2.0 + (index % 7) as f32))),
            ),
            1 => (px(9.0 + (index % 5) as f32), MaxSize::None),
            _ => (StyleSize::Auto, MaxSize::None),
        };
        children.push(tree.push_leaf(
            TestStyle {
                size: Size::new(StyleSize::Auto, px(10.0)),
                min_size: Size::new(min, StyleSize::Auto),
                max_size: Size::new(max, MaxSize::None),
                linear_weight: (1.0 + (index % 3) as f32).into(),
                ..TestStyle::default()
            },
            Size::new(1.0, 10.0),
            None,
        ));
    }
    let width = count as f32 * 8.0;
    let root = tree.push_linear(
        TestStyle {
            linear_direction: linear_direction::T::Row,
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
            0 => StyleSize::Auto,
            1 => pct(0.5),
            2 => StyleSize::FitContentFunction(NonNegative(lp(80.0))),
            _ => px(40.0),
        };
        children.push(tree.push_measured_leaf(
            TestStyle {
                size: Size::new(width, StyleSize::Auto),
                align_self: if index.is_multiple_of(3) {
                    SelfAlignment(AlignFlags::STRETCH)
                } else {
                    SelfAlignment::auto()
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
                        display: Display::None,
                        ..TestStyle::default()
                    },
                    vec![hidden_leaf],
                ));
            }
            2 => children.push(fixed_leaf(
                &mut tree,
                TestStyle {
                    position: PositionProperty::Absolute,
                    inset: Edges {
                        left: Inset::LengthPercentage(lp((index % 13) as f32)),
                        right: Inset::Auto,
                        top: Inset::LengthPercentage(lp((index % 11) as f32)),
                        bottom: Inset::Auto,
                    },
                    ..TestStyle::default()
                },
                8.0,
                10.0,
            )),
            3 => children.push(fixed_leaf(
                &mut tree,
                TestStyle {
                    position: PositionProperty::Absolute,
                    align_self: SelfAlignment(AlignFlags::CENTER),
                    ..TestStyle::default()
                },
                8.0,
                10.0,
            )),
            4 => children.push(fixed_leaf(
                &mut tree,
                TestStyle {
                    position: PositionProperty::Fixed,
                    align_self: SelfAlignment(AlignFlags::END),
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
            linear_direction: linear_direction::T::Row,
            justify_content: ContentDistribution::new(AlignFlags::SPACE_BETWEEN),
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
            Margin::LengthPercentage(pct_lp(percentage))
        } else {
            Margin::LengthPercentage(LengthPercentage::zero())
        };
        children.push(fixed_leaf(
            &mut tree,
            TestStyle {
                margin: Edges {
                    left: margin,
                    right: Margin::LengthPercentage(LengthPercentage::zero()),
                    top: Margin::LengthPercentage(LengthPercentage::zero()),
                    bottom: Margin::LengthPercentage(LengthPercentage::zero()),
                },
                ..TestStyle::default()
            },
            8.0,
            10.0,
        ));
    }
    let root = tree.push_linear(
        TestStyle {
            linear_direction: linear_direction::T::Row,
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
                    left: NonNegative(pct_lp(percentage)),
                    right: NonNegativeLengthPercentage::zero(),
                    top: NonNegativeLengthPercentage::zero(),
                    bottom: NonNegativeLengthPercentage::zero(),
                },
                ..TestStyle::default()
            },
            8.0,
            10.0,
        ));
    }
    let root = tree.push_linear(
        TestStyle {
            linear_direction: linear_direction::T::Row,
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
                size: Size::new(pct(0.5), px(10.0)),
                ..TestStyle::default()
            },
            Size::new(8.0, 10.0),
            None,
        ));
    }
    let root = tree.push_linear(
        TestStyle {
            linear_direction: linear_direction::T::Row,
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
                min_size: Size::new(pct(0.5), StyleSize::Auto),
                ..TestStyle::default()
            },
            8.0,
            10.0,
        ));
    }
    let root = tree.push_linear(
        TestStyle {
            linear_direction: linear_direction::T::Row,
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
                    left: Inset::LengthPercentage(pct_lp(percentage)),
                    right: Inset::Auto,
                    top: Inset::Auto,
                    bottom: Inset::Auto,
                },
                ..TestStyle::default()
            },
            8.0,
            10.0,
        ));
    }
    let root = tree.push_linear(
        TestStyle {
            linear_direction: linear_direction::T::Row,
            ..TestStyle::default()
        },
        children,
    );
    intrinsic_case(tree, root)
}

fn direction(index: usize) -> direction::T {
    if index.is_multiple_of(2) {
        direction::T::Ltr
    } else {
        direction::T::Rtl
    }
}

fn matrix_root(tree: &mut TestTree, children: Vec<TestId>, count: usize) -> TestId {
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
            left: Margin::LengthPercentage(lp((child_index % 2) as f32)),
            right: Margin::LengthPercentage(lp(((index + child_index) % 3) as f32)),
            top: Margin::LengthPercentage(lp(((child_index + 1) % 2) as f32)),
            bottom: Margin::LengthPercentage(LengthPercentage::zero()),
        },
        ..TestStyle::default()
    }
}

fn push_matrix_children(tree: &mut TestTree, index: usize) -> Vec<TestId> {
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

fn matrix_container_style(index: usize, orientation: linear_direction::T) -> TestStyle {
    let row = is_row(orientation);
    TestStyle {
        size: Size::new(
            px(if row { 118.0 } else { 54.0 }),
            px(if row { 42.0 } else { 96.0 }),
        ),
        direction: direction(index),
        linear_direction: orientation,
        align_self: SelfAlignment(AlignFlags::START),
        padding: Edges {
            left: NonNegative(lp(2.0)),
            right: NonNegative(lp(3.0)),
            top: NonNegative(lp(4.0)),
            bottom: NonNegative(lp(5.0)),
        },
        margin: Edges {
            left: Margin::LengthPercentage(LengthPercentage::zero()),
            right: Margin::LengthPercentage(LengthPercentage::zero()),
            top: Margin::LengthPercentage(lp(1.0)),
            bottom: Margin::LengthPercentage(LengthPercentage::zero()),
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
                justify_content: ContentDistribution::new(
                    MAIN_GRAVITIES[index % MAIN_GRAVITIES.len()],
                ),
                align_items: ItemPlacement(AlignFlags::FLEX_START),
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
        tree.source_node_mut(children[1]).style.align_self =
            SelfAlignment(LAYOUT_GRAVITIES[index % LAYOUT_GRAVITIES.len()]);
        containers.push(tree.push_linear(
            TestStyle {
                align_items: ItemPlacement(AlignFlags::STRETCH),
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
                align_items: ItemPlacement(CROSS_GRAVITIES[index % CROSS_GRAVITIES.len()]),
                ..matrix_container_style(index, orientation)
            },
            children,
        ));
    }
    let root = matrix_root(&mut tree, containers, count);
    fixed_case(tree, root, 340.0, count as f32 * 100.0)
}
