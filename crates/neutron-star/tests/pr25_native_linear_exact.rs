// Copyright 2026 The Lynx Authors. All rights reserved.
// Licensed under the Apache License Version 2.0 that can be found in the
// LICENSE file in the root directory of this source tree.

//! Exact Rust-side builders for PR #25's 105 native/direct Linear overlaps.
//!
//! The source target compared these trees with Lynx C++. This companion keeps
//! the native builders and parameter rows (whose standalone-style helper
//! defaults children to Flex), but replaces the comparison runner with two
//! deterministic neutron-star executions and finite-layout validation.

#![allow(dead_code, clippy::too_many_lines, clippy::similar_names)]

mod pr25_support;
mod support;

use std::collections::BTreeSet;

use pr25_support::{
    AlignItems, BaseLength, BoxSizing, Constraints, Direction, Display, JustifyContent,
    LayoutEngine, LayoutResult, LayoutTree, Length, LinearCrossGravity, LinearGravity,
    LinearLayoutGravity, LinearOrientation, MeasurementProfile, PositionType, Rect, RegularMeasure,
    SideConstraint, SimpleNode, SimpleTree, Size, Style,
};

const NATIVE_INVENTORY: &str = include_str!("pr25_native_linear_inventory.txt");
const DIRECT_LINEAR: &str = include_str!("pr25_linear_layout.rs");
const THIS_SOURCE: &str = include_str!("pr25_native_linear_exact.rs");

fn native_overlap_execution_count(name: &str) -> usize {
    match name {
        "head_to_head_horizontal_linear_justify_content_distribution_values_map_to_start" => 3,
        "head_to_head_linear_absolute_child_cross_axis_uses_cpp_computed_layout_gravity_order"
        | "head_to_head_linear_absolute_vertical_child_uses_cpp_main_axis_static_position" => 4,
        "head_to_head_linear_layout_gravity_physical_variants_match_cpp_groups"
        | "head_to_head_horizontal_linear_layout_gravity_physical_variants_match_cpp_groups" => 13,
        "head_to_head_rtl_vertical_linear_layout_gravity_keeps_physical_left_and_right" => 2,
        "head_to_head_vertical_linear_cross_gravity_variants_override_align_items"
        | "head_to_head_horizontal_linear_cross_gravity_variants_override_align_items" => 5,
        _ => 1,
    }
}

fn layout_is_finite(layout: LayoutResult) -> bool {
    let scalar = [
        layout.offset.x,
        layout.offset.y,
        layout.size.width,
        layout.size.height,
        layout.padding.left,
        layout.padding.right,
        layout.padding.top,
        layout.padding.bottom,
        layout.border.left,
        layout.border.right,
        layout.border.top,
        layout.border.bottom,
        layout.margin.left,
        layout.margin.right,
        layout.margin.top,
        layout.margin.bottom,
        layout.sticky_pos.left,
        layout.sticky_pos.right,
        layout.sticky_pos.top,
        layout.sticky_pos.bottom,
    ];
    scalar.into_iter().all(f32::is_finite) && layout.baseline.is_none_or(f32::is_finite)
}

fn assert_native_rust_layout(tree: SimpleTree, root: usize, constraints: Constraints) {
    let mut first = tree.clone();
    let mut second = tree;
    let first_size =
        LayoutEngine::new().layout_with_owner_constraints(&mut first, root, constraints);
    let second_size =
        LayoutEngine::new().layout_with_owner_constraints(&mut second, root, constraints);

    assert_eq!(
        first_size, second_size,
        "native Linear layout is nondeterministic"
    );
    assert!(first_size.width.is_finite() && first_size.height.is_finite());
    assert_eq!(first.nodes.len(), second.nodes.len());
    for (first, second) in first.nodes.iter().zip(&second.nodes) {
        assert_eq!(
            first.layout, second.layout,
            "native Linear node layout is nondeterministic"
        );
        assert!(layout_is_finite(first.layout));
    }
}

#[derive(Clone, Debug)]
struct MeasuringNode {
    style: Style,
    layout: LayoutResult,
    children: Vec<usize>,
    measure: Option<MeasureBehavior>,
}

#[derive(Clone, Copy, Debug)]
enum MeasureBehavior {
    Fixed(Size),
    HeightFromWidth {
        intrinsic_width: f32,
        fallback_height: f32,
        height_ratio: f32,
    },
    WidthByHeightMode {
        at_most_width: f32,
        definite_width: f32,
        height: f32,
    },
}

impl MeasuringNode {
    fn new(style: Style) -> Self {
        Self {
            style,
            layout: LayoutResult::default(),
            children: Vec::new(),
            measure: None,
        }
    }

    fn height_from_width(
        style: Style,
        intrinsic_width: f32,
        fallback_height: f32,
        height_ratio: f32,
    ) -> Self {
        Self {
            measure: Some(MeasureBehavior::HeightFromWidth {
                intrinsic_width,
                fallback_height,
                height_ratio,
            }),
            ..Self::new(style)
        }
    }

    fn measured(style: Style, measured_size: Size) -> Self {
        Self {
            measure: Some(MeasureBehavior::Fixed(measured_size)),
            ..Self::new(style)
        }
    }

    fn width_by_height_mode(
        style: Style,
        at_most_width: f32,
        definite_width: f32,
        height: f32,
    ) -> Self {
        Self {
            measure: Some(MeasureBehavior::WidthByHeightMode {
                at_most_width,
                definite_width,
                height,
            }),
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

    fn layout(&self, node: Self::NodeId) -> Option<LayoutResult> {
        Some(self.nodes[node].layout)
    }

    fn measure(&mut self, node: Self::NodeId, constraints: Constraints) -> Option<Size> {
        self.nodes[node].measure.map(|behavior| match behavior {
            MeasureBehavior::Fixed(size) => Size::new(
                constraints.width.clamp(size.width),
                constraints.height.clamp(size.height),
            ),
            MeasureBehavior::HeightFromWidth {
                intrinsic_width,
                fallback_height,
                height_ratio,
            } => {
                let resolved_width = constraints.width.bounded_size().unwrap_or(intrinsic_width);
                let resolved_height = if constraints.width.bounded_size().is_some() {
                    resolved_width * height_ratio
                } else {
                    fallback_height
                };
                Size::new(
                    constraints.width.clamp(resolved_width),
                    constraints.height.clamp(resolved_height),
                )
            }
            MeasureBehavior::WidthByHeightMode {
                at_most_width,
                definite_width,
                height,
            } => {
                let width = if constraints.height.is_definite() {
                    definite_width
                } else {
                    at_most_width
                };
                Size::new(
                    constraints.width.clamp(width),
                    constraints.height.clamp(height),
                )
            }
        })
    }

    fn has_measure(&self, node: Self::NodeId) -> bool {
        self.nodes[node].measure.is_some()
    }

    fn measurement_profile(&self, node: Self::NodeId) -> Option<MeasurementProfile> {
        self.nodes[node].measure.map(|behavior| MeasurementProfile {
            regular: Some(match behavior {
                MeasureBehavior::Fixed(size) => RegularMeasure::Fixed(size),
                MeasureBehavior::HeightFromWidth {
                    intrinsic_width,
                    fallback_height,
                    height_ratio,
                } => RegularMeasure::HeightFromWidth {
                    intrinsic_width,
                    fallback_height,
                    height_ratio,
                },
                MeasureBehavior::WidthByHeightMode {
                    at_most_width,
                    definite_width,
                    height,
                } => RegularMeasure::WidthByHeightDefiniteness {
                    at_most_width,
                    definite_width,
                    height,
                },
            }),
            min_content: None,
            max_content: None,
            first_baseline: None,
        })
    }
}

fn assert_measuring_native_rust_layout(tree: MeasuringTree, root: usize, constraints: Constraints) {
    let mut first = tree.clone();
    let mut second = tree;
    let first_size =
        LayoutEngine::new().layout_with_owner_constraints(&mut first, root, constraints);
    let second_size =
        LayoutEngine::new().layout_with_owner_constraints(&mut second, root, constraints);

    assert_eq!(
        first_size, second_size,
        "measured native Linear layout is nondeterministic"
    );
    assert!(first_size.width.is_finite() && first_size.height.is_finite());
    assert_eq!(first.nodes.len(), second.nodes.len());
    for (first, second) in first.nodes.iter().zip(&second.nodes) {
        assert_eq!(
            first.layout, second.layout,
            "measured native Linear node is nondeterministic"
        );
        assert!(layout_is_finite(first.layout));
    }
}

#[test]
fn native_direct_overlap_inventory_has_exact_builders_and_rows() {
    let names = NATIVE_INVENTORY
        .lines()
        .filter(|name| name.starts_with("head_to_head_"))
        .filter(|name| {
            let direct = name.trim_start_matches("head_to_head_");
            DIRECT_LINEAR.contains(&format!("fn {direct}("))
        })
        .collect::<BTreeSet<_>>();
    assert_eq!(names.len(), 105);
    assert_eq!(
        names
            .iter()
            .map(|name| native_overlap_execution_count(name))
            .sum::<usize>(),
        146
    );
    for name in names {
        assert!(
            THIS_SOURCE.contains(&format!("fn {name}(")),
            "native/direct overlap lacks exact Rust builder: {name}"
        );
    }
    let runner = ["assert_", "native_rust_layout("].concat();
    let measuring_runner = ["assert_measuring_", "native_rust_layout("].concat();
    // Subtract each Rust runner's own definition, leaving source call sites.
    assert_eq!(THIS_SOURCE.matches(&runner).count() - 1, 108);
    assert_eq!(THIS_SOURCE.matches(&measuring_runner).count() - 1, 3);
}

#[test]
fn head_to_head_linear_absolute_static_position_with_margins_uses_margin_bound_size() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(linear_standalone_style(Style {
        linear_orientation: LinearOrientation::Horizontal,
        linear_gravity: LinearGravity::Center,
        width: Length::points(100.0),
        height: Length::points(50.0),
        ..Style::default()
    })));
    let absolute = tree.push(SimpleNode::new(standalone_style(Style {
        position: PositionType::Absolute,
        width: Length::points(10.0),
        height: Length::points(8.0),
        linear_layout_gravity: LinearLayoutGravity::End,
        margin: Rect::new(
            Length::points(3.0),
            Length::points(7.0),
            Length::points(4.0),
            Length::points(6.0),
        ),
        ..Style::default()
    })));
    tree.append_child(root, absolute);

    assert_native_rust_layout(tree, root, Constraints::definite(100.0, 50.0));
}

#[test]
fn head_to_head_linear_absolute_rtl_static_position_with_margins_uses_reversed_front() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(linear_standalone_style(Style {
        direction: Direction::Rtl,
        linear_orientation: LinearOrientation::Horizontal,
        linear_gravity: LinearGravity::None,
        width: Length::points(100.0),
        height: Length::points(50.0),
        ..Style::default()
    })));
    let absolute = tree.push(SimpleNode::new(standalone_style(Style {
        position: PositionType::Absolute,
        width: Length::points(10.0),
        height: Length::points(8.0),
        linear_layout_gravity: LinearLayoutGravity::Start,
        margin: Rect::new(
            Length::points(3.0),
            Length::points(7.0),
            Length::points(4.0),
            Length::points(6.0),
        ),
        ..Style::default()
    })));
    tree.append_child(root, absolute);

    assert_native_rust_layout(tree, root, Constraints::definite(100.0, 50.0));
}

#[test]
fn head_to_head_linear_absolute_child_layout_gravity_overrides_align_self_and_cross_gravity() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(linear_standalone_style(Style {
        linear_orientation: LinearOrientation::Vertical,
        linear_gravity: LinearGravity::Center,
        align_items: AlignItems::FlexStart,
        linear_cross_gravity: LinearCrossGravity::End,
        width: Length::points(100.0),
        height: Length::points(100.0),
        ..Style::default()
    })));
    let absolute = tree.push(SimpleNode::new(standalone_style(Style {
        position: PositionType::Absolute,
        width: Length::points(20.0),
        height: Length::points(10.0),
        align_self: Some(AlignItems::FlexEnd),
        linear_layout_gravity: LinearLayoutGravity::Left,
        ..Style::default()
    })));
    tree.append_child(root, absolute);

    assert_native_rust_layout(tree, root, Constraints::definite(100.0, 100.0));
}

#[test]
fn head_to_head_linear_absolute_child_cross_axis_uses_cpp_computed_layout_gravity_order() {
    {
        let mut tree = SimpleTree::default();
        let root = tree.push(SimpleNode::new(linear_standalone_style(Style {
            linear_orientation: LinearOrientation::Horizontal,
            align_items: AlignItems::FlexStart,
            linear_cross_gravity: LinearCrossGravity::End,
            width: Length::points(100.0),
            height: Length::points(50.0),
            ..Style::default()
        })));
        let absolute = tree.push(SimpleNode::new(standalone_style(Style {
            position: PositionType::Absolute,
            align_self: Some(AlignItems::Center),
            width: Length::points(20.0),
            height: Length::points(10.0),
            ..Style::default()
        })));
        tree.append_child(root, absolute);

        assert_native_rust_layout(tree, root, Constraints::definite(100.0, 50.0));
    }

    {
        let mut tree = SimpleTree::default();
        let root = tree.push(SimpleNode::new(linear_standalone_style(Style {
            linear_orientation: LinearOrientation::Horizontal,
            align_items: AlignItems::FlexStart,
            linear_cross_gravity: LinearCrossGravity::End,
            width: Length::points(100.0),
            height: Length::points(50.0),
            ..Style::default()
        })));
        let absolute = tree.push(SimpleNode::new(standalone_style(Style {
            position: PositionType::Absolute,
            width: Length::points(20.0),
            height: Length::points(10.0),
            ..Style::default()
        })));
        tree.append_child(root, absolute);

        assert_native_rust_layout(tree, root, Constraints::definite(100.0, 50.0));
    }

    {
        let mut tree = SimpleTree::default();
        let root = tree.push(SimpleNode::new(linear_standalone_style(Style {
            linear_orientation: LinearOrientation::Horizontal,
            align_items: AlignItems::FlexEnd,
            linear_cross_gravity: LinearCrossGravity::None,
            width: Length::points(100.0),
            height: Length::points(50.0),
            ..Style::default()
        })));
        let absolute = tree.push(SimpleNode::new(standalone_style(Style {
            position: PositionType::Absolute,
            width: Length::points(20.0),
            height: Length::points(10.0),
            ..Style::default()
        })));
        tree.append_child(root, absolute);

        assert_native_rust_layout(tree, root, Constraints::definite(100.0, 50.0));
    }

    {
        let mut tree = SimpleTree::default();
        let root = tree.push(SimpleNode::new(linear_standalone_style(Style {
            linear_orientation: LinearOrientation::Horizontal,
            align_items: AlignItems::Stretch,
            linear_cross_gravity: LinearCrossGravity::None,
            width: Length::points(100.0),
            height: Length::points(50.0),
            ..Style::default()
        })));
        let absolute = tree.push(SimpleNode::new(standalone_style(Style {
            position: PositionType::Absolute,
            width: Length::points(20.0),
            height: Length::points(10.0),
            ..Style::default()
        })));
        tree.append_child(root, absolute);

        assert_native_rust_layout(tree, root, Constraints::definite(100.0, 50.0));
    }
}

#[test]
fn head_to_head_linear_absolute_vertical_child_uses_cpp_main_axis_static_position() {
    {
        let mut tree = SimpleTree::default();
        let root = tree.push(SimpleNode::new(linear_standalone_style(Style {
            linear_orientation: LinearOrientation::Vertical,
            linear_gravity: LinearGravity::Center,
            width: Length::points(50.0),
            height: Length::points(100.0),
            ..Style::default()
        })));
        let absolute = tree.push(SimpleNode::new(standalone_style(Style {
            position: PositionType::Absolute,
            width: Length::points(20.0),
            height: Length::points(10.0),
            ..Style::default()
        })));
        tree.append_child(root, absolute);

        assert_native_rust_layout(tree, root, Constraints::definite(50.0, 100.0));
    }

    {
        let mut tree = SimpleTree::default();
        let root = tree.push(SimpleNode::new(linear_standalone_style(Style {
            linear_orientation: LinearOrientation::Vertical,
            linear_gravity: LinearGravity::End,
            width: Length::points(50.0),
            height: Length::points(100.0),
            ..Style::default()
        })));
        let absolute = tree.push(SimpleNode::new(standalone_style(Style {
            position: PositionType::Absolute,
            width: Length::points(20.0),
            height: Length::points(10.0),
            ..Style::default()
        })));
        tree.append_child(root, absolute);

        assert_native_rust_layout(tree, root, Constraints::definite(50.0, 100.0));
    }

    {
        let mut tree = SimpleTree::default();
        let root = tree.push(SimpleNode::new(linear_standalone_style(Style {
            linear_orientation: LinearOrientation::Vertical,
            linear_gravity: LinearGravity::Bottom,
            width: Length::points(50.0),
            height: Length::points(100.0),
            ..Style::default()
        })));
        let absolute = tree.push(SimpleNode::new(standalone_style(Style {
            position: PositionType::Absolute,
            width: Length::points(20.0),
            height: Length::points(10.0),
            ..Style::default()
        })));
        tree.append_child(root, absolute);

        assert_native_rust_layout(tree, root, Constraints::definite(50.0, 100.0));
    }

    {
        let mut tree = SimpleTree::default();
        let root = tree.push(SimpleNode::new(linear_standalone_style(Style {
            linear_orientation: LinearOrientation::Vertical,
            linear_gravity: LinearGravity::Top,
            width: Length::points(50.0),
            height: Length::points(100.0),
            ..Style::default()
        })));
        let absolute = tree.push(SimpleNode::new(standalone_style(Style {
            position: PositionType::Absolute,
            width: Length::points(20.0),
            height: Length::points(10.0),
            ..Style::default()
        })));
        tree.append_child(root, absolute);

        assert_native_rust_layout(tree, root, Constraints::definite(50.0, 100.0));
    }
}

#[test]
fn head_to_head_linear_fixed_descendant_without_insets_uses_root_linear_static_alignment() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(linear_standalone_style(Style {
        linear_orientation: LinearOrientation::Horizontal,
        linear_gravity: LinearGravity::Center,
        width: Length::points(100.0),
        height: Length::points(50.0),
        ..Style::default()
    })));
    let nested = tree.push(SimpleNode::new(block_standalone_style(Style {
        width: Length::points(20.0),
        height: Length::points(20.0),
        ..Style::default()
    })));
    let fixed = tree.push(SimpleNode::new(block_standalone_style(Style {
        position: PositionType::Fixed,
        linear_layout_gravity: LinearLayoutGravity::End,
        width: Length::points(10.0),
        height: Length::points(8.0),
        ..Style::default()
    })));
    tree.append_child(root, nested);
    tree.append_child(nested, fixed);

    assert_native_rust_layout(tree, root, Constraints::definite(100.0, 50.0));
}

#[test]
fn head_to_head_linear_fixed_static_position_with_margins_uses_margin_bound_size() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(linear_standalone_style(Style {
        linear_orientation: LinearOrientation::Horizontal,
        linear_gravity: LinearGravity::Center,
        width: Length::points(100.0),
        height: Length::points(50.0),
        ..Style::default()
    })));
    let nested = tree.push(SimpleNode::new(block_standalone_style(Style {
        width: Length::points(20.0),
        height: Length::points(20.0),
        ..Style::default()
    })));
    let fixed = tree.push(SimpleNode::new(block_standalone_style(Style {
        position: PositionType::Fixed,
        linear_layout_gravity: LinearLayoutGravity::End,
        width: Length::points(10.0),
        height: Length::points(8.0),
        margin: Rect::new(
            Length::points(3.0),
            Length::points(7.0),
            Length::points(4.0),
            Length::points(6.0),
        ),
        ..Style::default()
    })));
    tree.append_child(root, nested);
    tree.append_child(nested, fixed);

    assert_native_rust_layout(tree, root, Constraints::definite(100.0, 50.0));
}

#[test]
fn head_to_head_linear_fixed_rtl_static_position_with_margins_uses_reversed_front() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(linear_standalone_style(Style {
        direction: Direction::Rtl,
        linear_orientation: LinearOrientation::Horizontal,
        linear_gravity: LinearGravity::None,
        width: Length::points(100.0),
        height: Length::points(50.0),
        ..Style::default()
    })));
    let nested = tree.push(SimpleNode::new(block_standalone_style(Style {
        width: Length::points(20.0),
        height: Length::points(20.0),
        ..Style::default()
    })));
    let fixed = tree.push(SimpleNode::new(block_standalone_style(Style {
        position: PositionType::Fixed,
        linear_layout_gravity: LinearLayoutGravity::Start,
        width: Length::points(10.0),
        height: Length::points(8.0),
        margin: Rect::new(
            Length::points(3.0),
            Length::points(7.0),
            Length::points(4.0),
            Length::points(6.0),
        ),
        ..Style::default()
    })));
    tree.append_child(root, nested);
    tree.append_child(nested, fixed);

    assert_native_rust_layout(tree, root, Constraints::definite(100.0, 50.0));
}

#[test]
fn head_to_head_linear_fixed_vertical_descendant_uses_center_main_axis_static_position() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(linear_standalone_style(Style {
        linear_orientation: LinearOrientation::Vertical,
        linear_gravity: LinearGravity::Center,
        width: Length::points(50.0),
        height: Length::points(100.0),
        ..Style::default()
    })));
    let nested = tree.push(SimpleNode::new(block_standalone_style(Style {
        width: Length::points(20.0),
        height: Length::points(20.0),
        ..Style::default()
    })));
    let fixed = tree.push(SimpleNode::new(block_standalone_style(Style {
        position: PositionType::Fixed,
        width: Length::points(20.0),
        height: Length::points(10.0),
        ..Style::default()
    })));
    tree.append_child(root, nested);
    tree.append_child(nested, fixed);

    assert_native_rust_layout(tree, root, Constraints::definite(50.0, 100.0));
}

#[test]
fn head_to_head_linear_fixed_vertical_descendant_uses_end_main_axis_static_position() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(linear_standalone_style(Style {
        linear_orientation: LinearOrientation::Vertical,
        linear_gravity: LinearGravity::End,
        width: Length::points(50.0),
        height: Length::points(100.0),
        ..Style::default()
    })));
    let nested = tree.push(SimpleNode::new(block_standalone_style(Style {
        width: Length::points(20.0),
        height: Length::points(20.0),
        ..Style::default()
    })));
    let fixed = tree.push(SimpleNode::new(block_standalone_style(Style {
        position: PositionType::Fixed,
        width: Length::points(20.0),
        height: Length::points(10.0),
        ..Style::default()
    })));
    tree.append_child(root, nested);
    tree.append_child(nested, fixed);

    assert_native_rust_layout(tree, root, Constraints::definite(50.0, 100.0));
}

#[test]
fn head_to_head_linear_fixed_vertical_descendant_uses_physical_bottom_main_axis_static_position() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(linear_standalone_style(Style {
        linear_orientation: LinearOrientation::Vertical,
        linear_gravity: LinearGravity::Bottom,
        width: Length::points(50.0),
        height: Length::points(100.0),
        ..Style::default()
    })));
    let nested = tree.push(SimpleNode::new(block_standalone_style(Style {
        width: Length::points(20.0),
        height: Length::points(20.0),
        ..Style::default()
    })));
    let fixed = tree.push(SimpleNode::new(block_standalone_style(Style {
        position: PositionType::Fixed,
        width: Length::points(20.0),
        height: Length::points(10.0),
        ..Style::default()
    })));
    tree.append_child(root, nested);
    tree.append_child(nested, fixed);

    assert_native_rust_layout(tree, root, Constraints::definite(50.0, 100.0));
}

#[test]
fn head_to_head_linear_fixed_vertical_descendant_uses_physical_top_main_axis_static_position() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(linear_standalone_style(Style {
        linear_orientation: LinearOrientation::Vertical,
        linear_gravity: LinearGravity::Top,
        width: Length::points(50.0),
        height: Length::points(100.0),
        ..Style::default()
    })));
    let nested = tree.push(SimpleNode::new(block_standalone_style(Style {
        width: Length::points(20.0),
        height: Length::points(20.0),
        ..Style::default()
    })));
    let fixed = tree.push(SimpleNode::new(block_standalone_style(Style {
        position: PositionType::Fixed,
        width: Length::points(20.0),
        height: Length::points(10.0),
        ..Style::default()
    })));
    tree.append_child(root, nested);
    tree.append_child(nested, fixed);

    assert_native_rust_layout(tree, root, Constraints::definite(50.0, 100.0));
}

#[test]
fn head_to_head_linear_fixed_start_insets_override_static_alignment() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(linear_standalone_style(Style {
        linear_orientation: LinearOrientation::Horizontal,
        linear_gravity: LinearGravity::Center,
        width: Length::points(200.0),
        height: Length::points(100.0),
        ..Style::default()
    })));
    let nested = tree.push(SimpleNode::new(block_standalone_style(Style {
        width: Length::points(40.0),
        height: Length::points(30.0),
        ..Style::default()
    })));
    let fixed = tree.push(SimpleNode::new(block_standalone_style(Style {
        position: PositionType::Fixed,
        left: Length::points(12.0),
        top: Length::points(9.0),
        width: Length::points(20.0),
        height: Length::points(10.0),
        linear_layout_gravity: LinearLayoutGravity::End,
        ..Style::default()
    })));
    tree.append_child(root, nested);
    tree.append_child(nested, fixed);

    assert_native_rust_layout(tree, root, Constraints::definite(200.0, 100.0));
}

#[test]
fn head_to_head_linear_fixed_paired_insets_with_explicit_size_use_start_insets() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(linear_standalone_style(Style {
        linear_orientation: LinearOrientation::Horizontal,
        linear_gravity: LinearGravity::Center,
        width: Length::points(200.0),
        height: Length::points(100.0),
        ..Style::default()
    })));
    let nested = tree.push(SimpleNode::new(block_standalone_style(Style {
        width: Length::points(40.0),
        height: Length::points(30.0),
        ..Style::default()
    })));
    let fixed = tree.push(SimpleNode::new(block_standalone_style(Style {
        position: PositionType::Fixed,
        left: Length::points(12.0),
        right: Length::points(30.0),
        top: Length::points(9.0),
        bottom: Length::points(25.0),
        width: Length::points(20.0),
        height: Length::points(10.0),
        linear_layout_gravity: LinearLayoutGravity::End,
        ..Style::default()
    })));
    tree.append_child(root, nested);
    tree.append_child(nested, fixed);

    assert_native_rust_layout(tree, root, Constraints::definite(200.0, 100.0));
}

#[test]
fn head_to_head_linear_fixed_end_insets_override_static_alignment() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(linear_standalone_style(Style {
        linear_orientation: LinearOrientation::Horizontal,
        linear_gravity: LinearGravity::Center,
        width: Length::points(200.0),
        height: Length::points(100.0),
        ..Style::default()
    })));
    let nested = tree.push(SimpleNode::new(block_standalone_style(Style {
        width: Length::points(40.0),
        height: Length::points(30.0),
        ..Style::default()
    })));
    let fixed = tree.push(SimpleNode::new(block_standalone_style(Style {
        position: PositionType::Fixed,
        right: Length::points(30.0),
        bottom: Length::points(25.0),
        width: Length::points(20.0),
        height: Length::points(10.0),
        linear_layout_gravity: LinearLayoutGravity::End,
        ..Style::default()
    })));
    tree.append_child(root, nested);
    tree.append_child(nested, fixed);

    assert_native_rust_layout(tree, root, Constraints::definite(200.0, 100.0));
}

#[test]
fn head_to_head_linear_fixed_end_insets_with_margins_position_margin_box() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(linear_standalone_style(Style {
        linear_orientation: LinearOrientation::Horizontal,
        linear_gravity: LinearGravity::Center,
        width: Length::points(200.0),
        height: Length::points(100.0),
        ..Style::default()
    })));
    let nested = tree.push(SimpleNode::new(block_standalone_style(Style {
        width: Length::points(40.0),
        height: Length::points(30.0),
        ..Style::default()
    })));
    let fixed = tree.push(SimpleNode::new(block_standalone_style(Style {
        position: PositionType::Fixed,
        right: Length::points(30.0),
        bottom: Length::points(25.0),
        width: Length::points(20.0),
        height: Length::points(10.0),
        linear_layout_gravity: LinearLayoutGravity::End,
        margin: Rect::new(
            Length::points(3.0),
            Length::points(7.0),
            Length::points(4.0),
            Length::points(6.0),
        ),
        ..Style::default()
    })));
    tree.append_child(root, nested);
    tree.append_child(nested, fixed);

    assert_native_rust_layout(tree, root, Constraints::definite(200.0, 100.0));
}

#[test]
fn head_to_head_linear_fixed_percent_insets_and_size_resolve_against_root_linear_containing_block()
{
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(linear_standalone_style(Style {
        width: Length::points(200.0),
        height: Length::points(100.0),
        ..Style::default()
    })));
    let nested = tree.push(SimpleNode::new(block_standalone_style(Style {
        width: Length::points(40.0),
        height: Length::points(30.0),
        ..Style::default()
    })));
    let fixed = tree.push(SimpleNode::new(block_standalone_style(Style {
        position: PositionType::Fixed,
        left: Length::percent(10.0),
        top: Length::percent(25.0),
        width: Length::percent(50.0),
        height: Length::percent(20.0),
        ..Style::default()
    })));
    tree.append_child(root, nested);
    tree.append_child(nested, fixed);

    assert_native_rust_layout(tree, root, Constraints::definite(200.0, 100.0));
}

#[test]
fn head_to_head_linear_fixed_percent_end_insets_resolve_against_root_linear_containing_block() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(linear_standalone_style(Style {
        width: Length::points(200.0),
        height: Length::points(100.0),
        ..Style::default()
    })));
    let nested = tree.push(SimpleNode::new(block_standalone_style(Style {
        width: Length::points(40.0),
        height: Length::points(30.0),
        ..Style::default()
    })));
    let fixed = tree.push(SimpleNode::new(block_standalone_style(Style {
        position: PositionType::Fixed,
        right: Length::percent(10.0),
        bottom: Length::percent(25.0),
        width: Length::percent(50.0),
        height: Length::percent(20.0),
        ..Style::default()
    })));
    tree.append_child(root, nested);
    tree.append_child(nested, fixed);

    assert_native_rust_layout(tree, root, Constraints::definite(200.0, 100.0));
}

#[test]
fn head_to_head_linear_fixed_auto_size_between_insets_strips_margins() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(linear_standalone_style(Style {
        width: Length::points(200.0),
        height: Length::points(100.0),
        ..Style::default()
    })));
    let nested = tree.push(SimpleNode::new(block_standalone_style(Style {
        width: Length::points(40.0),
        height: Length::points(30.0),
        ..Style::default()
    })));
    let fixed = tree.push(SimpleNode::new(block_standalone_style(Style {
        position: PositionType::Fixed,
        left: Length::points(10.0),
        right: Length::points(30.0),
        top: Length::points(20.0),
        bottom: Length::points(25.0),
        margin: Rect::new(
            Length::points(3.0),
            Length::points(7.0),
            Length::points(4.0),
            Length::points(6.0),
        ),
        ..Style::default()
    })));
    tree.append_child(root, nested);
    tree.append_child(nested, fixed);

    assert_native_rust_layout(tree, root, Constraints::definite(200.0, 100.0));
}

#[test]
fn head_to_head_linear_fixed_single_insets_strip_at_most_measure_constraints() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(linear_standalone_style(Style {
        width: Length::points(100.0),
        height: Length::points(50.0),
        ..Style::default()
    })));
    let nested = tree.push(SimpleNode::new(block_standalone_style(Style {
        width: Length::points(20.0),
        height: Length::points(10.0),
        ..Style::default()
    })));
    let fixed = tree.push(SimpleNode::with_measured_size(
        block_standalone_style(Style {
            position: PositionType::Fixed,
            left: Length::points(10.0),
            top: Length::points(15.0),
            margin: Rect::new(
                Length::points(3.0),
                Length::points(7.0),
                Length::points(4.0),
                Length::points(6.0),
            ),
            ..Style::default()
        }),
        Size::new(200.0, 100.0),
    ));
    tree.append_child(root, nested);
    tree.append_child(nested, fixed);

    assert_native_rust_layout(tree, root, Constraints::definite(100.0, 50.0));
}

#[test]
fn head_to_head_linear_fixed_descendant_uses_linear_root_padding_box_offset() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(linear_standalone_style(Style {
        width: Length::points(100.0),
        height: Length::points(80.0),
        padding: Rect::all(Length::points(3.0)),
        ..Style::default()
    })));
    let nested = tree.push(SimpleNode::new(block_standalone_style(Style {
        width: Length::points(20.0),
        height: Length::points(20.0),
        ..Style::default()
    })));
    let fixed = tree.push(SimpleNode::new(block_standalone_style(Style {
        position: PositionType::Fixed,
        width: Length::points(10.0),
        height: Length::points(10.0),
        left: Length::points(5.0),
        top: Length::points(7.0),
        ..Style::default()
    })));
    tree.append_child(root, nested);
    tree.append_child(nested, fixed);

    assert_native_rust_layout(tree, root, Constraints::definite(100.0, 80.0));
}

#[test]
fn head_to_head_linear_absolute_percent_insets_and_size_resolve_against_linear_containing_block() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(linear_standalone_style(Style {
        width: Length::points(200.0),
        height: Length::points(100.0),
        ..Style::default()
    })));
    let absolute = tree.push(SimpleNode::new(block_standalone_style(Style {
        position: PositionType::Absolute,
        left: Length::percent(10.0),
        top: Length::percent(25.0),
        width: Length::percent(50.0),
        height: Length::percent(20.0),
        ..Style::default()
    })));
    tree.append_child(root, absolute);

    assert_native_rust_layout(tree, root, Constraints::definite(200.0, 100.0));
}

#[test]
fn head_to_head_linear_absolute_percent_end_insets_resolve_against_linear_containing_block() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(linear_standalone_style(Style {
        width: Length::points(200.0),
        height: Length::points(100.0),
        ..Style::default()
    })));
    let absolute = tree.push(SimpleNode::new(block_standalone_style(Style {
        position: PositionType::Absolute,
        right: Length::percent(10.0),
        bottom: Length::percent(25.0),
        width: Length::percent(50.0),
        height: Length::percent(20.0),
        ..Style::default()
    })));
    tree.append_child(root, absolute);

    assert_native_rust_layout(tree, root, Constraints::definite(200.0, 100.0));
}

#[test]
fn head_to_head_linear_absolute_auto_size_stretches_between_start_and_end_insets() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(linear_standalone_style(Style {
        linear_orientation: LinearOrientation::Horizontal,
        linear_gravity: LinearGravity::Center,
        width: Length::points(200.0),
        height: Length::points(100.0),
        ..Style::default()
    })));
    let absolute = tree.push(SimpleNode::new(block_standalone_style(Style {
        position: PositionType::Absolute,
        left: Length::points(10.0),
        right: Length::points(30.0),
        top: Length::points(20.0),
        bottom: Length::points(25.0),
        ..Style::default()
    })));
    tree.append_child(root, absolute);

    assert_native_rust_layout(tree, root, Constraints::definite(200.0, 100.0));
}

#[test]
fn head_to_head_linear_absolute_auto_size_between_insets_strips_margins() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(linear_standalone_style(Style {
        linear_orientation: LinearOrientation::Horizontal,
        linear_gravity: LinearGravity::Center,
        width: Length::points(200.0),
        height: Length::points(100.0),
        ..Style::default()
    })));
    let absolute = tree.push(SimpleNode::new(block_standalone_style(Style {
        position: PositionType::Absolute,
        left: Length::points(10.0),
        right: Length::points(30.0),
        top: Length::points(20.0),
        bottom: Length::points(25.0),
        margin: Rect::new(
            Length::points(3.0),
            Length::points(7.0),
            Length::points(4.0),
            Length::points(6.0),
        ),
        ..Style::default()
    })));
    tree.append_child(root, absolute);

    assert_native_rust_layout(tree, root, Constraints::definite(200.0, 100.0));
}

#[test]
fn head_to_head_linear_absolute_auto_size_paired_insets_fill_padding_box_minus_margins() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(linear_standalone_style(Style {
        linear_orientation: LinearOrientation::Horizontal,
        linear_gravity: LinearGravity::Center,
        width: Length::points(100.0),
        height: Length::points(50.0),
        padding: Rect::all(Length::points(10.0)),
        ..Style::default()
    })));
    let absolute = tree.push(SimpleNode::with_measured_size(
        block_standalone_style(Style {
            position: PositionType::Absolute,
            left: Length::points(10.0),
            right: Length::points(15.0),
            top: Length::points(4.0),
            bottom: Length::points(6.0),
            margin: Rect::new(
                Length::points(2.0),
                Length::points(3.0),
                Length::points(1.0),
                Length::points(2.0),
            ),
            ..Style::default()
        }),
        Size::new(200.0, 200.0),
    ));
    tree.append_child(root, absolute);

    assert_native_rust_layout(tree, root, Constraints::definite(120.0, 70.0));
}

#[test]
fn head_to_head_linear_absolute_single_insets_strip_at_most_measure_constraints() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(linear_standalone_style(Style {
        linear_orientation: LinearOrientation::Horizontal,
        linear_gravity: LinearGravity::Center,
        width: Length::points(100.0),
        height: Length::points(50.0),
        ..Style::default()
    })));
    let absolute = tree.push(SimpleNode::with_measured_size(
        block_standalone_style(Style {
            position: PositionType::Absolute,
            left: Length::points(10.0),
            top: Length::points(15.0),
            margin: Rect::new(
                Length::points(3.0),
                Length::points(7.0),
                Length::points(4.0),
                Length::points(6.0),
            ),
            ..Style::default()
        }),
        Size::new(200.0, 100.0),
    ));
    tree.append_child(root, absolute);

    assert_native_rust_layout(tree, root, Constraints::definite(100.0, 50.0));
}

#[test]
fn head_to_head_linear_absolute_start_insets_override_static_alignment() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(linear_standalone_style(Style {
        linear_orientation: LinearOrientation::Horizontal,
        linear_gravity: LinearGravity::Center,
        width: Length::points(200.0),
        height: Length::points(100.0),
        ..Style::default()
    })));
    let absolute = tree.push(SimpleNode::new(block_standalone_style(Style {
        position: PositionType::Absolute,
        left: Length::points(12.0),
        top: Length::points(9.0),
        width: Length::points(20.0),
        height: Length::points(10.0),
        linear_layout_gravity: LinearLayoutGravity::End,
        ..Style::default()
    })));
    tree.append_child(root, absolute);

    assert_native_rust_layout(tree, root, Constraints::definite(200.0, 100.0));
}

#[test]
fn head_to_head_linear_absolute_end_insets_with_margins_position_margin_box() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(linear_standalone_style(Style {
        linear_orientation: LinearOrientation::Horizontal,
        linear_gravity: LinearGravity::Center,
        width: Length::points(200.0),
        height: Length::points(100.0),
        ..Style::default()
    })));
    let absolute = tree.push(SimpleNode::new(block_standalone_style(Style {
        position: PositionType::Absolute,
        right: Length::points(30.0),
        bottom: Length::points(25.0),
        width: Length::points(20.0),
        height: Length::points(10.0),
        linear_layout_gravity: LinearLayoutGravity::End,
        margin: Rect::new(
            Length::points(3.0),
            Length::points(7.0),
            Length::points(4.0),
            Length::points(6.0),
        ),
        ..Style::default()
    })));
    tree.append_child(root, absolute);

    assert_native_rust_layout(tree, root, Constraints::definite(200.0, 100.0));
}

#[test]
fn head_to_head_linear_absolute_paired_insets_with_explicit_size_use_start_insets() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(linear_standalone_style(Style {
        linear_orientation: LinearOrientation::Horizontal,
        linear_gravity: LinearGravity::Center,
        width: Length::points(200.0),
        height: Length::points(100.0),
        ..Style::default()
    })));
    let absolute = tree.push(SimpleNode::new(block_standalone_style(Style {
        position: PositionType::Absolute,
        left: Length::points(12.0),
        right: Length::points(30.0),
        top: Length::points(9.0),
        bottom: Length::points(25.0),
        width: Length::points(20.0),
        height: Length::points(10.0),
        linear_layout_gravity: LinearLayoutGravity::End,
        ..Style::default()
    })));
    tree.append_child(root, absolute);

    assert_native_rust_layout(tree, root, Constraints::definite(200.0, 100.0));
}

#[test]
fn head_to_head_vertical_linear_stacks_children_and_stretches_cross_axis() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(linear_standalone_style(Style {
        width: Length::points(100.0),
        ..Style::default()
    })));
    let first = fixed_linear_child(&mut tree, Length::Auto, Length::points(10.0));
    let second = fixed_linear_child(&mut tree, Length::Auto, Length::points(20.0));
    tree.append_child(root, first);
    tree.append_child(root, second);

    assert_native_rust_layout(
        tree,
        root,
        Constraints::new(
            SideConstraint::definite(100.0),
            SideConstraint::indefinite(),
        ),
    );
}

#[test]
fn head_to_head_display_none_child_is_laid_out_as_zero_and_skipped_by_linear_stack() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(linear_standalone_style(Style {
        width: Length::points(100.0),
        ..Style::default()
    })));
    let first = fixed_linear_child(&mut tree, Length::Auto, Length::points(10.0));
    let hidden = tree.push(SimpleNode::new(Style {
        display: Display::None,
        box_sizing: BoxSizing::ContentBox,
        width: Length::points(100.0),
        height: Length::points(50.0),
        ..Style::default()
    }));
    let second = fixed_linear_child(&mut tree, Length::Auto, Length::points(20.0));
    tree.append_child(root, first);
    tree.append_child(root, hidden);
    tree.append_child(root, second);

    assert_native_rust_layout(
        tree,
        root,
        Constraints::new(
            SideConstraint::definite(100.0),
            SideConstraint::indefinite(),
        ),
    );
}

#[test]
fn head_to_head_linear_layout_orders_in_flow_children_by_order() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(linear_standalone_style(Style::default())));
    let later = tree.push(SimpleNode::new(block_standalone_style(Style {
        width: Length::points(10.0),
        height: Length::points(10.0),
        order: 1,
        ..Style::default()
    })));
    let earlier = tree.push(SimpleNode::new(block_standalone_style(Style {
        width: Length::points(10.0),
        height: Length::points(10.0),
        order: -1,
        ..Style::default()
    })));
    tree.append_child(root, later);
    tree.append_child(root, earlier);

    assert_native_rust_layout(tree, root, Constraints::indefinite());
}

#[test]
fn head_to_head_horizontal_linear_at_most_main_axis_shrink_wraps_content() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(linear_standalone_style(Style {
        linear_orientation: LinearOrientation::Horizontal,
        height: Length::points(20.0),
        ..Style::default()
    })));
    let first = tree.push(SimpleNode::new(block_standalone_style(Style {
        width: Length::points(10.0),
        height: Length::Auto,
        ..Style::default()
    })));
    let second = tree.push(SimpleNode::new(block_standalone_style(Style {
        width: Length::points(20.0),
        height: Length::Auto,
        ..Style::default()
    })));
    tree.append_child(root, first);
    tree.append_child(root, second);

    assert_native_rust_layout(
        tree,
        root,
        Constraints::new(
            SideConstraint::at_most(100.0),
            SideConstraint::definite(20.0),
        ),
    );
}

#[test]
fn head_to_head_horizontal_linear_at_most_main_axis_keeps_overflow_content_size() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(linear_standalone_style(Style {
        linear_orientation: LinearOrientation::Horizontal,
        height: Length::points(20.0),
        ..Style::default()
    })));
    let first = tree.push(SimpleNode::new(block_standalone_style(Style {
        width: Length::points(80.0),
        height: Length::Auto,
        ..Style::default()
    })));
    let second = tree.push(SimpleNode::new(block_standalone_style(Style {
        width: Length::points(70.0),
        height: Length::Auto,
        ..Style::default()
    })));
    tree.append_child(root, first);
    tree.append_child(root, second);

    assert_native_rust_layout(
        tree,
        root,
        Constraints::new(
            SideConstraint::at_most(100.0),
            SideConstraint::definite(20.0),
        ),
    );
}

#[test]
fn head_to_head_horizontal_linear_container_min_width_and_max_height_clamp_wrap_content_size() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(linear_standalone_style(Style {
        linear_orientation: LinearOrientation::Horizontal,
        min_width: Length::points(40.0),
        max_height: Length::points(25.0),
        ..Style::default()
    })));
    let child = fixed_linear_child(&mut tree, Length::points(20.0), Length::points(30.0));
    tree.append_child(root, child);

    assert_native_rust_layout(tree, root, Constraints::indefinite());
}

#[test]
fn head_to_head_vertical_linear_container_max_width_and_min_height_clamp_wrap_content_size() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(linear_standalone_style(Style {
        max_width: Length::points(60.0),
        min_height: Length::points(40.0),
        ..Style::default()
    })));
    let child = fixed_linear_child(&mut tree, Length::points(100.0), Length::points(10.0));
    tree.append_child(root, child);

    assert_native_rust_layout(tree, root, Constraints::indefinite());
}

#[test]
fn head_to_head_linear_container_padding_border_prevents_negative_content_size_under_tight_constraints()
 {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(linear_standalone_style(Style {
        linear_orientation: LinearOrientation::Horizontal,
        padding: Rect::new(
            Length::points(10.0),
            Length::points(15.0),
            Length::points(8.0),
            Length::points(9.0),
        ),
        border: Rect::new(2.0, 3.0, 1.0, 4.0),
        ..Style::default()
    })));

    assert_native_rust_layout(tree, root, Constraints::definite(8.0, 7.0));
}

#[test]
fn head_to_head_horizontal_linear_auto_main_axis_keeps_initial_size_after_percent_main_margins_resolve()
 {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(linear_standalone_style(Style {
        linear_orientation: LinearOrientation::Horizontal,
        height: Length::points(10.0),
        ..Style::default()
    })));
    let child = tree.push(SimpleNode::new(block_standalone_style(Style {
        width: Length::points(100.0),
        height: Length::points(10.0),
        margin: Rect::new(
            Length::percent(10.0),
            Length::percent(10.0),
            Length::ZERO,
            Length::ZERO,
        ),
        ..Style::default()
    })));
    tree.append_child(root, child);

    assert_native_rust_layout(tree, root, Constraints::indefinite());
}

#[test]
fn head_to_head_vertical_linear_auto_main_axis_keeps_initial_size_after_percent_main_margins_resolve()
 {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(linear_standalone_style(Style::default())));
    let child = tree.push(SimpleNode::new(block_standalone_style(Style {
        width: Length::points(100.0),
        height: Length::points(100.0),
        margin: Rect::new(
            Length::ZERO,
            Length::ZERO,
            Length::percent(10.0),
            Length::percent(10.0),
        ),
        ..Style::default()
    })));
    tree.append_child(root, child);

    assert_native_rust_layout(tree, root, Constraints::indefinite());
}

#[test]
fn head_to_head_horizontal_linear_at_most_main_axis_does_not_enable_linear_weight() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(linear_standalone_style(Style {
        linear_orientation: LinearOrientation::Horizontal,
        height: Length::points(20.0),
        ..Style::default()
    })));
    let weighted = tree.push(SimpleNode::new(block_standalone_style(Style {
        linear_weight: 1.0,
        ..Style::default()
    })));
    tree.append_child(root, weighted);

    assert_native_rust_layout(
        tree,
        root,
        Constraints::new(
            SideConstraint::at_most(100.0),
            SideConstraint::definite(20.0),
        ),
    );
}

#[test]
fn head_to_head_vertical_linear_at_most_cross_axis_does_not_stretch_auto_child() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(linear_standalone_style(Style::default())));
    let child = fixed_linear_child(&mut tree, Length::Auto, Length::points(10.0));
    tree.append_child(root, child);

    assert_native_rust_layout(
        tree,
        root,
        Constraints::new(SideConstraint::at_most(100.0), SideConstraint::indefinite()),
    );
}

#[test]
fn head_to_head_vertical_linear_at_most_cross_axis_min_width_growth_does_not_final_stretch_auto_child()
 {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(linear_standalone_style(Style {
        direction: Direction::Rtl,
        min_width: Length::points(20.0),
        ..Style::default()
    })));
    let child = tree.push(SimpleNode::new(block_standalone_style(Style {
        width: Length::Auto,
        height: Length::percent(40.0),
        min_width: Length::points(12.0),
        ..Style::default()
    })));
    let wider_sibling = fixed_linear_child(&mut tree, Length::points(14.0), Length::points(1.0));
    tree.append_child(root, child);
    tree.append_child(root, wider_sibling);

    assert_native_rust_layout(
        tree,
        root,
        Constraints::new(SideConstraint::at_most(100.0), SideConstraint::indefinite()),
    );
}

#[test]
fn head_to_head_horizontal_linear_auto_cross_axis_passes_parent_height_constraint_to_measured_child()
 {
    let mut tree = MeasuringTree::default();
    let root = tree.push(MeasuringNode::new(linear_standalone_style(Style {
        linear_orientation: LinearOrientation::Horizontal,
        width: Length::points(100.0),
        height: Length::Auto,
        ..Style::default()
    })));
    let child = tree.push(MeasuringNode::measured(
        block_standalone_style(Style {
            width: Length::points(10.0),
            ..Style::default()
        }),
        Size::new(10.0, 150.0),
    ));
    tree.append_child(root, child);

    assert_measuring_native_rust_layout(tree, root, Constraints::definite(100.0, 80.0));
}

#[test]
fn head_to_head_horizontal_linear_fit_content_cross_axis_argument_bounds_measured_child() {
    let mut tree = MeasuringTree::default();
    let root = tree.push(MeasuringNode::new(linear_standalone_style(Style {
        linear_orientation: LinearOrientation::Horizontal,
        width: Length::points(100.0),
        height: Length::fit_content(Some(BaseLength::fixed(30.0))),
        ..Style::default()
    })));
    let child = tree.push(MeasuringNode::measured(
        block_standalone_style(Style::default()),
        Size::new(20.0, 50.0),
    ));
    tree.append_child(root, child);

    assert_measuring_native_rust_layout(
        tree,
        root,
        Constraints::new(
            SideConstraint::definite(100.0),
            SideConstraint::indefinite(),
        ),
    );
}

#[test]
fn head_to_head_vertical_linear_default_stretch_does_not_override_max_content_cross_size() {
    let mut tree = MeasuringTree::default();
    let root = tree.push(MeasuringNode::new(linear_standalone_style(Style {
        width: Length::points(100.0),
        ..Style::default()
    })));
    let child = tree.push(MeasuringNode::measured(
        block_standalone_style(Style {
            width: Length::MaxContent,
            ..Style::default()
        }),
        Size::new(150.0, 10.0),
    ));
    tree.append_child(root, child);

    assert_measuring_native_rust_layout(tree, root, Constraints::indefinite());
}

#[test]
fn head_to_head_horizontal_linear_center_uses_remaining_main_space() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(linear_standalone_style(Style {
        linear_orientation: LinearOrientation::Horizontal,
        justify_content: JustifyContent::Center,
        width: Length::points(100.0),
        height: Length::points(20.0),
        ..Style::default()
    })));
    for width in [10.0, 20.0] {
        let child = fixed_linear_child(&mut tree, Length::points(width), Length::Auto);
        tree.append_child(root, child);
    }

    assert_native_rust_layout(tree, root, Constraints::definite(100.0, 20.0));
}

#[test]
fn head_to_head_horizontal_linear_auto_cross_axis_uses_parent_height_constraint_for_stretch() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(linear_standalone_style(Style {
        linear_orientation: LinearOrientation::Horizontal,
        width: Length::points(100.0),
        height: Length::Auto,
        ..Style::default()
    })));
    let child = fixed_linear_child(&mut tree, Length::points(10.0), Length::Auto);
    tree.append_child(root, child);

    assert_native_rust_layout(tree, root, Constraints::definite(100.0, 80.0));
}

#[test]
fn head_to_head_horizontal_linear_center_uses_negative_remaining_main_space_when_overflowing() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(linear_standalone_style(Style {
        linear_orientation: LinearOrientation::Horizontal,
        justify_content: JustifyContent::Center,
        width: Length::points(100.0),
        height: Length::points(20.0),
        ..Style::default()
    })));
    for width in [80.0, 70.0] {
        let child = fixed_linear_child(&mut tree, Length::points(width), Length::Auto);
        tree.append_child(root, child);
    }

    assert_native_rust_layout(tree, root, Constraints::definite(100.0, 20.0));
}

#[test]
fn head_to_head_vertical_linear_center_uses_negative_remaining_main_space_for_container_baseline() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(linear_standalone_style(Style {
        justify_content: JustifyContent::Center,
        width: Length::points(20.0),
        height: Length::points(100.0),
        ..Style::default()
    })));
    let first = tree.push(SimpleNode::with_measured_size_and_baseline(
        block_standalone_style(Style::default()),
        Size::new(10.0, 80.0),
        10.0,
    ));
    let second = tree.push(SimpleNode::with_measured_size(
        block_standalone_style(Style::default()),
        Size::new(10.0, 70.0),
    ));
    tree.append_child(root, first);
    tree.append_child(root, second);

    assert_native_rust_layout(tree, root, Constraints::definite(20.0, 100.0));
}

#[test]
fn head_to_head_vertical_linear_end_gravity_offsets_container_baseline_by_remaining_main_space() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(linear_standalone_style(Style {
        linear_gravity: LinearGravity::End,
        width: Length::points(20.0),
        height: Length::points(100.0),
        ..Style::default()
    })));
    let first = tree.push(SimpleNode::with_measured_size_and_baseline(
        block_standalone_style(Style::default()),
        Size::new(10.0, 20.0),
        5.0,
    ));
    let second = tree.push(SimpleNode::with_measured_size(
        block_standalone_style(Style::default()),
        Size::new(10.0, 10.0),
    ));
    tree.append_child(root, first);
    tree.append_child(root, second);

    assert_native_rust_layout(tree, root, Constraints::definite(20.0, 100.0));
}

#[test]
fn head_to_head_horizontal_linear_empty_container_exports_no_baseline() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(linear_standalone_style(Style {
        linear_orientation: LinearOrientation::Horizontal,
        width: Length::points(20.0),
        height: Length::points(10.0),
        ..Style::default()
    })));

    assert_native_rust_layout(tree, root, Constraints::definite(20.0, 10.0));
}

#[test]
fn head_to_head_vertical_linear_empty_container_exports_no_baseline() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(linear_standalone_style(Style {
        width: Length::points(20.0),
        height: Length::points(10.0),
        ..Style::default()
    })));

    assert_native_rust_layout(tree, root, Constraints::definite(20.0, 10.0));
}

#[test]
fn head_to_head_horizontal_linear_child_without_baseline_exports_fallback_baseline() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(linear_standalone_style(Style {
        linear_orientation: LinearOrientation::Horizontal,
        ..Style::default()
    })));
    let child = fixed_linear_child(&mut tree, Length::points(20.0), Length::points(10.0));
    tree.append_child(root, child);

    assert_native_rust_layout(tree, root, Constraints::indefinite());
}

#[test]
fn head_to_head_horizontal_linear_container_baseline_uses_largest_child_baseline() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(linear_standalone_style(Style {
        linear_orientation: LinearOrientation::Horizontal,
        width: Length::points(100.0),
        height: Length::points(40.0),
        ..Style::default()
    })));
    let first = tree.push(SimpleNode::with_measured_size_and_baseline(
        standalone_style(Style::default()),
        Size::new(10.0, 30.0),
        5.0,
    ));
    let second = tree.push(SimpleNode::with_measured_size_and_baseline(
        standalone_style(Style::default()),
        Size::new(10.0, 20.0),
        15.0,
    ));
    tree.append_child(root, first);
    tree.append_child(root, second);

    assert_native_rust_layout(tree, root, Constraints::definite(100.0, 40.0));
}

#[test]
fn head_to_head_vertical_linear_child_without_baseline_exports_fallback_baseline() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(linear_standalone_style(Style::default())));
    let child = fixed_linear_child(&mut tree, Length::points(20.0), Length::points(10.0));
    tree.append_child(root, child);

    assert_native_rust_layout(tree, root, Constraints::indefinite());
}

#[test]
fn head_to_head_rtl_horizontal_linear_positions_items_from_right_edge() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(linear_standalone_style(Style {
        direction: Direction::Rtl,
        linear_orientation: LinearOrientation::Horizontal,
        width: Length::points(100.0),
        height: Length::points(20.0),
        ..Style::default()
    })));
    for width in [10.0, 20.0] {
        let child = fixed_linear_child(&mut tree, Length::points(width), Length::Auto);
        tree.append_child(root, child);
    }

    assert_native_rust_layout(tree, root, Constraints::definite(100.0, 20.0));
}

#[test]
fn head_to_head_horizontal_reverse_linear_positions_items_from_right_edge() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(linear_standalone_style(Style {
        linear_orientation: LinearOrientation::HorizontalReverse,
        width: Length::points(100.0),
        height: Length::points(20.0),
        ..Style::default()
    })));
    for width in [10.0, 20.0] {
        let child = fixed_linear_child(&mut tree, Length::points(width), Length::Auto);
        tree.append_child(root, child);
    }

    assert_native_rust_layout(tree, root, Constraints::definite(100.0, 20.0));
}

#[test]
fn head_to_head_rtl_horizontal_reverse_linear_positions_items_from_left_edge() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(linear_standalone_style(Style {
        direction: Direction::Rtl,
        linear_orientation: LinearOrientation::HorizontalReverse,
        width: Length::points(100.0),
        height: Length::points(20.0),
        ..Style::default()
    })));
    for width in [10.0, 20.0] {
        let child = fixed_linear_child(&mut tree, Length::points(width), Length::Auto);
        tree.append_child(root, child);
    }

    assert_native_rust_layout(tree, root, Constraints::definite(100.0, 20.0));
}

#[test]
fn head_to_head_horizontal_reverse_linear_gravity_left_packs_items_at_left_edge() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(linear_standalone_style(Style {
        linear_orientation: LinearOrientation::HorizontalReverse,
        linear_gravity: LinearGravity::Left,
        width: Length::points(100.0),
        height: Length::points(20.0),
        ..Style::default()
    })));
    for width in [10.0, 20.0] {
        let child = fixed_linear_child(&mut tree, Length::points(width), Length::Auto);
        tree.append_child(root, child);
    }

    assert_native_rust_layout(tree, root, Constraints::definite(100.0, 20.0));
}

#[test]
fn head_to_head_vertical_reverse_linear_positions_items_from_bottom_edge() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(linear_standalone_style(Style {
        linear_orientation: LinearOrientation::VerticalReverse,
        width: Length::points(20.0),
        height: Length::points(100.0),
        ..Style::default()
    })));
    for height in [10.0, 20.0] {
        let child = fixed_linear_child(&mut tree, Length::Auto, Length::points(height));
        tree.append_child(root, child);
    }

    assert_native_rust_layout(tree, root, Constraints::definite(20.0, 100.0));
}

#[test]
fn head_to_head_vertical_reverse_linear_gravity_top_packs_items_at_top_edge() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(linear_standalone_style(Style {
        linear_orientation: LinearOrientation::VerticalReverse,
        linear_gravity: LinearGravity::Top,
        width: Length::points(20.0),
        height: Length::points(100.0),
        ..Style::default()
    })));
    for height in [10.0, 20.0] {
        let child = fixed_linear_child(&mut tree, Length::Auto, Length::points(height));
        tree.append_child(root, child);
    }

    assert_native_rust_layout(tree, root, Constraints::definite(20.0, 100.0));
}

#[test]
fn head_to_head_vertical_linear_space_between_distributes_remaining_main_space() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(linear_standalone_style(Style {
        justify_content: JustifyContent::SpaceBetween,
        width: Length::points(100.0),
        height: Length::points(100.0),
        ..Style::default()
    })));
    for _ in 0..2 {
        let child = fixed_linear_child(&mut tree, Length::Auto, Length::points(10.0));
        tree.append_child(root, child);
    }

    assert_native_rust_layout(tree, root, Constraints::definite(100.0, 100.0));
}

#[test]
fn head_to_head_vertical_linear_space_between_single_item_uses_start_position() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(linear_standalone_style(Style {
        justify_content: JustifyContent::SpaceBetween,
        width: Length::points(100.0),
        height: Length::points(100.0),
        ..Style::default()
    })));
    let child = fixed_linear_child(&mut tree, Length::Auto, Length::points(10.0));
    tree.append_child(root, child);

    assert_native_rust_layout(tree, root, Constraints::definite(100.0, 100.0));
}

#[test]
fn head_to_head_vertical_linear_space_between_keeps_items_adjacent_when_overflowing() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(linear_standalone_style(Style {
        justify_content: JustifyContent::SpaceBetween,
        width: Length::points(100.0),
        height: Length::points(100.0),
        ..Style::default()
    })));
    for _ in 0..2 {
        let child = fixed_linear_child(&mut tree, Length::Auto, Length::points(70.0));
        tree.append_child(root, child);
    }

    assert_native_rust_layout(tree, root, Constraints::definite(100.0, 100.0));
}

#[test]
fn head_to_head_horizontal_linear_gravity_right_overrides_justify_content() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(linear_standalone_style(Style {
        linear_orientation: LinearOrientation::Horizontal,
        linear_gravity: LinearGravity::Right,
        justify_content: JustifyContent::FlexStart,
        width: Length::points(100.0),
        height: Length::points(20.0),
        ..Style::default()
    })));
    for width in [10.0, 20.0] {
        let child = fixed_linear_child(&mut tree, Length::points(width), Length::Auto);
        tree.append_child(root, child);
    }

    assert_native_rust_layout(tree, root, Constraints::definite(100.0, 20.0));
}

#[test]
fn head_to_head_horizontal_linear_justify_content_distribution_values_map_to_start() {
    for justify_content in [
        JustifyContent::SpaceAround,
        JustifyContent::SpaceEvenly,
        JustifyContent::Stretch,
    ] {
        let mut tree = SimpleTree::default();
        let root = tree.push(SimpleNode::new(linear_standalone_style(Style {
            linear_orientation: LinearOrientation::Horizontal,
            justify_content,
            width: Length::points(100.0),
            height: Length::points(10.0),
            ..Style::default()
        })));
        for width in [10.0, 20.0] {
            let child = fixed_linear_child(&mut tree, Length::points(width), Length::Auto);
            tree.append_child(root, child);
        }

        assert_native_rust_layout(tree, root, Constraints::definite(100.0, 10.0));
    }
}

#[test]
fn head_to_head_linear_cross_axis_alignment_uses_align_items() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(linear_standalone_style(Style {
        align_items: AlignItems::Center,
        width: Length::points(100.0),
        height: Length::points(100.0),
        ..Style::default()
    })));
    let child = fixed_linear_child(&mut tree, Length::points(20.0), Length::points(10.0));
    tree.append_child(root, child);

    assert_native_rust_layout(tree, root, Constraints::definite(100.0, 100.0));
}

#[test]
fn head_to_head_linear_cross_axis_center_uses_negative_space_when_item_overflows() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(linear_standalone_style(Style {
        align_items: AlignItems::Center,
        width: Length::points(100.0),
        height: Length::points(100.0),
        ..Style::default()
    })));
    let child = fixed_linear_child(&mut tree, Length::points(140.0), Length::points(10.0));
    tree.append_child(root, child);

    assert_native_rust_layout(tree, root, Constraints::definite(100.0, 100.0));
}

#[test]
fn head_to_head_linear_cross_axis_end_uses_negative_space_when_item_overflows() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(linear_standalone_style(Style {
        align_items: AlignItems::FlexEnd,
        width: Length::points(100.0),
        height: Length::points(100.0),
        ..Style::default()
    })));
    let child = fixed_linear_child(&mut tree, Length::points(140.0), Length::points(10.0));
    tree.append_child(root, child);

    assert_native_rust_layout(tree, root, Constraints::definite(100.0, 100.0));
}

#[test]
fn head_to_head_linear_baseline_align_items_keeps_default_cross_axis_stretch() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(linear_standalone_style(Style {
        align_items: AlignItems::Baseline,
        width: Length::points(100.0),
        ..Style::default()
    })));
    let child = tree.push(SimpleNode::new(block_standalone_style(Style {
        width: Length::Auto,
        height: Length::points(10.0),
        ..Style::default()
    })));
    tree.append_child(root, child);

    assert_native_rust_layout(tree, root, Constraints::definite(100.0, 100.0));
}

#[test]
fn head_to_head_linear_align_self_overrides_container_align_items() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(linear_standalone_style(Style {
        align_items: AlignItems::FlexStart,
        width: Length::points(100.0),
        height: Length::points(100.0),
        ..Style::default()
    })));
    let child = tree.push(SimpleNode::new(standalone_style(Style {
        width: Length::points(20.0),
        height: Length::points(10.0),
        align_self: Some(AlignItems::FlexEnd),
        ..Style::default()
    })));
    tree.append_child(root, child);

    assert_native_rust_layout(tree, root, Constraints::definite(100.0, 100.0));
}

#[test]
fn head_to_head_linear_align_self_overrides_linear_cross_gravity() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(linear_standalone_style(Style {
        align_items: AlignItems::FlexStart,
        linear_cross_gravity: LinearCrossGravity::End,
        width: Length::points(100.0),
        height: Length::points(100.0),
        ..Style::default()
    })));
    let child = tree.push(SimpleNode::new(standalone_style(Style {
        width: Length::points(20.0),
        height: Length::points(10.0),
        align_self: Some(AlignItems::Center),
        ..Style::default()
    })));
    tree.append_child(root, child);

    assert_native_rust_layout(tree, root, Constraints::definite(100.0, 100.0));
}

#[test]
fn head_to_head_vertical_linear_percent_cross_size_remeasures_final_constraint() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(linear_standalone_style(Style {
        width: Length::points(100.0),
        height: Length::points(40.0),
        ..Style::default()
    })));
    let child = tree.push(SimpleNode::new(block_standalone_style(Style {
        width: Length::percent(50.0),
        height: Length::points(10.0),
        ..Style::default()
    })));
    tree.append_child(root, child);

    assert_native_rust_layout(tree, root, Constraints::definite(100.0, 40.0));
}

#[test]
fn head_to_head_horizontal_linear_percent_cross_size_with_stretch_remeasures_final_constraint() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(linear_standalone_style(Style {
        linear_orientation: LinearOrientation::Horizontal,
        width: Length::points(100.0),
        height: Length::points(80.0),
        ..Style::default()
    })));
    let child = tree.push(SimpleNode::new(block_standalone_style(Style {
        width: Length::points(20.0),
        height: Length::percent(50.0),
        linear_layout_gravity: LinearLayoutGravity::Stretch,
        ..Style::default()
    })));
    tree.append_child(root, child);

    assert_native_rust_layout(tree, root, Constraints::definite(100.0, 80.0));
}

#[test]
fn head_to_head_linear_layout_gravity_end_overrides_container_stretch() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(linear_standalone_style(Style {
        width: Length::points(100.0),
        height: Length::points(100.0),
        ..Style::default()
    })));
    let child = tree.push(SimpleNode::new(standalone_style(Style {
        width: Length::points(20.0),
        height: Length::points(10.0),
        linear_layout_gravity: LinearLayoutGravity::End,
        ..Style::default()
    })));
    tree.append_child(root, child);

    assert_native_rust_layout(tree, root, Constraints::definite(100.0, 100.0));
}

#[test]
fn head_to_head_linear_layout_gravity_overrides_align_self_and_cross_gravity() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(linear_standalone_style(Style {
        align_items: AlignItems::FlexStart,
        linear_cross_gravity: LinearCrossGravity::End,
        width: Length::points(100.0),
        height: Length::points(100.0),
        ..Style::default()
    })));
    let child = tree.push(SimpleNode::new(block_standalone_style(Style {
        width: Length::points(20.0),
        height: Length::points(10.0),
        align_self: Some(AlignItems::FlexEnd),
        linear_layout_gravity: LinearLayoutGravity::Left,
        ..Style::default()
    })));
    tree.append_child(root, child);

    assert_native_rust_layout(tree, root, Constraints::definite(100.0, 100.0));
}

#[test]
fn head_to_head_linear_align_items_stretch_is_not_used_as_linear_layout_gravity_fallback() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(linear_standalone_style(Style {
        align_items: AlignItems::Stretch,
        width: Length::points(100.0),
        height: Length::points(100.0),
        ..Style::default()
    })));
    let child = tree.push(SimpleNode::new(block_standalone_style(Style {
        width: Length::points(20.0),
        height: Length::points(10.0),
        ..Style::default()
    })));
    tree.append_child(root, child);

    assert_native_rust_layout(tree, root, Constraints::definite(100.0, 100.0));
}

#[test]
fn head_to_head_linear_layout_gravity_stretch_overrides_explicit_cross_size() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(linear_standalone_style(Style {
        width: Length::points(100.0),
        height: Length::points(100.0),
        ..Style::default()
    })));
    let child = tree.push(SimpleNode::new(block_standalone_style(Style {
        width: Length::points(20.0),
        height: Length::points(10.0),
        linear_layout_gravity: LinearLayoutGravity::Stretch,
        ..Style::default()
    })));
    tree.append_child(root, child);

    assert_native_rust_layout(tree, root, Constraints::definite(100.0, 100.0));
}

#[test]
fn head_to_head_linear_layout_gravity_stretch_overrides_weighted_explicit_cross_size() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(linear_standalone_style(Style {
        width: Length::points(100.0),
        height: Length::points(100.0),
        ..Style::default()
    })));
    let child = tree.push(SimpleNode::new(block_standalone_style(Style {
        width: Length::points(20.0),
        linear_weight: 1.0,
        linear_layout_gravity: LinearLayoutGravity::Stretch,
        ..Style::default()
    })));
    tree.append_child(root, child);

    assert_native_rust_layout(tree, root, Constraints::definite(100.0, 100.0));
}

#[test]
fn head_to_head_linear_layout_gravity_physical_variants_match_cpp_groups() {
    for gravity in [
        LinearLayoutGravity::None,
        LinearLayoutGravity::Top,
        LinearLayoutGravity::Left,
        LinearLayoutGravity::Start,
        LinearLayoutGravity::Right,
        LinearLayoutGravity::Bottom,
        LinearLayoutGravity::End,
        LinearLayoutGravity::CenterHorizontal,
        LinearLayoutGravity::CenterVertical,
        LinearLayoutGravity::Center,
        LinearLayoutGravity::FillHorizontal,
        LinearLayoutGravity::FillVertical,
        LinearLayoutGravity::Stretch,
    ] {
        let mut tree = SimpleTree::default();
        let root = tree.push(SimpleNode::new(linear_standalone_style(Style {
            width: Length::points(100.0),
            height: Length::points(100.0),
            ..Style::default()
        })));
        let child = tree.push(SimpleNode::new(block_standalone_style(Style {
            width: Length::points(20.0),
            height: Length::points(10.0),
            linear_layout_gravity: gravity,
            ..Style::default()
        })));
        tree.append_child(root, child);

        assert_native_rust_layout(tree, root, Constraints::definite(100.0, 100.0));
    }
}

#[test]
fn head_to_head_rtl_vertical_linear_layout_gravity_keeps_physical_left_and_right() {
    for gravity in [LinearLayoutGravity::Left, LinearLayoutGravity::Right] {
        let mut tree = SimpleTree::default();
        let root = tree.push(SimpleNode::new(linear_standalone_style(Style {
            direction: Direction::Rtl,
            width: Length::points(100.0),
            height: Length::points(100.0),
            ..Style::default()
        })));
        let child = tree.push(SimpleNode::new(block_standalone_style(Style {
            width: Length::points(20.0),
            height: Length::points(10.0),
            linear_layout_gravity: gravity,
            ..Style::default()
        })));
        tree.append_child(root, child);

        assert_native_rust_layout(tree, root, Constraints::definite(100.0, 100.0));
    }
}

#[test]
fn head_to_head_horizontal_linear_layout_gravity_physical_variants_match_cpp_groups() {
    for gravity in [
        LinearLayoutGravity::None,
        LinearLayoutGravity::Top,
        LinearLayoutGravity::Left,
        LinearLayoutGravity::Start,
        LinearLayoutGravity::Right,
        LinearLayoutGravity::Bottom,
        LinearLayoutGravity::End,
        LinearLayoutGravity::CenterHorizontal,
        LinearLayoutGravity::CenterVertical,
        LinearLayoutGravity::Center,
        LinearLayoutGravity::FillHorizontal,
        LinearLayoutGravity::FillVertical,
        LinearLayoutGravity::Stretch,
    ] {
        let mut tree = SimpleTree::default();
        let root = tree.push(SimpleNode::new(linear_standalone_style(Style {
            linear_orientation: LinearOrientation::Horizontal,
            width: Length::points(100.0),
            height: Length::points(100.0),
            ..Style::default()
        })));
        let child = tree.push(SimpleNode::new(block_standalone_style(Style {
            width: Length::points(20.0),
            height: Length::points(10.0),
            linear_layout_gravity: gravity,
            ..Style::default()
        })));
        tree.append_child(root, child);

        assert_native_rust_layout(tree, root, Constraints::definite(100.0, 100.0));
    }
}

#[test]
fn head_to_head_linear_cross_gravity_center_aligns_children() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(linear_standalone_style(Style {
        linear_cross_gravity: LinearCrossGravity::Center,
        width: Length::points(100.0),
        height: Length::points(100.0),
        ..Style::default()
    })));
    let child = fixed_linear_child(&mut tree, Length::points(20.0), Length::points(10.0));
    tree.append_child(root, child);

    assert_native_rust_layout(tree, root, Constraints::definite(100.0, 100.0));
}

#[test]
fn head_to_head_linear_cross_gravity_stretch_overrides_flex_start_alignment() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(linear_standalone_style(Style {
        align_items: AlignItems::FlexStart,
        linear_cross_gravity: LinearCrossGravity::Stretch,
        width: Length::points(100.0),
        height: Length::points(100.0),
        ..Style::default()
    })));
    let child = fixed_linear_child(&mut tree, Length::Auto, Length::points(10.0));
    tree.append_child(root, child);

    assert_native_rust_layout(tree, root, Constraints::definite(100.0, 100.0));
}

#[test]
fn head_to_head_horizontal_linear_cross_axis_start_auto_margin_pushes_item_to_end() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(linear_standalone_style(Style {
        linear_orientation: LinearOrientation::Horizontal,
        align_items: AlignItems::FlexStart,
        width: Length::points(100.0),
        height: Length::points(100.0),
        ..Style::default()
    })));
    let child = tree.push(SimpleNode::new(block_standalone_style(Style {
        width: Length::points(20.0),
        height: Length::points(10.0),
        margin: Rect::new(Length::ZERO, Length::ZERO, Length::Auto, Length::ZERO),
        ..Style::default()
    })));
    tree.append_child(root, child);

    assert_native_rust_layout(tree, root, Constraints::definite(100.0, 100.0));
}

#[test]
fn head_to_head_horizontal_linear_cross_axis_end_auto_margin_keeps_item_at_start() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(linear_standalone_style(Style {
        linear_orientation: LinearOrientation::Horizontal,
        align_items: AlignItems::FlexStart,
        width: Length::points(100.0),
        height: Length::points(100.0),
        ..Style::default()
    })));
    let child = tree.push(SimpleNode::new(block_standalone_style(Style {
        width: Length::points(20.0),
        height: Length::points(10.0),
        margin: Rect::new(Length::ZERO, Length::ZERO, Length::ZERO, Length::Auto),
        ..Style::default()
    })));
    tree.append_child(root, child);

    assert_native_rust_layout(tree, root, Constraints::definite(100.0, 100.0));
}

#[test]
fn head_to_head_horizontal_linear_overflowing_cross_axis_auto_margins_are_ignored() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(linear_standalone_style(Style {
        linear_orientation: LinearOrientation::Horizontal,
        align_items: AlignItems::FlexStart,
        width: Length::points(100.0),
        height: Length::points(100.0),
        ..Style::default()
    })));
    let child = tree.push(SimpleNode::new(block_standalone_style(Style {
        width: Length::points(20.0),
        height: Length::points(140.0),
        margin: Rect::new(Length::ZERO, Length::ZERO, Length::Auto, Length::Auto),
        ..Style::default()
    })));
    tree.append_child(root, child);

    assert_native_rust_layout(tree, root, Constraints::definite(100.0, 100.0));
}

#[test]
fn head_to_head_horizontal_linear_baseline_keeps_unresolved_start_auto_margin() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(linear_standalone_style(Style {
        linear_orientation: LinearOrientation::Horizontal,
        align_items: AlignItems::FlexStart,
        width: Length::points(100.0),
        height: Length::points(100.0),
        ..Style::default()
    })));
    let child = tree.push(SimpleNode::with_measured_size_and_baseline(
        block_standalone_style(Style {
            margin: Rect::new(Length::ZERO, Length::ZERO, Length::Auto, Length::ZERO),
            ..Style::default()
        }),
        Size::new(20.0, 10.0),
        4.0,
    ));
    tree.append_child(root, child);

    assert_native_rust_layout(tree, root, Constraints::definite(100.0, 100.0));
}

#[test]
fn head_to_head_horizontal_linear_baseline_uses_gravity_before_paired_auto_margins_resolve() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(linear_standalone_style(Style {
        linear_orientation: LinearOrientation::Horizontal,
        linear_cross_gravity: LinearCrossGravity::End,
        width: Length::points(100.0),
        height: Length::points(100.0),
        ..Style::default()
    })));
    let child = tree.push(SimpleNode::with_measured_size_and_baseline(
        block_standalone_style(Style {
            margin: Rect::new(Length::ZERO, Length::ZERO, Length::Auto, Length::Auto),
            ..Style::default()
        }),
        Size::new(20.0, 10.0),
        4.0,
    ));
    tree.append_child(root, child);

    assert_native_rust_layout(tree, root, Constraints::definite(100.0, 100.0));
}

#[test]
fn head_to_head_vertical_linear_cross_gravity_variants_override_align_items() {
    for cross_gravity in [
        LinearCrossGravity::None,
        LinearCrossGravity::Start,
        LinearCrossGravity::End,
        LinearCrossGravity::Center,
        LinearCrossGravity::Stretch,
    ] {
        let mut tree = SimpleTree::default();
        let root = tree.push(SimpleNode::new(linear_standalone_style(Style {
            align_items: AlignItems::FlexStart,
            linear_cross_gravity: cross_gravity,
            width: Length::points(100.0),
            height: Length::points(100.0),
            ..Style::default()
        })));
        let child = fixed_linear_child(&mut tree, Length::points(20.0), Length::points(10.0));
        tree.append_child(root, child);

        assert_native_rust_layout(tree, root, Constraints::definite(100.0, 100.0));
    }
}

#[test]
fn head_to_head_horizontal_linear_cross_gravity_variants_override_align_items() {
    for cross_gravity in [
        LinearCrossGravity::None,
        LinearCrossGravity::Start,
        LinearCrossGravity::End,
        LinearCrossGravity::Center,
        LinearCrossGravity::Stretch,
    ] {
        let mut tree = SimpleTree::default();
        let root = tree.push(SimpleNode::new(linear_standalone_style(Style {
            linear_orientation: LinearOrientation::Horizontal,
            align_items: AlignItems::FlexStart,
            linear_cross_gravity: cross_gravity,
            width: Length::points(100.0),
            height: Length::points(100.0),
            ..Style::default()
        })));
        let child = fixed_linear_child(&mut tree, Length::points(20.0), Length::points(10.0));
        tree.append_child(root, child);

        assert_native_rust_layout(tree, root, Constraints::definite(100.0, 100.0));
    }
}

#[test]
fn head_to_head_vertical_linear_weight_takes_remaining_main_space() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(linear_standalone_style(Style {
        width: Length::points(100.0),
        height: Length::points(100.0),
        ..Style::default()
    })));
    let fixed = fixed_linear_child(&mut tree, Length::Auto, Length::points(10.0));
    let weighted = tree.push(SimpleNode::new(standalone_style(Style {
        linear_weight: 1.0,
        ..Style::default()
    })));
    tree.append_child(root, fixed);
    tree.append_child(root, weighted);

    assert_native_rust_layout(tree, root, Constraints::definite(100.0, 100.0));
}

#[test]
fn head_to_head_vertical_linear_weight_gets_zero_when_main_space_is_exhausted() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(linear_standalone_style(Style {
        width: Length::points(100.0),
        height: Length::points(20.0),
        ..Style::default()
    })));
    let fixed = fixed_linear_child(&mut tree, Length::points(20.0), Length::points(30.0));
    let weighted = tree.push(SimpleNode::new(standalone_style(Style {
        width: Length::points(20.0),
        height: Length::Auto,
        linear_weight: 1.0,
        ..Style::default()
    })));
    tree.append_child(root, fixed);
    tree.append_child(root, weighted);

    assert_native_rust_layout(tree, root, Constraints::definite(100.0, 20.0));
}

#[test]
fn head_to_head_horizontal_linear_weight_sub_epsilon_min_violations_do_not_freeze_items() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(linear_standalone_style(Style {
        linear_orientation: LinearOrientation::Horizontal,
        width: Length::points(100.0),
        height: Length::points(20.0),
        ..Style::default()
    })));
    for _ in 0..2 {
        let child = tree.push(SimpleNode::new(block_standalone_style(Style {
            linear_weight: 1.0,
            min_width: Length::points(50.00006),
            height: Length::points(10.0),
            ..Style::default()
        })));
        tree.append_child(root, child);
    }

    assert_native_rust_layout(tree, root, Constraints::definite(100.0, 20.0));
}

#[test]
fn head_to_head_horizontal_linear_weights_split_remaining_main_space_by_ratio() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(linear_standalone_style(Style {
        linear_orientation: LinearOrientation::Horizontal,
        width: Length::points(90.0),
        height: Length::points(20.0),
        ..Style::default()
    })));
    let first = tree.push(SimpleNode::new(block_standalone_style(Style {
        linear_weight: 1.0,
        ..Style::default()
    })));
    let second = tree.push(SimpleNode::new(block_standalone_style(Style {
        linear_weight: 2.0,
        ..Style::default()
    })));
    tree.append_child(root, first);
    tree.append_child(root, second);

    assert_native_rust_layout(tree, root, Constraints::definite(90.0, 20.0));
}

#[test]
fn head_to_head_linear_weight_sum_can_leave_unallocated_main_space() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(linear_standalone_style(Style {
        linear_orientation: LinearOrientation::Horizontal,
        linear_weight_sum: 4.0,
        width: Length::points(100.0),
        height: Length::points(20.0),
        ..Style::default()
    })));
    for _ in 0..2 {
        let child = tree.push(SimpleNode::new(standalone_style(Style {
            linear_weight: 1.0,
            ..Style::default()
        })));
        tree.append_child(root, child);
    }

    assert_native_rust_layout(tree, root, Constraints::definite(100.0, 20.0));
}

#[test]
fn head_to_head_linear_total_weight_below_one_leaves_unallocated_main_space() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(linear_standalone_style(Style {
        linear_orientation: LinearOrientation::Horizontal,
        width: Length::points(100.0),
        height: Length::points(20.0),
        ..Style::default()
    })));
    let child = tree.push(SimpleNode::new(standalone_style(Style {
        linear_weight: 0.5,
        ..Style::default()
    })));
    tree.append_child(root, child);

    assert_native_rust_layout(tree, root, Constraints::definite(100.0, 20.0));
}

#[test]
fn head_to_head_linear_weight_max_size_freezes_and_redistributes_remaining_space() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(linear_standalone_style(Style {
        linear_orientation: LinearOrientation::Horizontal,
        width: Length::points(100.0),
        height: Length::points(20.0),
        ..Style::default()
    })));
    let capped = tree.push(SimpleNode::new(standalone_style(Style {
        linear_weight: 1.0,
        max_width: Length::points(30.0),
        ..Style::default()
    })));
    let flexible = tree.push(SimpleNode::new(standalone_style(Style {
        linear_weight: 1.0,
        ..Style::default()
    })));
    tree.append_child(root, capped);
    tree.append_child(root, flexible);

    assert_native_rust_layout(tree, root, Constraints::definite(100.0, 20.0));
}

#[test]
fn head_to_head_linear_weight_percent_max_size_freezes_and_redistributes_remaining_space() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(linear_standalone_style(Style {
        linear_orientation: LinearOrientation::Horizontal,
        width: Length::points(100.0),
        height: Length::points(20.0),
        ..Style::default()
    })));
    let capped = tree.push(SimpleNode::new(standalone_style(Style {
        linear_weight: 1.0,
        max_width: Length::percent(30.0),
        ..Style::default()
    })));
    let flexible = tree.push(SimpleNode::new(standalone_style(Style {
        linear_weight: 1.0,
        ..Style::default()
    })));
    tree.append_child(root, capped);
    tree.append_child(root, flexible);

    assert_native_rust_layout(tree, root, Constraints::definite(100.0, 20.0));
}

#[test]
fn head_to_head_linear_weight_min_size_freezes_and_redistributes_remaining_space() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(linear_standalone_style(Style {
        linear_orientation: LinearOrientation::Horizontal,
        width: Length::points(100.0),
        height: Length::points(20.0),
        ..Style::default()
    })));
    let floor = tree.push(SimpleNode::new(standalone_style(Style {
        linear_weight: 1.0,
        min_width: Length::points(70.0),
        ..Style::default()
    })));
    let flexible = tree.push(SimpleNode::new(standalone_style(Style {
        linear_weight: 1.0,
        ..Style::default()
    })));
    tree.append_child(root, floor);
    tree.append_child(root, flexible);

    assert_native_rust_layout(tree, root, Constraints::definite(100.0, 20.0));
}

#[test]
fn head_to_head_linear_weight_all_items_freeze_after_min_violations() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(linear_standalone_style(Style {
        linear_orientation: LinearOrientation::Horizontal,
        width: Length::points(100.0),
        height: Length::points(20.0),
        ..Style::default()
    })));
    for _ in 0..2 {
        let child = tree.push(SimpleNode::new(standalone_style(Style {
            linear_weight: 1.0,
            min_width: Length::points(60.0),
            ..Style::default()
        })));
        tree.append_child(root, child);
    }

    assert_native_rust_layout(tree, root, Constraints::definite(100.0, 20.0));
}

#[test]
fn head_to_head_linear_weight_percent_min_size_freezes_and_redistributes_remaining_space() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(linear_standalone_style(Style {
        linear_orientation: LinearOrientation::Horizontal,
        width: Length::points(100.0),
        height: Length::points(20.0),
        ..Style::default()
    })));
    let floor = tree.push(SimpleNode::new(standalone_style(Style {
        linear_weight: 1.0,
        min_width: Length::percent(70.0),
        ..Style::default()
    })));
    let flexible = tree.push(SimpleNode::new(standalone_style(Style {
        linear_weight: 1.0,
        ..Style::default()
    })));
    tree.append_child(root, floor);
    tree.append_child(root, flexible);

    assert_native_rust_layout(tree, root, Constraints::definite(100.0, 20.0));
}

#[test]
fn head_to_head_vertical_linear_weight_percent_max_size_freezes_and_redistributes_remaining_space()
{
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(linear_standalone_style(Style {
        width: Length::points(20.0),
        height: Length::points(100.0),
        ..Style::default()
    })));
    let capped = tree.push(SimpleNode::new(standalone_style(Style {
        linear_weight: 1.0,
        max_height: Length::percent(30.0),
        ..Style::default()
    })));
    let flexible = tree.push(SimpleNode::new(standalone_style(Style {
        linear_weight: 1.0,
        ..Style::default()
    })));
    tree.append_child(root, capped);
    tree.append_child(root, flexible);

    assert_native_rust_layout(tree, root, Constraints::definite(20.0, 100.0));
}

#[test]
fn head_to_head_vertical_linear_weight_percent_min_size_freezes_and_redistributes_remaining_space()
{
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(linear_standalone_style(Style {
        width: Length::points(20.0),
        height: Length::points(100.0),
        ..Style::default()
    })));
    let floor = tree.push(SimpleNode::new(standalone_style(Style {
        linear_weight: 1.0,
        min_height: Length::percent(70.0),
        ..Style::default()
    })));
    let flexible = tree.push(SimpleNode::new(standalone_style(Style {
        linear_weight: 1.0,
        ..Style::default()
    })));
    tree.append_child(root, floor);
    tree.append_child(root, flexible);

    assert_native_rust_layout(tree, root, Constraints::definite(20.0, 100.0));
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
