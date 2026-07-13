//! Rust workload builders for all fourteen PR #25 benchmarks whose
//! source tree contains `display: linear`.
//!
//! Mixed scenarios preserve every source display branch and the source `N`
//! loop. Starlight Block boxes are host-dispatched as vertical Linear boxes,
//! exactly as PR #25's Rust engine does. List virtualization metadata
//! (`linear-column-count`, list gaps, and `ListComponentType`) is retained by
//! this benchmark host but not added to neutron-star's generic Linear protocol.

#![allow(dead_code, clippy::cast_precision_loss, clippy::too_many_lines)]

use std::hint::black_box;

use neutron_star::compute::{LeafMeasureInput, LeafMetrics};
use neutron_star::prelude::*;
use neutron_star::style::{
    AlignContent, AlignItems, AlignSelf, BoxGenerationMode, BoxSizing, Dimension, Direction,
    FlexDirection, FlexWrap, GridAutoFlow, GridLine, GridPlacement, JustifyContent, JustifyItems,
    LengthPercentage, LengthPercentageAuto, LinearCrossGravity, LinearGravity, LinearLayoutGravity,
    LinearOrientation, Position, RelativeCenter, RelativeReference, TrackSizingFunction,
};

use crate::support::{TestStyle, TestTree, perform_layout};

const STICKY_AUTO_INSET: f32 = -1e10;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Lowering {
    /// Every source display branch and source index is retained.
    CompleteLogicalTopology,
    /// Host-only list fields are retained by this benchmark but intentionally
    /// elided from neutron-star's production protocol.
    HostListProtocolElided,
}

#[derive(Clone, Copy)]
pub(super) struct Scenario {
    pub(super) name: &'static str,
    pub(super) lowering: Lowering,
    pub(super) build: fn(usize) -> BenchCase,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct StickyInsetMetadata {
    pub(super) node: NodeId,
    pub(super) insets: Edges<LengthPercentageAuto>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct StickyPositionMetadata {
    pub(super) node: NodeId,
    pub(super) sticky_pos: Edges<f32>,
}

/// The generic source `Length` variants authored by PR #25 before each value
/// is lowered into a property-specific neutron-star type.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum SourceLength {
    Points(f32),
    Percent(f32),
    Calc { length: f32, percentage: f32 },
    Auto,
    Fr(f32),
    MaxContent,
    FitContentNone,
    FitContentPoints(f32),
    FitContentCalc { length: f32, percentage: f32 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SourceListComponentType {
    Header,
    Default,
    ListRow,
    Footer,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct LinearListMetadata {
    pub(super) node: NodeId,
    pub(super) column_count: Option<usize>,
    pub(super) main_axis_gap: Option<SourceLength>,
    pub(super) cross_axis_gap: Option<SourceLength>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ListItemMetadata {
    pub(super) node: NodeId,
    pub(super) component_type: Option<SourceListComponentType>,
}

#[derive(Debug)]
pub(super) struct BenchCase {
    pub(super) tree: TestTree,
    pub(super) root: NodeId,
    pub(super) known_dimensions: Size<Option<f32>>,
    pub(super) available_space: Size<AvailableSpace>,
    /// Authored Sticky insets retained by the benchmark's test host. They are
    /// intentionally absent from `TestStyle::inset`, because neutron-star's
    /// production protocol treats that field as a Relative visual offset.
    pub(super) sticky_insets: Vec<StickyInsetMetadata>,
    /// Source-equivalent exported Sticky values produced during the timed run.
    pub(super) sticky_positions: Vec<StickyPositionMetadata>,
    /// Host-only list construction fields. They are timed and black-boxed but
    /// intentionally never enter neutron-star's production style traits.
    pub(super) linear_list_metadata: Vec<LinearListMetadata>,
    pub(super) list_item_metadata: Vec<ListItemMetadata>,
}

impl BenchCase {
    fn new(tree: TestTree, root: NodeId, available_space: Size<AvailableSpace>) -> Self {
        Self {
            tree,
            root,
            // PR #25 calls `layout_with_owner_constraints`: owner constraints
            // are availability, not authoritative root border-box sizes.
            known_dimensions: Size::NONE,
            available_space,
            sticky_insets: Vec::new(),
            sticky_positions: Vec::new(),
            linear_list_metadata: Vec::new(),
            list_item_metadata: Vec::new(),
        }
    }

    fn with_sticky_insets(mut self, sticky_insets: Vec<StickyInsetMetadata>) -> Self {
        self.sticky_positions = Vec::with_capacity(sticky_insets.len());
        self.sticky_insets = sticky_insets;
        self
    }

    fn with_list_metadata(
        mut self,
        linear_list_metadata: Vec<LinearListMetadata>,
        list_item_metadata: Vec<ListItemMetadata>,
    ) -> Self {
        self.linear_list_metadata = linear_list_metadata;
        self.list_item_metadata = list_item_metadata;
        self
    }

    pub(super) fn node_count(&self) -> usize {
        self.tree.source.nodes.len()
    }

    pub(super) fn run(&mut self) -> LayoutOutput {
        let output = perform_layout(
            &mut self.tree,
            self.root,
            self.known_dimensions,
            self.available_space,
        );

        self.sticky_positions.clear();
        self.sticky_positions
            .extend(self.sticky_insets.iter().map(|metadata| {
                let resolve = |value, basis| match value {
                    LengthPercentageAuto::Length(value) => value,
                    LengthPercentageAuto::Percent(value) => value * basis,
                    LengthPercentageAuto::Calc(handle) => {
                        self.tree.source.resolve_calc(handle, basis)
                    }
                    LengthPercentageAuto::Auto => STICKY_AUTO_INSET,
                };
                StickyPositionMetadata {
                    node: metadata.node,
                    sticky_pos: Edges {
                        left: resolve(metadata.insets.left, 320.0),
                        right: resolve(metadata.insets.right, 320.0),
                        top: resolve(metadata.insets.top, 40.0),
                        bottom: resolve(metadata.insets.bottom, 40.0),
                    },
                }
            }));

        black_box((
            &self.sticky_positions,
            &self.linear_list_metadata,
            &self.list_item_metadata,
        ));
        output
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
        CompleteLogicalTopology,
        build_at_most_owner_matrix
    ),
    scenario!(
        "baseline_propagation_matrix",
        CompleteLogicalTopology,
        build_baseline_propagation_matrix
    ),
    scenario!(
        "measured_callback_matrix",
        CompleteLogicalTopology,
        build_measured_callback_matrix
    ),
    scenario!(
        "in_flow_order_matrix",
        CompleteLogicalTopology,
        build_in_flow_order_matrix
    ),
    scenario!(
        "full_value_spacing_matrix",
        HostListProtocolElided,
        build_full_value_spacing_matrix
    ),
    scenario!(
        "staggered_linear_list",
        HostListProtocolElided,
        build_staggered_linear_list
    ),
    scenario!(
        "staggered_linear_raw_list_gaps",
        HostListProtocolElided,
        build_staggered_linear_raw_list_gaps
    ),
    scenario!(
        "linear_gravity_matrix",
        CompleteLogicalTopology,
        build_linear_gravity_matrix
    ),
    scenario!(
        "linear_layout_gravity_matrix",
        CompleteLogicalTopology,
        build_linear_layout_gravity_matrix
    ),
    scenario!(
        "linear_cross_gravity_matrix",
        CompleteLogicalTopology,
        build_linear_cross_gravity_matrix
    ),
    scenario!(
        "box_sizing_matrix",
        CompleteLogicalTopology,
        build_box_sizing_matrix
    ),
    scenario!(
        "fit_content_subtrees",
        CompleteLogicalTopology,
        build_fit_content_subtrees
    ),
    scenario!(
        "sticky_percent_insets",
        CompleteLogicalTopology,
        build_sticky_percent_insets
    ),
    scenario!(
        "mixed_display_none",
        CompleteLogicalTopology,
        build_mixed_display_none
    ),
];

pub(super) fn scenario_named(name: &str) -> Scenario {
    *SCENARIOS
        .iter()
        .find(|scenario| scenario.name == name)
        .unwrap_or_else(|| panic!("unknown PR #25 Linear benchmark scenario {name}"))
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SourceDisplay {
    Block,
    Flex,
    Linear,
    Grid,
    Relative,
}

fn display_five(index: usize) -> SourceDisplay {
    match index % 5 {
        0 => SourceDisplay::Block,
        1 => SourceDisplay::Flex,
        2 => SourceDisplay::Linear,
        3 => SourceDisplay::Grid,
        _ => SourceDisplay::Relative,
    }
}

fn push_source_container(tree: &mut TestTree, display: SourceDisplay, style: TestStyle) -> NodeId {
    match display {
        // PR #25's Rust engine changes only the effective display value from
        // Block to Linear before dispatch.
        SourceDisplay::Block | SourceDisplay::Linear => tree.push_linear(style, Vec::new()),
        SourceDisplay::Flex => tree.push_flex(style, Vec::new()),
        SourceDisplay::Grid => tree.push_grid(style, Vec::new()),
        SourceDisplay::Relative => tree.push_relative(style, Vec::new()),
    }
}

fn append_child(tree: &mut TestTree, parent: NodeId, child: NodeId) {
    // Match PR #25's `SimpleTree::append_child`: parents are already present
    // in source storage and their initially-empty child vectors grow as each
    // child is appended.
    tree.source_node_mut(parent).children.push(child);
}

fn fixed_track(value: f32) -> TrackSizingFunction {
    TrackSizingFunction::fixed(lp(value))
}

fn wrap_space() -> Size<AvailableSpace> {
    Size::new(AvailableSpace::Definite(320.0), AvailableSpace::MaxContent)
}

fn fixed_space(width: f32, height: f32) -> Size<AvailableSpace> {
    Size::new(
        AvailableSpace::Definite(width),
        AvailableSpace::Definite(height),
    )
}

fn column_wrapper(tree: &mut TestTree, style: TestStyle) -> NodeId {
    // neutron-star intentionally has no Block algorithm. A vertical Linear
    // wrapper preserves the source Block root's child sequence and keeps this
    // benchmark entirely on generic Rust layout paths.
    tree.push_linear(
        TestStyle {
            linear_orientation: LinearOrientation::Vertical,
            ..style
        },
        Vec::new(),
    )
}

fn push_measured_block(
    tree: &mut TestTree,
    style: TestStyle,
    intrinsic_size: Size<f32>,
    first_baseline: Option<f32>,
) -> NodeId {
    tree.push_leaf(style, intrinsic_size, first_baseline)
}

fn push_empty_block(tree: &mut TestTree, style: TestStyle) -> NodeId {
    // A childless `SimpleNode::new(Display::Block)` is still a Block box in
    // the source benchmark. Its effective host dispatch is an empty vertical
    // Linear container, not a leaf measurement callback.
    tree.push_linear(
        TestStyle {
            linear_orientation: LinearOrientation::Vertical,
            ..style
        },
        Vec::new(),
    )
}

fn build_at_most_owner_matrix(nodes: usize) -> BenchCase {
    let count = nodes.max(1);
    let mut tree = TestTree::default();

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
    );

    for index in 0..count {
        let display = display_five(index);
        let width = if index.is_multiple_of(2) {
            Dimension::Percent(0.42 + (index % 5) as f32 / 100.0)
        } else {
            let calc = tree.push_calc(18.0 + (index % 4) as f32, 0.45);
            Dimension::FitContent(LengthPercentage::Calc(calc))
        };
        let height = if index.is_multiple_of(3) {
            Dimension::Auto
        } else {
            Dimension::FitContent(lp(34.0 + (index % 6) as f32))
        };
        let max_width = tree.push_calc(24.0, 0.60);
        let max_height = tree.push_calc(12.0, 0.70);
        let mut style = TestStyle {
            size: Size::new(width, height),
            min_size: Size::new(px(36.0), px(18.0)),
            max_size: Size::new(Dimension::Calc(max_width), Dimension::Calc(max_height)),
            margin: margin(1.0, 0.0, 1.0, 0.0),
            padding: padding(1.0, 2.0, 1.0, 2.0),
            ..TestStyle::default()
        };
        match display {
            SourceDisplay::Flex => {
                style.flex_wrap = FlexWrap::Wrap;
                style.justify_content = Some(JustifyContent::FlexStart);
                style.align_items = Some(AlignItems::FlexStart);
                style.align_content = Some(AlignContent::FlexStart);
                style.gap = Size::new(lp(1.0), lp(1.0));
            }
            SourceDisplay::Linear => {
                style.linear_orientation = if index.is_multiple_of(2) {
                    LinearOrientation::Horizontal
                } else {
                    LinearOrientation::Vertical
                };
                style.linear_cross_gravity = LinearCrossGravity::Start;
            }
            SourceDisplay::Grid => {
                style.template_columns = vec![fixed_track(30.0), TrackSizingFunction::AUTO];
                style.template_rows = vec![fixed_track(14.0), TrackSizingFunction::AUTO];
                style.auto_flow = if (index / 5).is_multiple_of(2) {
                    GridAutoFlow::Row
                } else {
                    GridAutoFlow::Column
                };
                style.justify_items = Some(JustifyItems::Start);
                style.align_items = Some(AlignItems::FlexStart);
                style.gap = Size::new(lp(1.0), lp(1.0));
            }
            SourceDisplay::Block | SourceDisplay::Relative => {}
        }
        let container = push_source_container(&mut tree, display, style);
        append_child(&mut tree, root, container);

        for child_index in 0_usize..3 {
            let width = match child_index {
                0 => Dimension::Auto,
                1 => {
                    let calc = tree.push_calc(6.0, 0.40);
                    Dimension::FitContent(LengthPercentage::Calc(calc))
                }
                _ => Dimension::Percent(0.35),
            };
            let height = match child_index {
                0 => Dimension::FitContent(lp(18.0)),
                1 => Dimension::Auto,
                _ => Dimension::Percent(0.30),
            };
            let max_width = tree.push_calc(18.0 + child_index as f32 * 3.0, 0.45);
            let max_height = tree.push_calc(10.0 + child_index as f32 * 2.0, 0.55);
            let intrinsic = Size::new(
                20.0 + (index % 7) as f32 + child_index as f32 * 4.0,
                10.0 + (index % 5) as f32 + child_index as f32 * 3.0,
            );
            let mut style = TestStyle {
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
                ..TestStyle::default()
            };
            if display == SourceDisplay::Relative {
                style.relative_center = match child_index {
                    0 => RelativeCenter::Horizontal,
                    1 => RelativeCenter::Vertical,
                    _ => RelativeCenter::Both,
                };
                style.relative_align.left = RelativeReference::new(0);
                style.relative_align.top = RelativeReference::new(0);
            }
            let child = push_measured_block(&mut tree, style, intrinsic, None);
            append_child(&mut tree, container, child);
        }
    }
    BenchCase::new(tree, root, fixed_space(320.0, 220.0))
}

fn build_baseline_propagation_matrix(nodes: usize) -> BenchCase {
    let count = nodes.max(1);
    let mut tree = TestTree::default();
    let root = column_wrapper(
        &mut tree,
        TestStyle {
            size: Size::new(px(320.0), px(count as f32 * 54.0 + 8.0)),
            ..TestStyle::default()
        },
    );

    for index in 0..count {
        let container = tree.push_flex(
            TestStyle {
                size: Size::new(px(116.0), px(48.0)),
                flex_direction: FlexDirection::Row,
                align_items: Some(if index.is_multiple_of(2) {
                    AlignItems::Baseline
                } else {
                    AlignItems::FlexStart
                }),
                justify_content: Some(JustifyContent::FlexStart),
                padding: padding(1.0, 2.0, 1.0, 2.0),
                margin: margin(1.0, 0.0, 1.0, 0.0),
                ..TestStyle::default()
            },
            Vec::new(),
        );
        append_child(&mut tree, root, container);

        let reference = push_measured_block(
            &mut tree,
            TestStyle {
                size: Size::new(px(12.0), px(32.0)),
                flex_basis: px(12.0),
                margin: margin(1.0, 1.0, 2.0, 1.0),
                ..TestStyle::default()
            },
            Size::new(12.0, 32.0),
            Some(26.0),
        );
        append_child(&mut tree, container, reference);
        let candidate = push_baseline_candidate(&mut tree, index);
        append_child(&mut tree, container, candidate);
        let trailing = push_measured_block(
            &mut tree,
            TestStyle {
                size: Size::new(px(10.0), px(12.0)),
                flex_basis: px(10.0),
                margin: margin(0.0, 1.0, 0.0, 0.0),
                ..TestStyle::default()
            },
            Size::new(10.0, 12.0),
            None,
        );
        append_child(&mut tree, container, trailing);
    }
    BenchCase::new(tree, root, wrap_space())
}

fn baseline_trigger(index: usize) -> Option<AlignSelf> {
    (!index.is_multiple_of(2)).then_some(AlignSelf::Baseline)
}

fn append_baseline_children(
    tree: &mut TestTree,
    parent: NodeId,
    first_baseline: f32,
    second_baseline: f32,
) {
    let first = push_measured_block(
        tree,
        TestStyle {
            size: Size::new(px(10.0), px(18.0)),
            margin: margin(1.0, 0.0, 1.0, 2.0),
            ..TestStyle::default()
        },
        Size::new(10.0, 18.0),
        Some(first_baseline),
    );
    append_child(tree, parent, first);
    let second = push_measured_block(
        tree,
        TestStyle {
            size: Size::new(px(12.0), px(24.0)),
            margin: margin(0.0, 1.0, 2.0, 1.0),
            ..TestStyle::default()
        },
        Size::new(12.0, 24.0),
        Some(second_baseline),
    );
    append_child(tree, parent, second);
}

#[allow(clippy::too_many_lines)]
fn push_baseline_candidate(tree: &mut TestTree, index: usize) -> NodeId {
    match index % 6 {
        0 => push_measured_block(
            tree,
            TestStyle {
                size: Size::new(px(18.0), px(22.0)),
                flex_basis: px(18.0),
                margin: margin(1.0, 2.0, 1.0, 2.0),
                align_self: baseline_trigger(index),
                ..TestStyle::default()
            },
            Size::new(18.0, 22.0),
            Some(16.0),
        ),
        1 => {
            let nested = tree.push_flex(
                TestStyle {
                    size: Size::new(px(28.0), px(26.0)),
                    flex_basis: px(28.0),
                    align_items: Some(AlignItems::Baseline),
                    margin: margin(1.0, 1.0, 2.0, 1.0),
                    align_self: baseline_trigger(index),
                    ..TestStyle::default()
                },
                Vec::new(),
            );
            append_baseline_children(tree, nested, 6.0, 18.0);
            nested
        }
        2 => {
            let nested = tree.push_flex(
                TestStyle {
                    size: Size::new(px(26.0), px(40.0)),
                    flex_basis: px(26.0),
                    flex_direction: FlexDirection::Column,
                    justify_content: Some(JustifyContent::Center),
                    align_items: Some(AlignItems::FlexStart),
                    margin: margin(2.0, 1.0, 2.0, 1.0),
                    align_self: baseline_trigger(index),
                    ..TestStyle::default()
                },
                Vec::new(),
            );
            append_baseline_children(tree, nested, 7.0, 15.0);
            nested
        }
        3 => {
            let nested = tree.push_linear(
                TestStyle {
                    size: Size::new(px(30.0), px(26.0)),
                    flex_basis: px(30.0),
                    linear_orientation: LinearOrientation::Horizontal,
                    margin: margin(2.0, 1.0, 1.0, 2.0),
                    align_self: baseline_trigger(index),
                    ..TestStyle::default()
                },
                Vec::new(),
            );
            append_baseline_children(tree, nested, 8.0, 19.0);
            nested
        }
        4 => {
            let nested = tree.push_grid(
                TestStyle {
                    size: Size::new(px(26.0), px(20.0)),
                    flex_basis: px(26.0),
                    template_columns: vec![fixed_track(26.0)],
                    template_rows: vec![fixed_track(20.0)],
                    align_items: Some(AlignItems::Baseline),
                    margin: margin(1.0, 2.0, 2.0, 1.0),
                    align_self: baseline_trigger(index),
                    ..TestStyle::default()
                },
                Vec::new(),
            );
            let child = push_measured_block(
                tree,
                TestStyle {
                    size: Size::new(px(11.0), px(9.0)),
                    grid_column: Line::new(
                        GridPlacement::Line(GridLine::new(1)),
                        GridPlacement::Auto,
                    ),
                    grid_row: Line::new(GridPlacement::Line(GridLine::new(1)), GridPlacement::Auto),
                    ..TestStyle::default()
                },
                Size::new(11.0, 9.0),
                Some(6.0),
            );
            append_child(tree, nested, child);
            nested
        }
        _ => {
            let nested = tree.push_relative(
                TestStyle {
                    size: Size::new(px(24.0), px(18.0)),
                    flex_basis: px(24.0),
                    margin: margin(2.0, 1.0, 2.0, 1.0),
                    align_self: baseline_trigger(index),
                    ..TestStyle::default()
                },
                Vec::new(),
            );
            let child = push_measured_block(
                tree,
                TestStyle {
                    relative_align: Edges {
                        left: RelativeReference::new(0),
                        right: RelativeReference::NONE,
                        top: RelativeReference::new(0),
                        bottom: RelativeReference::NONE,
                    },
                    ..TestStyle::default()
                },
                Size::new(12.0, 9.0),
                None,
            );
            append_child(tree, nested, child);
            nested
        }
    }
}

pub(super) fn callback_metrics(input: LeafMeasureInput) -> LeafMetrics {
    let width = match input.available_space.width {
        AvailableSpace::Definite(value) => (value - 3.0).max(1.0),
        // The source callback receives an indefinite SideConstraint for both
        // intrinsic request kinds and returns the same 24px fallback.
        AvailableSpace::MinContent | AvailableSpace::MaxContent => 24.0,
    };
    let height = match input.available_space.height {
        AvailableSpace::Definite(value) => (value - 2.0).max(1.0),
        AvailableSpace::MinContent | AvailableSpace::MaxContent => 12.0,
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
    let root = column_wrapper(
        &mut tree,
        TestStyle {
            size: Size::new(px(320.0), px(count as f32 * 70.0 + 8.0)),
            ..TestStyle::default()
        },
    );

    for index in 0..count {
        let display = display_five(index);
        let mut style = TestStyle {
            size: Size::new(
                if index.is_multiple_of(3) {
                    Dimension::FitContent(lp(126.0))
                } else {
                    px(136.0)
                },
                if index.is_multiple_of(4) {
                    Dimension::FitContent(lp(44.0))
                } else {
                    px(58.0)
                },
            ),
            min_size: Size::new(px(72.0), px(28.0)),
            max_size: Size::new(px(180.0), px(92.0)),
            margin: margin(1.0, 0.0, 1.0, 0.0),
            padding: Edges::uniform(lp(1.0)),
            border: Edges::uniform(lp((index % 2) as f32 * 0.5)),
            ..TestStyle::default()
        };
        match display {
            SourceDisplay::Flex => {
                style.flex_wrap = FlexWrap::Wrap;
                style.align_items = Some(AlignItems::Baseline);
                style.justify_content = Some(JustifyContent::FlexStart);
                style.align_content = Some(AlignContent::FlexStart);
                style.gap = Size::new(lp(1.0), lp(1.0));
            }
            SourceDisplay::Linear => {
                style.linear_orientation = if index.is_multiple_of(2) {
                    LinearOrientation::Horizontal
                } else {
                    LinearOrientation::Vertical
                };
                style.linear_cross_gravity = LinearCrossGravity::Start;
            }
            SourceDisplay::Grid => {
                style.template_columns = vec![fixed_track(32.0), TrackSizingFunction::AUTO];
                style.template_rows = vec![fixed_track(16.0), TrackSizingFunction::AUTO];
                style.justify_items = Some(JustifyItems::Start);
                style.align_items = Some(AlignItems::FlexStart);
                style.gap = Size::new(lp(1.0), lp(1.0));
            }
            SourceDisplay::Block | SourceDisplay::Relative => {}
        }
        let container = push_source_container(&mut tree, display, style);
        append_child(&mut tree, root, container);

        for child_index in 0_usize..4 {
            let intrinsic = Size::new(
                18.0 + (index % 7) as f32 + child_index as f32 * 3.0,
                9.0 + (index % 5) as f32 + child_index as f32 * 2.0,
            );
            let mut style = TestStyle {
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
                align_self: child_index.is_multiple_of(2).then_some(AlignSelf::Baseline),
                margin: margin(
                    (child_index % 2) as f32,
                    (child_index % 3) as f32 * 0.5,
                    (index % 2) as f32 * 0.5,
                    0.0,
                ),
                ..TestStyle::default()
            };
            if display == SourceDisplay::Relative {
                style.relative_center = match child_index % 4 {
                    0 => RelativeCenter::None,
                    1 => RelativeCenter::Horizontal,
                    2 => RelativeCenter::Vertical,
                    _ => RelativeCenter::Both,
                };
                style.relative_align.left = RelativeReference::new(0);
                style.relative_align.top = RelativeReference::new(0);
            }
            let child = match child_index {
                0 => tree.push_measured_leaf(style, callback_metrics),
                1 => tree.push_measured_leaf(style, callback_metrics_without_baseline),
                2 => push_measured_block(
                    &mut tree,
                    style,
                    intrinsic,
                    Some((4.0 + child_index as f32 * 2.0).min(intrinsic.height)),
                ),
                _ => push_measured_block(&mut tree, style, intrinsic, None),
            };
            append_child(&mut tree, container, child);
        }
    }
    BenchCase::new(tree, root, wrap_space())
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

fn build_in_flow_order_matrix(nodes: usize) -> BenchCase {
    let count = nodes.max(1);
    let mut tree = TestTree::default();
    let root = column_wrapper(
        &mut tree,
        TestStyle {
            size: Size::new(px(320.0), px(count as f32 * 60.0 + 8.0)),
            ..TestStyle::default()
        },
    );

    for index in 0..count {
        let display = match index % 4 {
            0 => SourceDisplay::Block,
            1 => SourceDisplay::Flex,
            2 => SourceDisplay::Linear,
            _ => SourceDisplay::Grid,
        };
        let mut style = TestStyle {
            size: Size::new(px(122.0), px(52.0)),
            direction: direction(index),
            padding: Edges::uniform(lp(1.0)),
            margin: margin(1.0, 0.0, 1.0, 0.0),
            ..TestStyle::default()
        };
        match display {
            SourceDisplay::Flex => {
                style.flex_direction = flex_direction(index);
                style.justify_content = Some(JustifyContent::FlexStart);
                style.align_items = Some(AlignItems::FlexStart);
                style.gap.width = lp(1.0);
            }
            SourceDisplay::Linear => {
                style.linear_orientation = if index.is_multiple_of(2) {
                    LinearOrientation::Horizontal
                } else {
                    LinearOrientation::Vertical
                };
                style.linear_gravity = LinearGravity::None;
                style.linear_layout_gravity = LinearLayoutGravity::None;
            }
            SourceDisplay::Grid => {
                style.template_columns = vec![fixed_track(24.0), fixed_track(28.0)];
                style.template_rows = vec![fixed_track(12.0), fixed_track(14.0)];
                style.auto_flow = if (index / 4).is_multiple_of(2) {
                    GridAutoFlow::Row
                } else {
                    GridAutoFlow::Column
                };
                style.gap = Size::new(lp(1.0), lp(1.0));
            }
            SourceDisplay::Block | SourceDisplay::Relative => {}
        }
        let container = push_source_container(&mut tree, display, style);
        append_child(&mut tree, root, container);

        for child_index in 0_usize..5 {
            let order_delta = i32::try_from(index % 3).expect("modulo result always fits i32") - 1;
            let order = [-2, 3, 0, 1, -1][child_index] + order_delta;
            let width = 14.0 + child_index as f32 * 2.0;
            let height = 8.0 + (child_index % 3) as f32;
            let child = push_empty_block(
                &mut tree,
                TestStyle {
                    size: Size::new(px(width), px(height)),
                    flex_basis: px(width),
                    order,
                    margin: margin(
                        (child_index % 2) as f32 * 0.5,
                        0.0,
                        0.0,
                        (child_index % 3) as f32 * 0.5,
                    ),
                    ..TestStyle::default()
                },
            );
            append_child(&mut tree, container, child);
        }
    }
    BenchCase::new(tree, root, wrap_space())
}

pub(super) fn source_spacing_length(index: usize) -> SourceLength {
    match index % 9 {
        0 => SourceLength::Points(2.0 + (index % 5) as f32),
        1 => SourceLength::Percent((4.0 + (index % 7) as f32) / 100.0),
        2 => SourceLength::Calc {
            length: 1.0 + (index % 3) as f32,
            percentage: (3.0 + (index % 5) as f32) / 100.0,
        },
        3 => SourceLength::Auto,
        4 => SourceLength::Fr(1.0 + (index % 3) as f32),
        5 => SourceLength::MaxContent,
        6 => SourceLength::FitContentNone,
        7 => SourceLength::FitContentPoints(3.0 + (index % 6) as f32),
        _ => SourceLength::FitContentCalc {
            length: 1.0 + (index % 4) as f32,
            percentage: (8.0 + (index % 5) as f32) / 100.0,
        },
    }
}

pub(super) fn spacing_lp(tree: &mut TestTree, index: usize) -> LengthPercentage {
    match source_spacing_length(index) {
        SourceLength::Points(value)
        | SourceLength::Fr(value)
        | SourceLength::FitContentPoints(value) => LengthPercentage::Length(value),
        SourceLength::Percent(value) => LengthPercentage::Percent(value),
        SourceLength::Calc { length, percentage }
        | SourceLength::FitContentCalc { length, percentage } => {
            LengthPercentage::Calc(tree.push_calc(length, percentage))
        }
        // `auto`, max-content, and an unbounded fit-content are not valid
        // computed padding values and resolve to the property's zero fallback.
        SourceLength::Auto | SourceLength::MaxContent | SourceLength::FitContentNone => {
            LengthPercentage::ZERO
        }
    }
}

pub(super) fn spacing_lpa(tree: &mut TestTree, index: usize) -> LengthPercentageAuto {
    match source_spacing_length(index) {
        SourceLength::Points(value)
        | SourceLength::Fr(value)
        | SourceLength::FitContentPoints(value) => LengthPercentageAuto::Length(value),
        SourceLength::Percent(value) => LengthPercentageAuto::Percent(value),
        SourceLength::Calc { length, percentage }
        | SourceLength::FitContentCalc { length, percentage } => {
            LengthPercentageAuto::Calc(tree.push_calc(length, percentage))
        }
        SourceLength::Auto => LengthPercentageAuto::Auto,
        SourceLength::MaxContent | SourceLength::FitContentNone => LengthPercentageAuto::ZERO,
    }
}

fn build_full_value_spacing_matrix(nodes: usize) -> BenchCase {
    let count = nodes.max(1);
    let mut tree = TestTree::default();
    let mut linear_list_metadata = Vec::with_capacity(count.div_ceil(4));
    let root = column_wrapper(
        &mut tree,
        TestStyle {
            size: Size::new(px(320.0), px(count as f32 * 72.0 + 8.0)),
            ..TestStyle::default()
        },
    );

    for index in 0..count {
        let display = match index % 4 {
            0 => SourceDisplay::Block,
            1 => SourceDisplay::Flex,
            2 => SourceDisplay::Linear,
            _ => SourceDisplay::Grid,
        };
        let padding_left = spacing_lp(&mut tree, index);
        let padding_right = spacing_lp(&mut tree, index + 1);
        let padding_top = spacing_lp(&mut tree, index + 2);
        let padding_bottom = spacing_lp(&mut tree, index + 3);
        let row_gap = spacing_lp(&mut tree, index + 4);
        let column_gap = spacing_lp(&mut tree, index + 5);
        let mut style = TestStyle {
            size: Size::new(px(128.0), px(64.0)),
            direction: direction(index),
            padding: Edges {
                left: padding_left,
                right: padding_right,
                top: padding_top,
                bottom: padding_bottom,
            },
            border: padding(
                1.0 + (index % 2) as f32,
                (index % 3) as f32 * 0.5,
                0.5 + (index % 2) as f32,
                (index % 4) as f32 * 0.25,
            ),
            gap: Size::new(column_gap, row_gap),
            margin: margin(1.0, 0.0, 1.0, 0.0),
            ..TestStyle::default()
        };
        match display {
            SourceDisplay::Flex => {
                style.flex_wrap = FlexWrap::Wrap;
                style.flex_direction = flex_direction(index);
                style.justify_content = Some(JustifyContent::FlexStart);
                style.align_items = Some(AlignItems::FlexStart);
                style.align_content = Some(AlignContent::FlexStart);
            }
            SourceDisplay::Linear => {
                style.linear_orientation = if index.is_multiple_of(2) {
                    LinearOrientation::Horizontal
                } else {
                    LinearOrientation::Vertical
                };
            }
            SourceDisplay::Grid => {
                style.template_columns = vec![fixed_track(28.0), fixed_track(34.0)];
                style.template_rows = vec![fixed_track(14.0), fixed_track(16.0)];
                style.auto_flow = if (index / 4).is_multiple_of(2) {
                    GridAutoFlow::Row
                } else {
                    GridAutoFlow::Column
                };
                style.justify_items = Some(JustifyItems::Start);
                style.align_items = Some(AlignItems::FlexStart);
            }
            SourceDisplay::Block | SourceDisplay::Relative => {}
        }
        let container = push_source_container(&mut tree, display, style);
        append_child(&mut tree, root, container);
        if display == SourceDisplay::Linear {
            linear_list_metadata.push(LinearListMetadata {
                node: container,
                column_count: Some(2 + index % 2),
                main_axis_gap: Some(source_spacing_length(index + 6)),
                cross_axis_gap: Some(source_spacing_length(index + 7)),
            });
        }

        for child_index in 0_usize..4 {
            let base = index + child_index * 3;
            let left = spacing_lpa(&mut tree, base);
            let top = spacing_lpa(&mut tree, base + 1);
            let margin_left = spacing_lpa(&mut tree, base + 2);
            let margin_right = spacing_lpa(&mut tree, base + 3);
            let margin_top = spacing_lpa(&mut tree, base + 4);
            let margin_bottom = spacing_lpa(&mut tree, base + 5);
            let padding_left = spacing_lp(&mut tree, base + 6);
            let padding_right = spacing_lp(&mut tree, base + 7);
            let padding_top = spacing_lp(&mut tree, base + 8);
            let padding_bottom = spacing_lp(&mut tree, base + 9);
            let width = 18.0 + child_index as f32 * 3.0;
            let height = 8.0 + child_index as f32 * 2.0;
            let child = push_empty_block(
                &mut tree,
                TestStyle {
                    position: Position::Relative,
                    inset: Edges {
                        left,
                        right: LengthPercentageAuto::Auto,
                        top,
                        bottom: LengthPercentageAuto::Auto,
                    },
                    size: Size::new(px(width), px(height)),
                    flex_basis: px(width),
                    margin: Edges {
                        left: margin_left,
                        right: margin_right,
                        top: margin_top,
                        bottom: margin_bottom,
                    },
                    padding: Edges {
                        left: padding_left,
                        right: padding_right,
                        top: padding_top,
                        bottom: padding_bottom,
                    },
                    border: padding(
                        child_index as f32 * 0.5,
                        0.5 + (child_index % 2) as f32,
                        (child_index % 3) as f32 * 0.25,
                        1.0,
                    ),
                    ..TestStyle::default()
                },
            );
            append_child(&mut tree, container, child);
        }
    }
    BenchCase::new(tree, root, wrap_space()).with_list_metadata(linear_list_metadata, Vec::new())
}

fn build_staggered_linear_list(nodes: usize) -> BenchCase {
    let count = nodes.max(1);
    let mut tree = TestTree::default();
    let mut list_item_metadata = Vec::with_capacity(count);
    let root = tree.push_linear(
        TestStyle {
            size: Size::new(px(320.0), Dimension::Auto),
            linear_orientation: LinearOrientation::Vertical,
            align_items: Some(AlignItems::Stretch),
            ..TestStyle::default()
        },
        Vec::new(),
    );
    for index in 0..count {
        let height = 4.0 + (index % 3) as f32;
        let child = tree.push_flex(
            TestStyle {
                size: Size::new(Dimension::Auto, px(height)),
                margin: margin((index % 2) as f32, (index % 3) as f32, 0.0, 0.0),
                ..TestStyle::default()
            },
            Vec::new(),
        );
        list_item_metadata.push(ListItemMetadata {
            node: child,
            component_type: match index % 31 {
                0 => Some(SourceListComponentType::Header),
                10 => Some(SourceListComponentType::Default),
                15 => Some(SourceListComponentType::ListRow),
                30 => Some(SourceListComponentType::Footer),
                _ => None,
            },
        });
        append_child(&mut tree, root, child);
    }
    BenchCase::new(tree, root, wrap_space()).with_list_metadata(
        vec![LinearListMetadata {
            node: root,
            column_count: Some(4),
            main_axis_gap: None,
            cross_axis_gap: Some(SourceLength::Points(2.0)),
        }],
        list_item_metadata,
    )
}

fn integer_sqrt_ceil(value: usize) -> usize {
    let mut root = 1_usize;
    while root.saturating_mul(root) < value {
        root += 1;
    }
    root
}

fn build_staggered_linear_raw_list_gaps(nodes: usize) -> BenchCase {
    let count = nodes.max(1);
    let mut tree = TestTree::default();
    let container_count = integer_sqrt_ceil(count).max(1);
    let children_per_container = count.div_ceil(container_count);
    let mut emitted = 0_usize;
    let mut linear_list_metadata = Vec::with_capacity(container_count);
    let mut list_item_metadata = Vec::with_capacity(count);
    let root = tree.push_linear(
        TestStyle {
            size: Size::new(px(320.0), Dimension::Auto),
            linear_orientation: LinearOrientation::Vertical,
            ..TestStyle::default()
        },
        Vec::new(),
    );

    for container_index in 0..container_count {
        let container = tree.push_linear(
            TestStyle {
                size: Size::new(
                    px(160.0 + (container_index % 3) as f32 * 8.0),
                    Dimension::Auto,
                ),
                linear_orientation: LinearOrientation::Vertical,
                align_items: Some(AlignItems::Stretch),
                ..TestStyle::default()
            },
            Vec::new(),
        );
        append_child(&mut tree, root, container);
        linear_list_metadata.push(LinearListMetadata {
            node: container,
            column_count: Some(2 + container_index % 3),
            main_axis_gap: None,
            cross_axis_gap: Some(match container_index % 4 {
                0 => SourceLength::Auto,
                1 => SourceLength::Fr(4.0),
                2 => SourceLength::MaxContent,
                _ => SourceLength::FitContentPoints(14.0),
            }),
        });

        for child_index in 0..children_per_container {
            if emitted >= count {
                break;
            }
            let height = 4.0 + (emitted % 5) as f32;
            let child = tree.push_flex(
                TestStyle {
                    size: Size::new(Dimension::Auto, px(height)),
                    margin: margin((emitted % 3) as f32, (child_index % 2) as f32, 0.0, 0.0),
                    ..TestStyle::default()
                },
                Vec::new(),
            );
            list_item_metadata.push(ListItemMetadata {
                node: child,
                component_type: match child_index % 17 {
                    0 => Some(SourceListComponentType::Header),
                    8 => Some(SourceListComponentType::ListRow),
                    16 => Some(SourceListComponentType::Footer),
                    _ => None,
                },
            });
            append_child(&mut tree, container, child);
            emitted += 1;
        }
    }
    BenchCase::new(tree, root, wrap_space())
        .with_list_metadata(linear_list_metadata, list_item_metadata)
}

const ORIENTATIONS: [LinearOrientation; 8] = [
    LinearOrientation::Horizontal,
    LinearOrientation::HorizontalReverse,
    LinearOrientation::Vertical,
    LinearOrientation::VerticalReverse,
    LinearOrientation::Row,
    LinearOrientation::RowReverse,
    LinearOrientation::Column,
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

const LAYOUT_GRAVITIES: [LinearLayoutGravity; 13] = [
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

const CROSS_GRAVITIES: [LinearCrossGravity; 5] = [
    LinearCrossGravity::None,
    LinearCrossGravity::Start,
    LinearCrossGravity::End,
    LinearCrossGravity::Center,
    LinearCrossGravity::Stretch,
];

fn append_gravity_children(
    tree: &mut TestTree,
    parent: NodeId,
    index: usize,
    middle_gravity: Option<LinearLayoutGravity>,
) {
    for child_index in 0..3 {
        let width = 12.0 + ((index + child_index) % 5) as f32;
        let height = 8.0 + ((index + child_index * 2) % 4) as f32;
        let child = push_empty_block(
            tree,
            TestStyle {
                size: Size::new(px(width), px(height)),
                linear_layout_gravity: if child_index == 1 {
                    middle_gravity.unwrap_or(LinearLayoutGravity::None)
                } else {
                    LinearLayoutGravity::None
                },
                margin: margin(
                    (child_index % 2) as f32,
                    ((index + child_index) % 3) as f32,
                    ((child_index + 1) % 2) as f32,
                    0.0,
                ),
                ..TestStyle::default()
            },
        );
        append_child(tree, parent, child);
    }
}

fn gravity_container_style(index: usize) -> TestStyle {
    let orientation = ORIENTATIONS[index % ORIENTATIONS.len()];
    let is_row = orientation.is_horizontal();
    TestStyle {
        size: Size::new(
            px(if is_row { 118.0 } else { 54.0 }),
            px(if is_row { 42.0 } else { 96.0 }),
        ),
        direction: direction(index),
        linear_orientation: orientation,
        padding: padding(2.0, 3.0, 4.0, 5.0),
        margin: margin(0.0, 0.0, 1.0, 0.0),
        ..TestStyle::default()
    }
}

fn gravity_root(tree: &mut TestTree) -> NodeId {
    tree.push_linear(
        TestStyle {
            size: Size::new(px(340.0), Dimension::Auto),
            linear_orientation: LinearOrientation::Vertical,
            ..TestStyle::default()
        },
        Vec::new(),
    )
}

fn build_linear_gravity_matrix(nodes: usize) -> BenchCase {
    let count = nodes.max(1);
    let mut tree = TestTree::default();
    let root = gravity_root(&mut tree);
    for index in 0..count {
        let container = tree.push_linear(
            TestStyle {
                linear_gravity: MAIN_GRAVITIES[index % MAIN_GRAVITIES.len()],
                justify_content: Some(JustifyContent::FlexEnd),
                align_items: Some(AlignItems::FlexStart),
                ..gravity_container_style(index)
            },
            Vec::new(),
        );
        append_child(&mut tree, root, container);
        append_gravity_children(&mut tree, container, index, None);
    }
    BenchCase::new(tree, root, wrap_space())
}

fn build_linear_layout_gravity_matrix(nodes: usize) -> BenchCase {
    let count = nodes.max(1);
    let mut tree = TestTree::default();
    let root = gravity_root(&mut tree);
    for index in 0..count {
        let container = tree.push_linear(
            TestStyle {
                align_items: Some(AlignItems::Stretch),
                ..gravity_container_style(index)
            },
            Vec::new(),
        );
        append_child(&mut tree, root, container);
        append_gravity_children(
            &mut tree,
            container,
            index,
            Some(LAYOUT_GRAVITIES[index % LAYOUT_GRAVITIES.len()]),
        );
    }
    BenchCase::new(tree, root, wrap_space())
}

fn build_linear_cross_gravity_matrix(nodes: usize) -> BenchCase {
    let count = nodes.max(1);
    let mut tree = TestTree::default();
    let root = gravity_root(&mut tree);
    for index in 0..count {
        let container = tree.push_linear(
            TestStyle {
                align_items: Some(AlignItems::FlexStart),
                linear_cross_gravity: CROSS_GRAVITIES[index % CROSS_GRAVITIES.len()],
                ..gravity_container_style(index)
            },
            Vec::new(),
        );
        append_child(&mut tree, root, container);
        append_gravity_children(&mut tree, container, index, None);
    }
    BenchCase::new(tree, root, wrap_space())
}

fn build_box_sizing_matrix(nodes: usize) -> BenchCase {
    let count = nodes.max(1);
    let mut tree = TestTree::default();
    let root = column_wrapper(
        &mut tree,
        TestStyle {
            size: Size::new(px(360.0), Dimension::Auto),
            padding: Edges::uniform(lp(2.0)),
            border: Edges::uniform(lp(1.0)),
            ..TestStyle::default()
        },
    );

    for index in 0..count {
        let display = match index % 5 {
            0 => SourceDisplay::Block,
            1 => SourceDisplay::Flex,
            2 => SourceDisplay::Linear,
            3 => SourceDisplay::Relative,
            _ => SourceDisplay::Grid,
        };
        let width = match index % 3 {
            0 => px(42.0 + (index % 11) as f32),
            1 => Dimension::Percent(0.26 + (index % 7) as f32 / 100.0),
            _ => Dimension::Calc(
                tree.push_calc(8.0 + (index % 5) as f32, 0.18 + (index % 4) as f32 / 100.0),
            ),
        };
        let max_width = tree.push_calc(40.0 + (index % 9) as f32, 0.32);
        let max_height = tree.push_calc(24.0 + (index % 6) as f32, 0.45);
        let mut style = TestStyle {
            box_sizing: if index.is_multiple_of(2) {
                BoxSizing::ContentBox
            } else {
                BoxSizing::BorderBox
            },
            size: Size::new(
                width,
                if index.is_multiple_of(4) {
                    Dimension::Auto
                } else {
                    px(20.0 + (index % 9) as f32)
                },
            ),
            min_size: Size::new(px(24.0 + (index % 5) as f32), px(12.0 + (index % 4) as f32)),
            max_size: Size::new(Dimension::Calc(max_width), Dimension::Calc(max_height)),
            aspect_ratio: index
                .is_multiple_of(4)
                .then_some(1.15 + (index % 5) as f32 * 0.12),
            margin: margin(
                (index % 3) as f32,
                (index % 4) as f32 * 0.5,
                (index % 2) as f32,
                0.0,
            ),
            padding: padding(
                1.0 + (index % 2) as f32,
                2.0 + (index % 3) as f32,
                1.0 + (index % 4) as f32 * 0.5,
                1.0,
            ),
            border: padding(
                1.0 + (index % 2) as f32,
                0.5 + (index % 3) as f32 * 0.5,
                1.0,
                0.5 + (index % 2) as f32,
            ),
            align_items: Some(AlignItems::Center),
            justify_content: Some(JustifyContent::Center),
            ..TestStyle::default()
        };
        match display {
            SourceDisplay::Flex => {
                style.flex_direction = if index.is_multiple_of(2) {
                    FlexDirection::Row
                } else {
                    FlexDirection::Column
                };
            }
            SourceDisplay::Linear => {
                style.linear_orientation = if index.is_multiple_of(2) {
                    LinearOrientation::Horizontal
                } else {
                    LinearOrientation::Vertical
                };
            }
            SourceDisplay::Grid => {
                style.template_columns = vec![fixed_track(20.0), TrackSizingFunction::AUTO];
                style.template_rows = vec![fixed_track(12.0), TrackSizingFunction::AUTO];
                style.gap = Size::new(lp(1.0), lp(1.0));
            }
            SourceDisplay::Block | SourceDisplay::Relative => {}
        }
        let container = push_source_container(&mut tree, display, style);
        append_child(&mut tree, root, container);

        let content_width = 18.0 + (index % 9) as f32;
        let content_height = 8.0 + (index % 5) as f32;
        let content = push_empty_block(
            &mut tree,
            TestStyle {
                size: Size::new(px(content_width), px(content_height)),
                margin: margin(
                    (index % 2) as f32,
                    0.0,
                    (index % 3) as f32 * 0.5,
                    (index % 2) as f32,
                ),
                padding: Edges::uniform(lp((index % 3) as f32 * 0.5)),
                border: Edges::uniform(lp((index % 2) as f32)),
                ..TestStyle::default()
            },
        );
        append_child(&mut tree, container, content);
    }
    BenchCase::new(
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
    let root = column_wrapper(
        &mut tree,
        TestStyle {
            size: Size::new(px(320.0), Dimension::Auto),
            padding: Edges::uniform(lp(2.0)),
            ..TestStyle::default()
        },
    );

    for index in 0..count {
        let display = match index % 5 {
            0 => SourceDisplay::Block,
            1 => SourceDisplay::Flex,
            2 => SourceDisplay::Linear,
            3 => SourceDisplay::Relative,
            _ => SourceDisplay::Grid,
        };
        let width = tree.push_calc(4.0 + (index % 3) as f32, 0.40 + (index % 5) as f32 * 0.03);
        let height = tree.push_calc(2.0 + (index % 2) as f32, 0.25 + (index % 4) as f32 * 0.04);
        let mut style = TestStyle {
            size: Size::new(
                Dimension::FitContent(LengthPercentage::Calc(width)),
                Dimension::FitContent(LengthPercentage::Calc(height)),
            ),
            margin: margin(
                (index % 2) as f32,
                (index % 3) as f32,
                (index % 4) as f32 * 0.5,
                0.0,
            ),
            padding: Edges::uniform(lp((index % 3) as f32 * 0.5)),
            border: Edges::uniform(lp((index % 2) as f32)),
            align_items: Some(AlignItems::FlexStart),
            justify_content: Some(JustifyContent::FlexStart),
            ..TestStyle::default()
        };
        match display {
            SourceDisplay::Flex => {
                style.flex_direction = if index.is_multiple_of(2) {
                    FlexDirection::Row
                } else {
                    FlexDirection::Column
                };
            }
            SourceDisplay::Linear => {
                style.linear_orientation = if index.is_multiple_of(2) {
                    LinearOrientation::Horizontal
                } else {
                    LinearOrientation::Vertical
                };
            }
            SourceDisplay::Grid => {
                style.template_columns = vec![fixed_track(24.0), TrackSizingFunction::AUTO];
                style.template_rows = vec![fixed_track(12.0), TrackSizingFunction::AUTO];
                style.gap = Size::new(lp(1.0), lp(1.0));
            }
            SourceDisplay::Block | SourceDisplay::Relative => {}
        }
        let container = push_source_container(&mut tree, display, style);
        append_child(&mut tree, root, container);

        let content_width = 20.0 + (index % 17) as f32;
        let content_height = 8.0 + (index % 7) as f32;
        let grid_line = Line::new(GridPlacement::Line(GridLine::new(1)), GridPlacement::Auto);
        let content = push_empty_block(
            &mut tree,
            TestStyle {
                size: Size::new(px(content_width), px(content_height)),
                padding: Edges::uniform(lp((index % 2) as f32)),
                border: Edges::uniform(lp((index % 3) as f32 * 0.5)),
                grid_column: if display == SourceDisplay::Grid {
                    grid_line
                } else {
                    Line::new(GridPlacement::Auto, GridPlacement::Auto)
                },
                grid_row: if display == SourceDisplay::Grid {
                    grid_line
                } else {
                    Line::new(GridPlacement::Auto, GridPlacement::Auto)
                },
                ..TestStyle::default()
            },
        );
        append_child(&mut tree, container, content);
    }
    BenchCase::new(tree, root, wrap_space())
}

fn build_sticky_percent_insets(nodes: usize) -> BenchCase {
    let count = nodes.max(1);
    let mut tree = TestTree::default();
    let mut sticky_insets = Vec::with_capacity(count);
    let root = column_wrapper(
        &mut tree,
        TestStyle {
            size: Size::new(px(320.0), px(count as f32 * 44.0 + 8.0)),
            ..TestStyle::default()
        },
    );

    for index in 0..count {
        let display = match index % 4 {
            0 => SourceDisplay::Flex,
            1 => SourceDisplay::Linear,
            2 => SourceDisplay::Grid,
            _ => SourceDisplay::Relative,
        };
        let mut style = TestStyle {
            size: Size::new(px(320.0), px(40.0)),
            ..TestStyle::default()
        };
        match display {
            SourceDisplay::Flex => style.align_items = Some(AlignItems::FlexStart),
            SourceDisplay::Linear => {
                style.linear_orientation = LinearOrientation::Horizontal;
                style.align_items = Some(AlignItems::FlexStart);
            }
            SourceDisplay::Grid => {
                style.template_columns = vec![fixed_track(320.0)];
                style.template_rows = vec![fixed_track(40.0)];
                style.align_items = Some(AlignItems::FlexStart);
            }
            SourceDisplay::Relative => {}
            SourceDisplay::Block => unreachable!("sticky source cycle has no Block branch"),
        }
        let container = push_source_container(&mut tree, display, style);
        append_child(&mut tree, root, container);

        let sticky_width = 20.0 + (index % 5) as f32;
        let sticky_height = 10.0 + (index % 3) as f32;
        let authored_insets = Edges {
            left: LengthPercentageAuto::Percent(0.10),
            right: if index.is_multiple_of(3) {
                LengthPercentageAuto::Percent(0.05)
            } else {
                LengthPercentageAuto::Auto
            },
            top: LengthPercentageAuto::Percent(0.25),
            bottom: if index.is_multiple_of(5) {
                LengthPercentageAuto::Percent(0.10)
            } else {
                LengthPercentageAuto::Auto
            },
        };
        let sticky = push_empty_block(
            &mut tree,
            TestStyle {
                // Sticky remains an in-flow layout item; its scroll-time
                // clamping is a host post-pass. Auto engine insets preserve
                // normal-flow geometry; `authored_insets` stays in host
                // metadata below.
                position: Position::Relative,
                size: Size::new(px(sticky_width), px(sticky_height)),
                ..TestStyle::default()
            },
        );
        sticky_insets.push(StickyInsetMetadata {
            node: sticky,
            insets: authored_insets,
        });
        append_child(&mut tree, container, sticky);
        let normal_width = 8.0 + (index % 7) as f32;
        let normal_height = 6.0 + (index % 5) as f32;
        let normal = push_empty_block(
            &mut tree,
            TestStyle {
                size: Size::new(px(normal_width), px(normal_height)),
                ..TestStyle::default()
            },
        );
        append_child(&mut tree, container, normal);
    }
    BenchCase::new(tree, root, fixed_space(320.0, 240.0)).with_sticky_insets(sticky_insets)
}

fn build_mixed_display_none(nodes: usize) -> BenchCase {
    let count = nodes.max(1);
    let mut tree = TestTree::default();
    let root = column_wrapper(
        &mut tree,
        TestStyle {
            size: Size::new(px(320.0), Dimension::Auto),
            ..TestStyle::default()
        },
    );

    for index in 0..count {
        let hidden = |tree: &mut TestTree, width: f32, height: f32| {
            push_empty_block(
                tree,
                TestStyle {
                    box_generation_mode: BoxGenerationMode::None,
                    size: Size::new(px(width), px(height)),
                    ..TestStyle::default()
                },
            )
        };
        match index % 4 {
            0 => {
                let container = tree.push_flex(
                    TestStyle {
                        size: Size::new(px(320.0), px(12.0)),
                        align_items: Some(AlignItems::FlexStart),
                        ..TestStyle::default()
                    },
                    Vec::new(),
                );
                append_child(&mut tree, root, container);
                let first_width = 10.0 + (index % 5) as f32;
                let second_width = 12.0 + (index % 3) as f32;
                let first = push_empty_block(
                    &mut tree,
                    TestStyle {
                        size: Size::new(px(first_width), px(10.0)),
                        ..TestStyle::default()
                    },
                );
                append_child(&mut tree, container, first);
                let hidden = hidden(&mut tree, 80.0, 20.0);
                append_child(&mut tree, container, hidden);
                let second = push_empty_block(
                    &mut tree,
                    TestStyle {
                        size: Size::new(px(second_width), px(10.0)),
                        ..TestStyle::default()
                    },
                );
                append_child(&mut tree, container, second);
            }
            1 => {
                let container = tree.push_linear(
                    TestStyle {
                        size: Size::new(px(320.0), Dimension::Auto),
                        ..TestStyle::default()
                    },
                    Vec::new(),
                );
                append_child(&mut tree, root, container);
                let first_height = 5.0 + (index % 3) as f32;
                let second_height = 6.0 + (index % 4) as f32;
                let first = push_empty_block(
                    &mut tree,
                    TestStyle {
                        size: Size::new(Dimension::Auto, px(first_height)),
                        ..TestStyle::default()
                    },
                );
                append_child(&mut tree, container, first);
                let hidden = hidden(&mut tree, 300.0, 50.0);
                append_child(&mut tree, container, hidden);
                let second = push_empty_block(
                    &mut tree,
                    TestStyle {
                        size: Size::new(Dimension::Auto, px(second_height)),
                        ..TestStyle::default()
                    },
                );
                append_child(&mut tree, container, second);
            }
            2 => {
                let container = tree.push_grid(
                    TestStyle {
                        size: Size::new(px(320.0), px(24.0)),
                        template_columns: vec![fixed_track(160.0), fixed_track(160.0)],
                        template_rows: vec![fixed_track(24.0)],
                        ..TestStyle::default()
                    },
                    Vec::new(),
                );
                append_child(&mut tree, root, container);
                let first = push_empty_block(&mut tree, TestStyle::default());
                append_child(&mut tree, container, first);
                let hidden = hidden(&mut tree, 160.0, 24.0);
                append_child(&mut tree, container, hidden);
                let second = push_empty_block(&mut tree, TestStyle::default());
                append_child(&mut tree, container, second);
            }
            _ => {
                let container = tree.push_relative(
                    TestStyle {
                        size: Size::new(px(320.0), px(24.0)),
                        ..TestStyle::default()
                    },
                    Vec::new(),
                );
                append_child(&mut tree, root, container);
                let relative_id = i32::try_from(index + 1).expect("benchmark id fits i32");
                let anchor_width = 20.0 + (index % 5) as f32;
                let anchor_height = 8.0 + (index % 3) as f32;
                let anchor = push_empty_block(
                    &mut tree,
                    TestStyle {
                        size: Size::new(px(anchor_width), px(anchor_height)),
                        relative_id: RelativeReference::new(relative_id),
                        ..TestStyle::default()
                    },
                );
                append_child(&mut tree, container, anchor);
                let follower_width = 5.0 + (index % 4) as f32;
                let follower_height = 4.0 + (index % 2) as f32;
                let follower = push_empty_block(
                    &mut tree,
                    TestStyle {
                        size: Size::new(px(follower_width), px(follower_height)),
                        relative_adjacent: Edges {
                            left: RelativeReference::NONE,
                            right: RelativeReference::new(relative_id),
                            top: RelativeReference::NONE,
                            bottom: RelativeReference::new(relative_id),
                        },
                        ..TestStyle::default()
                    },
                );
                append_child(&mut tree, container, follower);
                let hidden_style = TestStyle {
                    box_generation_mode: BoxGenerationMode::None,
                    size: Size::new(px(80.0), px(30.0)),
                    relative_id: RelativeReference::new(relative_id),
                    ..TestStyle::default()
                };
                let hidden = push_empty_block(&mut tree, hidden_style);
                append_child(&mut tree, container, hidden);
            }
        }
    }
    BenchCase::new(tree, root, wrap_space())
}
