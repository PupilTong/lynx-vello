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
use style_traits::values::specified::AllowedNumericType;
use stylo::Zero;
use stylo::computed_values::{box_sizing, direction, flex_direction, flex_wrap};
use stylo::values::computed::length::NonNegativeLengthPercentageOrNormal;
use stylo::values::computed::length_percentage::{CalcNode, ComputedLeaf};
use stylo::values::computed::{
    AspectRatio, Au, BorderSideWidth, ContentDistribution, Display, FlexBasis, Inset,
    ItemPlacement, Length, LengthPercentage, Margin, MaxSize, NonNegativeLengthPercentage,
    Percentage, PositionProperty, Ratio, SelfAlignment, Size as StyleSize,
};
use stylo::values::generics::NonNegative;
use stylo::values::generics::position::PreferredRatio;
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

fn uniform<T: Clone>(value: T) -> Edges<T> {
    Edges {
        left: value.clone(),
        right: value.clone(),
        top: value.clone(),
        bottom: value,
    }
}

fn lp(value: f32) -> LengthPercentage {
    LengthPercentage::new_length(Length::new(value))
}

fn pct_lp(fraction: f32) -> LengthPercentage {
    LengthPercentage::new_percent(Percentage(fraction))
}

/// `calc(<percentage> + <length>)`, the shape the deleted test-calc arena
/// used to describe symbolically.
fn calc_lp(length: f32, percentage: f32) -> LengthPercentage {
    LengthPercentage::new_calc(
        CalcNode::Sum(
            vec![
                CalcNode::Leaf(ComputedLeaf::Percentage(Percentage(percentage))),
                CalcNode::Leaf(ComputedLeaf::Length(Length::new(length))),
            ]
            .into(),
        ),
        AllowedNumericType::All,
    )
}

fn px(value: f32) -> StyleSize {
    StyleSize::LengthPercentage(NonNegative(lp(value)))
}

fn pct(fraction: f32) -> StyleSize {
    StyleSize::LengthPercentage(NonNegative(pct_lp(fraction)))
}

fn calc_size(length: f32, percentage: f32) -> StyleSize {
    StyleSize::LengthPercentage(NonNegative(calc_lp(length, percentage)))
}

fn fit_content(limit: LengthPercentage) -> StyleSize {
    StyleSize::FitContentFunction(NonNegative(limit))
}

fn max_px(value: f32) -> MaxSize {
    MaxSize::LengthPercentage(NonNegative(lp(value)))
}

fn max_pct(fraction: f32) -> MaxSize {
    MaxSize::LengthPercentage(NonNegative(pct_lp(fraction)))
}

fn max_calc(length: f32, percentage: f32) -> MaxSize {
    MaxSize::LengthPercentage(NonNegative(calc_lp(length, percentage)))
}

fn basis_px(value: f32) -> FlexBasis {
    FlexBasis::Size(px(value))
}

fn margin_len(value: f32) -> Margin {
    Margin::LengthPercentage(lp(value))
}

fn margin_px(left: f32, right: f32, top: f32, bottom: f32) -> Edges<Margin> {
    edges(
        margin_len(left),
        margin_len(right),
        margin_len(top),
        margin_len(bottom),
    )
}

fn pad(value: f32) -> NonNegativeLengthPercentage {
    NonNegative(lp(value))
}

fn bw(value: f32) -> BorderSideWidth {
    BorderSideWidth(Au::from_f32_px(value))
}

fn gap_of(value: NonNegativeLengthPercentage) -> NonNegativeLengthPercentageOrNormal {
    NonNegativeLengthPercentageOrNormal::LengthPercentage(value)
}

fn gap_px(value: f32) -> NonNegativeLengthPercentageOrNormal {
    gap_of(pad(value))
}

fn ratio(value: f32) -> AspectRatio {
    AspectRatio {
        auto: false,
        ratio: PreferredRatio::Ratio(Ratio::new(value, 1.0)),
    }
}

fn direction(index: usize) -> direction::T {
    if index.is_multiple_of(2) {
        direction::T::Ltr
    } else {
        direction::T::Rtl
    }
}

fn flex_direction(index: usize) -> flex_direction::T {
    match index % 4 {
        0 => flex_direction::T::Row,
        1 => flex_direction::T::RowReverse,
        2 => flex_direction::T::Column,
        _ => flex_direction::T::ColumnReverse,
    }
}

fn is_row(axis: flex_direction::T) -> bool {
    matches!(axis, flex_direction::T::Row | flex_direction::T::RowReverse)
}

fn justify_content(index: usize) -> ContentDistribution {
    ContentDistribution::new(match index % 9 {
        0 => AlignFlags::STRETCH,
        1 => AlignFlags::FLEX_START,
        2 => AlignFlags::START,
        3 => AlignFlags::CENTER,
        4 => AlignFlags::FLEX_END,
        5 => AlignFlags::END,
        6 => AlignFlags::SPACE_BETWEEN,
        7 => AlignFlags::SPACE_AROUND,
        _ => AlignFlags::SPACE_EVENLY,
    })
}

fn align_items(index: usize) -> ItemPlacement {
    ItemPlacement(match (index / 9) % 7 {
        0 => AlignFlags::STRETCH,
        1 => AlignFlags::FLEX_START,
        2 => AlignFlags::START,
        3 => AlignFlags::CENTER,
        4 => AlignFlags::FLEX_END,
        5 => AlignFlags::END,
        _ => AlignFlags::BASELINE,
    })
}

fn flex_wrap(index: usize) -> flex_wrap::T {
    match index % 3 {
        0 => flex_wrap::T::Nowrap,
        1 => flex_wrap::T::Wrap,
        _ => flex_wrap::T::WrapReverse,
    }
}

fn align_content(index: usize) -> ContentDistribution {
    ContentDistribution::new(match index % 9 {
        0 => AlignFlags::FLEX_START,
        1 => AlignFlags::START,
        2 => AlignFlags::CENTER,
        3 => AlignFlags::FLEX_END,
        4 => AlignFlags::END,
        5 => AlignFlags::SPACE_BETWEEN,
        6 => AlignFlags::SPACE_AROUND,
        7 => AlignFlags::SPACE_EVENLY,
        _ => AlignFlags::STRETCH,
    })
}

fn baseline_self(trigger: bool) -> SelfAlignment {
    if trigger {
        SelfAlignment(AlignFlags::BASELINE)
    } else {
        SelfAlignment::auto()
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
    root: TestId,
    constraints: (Size<Option<f32>>, Size<AvailableSpace>),
) -> BenchCase {
    BenchCase::new(tree, root, constraints.0, constraints.1)
}

fn fixed_leaf(tree: &mut TestTree, style: TestStyle, width: f32, height: f32) -> TestId {
    tree.push_leaf(style, Size::new(width, height), None)
}

fn column_wrapper(tree: &mut TestTree, children: Vec<TestId>, width: f32, height: f32) -> TestId {
    tree.push_flex(
        TestStyle {
            size: Size::new(px(width), px(height)),
            flex_direction: flex_direction::T::Column,
            align_items: ItemPlacement(AlignFlags::FLEX_START),
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
                flex_basis: basis_px(basis),
                flex_grow: (1.0 + (index % 3) as f32).into(),
                ..TestStyle::default()
            },
            basis,
            10.0,
        ));
    }
    let root = tree.push_flex(
        TestStyle {
            size: Size::new(StyleSize::Auto, px(10.0)),
            align_items: ItemPlacement(AlignFlags::STRETCH),
            justify_content: ContentDistribution::new(AlignFlags::FLEX_START),
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
                flex_basis: basis_px(basis),
                ..TestStyle::default()
            },
            basis,
            height,
        ));
    }
    let root = tree.push_flex(
        TestStyle {
            size: Size::new(px(320.0), StyleSize::Auto),
            flex_wrap: flex_wrap::T::Wrap,
            gap: Size::new(gap_px(1.0), gap_px(1.0)),
            align_items: ItemPlacement(AlignFlags::FLEX_START),
            justify_content: ContentDistribution::new(AlignFlags::FLEX_START),
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
                size: Size::new(StyleSize::Auto, px(height)),
                flex_basis: basis_px(basis),
                ..TestStyle::default()
            },
            basis,
            height,
        ));
    }
    let root = tree.push_flex(
        TestStyle {
            align_items: ItemPlacement(AlignFlags::FLEX_START),
            justify_content: ContentDistribution::new(AlignFlags::FLEX_START),
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
        let max_width = max_calc(24.0, 0.60);
        let max_height = max_calc(12.0, 0.70);
        let width = if case_index.is_multiple_of(2) {
            pct(0.42 + (case_index % 5) as f32 / 100.0)
        } else {
            fit_content(calc_lp(18.0 + (case_index % 4) as f32, 0.45))
        };
        let height = if case_index.is_multiple_of(3) {
            StyleSize::Auto
        } else {
            fit_content(lp(34.0 + (case_index % 6) as f32))
        };

        let mut children = Vec::with_capacity(3);
        for child_index in 0..3 {
            let child_max_width = max_calc(18.0 + child_index as f32 * 3.0, 0.45);
            let child_max_height = max_calc(10.0 + child_index as f32 * 2.0, 0.55);
            let intrinsic = Size::new(
                20.0 + (case_index % 7) as f32 + child_index as f32 * 4.0,
                10.0 + (case_index % 5) as f32 + child_index as f32 * 3.0,
            );
            children.push(tree.push_leaf(
                TestStyle {
                    size: Size::new(
                        match child_index {
                            0 => StyleSize::Auto,
                            1 => fit_content(calc_lp(6.0, 0.40)),
                            _ => pct(0.35),
                        },
                        match child_index {
                            0 => fit_content(lp(18.0)),
                            1 => StyleSize::Auto,
                            _ => pct(0.30),
                        },
                    ),
                    min_size: Size::new(
                        px(12.0 + child_index as f32 * 2.0),
                        px(8.0 + child_index as f32),
                    ),
                    max_size: Size::new(child_max_width, child_max_height),
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
                max_size: Size::new(max_width, max_height),
                flex_wrap: flex_wrap::T::Wrap,
                align_items: ItemPlacement(AlignFlags::FLEX_START),
                align_content: ContentDistribution::new(AlignFlags::FLEX_START),
                justify_content: ContentDistribution::new(AlignFlags::FLEX_START),
                gap: Size::new(gap_px(1.0), gap_px(1.0)),
                padding: uniform(pad(1.0)),
                margin: margin_px(1.0, 0.0, 1.0, 0.0),
                ..TestStyle::default()
            },
            children,
        ));
    }

    // A column Flex wrapper supplies the owner constraints for this workload.
    let root = tree.push_flex(
        TestStyle {
            size: Size::new(
                fit_content(calc_lp(12.0, 0.80)),
                fit_content(calc_lp(8.0, 0.70)),
            ),
            min_size: Size::new(pct(0.35), px(48.0)),
            max_size: Size::new(max_calc(36.0, 0.80), max_calc(20.0, 0.85)),
            flex_direction: flex_direction::T::Column,
            align_items: ItemPlacement(AlignFlags::FLEX_START),
            padding: uniform(pad(1.0)),
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
        for row_direction in [direction::T::Rtl, direction::T::Ltr] {
            let width = 10.0 + (index % 3) as f32;
            let height = 5.0 + (index % 2) as f32;
            let leaf = fixed_leaf(
                &mut tree,
                TestStyle {
                    size: Size::new(px(width), px(height)),
                    flex_basis: basis_px(width),
                    direction: row_direction,
                    ..TestStyle::default()
                },
                width,
                height,
            );
            rows.push(tree.push_flex(
                TestStyle {
                    size: Size::new(px(30.0), px(10.0 + (index % 2) as f32)),
                    flex_basis: basis_px(10.0),
                    direction: row_direction,
                    justify_content: ContentDistribution::new(AlignFlags::FLEX_START),
                    align_items: ItemPlacement(AlignFlags::FLEX_START),
                    ..TestStyle::default()
                },
                vec![leaf],
            ));
        }
    }
    let root = tree.push_flex(
        TestStyle {
            size: Size::new(px(30.0), px(count as f32 * 20.0)),
            direction: direction::T::Rtl,
            flex_direction: flex_direction::T::Column,
            align_items: ItemPlacement(AlignFlags::FLEX_START),
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
            let size = if is_row(axis) {
                Size::new(
                    px(basis),
                    if auto_cross {
                        StyleSize::Auto
                    } else {
                        px(cross)
                    },
                )
            } else {
                Size::new(
                    if auto_cross {
                        StyleSize::Auto
                    } else {
                        px(cross)
                    },
                    px(basis),
                )
            };
            children.push(tree.push_leaf(
                TestStyle {
                    size,
                    flex_basis: basis_px(basis),
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
                flex_basis: basis_px(80.0),
                direction: direction(index),
                flex_direction: axis,
                justify_content: justify_content(index),
                align_items: align_items(index),
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
        let container_size = if is_row(axis) {
            Size::new(px(main), px(cross))
        } else {
            Size::new(px(cross), px(main))
        };
        let mut children = Vec::with_capacity(5);
        for child_index in 0..5 {
            let basis = match child_index {
                0 => basis_px(18.0),
                1 => basis_px(28.0),
                2 => basis_px(36.0),
                3 => basis_px(22.0),
                _ => FlexBasis::Size(pct(0.18)),
            };
            let grow = [0.0, 1.0, 2.0, 1.5, 0.5][child_index];
            let shrink = [1.0, 2.0, 0.5, 1.5, 0.0][child_index];
            let order = [-1, 2, 0, 3, -2][child_index] + [0, 1][index % 2];
            let cross_size = 12.0 + child_index as f32 * 2.0;
            let (min_main, max_main) = match child_index {
                0 => (px(24.0), MaxSize::None),
                1 => (StyleSize::Auto, max_px(32.0)),
                2 => (pct(0.22), MaxSize::None),
                3 => (StyleSize::Auto, max_pct(0.24)),
                _ => (StyleSize::Auto, MaxSize::None),
            };
            let (size, min_size, max_size) = if is_row(axis) {
                (
                    Size::new(StyleSize::Auto, px(cross_size)),
                    Size::new(min_main, StyleSize::Auto),
                    Size::new(max_main, MaxSize::None),
                )
            } else {
                (
                    Size::new(px(cross_size), StyleSize::Auto),
                    Size::new(StyleSize::Auto, min_main),
                    Size::new(MaxSize::None, max_main),
                )
            };
            children.push(tree.push_leaf(
                TestStyle {
                    size,
                    min_size,
                    max_size,
                    flex_basis: basis,
                    flex_grow: grow.into(),
                    flex_shrink: shrink.into(),
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
                flex_basis: basis_px(84.0),
                direction: direction(index),
                flex_direction: axis,
                flex_wrap: flex_wrap::T::Nowrap,
                justify_content: ContentDistribution::new(AlignFlags::FLEX_START),
                align_items: ItemPlacement(AlignFlags::FLEX_START),
                gap: Size::new(gap_px(1.0), gap_px(1.0)),
                padding: edges(pad(2.0), pad(2.0), pad(1.0), pad(1.0)),
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
                    flex_basis: basis_px(if is_row(axis) { width } else { height }),
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
                flex_basis: basis_px(70.0),
                direction: direction(index),
                flex_direction: axis,
                flex_wrap: flex_wrap(index),
                justify_content: ContentDistribution::new(AlignFlags::FLEX_START),
                align_content: align_content(index),
                align_items: align_items(index),
                gap: Size::new(gap_px(2.0), gap_px(3.0)),
                padding: edges(pad(3.0), pad(5.0), pad(2.0), pad(4.0)),
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
                align_self: baseline_self(index.is_multiple_of(3)),
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
            size: Size::new(px(320.0), StyleSize::Auto),
            align_items: ItemPlacement(AlignFlags::BASELINE),
            justify_content: ContentDistribution::new(AlignFlags::FLEX_START),
            ..TestStyle::default()
        },
        children,
    );
    finish(tree, root, known_width(320.0))
}

fn baseline_leaf(tree: &mut TestTree, width: f32, height: f32, baseline: f32) -> TestId {
    tree.push_leaf(
        TestStyle {
            size: Size::new(px(width), px(height)),
            flex_basis: basis_px(width),
            ..TestStyle::default()
        },
        Size::new(width, height),
        Some(baseline.min(height)),
    )
}

fn nested_baseline_flex(tree: &mut TestTree, column: bool, trigger: bool) -> TestId {
    let first = baseline_leaf(tree, 10.0, 18.0, 7.0);
    let second = baseline_leaf(tree, 12.0, 24.0, 18.0);
    tree.push_flex(
        TestStyle {
            size: Size::new(px(28.0), px(if column { 40.0 } else { 26.0 })),
            flex_basis: basis_px(28.0),
            flex_direction: if column {
                flex_direction::T::Column
            } else {
                flex_direction::T::Row
            },
            align_items: ItemPlacement(if column {
                AlignFlags::FLEX_START
            } else {
                AlignFlags::BASELINE
            }),
            justify_content: ContentDistribution::new(if column {
                AlignFlags::CENTER
            } else {
                AlignFlags::FLEX_START
            }),
            align_self: baseline_self(trigger),
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
                flex_basis: basis_px(12.0),
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
                    flex_basis: basis_px(18.0),
                    align_self: baseline_self(trigger),
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
                flex_basis: basis_px(10.0),
                margin: margin_px(0.0, 1.0, 0.0, 0.0),
                ..TestStyle::default()
            },
            Size::new(10.0, 12.0),
            None,
        );
        rows.push(tree.push_flex(
            TestStyle {
                size: Size::new(px(116.0), px(48.0)),
                flex_basis: basis_px(52.0),
                align_items: ItemPlacement(if index.is_multiple_of(2) {
                    AlignFlags::BASELINE
                } else {
                    AlignFlags::FLEX_START
                }),
                justify_content: ContentDistribution::new(AlignFlags::FLEX_START),
                padding: edges(pad(2.0), pad(2.0), pad(1.0), pad(1.0)),
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
                        fit_content(lp(36.0))
                    } else {
                        StyleSize::Auto
                    },
                    if child_index == 1 {
                        fit_content(lp(18.0))
                    } else {
                        StyleSize::Auto
                    },
                ),
                min_size: Size::new(
                    if child_index == 2 {
                        px(20.0)
                    } else {
                        StyleSize::Auto
                    },
                    if child_index == 1 {
                        px(10.0)
                    } else {
                        StyleSize::Auto
                    },
                ),
                max_size: Size::new(
                    if child_index == 3 {
                        max_px(54.0)
                    } else {
                        MaxSize::None
                    },
                    if child_index == 2 {
                        max_px(32.0)
                    } else {
                        MaxSize::None
                    },
                ),
                align_self: baseline_self(child_index.is_multiple_of(2)),
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
                        fit_content(lp(126.0))
                    } else {
                        px(136.0)
                    },
                    if case_index.is_multiple_of(4) {
                        fit_content(lp(44.0))
                    } else {
                        px(58.0)
                    },
                ),
                min_size: Size::new(px(72.0), px(28.0)),
                max_size: Size::new(max_px(180.0), max_px(92.0)),
                flex_basis: basis_px(68.0),
                flex_wrap: flex_wrap::T::Wrap,
                align_items: ItemPlacement(AlignFlags::BASELINE),
                align_content: ContentDistribution::new(AlignFlags::FLEX_START),
                justify_content: ContentDistribution::new(AlignFlags::FLEX_START),
                gap: Size::new(gap_px(1.0), gap_px(1.0)),
                padding: uniform(pad(1.0)),
                border: uniform(bw((case_index % 2) as f32 * 0.5)),
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
                position: PositionProperty::Absolute,
                inset: edges(
                    Inset::LengthPercentage(lp((index % 64) as f32 * 5.0)),
                    Inset::Auto,
                    Inset::LengthPercentage(lp((index / 64) as f32 * 4.0)),
                    Inset::Auto,
                ),
                size: Size::new(px(4.0), px(3.0)),
                flex_basis: basis_px(4.0),
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
                    flex_basis: basis_px(2.0),
                    ..TestStyle::default()
                },
                width,
                2.0,
            ));
        }
        branches.push(tree.push_flex(
            TestStyle {
                flex_direction: flex_direction::T::Column,
                flex_basis: basis_px(8.0 + (branch_index % 4) as f32),
                gap: Size::new(gap_px(0.0), gap_px(0.5)),
                align_items: ItemPlacement(AlignFlags::FLEX_START),
                ..TestStyle::default()
            },
            leaves,
        ));
    }
    let root = tree.push_flex(
        TestStyle {
            align_items: ItemPlacement(AlignFlags::FLEX_START),
            justify_content: ContentDistribution::new(AlignFlags::FLEX_START),
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
                    flex_basis: basis_px(width),
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
                flex_basis: basis_px(58.0),
                direction: direction(case_index),
                flex_direction: flex_direction(case_index),
                justify_content: ContentDistribution::new(AlignFlags::FLEX_START),
                align_items: ItemPlacement(AlignFlags::FLEX_START),
                gap: Size::new(gap_px(1.0), gap_px(0.0)),
                padding: uniform(pad(1.0)),
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

fn spacing(index: usize) -> NonNegativeLengthPercentage {
    match index % 9 {
        0 => pad(2.0 + (index % 5) as f32),
        1 => NonNegative(pct_lp(0.04 + (index % 7) as f32 / 100.0)),
        2 => NonNegative(calc_lp(
            1.0 + (index % 3) as f32,
            0.03 + (index % 5) as f32 / 100.0,
        )),
        // auto/fr/intrinsic values are not valid for CSS padding/gap. Keep
        // their phase in the nine-value period while mapping unsupported raw
        // values to the host's initial zero.
        3..=8 => NonNegativeLengthPercentage::zero(),
        _ => unreachable!(),
    }
}

fn margin_spacing(index: usize) -> Margin {
    match index % 9 {
        0 => margin_len(2.0 + (index % 5) as f32),
        1 => Margin::LengthPercentage(pct_lp(0.04 + (index % 7) as f32 / 100.0)),
        2 => Margin::LengthPercentage(calc_lp(
            1.0 + (index % 3) as f32,
            0.03 + (index % 5) as f32 / 100.0,
        )),
        3 => Margin::Auto,
        // fr/intrinsic/fit-content are not valid CSS margin values.
        4..=8 => Margin::LengthPercentage(LengthPercentage::zero()),
        _ => unreachable!(),
    }
}

fn inset_spacing(index: usize) -> Inset {
    match index % 9 {
        0 => Inset::LengthPercentage(lp(2.0 + (index % 5) as f32)),
        1 => Inset::LengthPercentage(pct_lp(0.04 + (index % 7) as f32 / 100.0)),
        2 => Inset::LengthPercentage(calc_lp(
            1.0 + (index % 3) as f32,
            0.03 + (index % 5) as f32 / 100.0,
        )),
        3 => Inset::Auto,
        // fr/intrinsic/fit-content are not valid CSS inset values.
        4..=8 => Inset::LengthPercentage(LengthPercentage::zero()),
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
            spacing(case_index),
            spacing(case_index + 1),
            spacing(case_index + 2),
            spacing(case_index + 3),
        );
        let gap = Size::new(
            gap_of(spacing(case_index + 5)),
            gap_of(spacing(case_index + 4)),
        );
        let mut children = Vec::with_capacity(4);
        for child_index in 0..4 {
            let base = case_index + child_index * 3;
            let inset = edges(
                inset_spacing(base),
                Inset::Auto,
                inset_spacing(base + 1),
                Inset::Auto,
            );
            let margin = edges(
                margin_spacing(base + 2),
                margin_spacing(base + 3),
                margin_spacing(base + 4),
                margin_spacing(base + 5),
            );
            let child_padding = edges(
                spacing(base + 6),
                spacing(base + 7),
                spacing(base + 8),
                spacing(base + 9),
            );
            let width = 18.0 + child_index as f32 * 3.0;
            let height = 8.0 + child_index as f32 * 2.0;
            children.push(fixed_leaf(
                &mut tree,
                TestStyle {
                    position: PositionProperty::Relative,
                    inset,
                    size: Size::new(px(width), px(height)),
                    flex_basis: basis_px(width),
                    margin,
                    padding: child_padding,
                    border: edges(
                        bw(child_index as f32 * 0.5),
                        bw(0.5 + (child_index % 2) as f32),
                        bw((child_index % 3) as f32 * 0.25),
                        bw(1.0),
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
                flex_basis: basis_px(70.0),
                direction: direction(case_index),
                flex_direction: flex_direction(case_index),
                flex_wrap: flex_wrap::T::Wrap,
                justify_content: ContentDistribution::new(AlignFlags::FLEX_START),
                align_items: ItemPlacement(AlignFlags::FLEX_START),
                align_content: ContentDistribution::new(AlignFlags::FLEX_START),
                padding,
                border: edges(
                    bw(1.0 + (case_index % 2) as f32),
                    bw((case_index % 3) as f32 * 0.5),
                    bw(0.5 + (case_index % 2) as f32),
                    bw((case_index % 4) as f32 * 0.25),
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
            1 => pct(0.26 + (case_index % 7) as f32 / 100.0),
            _ => calc_size(
                8.0 + (case_index % 5) as f32,
                0.18 + (case_index % 4) as f32 / 100.0,
            ),
        };
        let max_width = max_calc(40.0 + (case_index % 9) as f32, 0.32);
        let max_height = max_calc(24.0 + (case_index % 6) as f32, 0.45);
        let content_width = 18.0 + (case_index % 9) as f32;
        let content_height = 8.0 + (case_index % 5) as f32;
        let content = fixed_leaf(
            &mut tree,
            TestStyle {
                size: Size::new(px(content_width), px(content_height)),
                flex_basis: basis_px(content_width),
                margin: margin_px(
                    (case_index % 2) as f32,
                    0.0,
                    (case_index % 3) as f32 * 0.5,
                    (case_index % 2) as f32,
                ),
                padding: uniform(pad((case_index % 3) as f32 * 0.5)),
                border: uniform(bw((case_index % 2) as f32)),
                ..TestStyle::default()
            },
            content_width,
            content_height,
        );
        containers.push(tree.push_flex(
            TestStyle {
                box_sizing: if case_index.is_multiple_of(2) {
                    box_sizing::T::ContentBox
                } else {
                    box_sizing::T::BorderBox
                },
                size: Size::new(
                    width,
                    if case_index.is_multiple_of(4) {
                        StyleSize::Auto
                    } else {
                        px(20.0 + (case_index % 9) as f32)
                    },
                ),
                min_size: Size::new(
                    px(24.0 + (case_index % 5) as f32),
                    px(12.0 + (case_index % 4) as f32),
                ),
                max_size: Size::new(max_width, max_height),
                aspect_ratio: if case_index.is_multiple_of(4) {
                    ratio(1.15 + (case_index % 5) as f32 * 0.12)
                } else {
                    AspectRatio::auto()
                },
                flex_direction: if case_index.is_multiple_of(2) {
                    flex_direction::T::Row
                } else {
                    flex_direction::T::Column
                },
                align_items: ItemPlacement(AlignFlags::CENTER),
                justify_content: ContentDistribution::new(AlignFlags::CENTER),
                margin: margin_px(
                    (case_index % 3) as f32,
                    (case_index % 4) as f32 * 0.5,
                    (case_index % 2) as f32,
                    0.0,
                ),
                padding: edges(
                    pad(1.0 + (case_index % 2) as f32),
                    pad(2.0 + (case_index % 3) as f32),
                    pad(1.0 + (case_index % 4) as f32 * 0.5),
                    pad(1.0),
                ),
                border: edges(
                    bw(1.0 + (case_index % 2) as f32),
                    bw(0.5 + (case_index % 3) as f32 * 0.5),
                    bw(1.0),
                    bw(0.5 + (case_index % 2) as f32),
                ),
                ..TestStyle::default()
            },
            vec![content],
        ));
    }
    let root = tree.push_flex(
        TestStyle {
            size: Size::new(px(360.0), StyleSize::Auto),
            flex_direction: flex_direction::T::Column,
            align_items: ItemPlacement(AlignFlags::FLEX_START),
            padding: uniform(pad(2.0)),
            border: uniform(bw(1.0)),
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
                flex_basis: basis_px(content_width),
                padding: uniform(pad((case_index % 2) as f32)),
                border: uniform(bw((case_index % 3) as f32 * 0.5)),
                ..TestStyle::default()
            },
            content_width,
            content_height,
        );
        containers.push(tree.push_flex(
            TestStyle {
                size: Size::new(
                    fit_content(calc_lp(
                        4.0 + (case_index % 3) as f32,
                        0.40 + (case_index % 5) as f32 * 0.03,
                    )),
                    fit_content(calc_lp(
                        2.0 + (case_index % 2) as f32,
                        0.25 + (case_index % 4) as f32 * 0.04,
                    )),
                ),
                flex_direction: if case_index.is_multiple_of(2) {
                    flex_direction::T::Row
                } else {
                    flex_direction::T::Column
                },
                align_items: ItemPlacement(AlignFlags::FLEX_START),
                justify_content: ContentDistribution::new(AlignFlags::FLEX_START),
                margin: margin_px(
                    (case_index % 2) as f32,
                    (case_index % 3) as f32,
                    (case_index % 4) as f32 * 0.5,
                    0.0,
                ),
                padding: uniform(pad((case_index % 3) as f32 * 0.5)),
                border: uniform(bw((case_index % 2) as f32)),
                ..TestStyle::default()
            },
            vec![content],
        ));
    }
    let root = tree.push_flex(
        TestStyle {
            size: Size::new(px(320.0), StyleSize::Auto),
            flex_direction: flex_direction::T::Column,
            align_items: ItemPlacement(AlignFlags::FLEX_START),
            padding: uniform(pad(2.0)),
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
                flex_basis: basis_px(first_width),
                ..TestStyle::default()
            },
            first_width,
            10.0,
        );
        let hidden = fixed_leaf(
            &mut tree,
            TestStyle {
                display: Display::None,
                size: Size::new(px(80.0), px(20.0)),
                flex_basis: basis_px(80.0),
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
                flex_basis: basis_px(second_width),
                ..TestStyle::default()
            },
            second_width,
            10.0,
        );
        rows.push(tree.push_flex(
            TestStyle {
                size: Size::new(px(320.0), px(12.0)),
                flex_basis: basis_px(12.0),
                align_items: ItemPlacement(AlignFlags::FLEX_START),
                ..TestStyle::default()
            },
            vec![first, hidden, second],
        ));
    }
    let height = count as f32 * 12.0;
    let root = column_wrapper(&mut tree, rows, 320.0, height);
    finish(tree, root, known_width(320.0))
}
