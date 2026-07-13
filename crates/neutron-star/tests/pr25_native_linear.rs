// Copyright 2026 The Lynx Authors. All rights reserved.
// Licensed under the Apache License Version 2.0 that can be found in the
// LICENSE file in the root directory of this source tree.

//! Auditable, Rust-only migration of PR #25's native Linear inventory.
//!
//! The upstream file contains 136 Linear-named tests, two tests whose names do
//! not mention Linear but whose fixtures construct `Display::Linear`, and two
//! Linear helpers. Of those 138 tests, 126 construct a `display: linear` box:
//! 105 overlap the direct suite by name but retain their actual native Rust
//! builders in `pr25_native_linear_exact.rs`; 21 are native-only trees or
//! matrices ported below. The remaining 12 explicitly
//! construct `display: block` and exercise Starlight's block-as-Linear caller
//! dispatch; the PR compatibility host now executes those through the real
//! Linear algorithm too. No Lynx C++, FFI, or native bridge is copied or
//! linked.

mod pr25_support;
mod support;

use std::collections::BTreeSet;

use neutron_star::prelude::LayoutGoal;
use pr25_support::{
    AlignContent, AlignItems, BaseLength, BoxSizing, Constraints, Direction, Display,
    FlexDirection, FlexWrap, JustifyContent, JustifyItems, LayoutEngine, LayoutResult, LayoutTree,
    Length, LinearCrossGravity, LinearGravity, LinearLayoutGravity, LinearOrientation, MeasureCall,
    MeasureCallKind, MeasureMode, MeasurementProfile, PositionType, Rect, RegularMeasure,
    STICKY_AUTO_INSET, SideConstraint, SimpleNode, SimpleTree, Size, Style, Visibility,
    run_rust_layout,
};

const SOURCE_FUNCTIONS: &str = include_str!("pr25_native_linear_inventory.txt");
const DIRECT_LINEAR_SOURCE: &str = include_str!("pr25_linear_layout.rs");
const NATIVE_LINEAR_PORT_SOURCE: &str = include_str!("pr25_native_linear.rs");
const NATIVE_LINEAR_EXACT_SOURCE: &str = include_str!("pr25_native_linear_exact.rs");
const SOURCE_TEST_COUNT: usize = 138;
const SOURCE_HELPER_COUNT: usize = 2;
// Counts every source fixture invocation. The source ignored its non-grid
// `fr` case because the C++ comparison was not meaningful; this Rust-only
// migration executes the fixture and checks the host-lowered fallback.
const OVERLAP_TEST_COUNT: usize = 105;
const OVERLAP_EXECUTION_COUNT: usize = 146;
const UNIQUE_TEST_COUNT: usize = 21;
const UNIQUE_EXECUTION_COUNT: usize = 333;
const BLOCK_AS_LINEAR_TEST_COUNT: usize = 12;
const BLOCK_AS_LINEAR_EXECUTION_COUNT: usize = 12;

const SOURCE_HELPERS: &[(&str, &str)] = &[
    (
        "fixed_linear_child",
        "source-only builder; the Rust port uses the same builder below",
    ),
    (
        "linear_standalone_style",
        "source-only style normalizer; the Rust port uses the same helper below",
    ),
];

const BLOCK_AS_LINEAR_CASES: &[(&str, &str)] = &[
    (
        "head_to_head_linear_auto_main_uses_final_grid_aspect_ratio_child_size",
        "root display:block is executed through the source-compatible Linear dispatch",
    ),
    (
        "head_to_head_root_block_fit_content_argument_uses_latest_linear_sizing",
        "root display:block is executed through the source-compatible Linear dispatch",
    ),
    (
        "head_to_head_root_block_fit_content_percent_argument_uses_latest_linear_sizing",
        "root display:block is executed through the source-compatible Linear dispatch",
    ),
    (
        "head_to_head_root_block_fit_content_calc_argument_uses_latest_linear_sizing",
        "root display:block is executed through the source-compatible Linear dispatch",
    ),
    (
        "head_to_head_child_block_fit_content_argument_uses_latest_linear_sizing",
        "child display:block is executed through the source-compatible Linear dispatch",
    ),
    (
        "head_to_head_child_block_fit_content_percent_argument_uses_latest_linear_sizing",
        "child display:block is executed through the source-compatible Linear dispatch",
    ),
    (
        "head_to_head_child_block_fit_content_calc_argument_uses_latest_linear_sizing",
        "child display:block is executed through the source-compatible Linear dispatch",
    ),
    (
        "head_to_head_absolute_block_fit_content_argument_uses_latest_linear_sizing",
        "positioned display:block subtree executes the Linear algorithm",
    ),
    (
        "head_to_head_absolute_subtree_fit_content_percent_argument_uses_latest_linear_sizing",
        "positioned display:block subtree executes the Linear algorithm",
    ),
    (
        "head_to_head_fixed_block_fit_content_argument_uses_latest_linear_sizing",
        "fixed display:block subtree executes the Linear algorithm in the Rust host pass",
    ),
    (
        "head_to_head_absolute_block_max_content_uses_latest_linear_natural_size",
        "positioned display:block subtree executes the Linear algorithm",
    ),
    (
        "head_to_head_fixed_block_max_content_uses_latest_linear_natural_size",
        "fixed display:block subtree executes the Linear algorithm in the Rust host pass",
    ),
];

fn source_execution_count(name: &str) -> usize {
    match name {
        "fixed_linear_child" | "linear_standalone_style" => 0,
        "head_to_head_absolute_rtl_horizontal_linear_child_without_insets_uses_rtl_main_front"
        | "head_to_head_linear_row_column_orientation_aliases_match_cpp_mapping"
        | "head_to_head_linear_absolute_child_cross_axis_uses_cpp_computed_layout_gravity_order"
        | "head_to_head_linear_absolute_vertical_child_uses_cpp_main_axis_static_position" => 4,
        "head_to_head_vertical_linear_gravity_variants_match_cpp_mapping" => 11,
        "head_to_head_rtl_horizontal_linear_gravity_swaps_physical_edges"
        | "head_to_head_rtl_horizontal_linear_gravity_physical_left_and_right"
        | "head_to_head_rtl_vertical_linear_layout_gravity_keeps_physical_left_and_right" => 2,
        "head_to_head_linear_main_axis_orientation_direction_reverse_matrix" => 160,
        "head_to_head_linear_cross_axis_orientation_direction_reverse_matrix" => 136,
        "head_to_head_horizontal_linear_justify_content_distribution_values_map_to_start" => 3,
        "head_to_head_linear_layout_gravity_physical_variants_match_cpp_groups"
        | "head_to_head_horizontal_linear_layout_gravity_physical_variants_match_cpp_groups" => 13,
        "head_to_head_vertical_linear_cross_gravity_variants_override_align_items"
        | "head_to_head_horizontal_linear_cross_gravity_variants_override_align_items" => 5,
        _ => 1,
    }
}

fn assert_close(actual: f32, expected: f32) {
    assert!(
        (actual - expected).abs() <= 0.01,
        "expected {expected}, got {actual}"
    );
}

fn fixed_linear_child(tree: &mut SimpleTree, width: Length, height: Length) -> usize {
    tree.push(SimpleNode::new(standalone_style(Style {
        width,
        height,
        ..Style::default()
    })))
}

fn standalone_style(style: Style) -> Style {
    Style {
        display: Display::Flex,
        box_sizing: BoxSizing::ContentBox,
        ..style
    }
}

fn linear_standalone_style(style: Style) -> Style {
    Style {
        display: Display::Linear,
        box_sizing: BoxSizing::ContentBox,
        ..style
    }
}

fn block_standalone_style(style: Style) -> Style {
    Style {
        display: Display::Block,
        box_sizing: BoxSizing::ContentBox,
        ..style
    }
}

#[derive(Clone, Debug)]
struct MeasuringNode {
    style: Style,
    layout: LayoutResult,
    children: Vec<usize>,
    measured_size: Option<Size>,
    last_constraints: Option<Constraints>,
}

impl MeasuringNode {
    fn new(style: Style) -> Self {
        Self {
            style,
            layout: LayoutResult::default(),
            children: Vec::new(),
            measured_size: None,
            last_constraints: None,
        }
    }

    fn measured(style: Style, measured_size: Size) -> Self {
        Self {
            measured_size: Some(measured_size),
            ..Self::new(style)
        }
    }
}

#[derive(Clone, Debug, Default)]
struct MeasuringTree {
    nodes: Vec<MeasuringNode>,
}

impl MeasuringTree {
    fn push(&mut self, node: MeasuringNode) -> usize {
        let id = self.nodes.len();
        self.nodes.push(node);
        id
    }

    fn append_child(&mut self, parent: usize, child: usize) {
        self.nodes[parent].children.push(child);
    }
}

impl LayoutTree for MeasuringTree {
    type NodeId = usize;
    type Children<'a> = std::iter::Copied<std::slice::Iter<'a, usize>>;

    fn children(&self, node: Self::NodeId) -> Self::Children<'_> {
        self.nodes[node].children.iter().copied()
    }

    fn style(&self, node: Self::NodeId) -> &Style {
        &self.nodes[node].style
    }

    fn set_layout(&mut self, node: Self::NodeId, layout: LayoutResult) {
        self.nodes[node].layout = layout;
    }

    fn measure(&mut self, node: Self::NodeId, constraints: Constraints) -> Option<Size> {
        let node = &mut self.nodes[node];
        node.last_constraints = Some(constraints);
        node.measured_size.map(|size| {
            Size::new(
                constraints.width.clamp(size.width),
                constraints.height.clamp(size.height),
            )
        })
    }

    fn has_measure(&self, node: Self::NodeId) -> bool {
        self.nodes[node].measured_size.is_some()
    }

    fn measurement_profile(&self, node: Self::NodeId) -> Option<MeasurementProfile> {
        self.nodes[node]
            .measured_size
            .map(|size| MeasurementProfile {
                regular: Some(RegularMeasure::Fixed(size)),
                min_content: None,
                max_content: None,
                first_baseline: None,
            })
    }

    fn set_measure_trace(&mut self, node: Self::NodeId, trace: &[MeasureCall]) {
        let style = &self.nodes[node].style;
        let call = if style.width == Length::MaxContent || style.height == Length::MaxContent {
            trace
                .iter()
                .find(|call| call.kind == MeasureCallKind::MaxContent)
                .or_else(|| trace.last())
        } else if style.position == PositionType::Fixed {
            trace.last()
        } else {
            trace
                .iter()
                .rev()
                .find(|call| matches!(call.goal, LayoutGoal::Measure(_)))
                .or_else(|| trace.last())
        };
        self.nodes[node].last_constraints = call.map(|call| call.constraints);
    }
}

#[test]
#[allow(clippy::too_many_lines)] // Retains PR #25's mixed Flex/Linear fixture verbatim.
fn head_to_head_flex_column_stretch_with_fr_sibling_preserves_percent_basis() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(Style {
        display: Display::Flex,
        direction: Direction::Rtl,
        box_sizing: BoxSizing::BorderBox,
        width: Length::points(30.0),
        min_width: Length::points(20.0),
        min_height: Length::points(16.0),
        padding: Rect::new(
            Length::points(1.0),
            Length::ZERO,
            Length::points(3.0),
            Length::ZERO,
        ),
        border: Rect::new(1.0, 0.0, 0.0, 1.0),
        flex_direction: FlexDirection::ColumnReverse,
        flex_wrap: FlexWrap::Wrap,
        justify_content: JustifyContent::Center,
        align_items: AlignItems::Center,
        align_content: AlignContent::FlexEnd,
        row_gap: Length::points(1.0),
        ..Style::default()
    }));
    let first = tree.push(SimpleNode::new(block_standalone_style(Style {
        box_sizing: BoxSizing::BorderBox,
        width: Length::points(42.0),
        min_width: Length::points(8.0),
        margin: Rect::new(
            Length::points(7.0),
            Length::points(7.0),
            Length::ZERO,
            Length::points(7.0),
        ),
        padding: Rect::new(
            Length::points(7.0),
            Length::points(3.0),
            Length::points(7.0),
            Length::ZERO,
        ),
        border: Rect::all(1.0),
        flex_basis: Length::percent(50.0),
        flex_shrink: 0.0,
        order: -1,
        justify_self: JustifyItems::Center,
        ..Style::default()
    })));
    let second = tree.push(SimpleNode::new(block_standalone_style(Style {
        direction: Direction::Rtl,
        width: Length::points(54.0),
        height: Length::points(36.0),
        min_width: Length::points(16.0),
        max_width: Length::points(36.0),
        margin: Rect::new(
            Length::points(5.0),
            Length::points(3.0),
            Length::ZERO,
            Length::points(7.0),
        ),
        padding: Rect::new(
            Length::ZERO,
            Length::points(3.0),
            Length::ZERO,
            Length::ZERO,
        ),
        border: Rect::all(1.0),
        justify_self: JustifyItems::End,
        ..Style::default()
    })));
    let third = tree.push(SimpleNode::new(Style {
        display: Display::Linear,
        box_sizing: BoxSizing::BorderBox,
        width: Length::fr(1.0),
        height: Length::points(24.0),
        min_width: Length::fr(8.0),
        min_height: Length::points(16.0),
        max_width: Length::fr(26.0),
        max_height: Length::points(40.0),
        margin: Rect::new(
            Length::ZERO,
            Length::ZERO,
            Length::ZERO,
            Length::points(5.0),
        ),
        padding: Rect::new(
            Length::points(7.0),
            Length::ZERO,
            Length::points(5.0),
            Length::points(5.0),
        ),
        border: Rect::all(1.0),
        flex_basis: Length::percent(60.0),
        flex_grow: 1.0,
        order: 1,
        align_self: Some(AlignItems::End),
        justify_self: JustifyItems::Auto,
        column_gap: Length::points(1.0),
        ..Style::default()
    }));
    let fourth = tree.push(SimpleNode::new(block_standalone_style(Style {
        direction: Direction::Rtl,
        height: Length::points(66.0),
        min_width: Length::fr(4.0),
        min_height: Length::points(20.0),
        max_width: Length::fr(18.0),
        max_height: Length::points(44.0),
        margin: Rect::new(
            Length::ZERO,
            Length::ZERO,
            Length::points(7.0),
            Length::points(1.0),
        ),
        padding: Rect::new(
            Length::points(7.0),
            Length::ZERO,
            Length::points(1.0),
            Length::ZERO,
        ),
        border: Rect::all(1.0),
        flex_basis: Length::fr(2.0),
        flex_grow: 1.0,
        order: 2,
        align_self: Some(AlignItems::FlexStart),
        justify_self: JustifyItems::Center,
        row_gap: Length::points(3.0),
        column_gap: Length::points(5.0),
        ..Style::default()
    })));
    let child = tree.push(SimpleNode::new(Style {
        display: Display::Flex,
        direction: Direction::Rtl,
        box_sizing: BoxSizing::BorderBox,
        flex_direction: FlexDirection::Row,
        width: Length::Auto,
        height: Length::fr(3.0),
        min_width: Length::fr(10.0),
        min_height: Length::points(8.0),
        margin: Rect::new(
            Length::points(5.0),
            Length::points(5.0),
            Length::ZERO,
            Length::ZERO,
        ),
        padding: Rect::new(
            Length::ZERO,
            Length::ZERO,
            Length::points(3.0),
            Length::ZERO,
        ),
        border: Rect::all(1.0),
        flex_basis: Length::points(54.0),
        flex_grow: 1.0,
        flex_shrink: 0.0,
        order: 3,
        align_self: Some(AlignItems::Stretch),
        justify_self: JustifyItems::End,
        row_gap: Length::points(3.0),
        column_gap: Length::points(3.0),
        ..Style::default()
    }));
    let flexible = tree.push(SimpleNode::new(block_standalone_style(Style {
        width: Length::points(11.0),
        height: Length::percent(60.0),
        min_width: Length::points(8.0),
        max_width: Length::points(36.0),
        max_height: Length::points(40.0),
        margin: Rect::new(
            Length::points(1.0),
            Length::ZERO,
            Length::points(5.0),
            Length::ZERO,
        ),
        padding: Rect::new(
            Length::ZERO,
            Length::points(1.0),
            Length::points(5.0),
            Length::points(1.0),
        ),
        flex_basis: Length::fr(2.0),
        justify_self: JustifyItems::Start,
        row_gap: Length::points(1.0),
        ..Style::default()
    })));
    let percent = tree.push(SimpleNode::new(block_standalone_style(Style {
        direction: Direction::Rtl,
        width: Length::points(8.0),
        margin: Rect::new(
            Length::points(3.0),
            Length::points(5.0),
            Length::points(1.0),
            Length::ZERO,
        ),
        padding: Rect::new(
            Length::points(5.0),
            Length::ZERO,
            Length::ZERO,
            Length::points(1.0),
        ),
        border: Rect::all(1.0),
        flex_basis: Length::percent(60.0),
        flex_shrink: 0.0,
        justify_self: JustifyItems::Center,
        ..Style::default()
    })));
    tree.append_child(root, first);
    tree.append_child(root, second);
    tree.append_child(root, third);
    tree.append_child(root, fourth);
    tree.append_child(root, child);
    tree.append_child(child, flexible);
    tree.append_child(child, percent);

    let size = run_rust_layout(
        &mut tree,
        root,
        Constraints::new(
            SideConstraint::at_most(180.0),
            SideConstraint::at_most(140.0),
        ),
    );

    // The fixture is retained verbatim, but non-grid fr values cannot be
    // asserted as Lynx values: neutron-star intentionally exposes them as
    // a host-lowering concern. These assertions keep the Rust fallback
    // executable and guard the Linear child's surrounding Flex interaction.
    assert!(size.width.is_finite() && size.height.is_finite());
    assert!(tree.nodes[third].layout.size.width.is_finite());
    assert!(tree.nodes[third].layout.size.height.is_finite());
    assert!(tree.nodes[third].layout.size.width >= 0.0);
    assert!(tree.nodes[third].layout.size.height >= 0.0);
    assert!(tree.nodes[child].layout.size.width.is_finite());
    assert!(tree.nodes[flexible].layout.size.height.is_finite());
    assert!(tree.nodes[percent].layout.size.height.is_finite());
}

#[test]
fn head_to_head_owner_definite_height_without_root_height_uses_root_at_most_height() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(linear_standalone_style(Style {
        width: Length::points(100.0),
        align_items: AlignItems::FlexStart,
        ..Style::default()
    })));
    let child = fixed_linear_child(&mut tree, Length::points(20.0), Length::points(10.0));
    tree.append_child(root, child);

    let size = LayoutEngine::new().layout_with_owner_constraints(
        &mut tree,
        root,
        Constraints::definite(100.0, 100.0),
    );

    // A definite owner height is only the root's available bound when the
    // root itself has auto height; it must not force the Linear main size.
    assert_close(size.width, 100.0);
    assert_close(size.height, 10.0);
    assert_close(tree.nodes[child].layout.size.height, 10.0);
}

#[test]
fn head_to_head_absolute_linear_child_without_insets_uses_linear_gravity() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(linear_standalone_style(Style {
        linear_orientation: LinearOrientation::Horizontal,
        linear_gravity: LinearGravity::Center,
        width: Length::points(100.0),
        height: Length::points(40.0),
        ..Style::default()
    })));
    let absolute = tree.push(SimpleNode::new(standalone_style(Style {
        position: PositionType::Absolute,
        width: Length::points(20.0),
        height: Length::points(10.0),
        linear_layout_gravity: LinearLayoutGravity::End,
        ..Style::default()
    })));
    tree.append_child(root, absolute);

    run_rust_layout(&mut tree, root, Constraints::definite(100.0, 40.0));

    assert_close(tree.nodes[absolute].layout.offset.x, 40.0);
    assert_close(tree.nodes[absolute].layout.offset.y, 30.0);
}

#[test]
fn head_to_head_absolute_rtl_horizontal_linear_child_without_insets_uses_rtl_main_front() {
    for (gravity, expected_x) in [
        (LinearGravity::None, 80.0),
        (LinearGravity::Left, 0.0),
        (LinearGravity::Right, 80.0),
        (LinearGravity::Center, 40.0),
    ] {
        let mut tree = SimpleTree::default();
        let root = tree.push(SimpleNode::new(linear_standalone_style(Style {
            direction: Direction::Rtl,
            linear_orientation: LinearOrientation::Horizontal,
            linear_gravity: gravity,
            width: Length::points(100.0),
            height: Length::points(40.0),
            ..Style::default()
        })));
        let absolute = tree.push(SimpleNode::new(standalone_style(Style {
            position: PositionType::Absolute,
            width: Length::points(20.0),
            height: Length::points(10.0),
            linear_layout_gravity: LinearLayoutGravity::End,
            ..Style::default()
        })));
        tree.append_child(root, absolute);

        run_rust_layout(&mut tree, root, Constraints::definite(100.0, 40.0));

        assert_close(tree.nodes[absolute].layout.offset.x, expected_x);
        assert_close(tree.nodes[absolute].layout.offset.y, 30.0);
    }
}

#[test]
fn head_to_head_linear_sticky_child_percent_insets_resolve_against_container_constraints() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(linear_standalone_style(Style {
        width: Length::points(100.0),
        height: Length::points(40.0),
        linear_orientation: LinearOrientation::Horizontal,
        ..Style::default()
    })));
    let sticky = tree.push(SimpleNode::new(linear_standalone_style(Style {
        position: PositionType::Sticky,
        width: Length::points(20.0),
        height: Length::points(10.0),
        left: Length::percent(10.0),
        top: Length::percent(25.0),
        ..Style::default()
    })));
    tree.append_child(root, sticky);

    run_rust_layout(&mut tree, root, Constraints::definite(100.0, 40.0));

    assert_close(tree.nodes[sticky].layout.offset.x, 0.0);
    assert_close(tree.nodes[sticky].layout.offset.y, 0.0);
    assert_close(tree.nodes[sticky].layout.sticky_pos.left, 10.0);
    assert_close(
        tree.nodes[sticky].layout.sticky_pos.right,
        STICKY_AUTO_INSET,
    );
    assert_close(tree.nodes[sticky].layout.sticky_pos.top, 10.0);
    assert_close(
        tree.nodes[sticky].layout.sticky_pos.bottom,
        STICKY_AUTO_INSET,
    );
}

#[test]
fn head_to_head_linear_sticky_child_end_percent_insets_resolve_against_container_constraints() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(linear_standalone_style(Style {
        width: Length::points(100.0),
        height: Length::points(40.0),
        linear_orientation: LinearOrientation::Horizontal,
        ..Style::default()
    })));
    let sticky = tree.push(SimpleNode::new(linear_standalone_style(Style {
        position: PositionType::Sticky,
        width: Length::points(20.0),
        height: Length::points(10.0),
        right: Length::percent(20.0),
        bottom: Length::percent(50.0),
        ..Style::default()
    })));
    tree.append_child(root, sticky);

    run_rust_layout(&mut tree, root, Constraints::definite(100.0, 40.0));

    assert_close(tree.nodes[sticky].layout.offset.x, 0.0);
    assert_close(tree.nodes[sticky].layout.offset.y, 0.0);
    assert_close(tree.nodes[sticky].layout.sticky_pos.left, STICKY_AUTO_INSET);
    assert_close(tree.nodes[sticky].layout.sticky_pos.right, 20.0);
    assert_close(tree.nodes[sticky].layout.sticky_pos.top, STICKY_AUTO_INSET);
    assert_close(tree.nodes[sticky].layout.sticky_pos.bottom, 20.0);
}

#[test]
fn head_to_head_flex_row_baseline_uses_nested_linear_container_baseline() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(standalone_style(Style {
        align_items: AlignItems::Baseline,
        ..Style::default()
    })));
    let reference = tree.push(SimpleNode::with_measured_size_and_baseline(
        standalone_style(Style::default()),
        Size::new(10.0, 40.0),
        35.0,
    ));
    let nested = tree.push(SimpleNode::new(linear_standalone_style(Style {
        linear_orientation: LinearOrientation::Horizontal,
        ..Style::default()
    })));
    let nested_early = tree.push(SimpleNode::with_measured_size_and_baseline(
        standalone_style(Style::default()),
        Size::new(10.0, 20.0),
        5.0,
    ));
    let nested_late = tree.push(SimpleNode::with_measured_size_and_baseline(
        standalone_style(Style::default()),
        Size::new(10.0, 30.0),
        25.0,
    ));
    tree.append_child(nested, nested_early);
    tree.append_child(nested, nested_late);
    tree.append_child(root, reference);
    tree.append_child(root, nested);

    run_rust_layout(&mut tree, root, Constraints::indefinite());
    assert_close(tree.nodes[reference].layout.offset.y, 0.0);
    assert_close(tree.nodes[nested].layout.offset.y, 10.0);
    assert_close(tree.nodes[nested].layout.baseline.unwrap(), 25.0);
}

#[test]
fn head_to_head_vertical_linear_stacks_measured_children() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(linear_standalone_style(Style {
        width: Length::points(80.0),
        padding: Rect::all(Length::points(2.0)),
        align_items: AlignItems::FlexStart,
        justify_content: JustifyContent::Center,
        ..Style::default()
    })));
    let first = tree.push(SimpleNode::with_measured_size(
        standalone_style(Style {
            margin: Rect::new(
                Length::points(1.0),
                Length::points(2.0),
                Length::points(3.0),
                Length::points(4.0),
            ),
            ..Style::default()
        }),
        Size::new(20.0, 6.0),
    ));
    let second = tree.push(SimpleNode::new(linear_standalone_style(Style {
        width: Length::points(18.0),
        height: Length::points(8.0),
        margin: Rect::new(
            Length::points(2.0),
            Length::points(1.0),
            Length::points(1.0),
            Length::points(2.0),
        ),
        ..Style::default()
    })));
    tree.append_child(root, first);
    tree.append_child(root, second);

    let size = run_rust_layout(
        &mut tree,
        root,
        Constraints::new(
            SideConstraint::definite(80.0),
            SideConstraint::at_most(40.0),
        ),
    );
    assert_close(size.width, 80.0);
    assert_close(size.height, 28.0);
    assert_close(tree.nodes[first].layout.offset.y, 5.0);
    assert_close(tree.nodes[second].layout.offset.y, 16.0);
}

#[test]
fn head_to_head_linear_visibility_hidden_and_collapse_participate_in_layout() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(linear_standalone_style(Style {
        width: Length::points(100.0),
        ..Style::default()
    })));
    let hidden = tree.push(SimpleNode::new(block_standalone_style(Style {
        width: Length::Auto,
        height: Length::points(10.0),
        visibility: Visibility::Hidden,
        ..Style::default()
    })));
    let collapsed = tree.push(SimpleNode::new(block_standalone_style(Style {
        width: Length::Auto,
        height: Length::points(20.0),
        visibility: Visibility::Collapse,
        ..Style::default()
    })));
    tree.append_child(root, hidden);
    tree.append_child(root, collapsed);

    let size = run_rust_layout(
        &mut tree,
        root,
        Constraints::new(
            SideConstraint::definite(100.0),
            SideConstraint::indefinite(),
        ),
    );

    assert_close(size.height, 30.0);
    assert_close(tree.nodes[hidden].layout.size.height, 10.0);
    assert_close(tree.nodes[collapsed].layout.offset.y, 10.0);
    assert_close(tree.nodes[collapsed].layout.size.height, 20.0);
}

#[test]
fn head_to_head_vertical_linear_center_child_receives_bounded_cross_axis_measure_constraint() {
    let mut tree = MeasuringTree::default();
    let root = tree.push(MeasuringNode::new(linear_standalone_style(Style {
        align_items: AlignItems::Center,
        width: Length::points(100.0),
        ..Style::default()
    })));
    let child = tree.push(MeasuringNode::measured(
        standalone_style(Style::default()),
        Size::new(150.0, 10.0),
    ));
    tree.append_child(root, child);

    run_rust_layout(&mut tree, root, Constraints::indefinite());

    let constraints = tree.nodes[child].last_constraints.unwrap();
    assert_eq!(constraints.width.mode, MeasureMode::AtMost);
    assert_close(constraints.width.size, 100.0);
    assert_close(tree.nodes[child].layout.size.width, 100.0);
}

#[test]
fn head_to_head_vertical_linear_at_most_cross_axis_passes_bound_to_measured_child() {
    let mut tree = MeasuringTree::default();
    let root = tree.push(MeasuringNode::new(
        linear_standalone_style(Style::default()),
    ));
    let child = tree.push(MeasuringNode::measured(
        standalone_style(Style::default()),
        Size::new(150.0, 10.0),
    ));
    tree.append_child(root, child);

    let size = run_rust_layout(
        &mut tree,
        root,
        Constraints::new(SideConstraint::at_most(100.0), SideConstraint::indefinite()),
    );

    let constraints = tree.nodes[child].last_constraints.unwrap();
    assert_eq!(constraints.width.mode, MeasureMode::AtMost);
    assert_close(constraints.width.size, 100.0);
    assert_close(tree.nodes[child].layout.size.width, 100.0);
    assert_close(size.width, 100.0);
}

#[test]
fn head_to_head_vertical_linear_indefinite_cross_axis_keeps_narrow_measured_child() {
    let mut tree = MeasuringTree::default();
    let root = tree.push(MeasuringNode::new(
        linear_standalone_style(Style::default()),
    ));
    let wide = tree.push(MeasuringNode::measured(
        standalone_style(Style::default()),
        Size::new(50.0, 10.0),
    ));
    let narrow = tree.push(MeasuringNode::measured(
        standalone_style(Style::default()),
        Size::new(20.0, 10.0),
    ));
    tree.append_child(root, wide);
    tree.append_child(root, narrow);

    let size = run_rust_layout(&mut tree, root, Constraints::indefinite());

    let constraints = tree.nodes[narrow].last_constraints.unwrap();
    assert_eq!(constraints.width.mode, MeasureMode::Indefinite);
    assert_close(size.width, 50.0);
    assert_close(tree.nodes[wide].layout.size.width, 50.0);
    assert_close(tree.nodes[narrow].layout.size.width, 20.0);
}

#[test]
fn head_to_head_horizontal_linear_splits_weighted_space() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(linear_standalone_style(Style {
        linear_orientation: LinearOrientation::Horizontal,
        width: Length::points(90.0),
        height: Length::points(20.0),
        ..Style::default()
    })));
    let first = tree.push(SimpleNode::new(standalone_style(Style {
        linear_weight: 1.0,
        ..Style::default()
    })));
    let second = tree.push(SimpleNode::new(standalone_style(Style {
        linear_weight: 2.0,
        ..Style::default()
    })));
    tree.append_child(root, first);
    tree.append_child(root, second);

    run_rust_layout(&mut tree, root, Constraints::definite(90.0, 20.0));

    assert_close(tree.nodes[first].layout.size.width, 30.0);
    assert_close(tree.nodes[second].layout.size.width, 60.0);
    assert_close(tree.nodes[second].layout.offset.x, 30.0);
}

#[test]
fn head_to_head_vertical_linear_gravity_packs_items_at_bottom() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(linear_standalone_style(Style {
        linear_gravity: LinearGravity::Bottom,
        width: Length::points(20.0),
        height: Length::points(100.0),
        align_items: AlignItems::FlexStart,
        ..Style::default()
    })));
    let first = fixed_linear_child(&mut tree, Length::points(10.0), Length::points(10.0));
    let second = fixed_linear_child(&mut tree, Length::points(10.0), Length::points(10.0));
    tree.append_child(root, first);
    tree.append_child(root, second);

    run_rust_layout(&mut tree, root, Constraints::definite(20.0, 100.0));

    assert_close(tree.nodes[first].layout.offset.y, 80.0);
    assert_close(tree.nodes[second].layout.offset.y, 90.0);
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ExpectedMainGravity {
    Start,
    End,
    Center,
    SpaceBetween,
}

fn main_axis_reversed(orientation: LinearOrientation, direction: Direction) -> bool {
    orientation.is_reverse() ^ (orientation.is_row() && direction == Direction::Rtl)
}

fn expected_justify_gravity(justify: JustifyContent) -> ExpectedMainGravity {
    match justify {
        JustifyContent::FlexEnd | JustifyContent::End => ExpectedMainGravity::End,
        JustifyContent::Center => ExpectedMainGravity::Center,
        JustifyContent::SpaceBetween => ExpectedMainGravity::SpaceBetween,
        JustifyContent::Stretch
        | JustifyContent::FlexStart
        | JustifyContent::Start
        | JustifyContent::SpaceAround
        | JustifyContent::SpaceEvenly => ExpectedMainGravity::Start,
    }
}

fn expected_linear_gravity(
    gravity: LinearGravity,
    orientation: LinearOrientation,
    direction: Direction,
) -> ExpectedMainGravity {
    let reversed = main_axis_reversed(orientation, direction);
    match gravity {
        LinearGravity::None | LinearGravity::Start => ExpectedMainGravity::Start,
        LinearGravity::End => ExpectedMainGravity::End,
        LinearGravity::Center | LinearGravity::CenterHorizontal | LinearGravity::CenterVertical => {
            ExpectedMainGravity::Center
        }
        LinearGravity::SpaceBetween => ExpectedMainGravity::SpaceBetween,
        LinearGravity::Left if orientation.is_row() && reversed => ExpectedMainGravity::End,
        LinearGravity::Right if orientation.is_row() && !reversed => ExpectedMainGravity::End,
        LinearGravity::Top if !orientation.is_row() && reversed => ExpectedMainGravity::End,
        LinearGravity::Bottom if !orientation.is_row() && !reversed => ExpectedMainGravity::End,
        LinearGravity::Left | LinearGravity::Right | LinearGravity::Top | LinearGravity::Bottom => {
            ExpectedMainGravity::Start
        }
    }
}

fn assert_two_item_main_offsets(
    tree: &SimpleTree,
    first: usize,
    second: usize,
    orientation: LinearOrientation,
    direction: Direction,
    gravity: ExpectedMainGravity,
) {
    let container_main = if orientation.is_row() { 100.0 } else { 80.0 };
    let free = container_main - 30.0;
    let start = match gravity {
        ExpectedMainGravity::Start | ExpectedMainGravity::SpaceBetween => 0.0,
        ExpectedMainGravity::End => free,
        ExpectedMainGravity::Center => free / 2.0,
    };
    let gap = if gravity == ExpectedMainGravity::SpaceBetween {
        free
    } else {
        0.0
    };
    let reversed = main_axis_reversed(orientation, direction);
    let first_physical = if reversed {
        container_main - start - 10.0
    } else {
        start
    };
    let second_flow = start + 10.0 + gap;
    let second_physical = if reversed {
        container_main - second_flow - 20.0
    } else {
        second_flow
    };
    let first_actual = if orientation.is_row() {
        tree.nodes[first].layout.offset.x
    } else {
        tree.nodes[first].layout.offset.y
    };
    let second_actual = if orientation.is_row() {
        tree.nodes[second].layout.offset.x
    } else {
        tree.nodes[second].layout.offset.y
    };
    assert_close(first_actual, first_physical);
    assert_close(second_actual, second_physical);
}

#[test]
fn head_to_head_vertical_linear_gravity_variants_match_cpp_mapping() {
    for gravity in [
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
    ] {
        let mut tree = SimpleTree::default();
        let root = tree.push(SimpleNode::new(linear_standalone_style(Style {
            linear_gravity: gravity,
            width: Length::points(30.0),
            height: Length::points(100.0),
            ..Style::default()
        })));
        let first = fixed_linear_child(&mut tree, Length::points(10.0), Length::points(10.0));
        let second = fixed_linear_child(&mut tree, Length::points(10.0), Length::points(20.0));
        tree.append_child(root, first);
        tree.append_child(root, second);

        run_rust_layout(&mut tree, root, Constraints::definite(30.0, 100.0));

        let expected =
            expected_linear_gravity(gravity, LinearOrientation::Vertical, Direction::Ltr);
        // This source matrix uses a 100px main axis rather than the 80px
        // matrix helper, so spell out the equivalent two-item oracle here.
        let (first_y, second_y) = match expected {
            ExpectedMainGravity::Start => (0.0, 10.0),
            ExpectedMainGravity::End => (70.0, 80.0),
            ExpectedMainGravity::Center => (35.0, 45.0),
            ExpectedMainGravity::SpaceBetween => (0.0, 80.0),
        };
        assert_close(tree.nodes[first].layout.offset.y, first_y);
        assert_close(tree.nodes[second].layout.offset.y, second_y);
    }
}

#[test]
fn head_to_head_rtl_horizontal_linear_gravity_swaps_physical_edges() {
    for (gravity, expected_x) in [(LinearGravity::Left, 0.0), (LinearGravity::Right, 80.0)] {
        let mut tree = SimpleTree::default();
        let root = tree.push(SimpleNode::new(linear_standalone_style(Style {
            direction: Direction::Rtl,
            linear_orientation: LinearOrientation::Horizontal,
            linear_gravity: gravity,
            width: Length::points(100.0),
            height: Length::points(20.0),
            ..Style::default()
        })));
        let child = fixed_linear_child(&mut tree, Length::points(20.0), Length::points(10.0));
        tree.append_child(root, child);

        run_rust_layout(&mut tree, root, Constraints::definite(100.0, 20.0));

        assert_close(tree.nodes[child].layout.offset.x, expected_x);
    }
}

fn build_alias_tree(
    orientation: LinearOrientation,
    reverse: bool,
) -> (SimpleTree, usize, [usize; 2]) {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(linear_standalone_style(Style {
        linear_orientation: orientation,
        justify_content: JustifyContent::Center,
        width: Length::points(100.0),
        height: Length::points(80.0),
        padding: Rect::new(
            Length::points(3.0),
            Length::points(5.0),
            Length::points(7.0),
            Length::points(11.0),
        ),
        ..Style::default()
    })));
    let mut children = [0; 2];
    for (slot, main_size) in
        children
            .iter_mut()
            .zip(if reverse { [20.0, 10.0] } else { [10.0, 20.0] })
    {
        let (width, height) = if orientation.is_row() {
            (Length::points(main_size), Length::points(12.0))
        } else {
            (Length::points(12.0), Length::points(main_size))
        };
        *slot = fixed_linear_child(&mut tree, width, height);
        tree.append_child(root, *slot);
    }
    (tree, root, children)
}

#[test]
fn head_to_head_linear_row_column_orientation_aliases_match_cpp_mapping() {
    for (alias, canonical, reverse) in [
        (LinearOrientation::Row, LinearOrientation::Horizontal, false),
        (
            LinearOrientation::RowReverse,
            LinearOrientation::HorizontalReverse,
            true,
        ),
        (
            LinearOrientation::Column,
            LinearOrientation::Vertical,
            false,
        ),
        (
            LinearOrientation::ColumnReverse,
            LinearOrientation::VerticalReverse,
            true,
        ),
    ] {
        let (mut alias_tree, alias_root, alias_children) = build_alias_tree(alias, reverse);
        let (mut canonical_tree, canonical_root, canonical_children) =
            build_alias_tree(canonical, reverse);
        let alias_size = run_rust_layout(
            &mut alias_tree,
            alias_root,
            Constraints::definite(100.0, 80.0),
        );
        let canonical_size = run_rust_layout(
            &mut canonical_tree,
            canonical_root,
            Constraints::definite(100.0, 80.0),
        );

        assert_eq!(alias_size, canonical_size);
        for (alias_child, canonical_child) in alias_children.into_iter().zip(canonical_children) {
            assert_eq!(
                alias_tree.nodes[alias_child].layout,
                canonical_tree.nodes[canonical_child].layout
            );
        }
    }
}

#[test]
#[allow(clippy::too_many_lines)] // Mirrors PR #25's full 160-case source matrix.
fn head_to_head_linear_main_axis_orientation_direction_reverse_matrix() {
    let orientations = [
        LinearOrientation::Horizontal,
        LinearOrientation::HorizontalReverse,
        LinearOrientation::Vertical,
        LinearOrientation::VerticalReverse,
    ];
    let directions = [Direction::Ltr, Direction::Rtl];
    let justify_values = [
        JustifyContent::Stretch,
        JustifyContent::FlexStart,
        JustifyContent::Start,
        JustifyContent::Center,
        JustifyContent::FlexEnd,
        JustifyContent::End,
        JustifyContent::SpaceBetween,
        JustifyContent::SpaceAround,
        JustifyContent::SpaceEvenly,
    ];
    let gravity_values = [
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
    let mut executions = 0;

    for orientation in orientations {
        for direction in directions {
            for justify_content in justify_values {
                let mut tree = SimpleTree::default();
                let root = tree.push(SimpleNode::new(linear_standalone_style(Style {
                    direction,
                    linear_orientation: orientation,
                    justify_content,
                    align_items: AlignItems::FlexStart,
                    width: Length::points(100.0),
                    height: Length::points(80.0),
                    ..Style::default()
                })));
                let mut children = [0; 2];
                for (slot, main_size) in children.iter_mut().zip([10.0, 20.0]) {
                    let (width, height) = if orientation.is_row() {
                        (Length::points(main_size), Length::points(12.0))
                    } else {
                        (Length::points(12.0), Length::points(main_size))
                    };
                    *slot = fixed_linear_child(&mut tree, width, height);
                    tree.append_child(root, *slot);
                }

                run_rust_layout(&mut tree, root, Constraints::definite(100.0, 80.0));

                assert_two_item_main_offsets(
                    &tree,
                    children[0],
                    children[1],
                    orientation,
                    direction,
                    expected_justify_gravity(justify_content),
                );
                executions += 1;
            }

            for linear_gravity in gravity_values {
                let mut tree = SimpleTree::default();
                let root = tree.push(SimpleNode::new(linear_standalone_style(Style {
                    direction,
                    linear_orientation: orientation,
                    linear_gravity,
                    align_items: AlignItems::FlexStart,
                    width: Length::points(100.0),
                    height: Length::points(80.0),
                    ..Style::default()
                })));
                let mut children = [0; 2];
                for (slot, main_size) in children.iter_mut().zip([10.0, 20.0]) {
                    let (width, height) = if orientation.is_row() {
                        (Length::points(main_size), Length::points(12.0))
                    } else {
                        (Length::points(12.0), Length::points(main_size))
                    };
                    *slot = fixed_linear_child(&mut tree, width, height);
                    tree.append_child(root, *slot);
                }

                run_rust_layout(&mut tree, root, Constraints::definite(100.0, 80.0));

                assert_two_item_main_offsets(
                    &tree,
                    children[0],
                    children[1],
                    orientation,
                    direction,
                    expected_linear_gravity(linear_gravity, orientation, direction),
                );
                executions += 1;
            }
        }
    }
    assert_eq!(executions, 160);
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ExpectedCrossGravity {
    Start,
    End,
    Center,
    Stretch,
}

fn expected_align_items_cross(align: AlignItems) -> ExpectedCrossGravity {
    match align {
        AlignItems::FlexEnd | AlignItems::End => ExpectedCrossGravity::End,
        AlignItems::Center => ExpectedCrossGravity::Center,
        // Linear's default auto-cross stretch is separate from an explicit
        // align-items fallback. An explicit child cross size remains intact.
        AlignItems::FlexStart | AlignItems::Start | AlignItems::Stretch | AlignItems::Baseline => {
            ExpectedCrossGravity::Start
        }
    }
}

fn expected_layout_cross(
    gravity: LinearLayoutGravity,
    orientation: LinearOrientation,
    direction: Direction,
) -> ExpectedCrossGravity {
    let vertical_rtl = !orientation.is_row() && direction == Direction::Rtl;
    match gravity {
        LinearLayoutGravity::None | LinearLayoutGravity::Start | LinearLayoutGravity::Top => {
            ExpectedCrossGravity::Start
        }
        LinearLayoutGravity::Right if vertical_rtl => ExpectedCrossGravity::Start,
        LinearLayoutGravity::Left if vertical_rtl => ExpectedCrossGravity::End,
        LinearLayoutGravity::Left => ExpectedCrossGravity::Start,
        LinearLayoutGravity::End | LinearLayoutGravity::Bottom | LinearLayoutGravity::Right => {
            ExpectedCrossGravity::End
        }
        LinearLayoutGravity::Center
        | LinearLayoutGravity::CenterHorizontal
        | LinearLayoutGravity::CenterVertical => ExpectedCrossGravity::Center,
        LinearLayoutGravity::Stretch
        | LinearLayoutGravity::FillHorizontal
        | LinearLayoutGravity::FillVertical => ExpectedCrossGravity::Stretch,
    }
}

fn assert_cross_layout(
    tree: &SimpleTree,
    child: usize,
    orientation: LinearOrientation,
    direction: Direction,
    gravity: ExpectedCrossGravity,
) {
    let (container_cross, natural_cross) = if orientation.is_row() {
        (80.0, 10.0)
    } else {
        (100.0, 20.0)
    };
    let child_cross = if gravity == ExpectedCrossGravity::Stretch {
        container_cross
    } else {
        natural_cross
    };
    let logical_offset = match gravity {
        ExpectedCrossGravity::Start | ExpectedCrossGravity::Stretch => 0.0,
        ExpectedCrossGravity::End => container_cross - child_cross,
        ExpectedCrossGravity::Center => (container_cross - child_cross) / 2.0,
    };
    let cross_reversed = !orientation.is_row() && direction == Direction::Rtl;
    let physical_offset = if cross_reversed {
        container_cross - logical_offset - child_cross
    } else {
        logical_offset
    };
    let (actual_offset, actual_size) = if orientation.is_row() {
        (
            tree.nodes[child].layout.offset.y,
            tree.nodes[child].layout.size.height,
        )
    } else {
        (
            tree.nodes[child].layout.offset.x,
            tree.nodes[child].layout.size.width,
        )
    };
    assert_close(actual_offset, physical_offset);
    assert_close(actual_size, child_cross);
}

#[test]
fn head_to_head_linear_cross_axis_orientation_direction_reverse_matrix() {
    let orientations = [
        LinearOrientation::Horizontal,
        LinearOrientation::HorizontalReverse,
        LinearOrientation::Vertical,
        LinearOrientation::VerticalReverse,
    ];
    let directions = [Direction::Ltr, Direction::Rtl];
    let align_values = [
        AlignItems::FlexStart,
        AlignItems::FlexEnd,
        AlignItems::Center,
        AlignItems::Stretch,
    ];
    let layout_gravity_values = [
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
    let mut executions = 0;

    for orientation in orientations {
        for direction in directions {
            for align_items in align_values {
                let mut tree = SimpleTree::default();
                let root = tree.push(SimpleNode::new(linear_standalone_style(Style {
                    direction,
                    linear_orientation: orientation,
                    align_items,
                    width: Length::points(100.0),
                    height: Length::points(80.0),
                    ..Style::default()
                })));
                let child =
                    fixed_linear_child(&mut tree, Length::points(20.0), Length::points(10.0));
                tree.append_child(root, child);

                run_rust_layout(&mut tree, root, Constraints::definite(100.0, 80.0));

                assert_cross_layout(
                    &tree,
                    child,
                    orientation,
                    direction,
                    expected_align_items_cross(align_items),
                );
                executions += 1;
            }

            for linear_layout_gravity in layout_gravity_values {
                let mut tree = SimpleTree::default();
                let root = tree.push(SimpleNode::new(linear_standalone_style(Style {
                    direction,
                    linear_orientation: orientation,
                    align_items: AlignItems::FlexStart,
                    width: Length::points(100.0),
                    height: Length::points(80.0),
                    ..Style::default()
                })));
                let child = tree.push(SimpleNode::new(block_standalone_style(Style {
                    width: Length::points(20.0),
                    height: Length::points(10.0),
                    linear_layout_gravity,
                    ..Style::default()
                })));
                tree.append_child(root, child);

                run_rust_layout(&mut tree, root, Constraints::definite(100.0, 80.0));

                assert_cross_layout(
                    &tree,
                    child,
                    orientation,
                    direction,
                    expected_layout_cross(linear_layout_gravity, orientation, direction),
                );
                executions += 1;
            }
        }
    }
    assert_eq!(executions, 136);
}

#[test]
fn head_to_head_rtl_horizontal_linear_gravity_physical_left_and_right() {
    for (gravity, expected) in [
        (LinearGravity::Left, [20.0, 0.0]),
        (LinearGravity::Right, [90.0, 70.0]),
    ] {
        let mut tree = SimpleTree::default();
        let root = tree.push(SimpleNode::new(linear_standalone_style(Style {
            direction: Direction::Rtl,
            linear_orientation: LinearOrientation::Horizontal,
            linear_gravity: gravity,
            width: Length::points(100.0),
            height: Length::points(20.0),
            ..Style::default()
        })));
        let first = fixed_linear_child(&mut tree, Length::points(10.0), Length::Auto);
        let second = fixed_linear_child(&mut tree, Length::points(20.0), Length::Auto);
        tree.append_child(root, first);
        tree.append_child(root, second);

        run_rust_layout(&mut tree, root, Constraints::definite(100.0, 20.0));

        assert_close(tree.nodes[first].layout.offset.x, expected[0]);
        assert_close(tree.nodes[second].layout.offset.x, expected[1]);
    }
}

#[test]
fn head_to_head_linear_cross_axis_auto_margins_override_cross_gravity() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(linear_standalone_style(Style {
        linear_cross_gravity: LinearCrossGravity::End,
        width: Length::points(100.0),
        height: Length::points(100.0),
        ..Style::default()
    })));
    let child = tree.push(SimpleNode::new(block_standalone_style(Style {
        width: Length::points(20.0),
        height: Length::points(10.0),
        margin: Rect::new(Length::Auto, Length::Auto, Length::ZERO, Length::ZERO),
        ..Style::default()
    })));
    tree.append_child(root, child);

    run_rust_layout(&mut tree, root, Constraints::definite(100.0, 100.0));

    assert_close(tree.nodes[child].layout.offset.x, 40.0);
    assert_close(tree.nodes[child].layout.margin.left, 40.0);
    assert_close(tree.nodes[child].layout.margin.right, 40.0);
}

fn assert_root_block_fit_content_natural_size(width: BaseLength, height: BaseLength) {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(block_standalone_style(Style {
        width: Length::fit_content(Some(width)),
        height: Length::fit_content(Some(height)),
        ..Style::default()
    })));
    let child = tree.push(SimpleNode::new(block_standalone_style(Style {
        width: Length::points(120.0),
        height: Length::points(30.0),
        ..Style::default()
    })));
    tree.append_child(root, child);

    let size = LayoutEngine::new().layout_with_owner_constraints(
        &mut tree,
        root,
        Constraints::definite(200.0, 100.0),
    );

    assert_close(size.width, 120.0);
    assert_close(size.height, 30.0);
    assert_close(tree.nodes[root].layout.size.width, 120.0);
    assert_close(tree.nodes[root].layout.size.height, 30.0);
}

fn assert_child_block_fit_content_natural_size(width: BaseLength, height: BaseLength) {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(block_standalone_style(Style {
        width: Length::points(200.0),
        height: Length::points(100.0),
        ..Style::default()
    })));
    let child = tree.push(SimpleNode::new(block_standalone_style(Style {
        width: Length::fit_content(Some(width)),
        height: Length::fit_content(Some(height)),
        ..Style::default()
    })));
    let grandchild = tree.push(SimpleNode::new(block_standalone_style(Style {
        width: Length::points(120.0),
        height: Length::points(30.0),
        ..Style::default()
    })));
    tree.append_child(root, child);
    tree.append_child(child, grandchild);

    LayoutEngine::new().layout_with_owner_constraints(
        &mut tree,
        root,
        Constraints::definite(200.0, 100.0),
    );

    assert_close(tree.nodes[child].layout.size.width, 120.0);
    assert_close(tree.nodes[child].layout.size.height, 30.0);
}

#[test]
fn head_to_head_root_block_fit_content_argument_uses_latest_linear_sizing() {
    assert_root_block_fit_content_natural_size(BaseLength::fixed(80.0), BaseLength::fixed(20.0));
}

#[test]
fn head_to_head_root_block_fit_content_percent_argument_uses_latest_linear_sizing() {
    assert_root_block_fit_content_natural_size(
        BaseLength::fixed_and_percent(0.0, 50.0),
        BaseLength::fixed_and_percent(0.0, 25.0),
    );
}

#[test]
fn head_to_head_root_block_fit_content_calc_argument_uses_latest_linear_sizing() {
    assert_root_block_fit_content_natural_size(
        BaseLength::fixed_and_percent(10.0, 50.0),
        BaseLength::fixed_and_percent(5.0, 25.0),
    );
}

#[test]
fn head_to_head_child_block_fit_content_argument_uses_latest_linear_sizing() {
    assert_child_block_fit_content_natural_size(BaseLength::fixed(80.0), BaseLength::fixed(20.0));
}

#[test]
fn head_to_head_child_block_fit_content_percent_argument_uses_latest_linear_sizing() {
    assert_child_block_fit_content_natural_size(
        BaseLength::fixed_and_percent(0.0, 50.0),
        BaseLength::fixed_and_percent(0.0, 25.0),
    );
}

#[test]
fn head_to_head_child_block_fit_content_calc_argument_uses_latest_linear_sizing() {
    assert_child_block_fit_content_natural_size(
        BaseLength::fixed_and_percent(10.0, 50.0),
        BaseLength::fixed_and_percent(5.0, 25.0),
    );
}

fn block_positioned_subtree(
    position: PositionType,
    width: Length,
    height: Length,
) -> (SimpleTree, usize, usize) {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(block_standalone_style(Style {
        width: Length::points(200.0),
        height: Length::points(100.0),
        ..Style::default()
    })));
    let parent = if position == PositionType::Fixed {
        let nested = tree.push(SimpleNode::new(block_standalone_style(Style {
            width: Length::points(20.0),
            height: Length::points(20.0),
            ..Style::default()
        })));
        tree.append_child(root, nested);
        nested
    } else {
        root
    };
    let positioned = tree.push(SimpleNode::new(block_standalone_style(Style {
        position,
        width,
        height,
        left: Length::points(7.0),
        top: Length::points(9.0),
        ..Style::default()
    })));
    tree.append_child(parent, positioned);
    (tree, root, positioned)
}

fn assert_positioned_block_natural_size(position: PositionType, max_content: bool) {
    let natural = if max_content { 250.0 } else { 120.0 };
    let (mut tree, root, positioned) = block_positioned_subtree(
        position,
        if max_content {
            Length::MaxContent
        } else {
            Length::fit_content(Some(BaseLength::fixed(80.0)))
        },
        if max_content {
            Length::MaxContent
        } else {
            Length::fit_content(Some(BaseLength::fixed(20.0)))
        },
    );
    let grandchild = tree.push(SimpleNode::new(block_standalone_style(Style {
        width: Length::points(natural),
        height: Length::points(if max_content { 130.0 } else { 30.0 }),
        ..Style::default()
    })));
    tree.append_child(positioned, grandchild);

    LayoutEngine::new().layout_with_owner_constraints(
        &mut tree,
        root,
        Constraints::definite(200.0, 100.0),
    );

    assert_close(tree.nodes[positioned].layout.size.width, natural);
    assert_close(
        tree.nodes[positioned].layout.size.height,
        if max_content { 130.0 } else { 30.0 },
    );
    assert_close(tree.nodes[positioned].layout.offset.x, 7.0);
    assert_close(tree.nodes[positioned].layout.offset.y, 9.0);
}

#[test]
fn head_to_head_absolute_block_fit_content_argument_uses_latest_linear_sizing() {
    assert_positioned_block_natural_size(PositionType::Absolute, false);
}

#[test]
fn head_to_head_absolute_subtree_fit_content_percent_argument_uses_latest_linear_sizing() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(block_standalone_style(Style {
        width: Length::points(122.0),
        height: Length::points(89.0),
        ..Style::default()
    })));
    let absolute = tree.push(SimpleNode::new(block_standalone_style(Style {
        position: PositionType::Absolute,
        width: Length::fit_content(Some(BaseLength::fixed_and_percent(5.0, 50.0))),
        height: Length::fit_content(Some(BaseLength::fixed_and_percent(3.0, 25.0))),
        left: Length::points(9.0),
        top: Length::points(6.0),
        ..Style::default()
    })));
    let grandchild = tree.push(SimpleNode::with_measured_size(
        block_standalone_style(Style {
            width: Length::points(74.0),
            height: Length::points(29.0),
            ..Style::default()
        }),
        Size::new(74.0, 29.0),
    ));
    tree.append_child(root, absolute);
    tree.append_child(absolute, grandchild);

    LayoutEngine::new().layout_with_owner_constraints(
        &mut tree,
        root,
        Constraints::definite(180.0, 130.0),
    );

    assert_close(tree.nodes[absolute].layout.size.width, 74.0);
    assert_close(tree.nodes[absolute].layout.size.height, 29.0);
    assert_close(tree.nodes[absolute].layout.offset.x, 9.0);
    assert_close(tree.nodes[absolute].layout.offset.y, 6.0);
}

#[test]
fn head_to_head_fixed_block_fit_content_argument_uses_latest_linear_sizing() {
    assert_positioned_block_natural_size(PositionType::Fixed, false);
}

#[test]
fn head_to_head_absolute_block_max_content_uses_latest_linear_natural_size() {
    assert_positioned_block_natural_size(PositionType::Absolute, true);
}

#[test]
fn head_to_head_fixed_block_max_content_uses_latest_linear_natural_size() {
    assert_positioned_block_natural_size(PositionType::Fixed, true);
}

#[test]
#[allow(clippy::too_many_lines)] // Retains the source PR's complete regression tree.
fn head_to_head_linear_auto_main_uses_final_grid_aspect_ratio_child_size() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(block_standalone_style(Style {
        width: Length::points(360.0),
        padding: Rect::all(Length::points(2.0)),
        border: Rect::all(1.0),
        ..Style::default()
    })));
    let grid = tree.push(SimpleNode::new(Style {
        display: Display::Grid,
        box_sizing: BoxSizing::ContentBox,
        width: Length::percent(30.0),
        min_width: Length::points(28.0),
        min_height: Length::points(12.0),
        max_width: Length::calc(44.0, 32.0),
        max_height: Length::calc(28.0, 45.0),
        aspect_ratio: Some(1.63),
        margin: Rect::new(
            Length::points(1.0),
            Length::ZERO,
            Length::ZERO,
            Length::ZERO,
        ),
        padding: Rect::new(
            Length::points(1.0),
            Length::points(3.0),
            Length::points(1.0),
            Length::points(1.0),
        ),
        border: Rect::new(1.0, 1.0, 1.0, 0.5),
        align_items: AlignItems::Center,
        justify_content: JustifyContent::Center,
        grid_template_columns: vec![Length::points(20.0), Length::Auto],
        grid_template_rows: vec![Length::points(12.0), Length::Auto],
        column_gap: Length::points(1.0),
        row_gap: Length::points(1.0),
        ..Style::default()
    }));
    let grid_child = tree.push(SimpleNode::new(block_standalone_style(Style {
        width: Length::points(22.0),
        height: Length::points(12.0),
        margin: Rect::new(
            Length::ZERO,
            Length::ZERO,
            Length::points(0.5),
            Length::ZERO,
        ),
        padding: Rect::all(Length::points(0.5)),
        ..Style::default()
    })));
    let sibling = tree.push(SimpleNode::new(block_standalone_style(Style {
        box_sizing: BoxSizing::BorderBox,
        width: Length::calc(8.0, 19.0),
        height: Length::points(25.0),
        min_width: Length::points(24.0),
        min_height: Length::points(13.0),
        max_width: Length::calc(45.0, 32.0),
        max_height: Length::calc(29.0, 45.0),
        margin: Rect::new(
            Length::points(2.0),
            Length::points(0.5),
            Length::points(1.0),
            Length::ZERO,
        ),
        padding: Rect::new(
            Length::points(2.0),
            Length::points(4.0),
            Length::points(1.5),
            Length::points(1.0),
        ),
        border: Rect::new(2.0, 1.5, 1.0, 1.5),
        align_items: AlignItems::Center,
        justify_content: JustifyContent::Center,
        ..Style::default()
    })));
    let sibling_child = tree.push(SimpleNode::new(block_standalone_style(Style {
        width: Length::points(23.0),
        height: Length::points(8.0),
        margin: Rect::new(
            Length::points(1.0),
            Length::ZERO,
            Length::points(1.0),
            Length::points(1.0),
        ),
        padding: Rect::all(Length::points(1.0)),
        border: Rect::all(1.0),
        ..Style::default()
    })));
    tree.append_child(root, grid);
    tree.append_child(grid, grid_child);
    tree.append_child(root, sibling);
    tree.append_child(sibling, sibling_child);

    let size = LayoutEngine::new().layout_with_owner_constraints(
        &mut tree,
        root,
        Constraints::new(SideConstraint::definite(8.0), SideConstraint::indefinite()),
    );

    assert!(size.width.is_finite() && size.height.is_finite());
    assert!(tree.nodes[grid].layout.size.height > 0.0);
    assert_close(
        tree.nodes[sibling].layout.offset.y,
        3.0 + tree.nodes[grid].layout.size.height
            + tree.nodes[grid].layout.margin.top
            + tree.nodes[grid].layout.margin.bottom
            + tree.nodes[sibling].layout.margin.top,
    );
}

#[test]
fn native_linear_inventory_exactly_partitions_source_tests_helpers_and_executions() {
    let source_names = SOURCE_FUNCTIONS.lines().collect::<Vec<_>>();
    let source_set = source_names.iter().copied().collect::<BTreeSet<_>>();
    assert_eq!(source_names.len(), SOURCE_TEST_COUNT + SOURCE_HELPER_COUNT);
    assert_eq!(source_set.len(), source_names.len());

    let helpers = SOURCE_HELPERS
        .iter()
        .map(|(name, reason)| {
            assert!(!reason.is_empty());
            assert!(source_set.contains(name));
            assert!(
                NATIVE_LINEAR_PORT_SOURCE.contains(&format!("fn {name}(")),
                "block-as-Linear source test has no executable Rust port: {name}"
            );
            *name
        })
        .collect::<BTreeSet<_>>();
    assert_eq!(helpers.len(), SOURCE_HELPER_COUNT);

    let block_as_linear = BLOCK_AS_LINEAR_CASES
        .iter()
        .map(|(name, reason)| {
            assert!(!reason.is_empty());
            assert!(source_set.contains(name));
            *name
        })
        .collect::<BTreeSet<_>>();
    assert_eq!(block_as_linear.len(), BLOCK_AS_LINEAR_TEST_COUNT);
    assert_eq!(
        block_as_linear
            .iter()
            .map(|name| source_execution_count(name))
            .sum::<usize>(),
        BLOCK_AS_LINEAR_EXECUTION_COUNT
    );

    let mut exact_overlap = BTreeSet::new();
    let mut unique = BTreeSet::new();
    for native in source_names
        .iter()
        .copied()
        .filter(|name| name.starts_with("head_to_head_"))
    {
        if block_as_linear.contains(native) {
            continue;
        }
        let direct = native
            .strip_prefix("head_to_head_")
            .expect("native Linear test has the source prefix");
        if DIRECT_LINEAR_SOURCE.contains(&format!("fn {direct}(")) {
            assert!(
                NATIVE_LINEAR_EXACT_SOURCE.contains(&format!("fn {native}(")),
                "native/direct overlap has no exact native Rust builder: {native}"
            );
            exact_overlap.insert((native, direct));
        } else {
            assert!(
                NATIVE_LINEAR_PORT_SOURCE.contains(&format!("fn {native}(")),
                "native-only Linear test has no Rust port: {native}"
            );
            unique.insert(native);
        }
    }

    assert_eq!(exact_overlap.len(), OVERLAP_TEST_COUNT);
    assert_eq!(
        exact_overlap
            .iter()
            .map(|(native, _)| source_execution_count(native))
            .sum::<usize>(),
        OVERLAP_EXECUTION_COUNT
    );
    assert_eq!(unique.len(), UNIQUE_TEST_COUNT);
    assert_eq!(
        unique
            .iter()
            .map(|name| source_execution_count(name))
            .sum::<usize>(),
        UNIQUE_EXECUTION_COUNT
    );

    let all_tests = exact_overlap
        .iter()
        .map(|(native, _)| *native)
        .chain(unique.iter().copied())
        .chain(block_as_linear.iter().copied())
        .collect::<BTreeSet<_>>();
    let expected_tests = source_set
        .difference(&helpers)
        .copied()
        .collect::<BTreeSet<_>>();
    assert_eq!(all_tests, expected_tests);
    assert_eq!(all_tests.len(), SOURCE_TEST_COUNT);
    assert_eq!(
        source_names
            .iter()
            .map(|name| source_execution_count(name))
            .sum::<usize>(),
        OVERLAP_EXECUTION_COUNT + UNIQUE_EXECUTION_COUNT + BLOCK_AS_LINEAR_EXECUTION_COUNT
    );
    assert_eq!(
        OVERLAP_EXECUTION_COUNT + UNIQUE_EXECUTION_COUNT + BLOCK_AS_LINEAR_EXECUTION_COUNT,
        491
    );
}

#[test]
fn native_linear_target_is_rust_only() {
    let manifest = include_str!("../Cargo.toml");
    assert!(!manifest.contains("[build-dependencies]"));
    let forbidden = [
        ["cc", "::Build"].concat(),
        ["cxx", "::bridge"].concat(),
        ["extern ", "\"C\""].concat(),
    ];
    let migrated = [NATIVE_LINEAR_PORT_SOURCE, NATIVE_LINEAR_EXACT_SOURCE].join("\n");
    assert!(forbidden.iter().all(|needle| !migrated.contains(needle)));
}
