//! Rust-only subset of PR #25's `starlight_standalone` API.
//!
//! The standalone head-to-head source fixtures build their trees through a
//! mutable convenience API.  Keeping that API here lets the migrated tests
//! retain their exact Rust builders and matrix rows while lowering layout to
//! neutron-star through the PR #25 compatibility facade.  The C++ baseline is
//! deliberately not part of this module.

#![allow(dead_code)]

use std::fmt;

use neutron_star::compute::round_layout;
use neutron_star::geometry::{Edges as NeutronEdges, Point as NeutronPoint, Size as NeutronSize};
use neutron_star::tree::{
    Layout as NeutronLayout, NodeId as NeutronNodeId, RoundState, TraverseTree,
};

pub(crate) use crate::pr25_support::{
    AlignItems, BaseLength, BoxSizing, Constraints, Direction, Display, JustifyContent, Length,
    LinearCrossGravity, LinearGravity, LinearLayoutGravity, LinearOrientation, PositionType, Rect,
    SideConstraint, Size, Style,
};
use crate::pr25_support::{LayoutEngine, LayoutResult, LayoutTree};

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub(crate) struct NodeId(usize);

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum TreeError {
    MissingNode(NodeId),
    CannotParentNodeToItself(NodeId),
}

impl fmt::Display for TreeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingNode(node) => write!(formatter, "missing standalone node {node:?}"),
            Self::CannotParentNodeToItself(node) => {
                write!(formatter, "node {node:?} cannot be parented to itself")
            }
        }
    }
}

impl std::error::Error for TreeError {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum StandaloneEdge {
    Left,
    Right,
    Top,
    Bottom,
    Start,
    End,
    Horizontal,
    Vertical,
    All,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum StandaloneGap {
    Column,
    Row,
    All,
}

pub(crate) fn standalone_default_style() -> Style {
    Style {
        display: Display::Flex,
        position: PositionType::Relative,
        box_sizing: BoxSizing::ContentBox,
        ..Style::default()
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct StandaloneConfig {
    physical_pixels_per_layout_unit: f32,
}

impl StandaloneConfig {
    pub(crate) const fn with_physical_pixels_per_layout_unit(
        physical_pixels_per_layout_unit: f32,
    ) -> Self {
        Self {
            physical_pixels_per_layout_unit,
        }
    }
}

impl Default for StandaloneConfig {
    fn default() -> Self {
        Self {
            physical_pixels_per_layout_unit: 1.0,
        }
    }
}

type StandaloneMeasureFunc = fn(Constraints) -> Size;

#[derive(Clone, Copy, Debug)]
enum StandaloneMeasurement {
    Static(Size),
    Callback(StandaloneMeasureFunc),
}

#[derive(Clone, Debug)]
struct StandaloneNode {
    config: StandaloneConfig,
    style: Style,
    layout: LayoutResult,
    parent: Option<NodeId>,
    children: Vec<NodeId>,
    measurement: Option<StandaloneMeasurement>,
    baseline: Option<f32>,
}

#[derive(Clone, Debug)]
struct StandaloneTopology {
    children: Vec<Vec<NeutronNodeId>>,
}

impl From<&StandaloneTree> for StandaloneTopology {
    fn from(tree: &StandaloneTree) -> Self {
        Self {
            children: tree
                .nodes
                .iter()
                .map(|node| {
                    node.children
                        .iter()
                        .map(|child| NeutronNodeId::from(child.0))
                        .collect()
                })
                .collect(),
        }
    }
}

impl TraverseTree for StandaloneTopology {
    type ChildIter<'a> = std::iter::Copied<std::slice::Iter<'a, NeutronNodeId>>;

    fn child_ids(&self, parent: NeutronNodeId) -> Self::ChildIter<'_> {
        self.children[usize::from(parent)].iter().copied()
    }

    fn child_count(&self, parent: NeutronNodeId) -> usize {
        self.children[usize::from(parent)].len()
    }

    fn child_id(&self, parent: NeutronNodeId, index: usize) -> NeutronNodeId {
        self.children[usize::from(parent)][index]
    }
}

impl StandaloneNode {
    fn new(style: Style, config: StandaloneConfig) -> Self {
        Self {
            config,
            style,
            layout: LayoutResult::default(),
            parent: None,
            children: Vec::new(),
            measurement: None,
            baseline: None,
        }
    }

    fn measure(&self, constraints: Constraints) -> Option<Size> {
        match self.measurement {
            Some(StandaloneMeasurement::Static(size)) => Some(size),
            Some(StandaloneMeasurement::Callback(measure)) => Some(measure(constraints)),
            None => None,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub(crate) struct StandaloneTree {
    nodes: Vec<StandaloneNode>,
}

impl StandaloneTree {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn create_default_node(&mut self) -> NodeId {
        self.create_default_node_with_config(StandaloneConfig::default())
    }

    pub(crate) fn create_default_node_with_config(&mut self, config: StandaloneConfig) -> NodeId {
        self.push(StandaloneNode::new(standalone_default_style(), config))
    }

    pub(crate) fn create_default_measured_node(&mut self, measured_size: Size) -> NodeId {
        let node = self.create_default_node();
        self.nodes[node.0].measurement = Some(StandaloneMeasurement::Static(measured_size));
        node
    }

    fn push(&mut self, node: StandaloneNode) -> NodeId {
        let id = NodeId(self.nodes.len());
        self.nodes.push(node);
        id
    }

    fn ensure_node(&self, node: NodeId) -> Result<(), TreeError> {
        self.nodes
            .get(node.0)
            .map(|_| ())
            .ok_or(TreeError::MissingNode(node))
    }

    fn update_style(
        &mut self,
        node: NodeId,
        update: impl FnOnce(&mut Style),
    ) -> Result<(), TreeError> {
        self.ensure_node(node)?;
        update(&mut self.nodes[node.0].style);
        Ok(())
    }

    pub(crate) fn append_child(&mut self, parent: NodeId, child: NodeId) -> Result<(), TreeError> {
        if parent == child {
            return Err(TreeError::CannotParentNodeToItself(parent));
        }
        self.ensure_node(parent)?;
        self.ensure_node(child)?;
        if let Some(old_parent) = self.nodes[child.0].parent
            && let Some(index) = self.nodes[old_parent.0]
                .children
                .iter()
                .position(|candidate| *candidate == child)
        {
            self.nodes[old_parent.0].children.remove(index);
        }
        self.nodes[parent.0].children.push(child);
        self.nodes[child.0].parent = Some(parent);
        Ok(())
    }

    pub(crate) fn set_measured_size(
        &mut self,
        node: NodeId,
        measured_size: Option<Size>,
    ) -> Result<(), TreeError> {
        self.ensure_node(node)?;
        self.nodes[node.0].measurement = measured_size.map(StandaloneMeasurement::Static);
        Ok(())
    }

    pub(crate) fn set_measure_func(
        &mut self,
        node: NodeId,
        measure_func: Option<StandaloneMeasureFunc>,
    ) -> Result<(), TreeError> {
        self.ensure_node(node)?;
        self.nodes[node.0].measurement = measure_func.map(StandaloneMeasurement::Callback);
        Ok(())
    }

    pub(crate) fn set_baseline(
        &mut self,
        node: NodeId,
        baseline: Option<f32>,
    ) -> Result<(), TreeError> {
        self.ensure_node(node)?;
        self.nodes[node.0].baseline = baseline;
        Ok(())
    }

    pub(crate) fn set_display(&mut self, node: NodeId, value: Display) -> Result<(), TreeError> {
        self.update_style(node, |style| style.display = value)
    }

    pub(crate) fn set_position_type(
        &mut self,
        node: NodeId,
        value: PositionType,
    ) -> Result<(), TreeError> {
        self.update_style(node, |style| style.position = value)
    }

    pub(crate) fn set_box_sizing(
        &mut self,
        node: NodeId,
        value: BoxSizing,
    ) -> Result<(), TreeError> {
        self.update_style(node, |style| style.box_sizing = value)
    }

    pub(crate) fn set_direction(
        &mut self,
        node: NodeId,
        value: Direction,
    ) -> Result<(), TreeError> {
        self.update_style(node, |style| style.direction = value)
    }

    pub(crate) fn set_justify_content(
        &mut self,
        node: NodeId,
        value: JustifyContent,
    ) -> Result<(), TreeError> {
        self.update_style(node, |style| style.justify_content = value)
    }

    pub(crate) fn set_align_items(
        &mut self,
        node: NodeId,
        value: AlignItems,
    ) -> Result<(), TreeError> {
        self.update_style(node, |style| style.align_items = value)
    }

    pub(crate) fn set_align_self(
        &mut self,
        node: NodeId,
        value: Option<AlignItems>,
    ) -> Result<(), TreeError> {
        self.update_style(node, |style| style.align_self = value)
    }

    pub(crate) fn set_aspect_ratio(
        &mut self,
        node: NodeId,
        value: Option<f32>,
    ) -> Result<(), TreeError> {
        self.update_style(node, |style| style.aspect_ratio = value)
    }

    pub(crate) fn set_order(&mut self, node: NodeId, value: i32) -> Result<(), TreeError> {
        self.update_style(node, |style| style.order = value)
    }

    pub(crate) fn set_linear_orientation(
        &mut self,
        node: NodeId,
        value: LinearOrientation,
    ) -> Result<(), TreeError> {
        self.update_style(node, |style| style.linear_orientation = value)
    }

    pub(crate) fn set_linear_gravity(
        &mut self,
        node: NodeId,
        value: LinearGravity,
    ) -> Result<(), TreeError> {
        self.update_style(node, |style| style.linear_gravity = value)
    }

    pub(crate) fn set_linear_layout_gravity(
        &mut self,
        node: NodeId,
        value: LinearLayoutGravity,
    ) -> Result<(), TreeError> {
        self.update_style(node, |style| style.linear_layout_gravity = value)
    }

    pub(crate) fn set_linear_cross_gravity(
        &mut self,
        node: NodeId,
        value: LinearCrossGravity,
    ) -> Result<(), TreeError> {
        self.update_style(node, |style| style.linear_cross_gravity = value)
    }

    pub(crate) fn set_linear_weight(&mut self, node: NodeId, value: f32) -> Result<(), TreeError> {
        self.update_style(node, |style| style.linear_weight = value)
    }

    pub(crate) fn set_linear_weight_sum(
        &mut self,
        node: NodeId,
        value: f32,
    ) -> Result<(), TreeError> {
        self.update_style(node, |style| style.linear_weight_sum = value)
    }

    pub(crate) fn set_width(&mut self, node: NodeId, value: Length) -> Result<(), TreeError> {
        self.update_style(node, |style| style.width = value)
    }

    pub(crate) fn set_height(&mut self, node: NodeId, value: Length) -> Result<(), TreeError> {
        self.update_style(node, |style| style.height = value)
    }

    pub(crate) fn set_min_width(&mut self, node: NodeId, value: Length) -> Result<(), TreeError> {
        self.update_style(node, |style| style.min_width = value)
    }

    pub(crate) fn set_min_height(&mut self, node: NodeId, value: Length) -> Result<(), TreeError> {
        self.update_style(node, |style| style.min_height = value)
    }

    pub(crate) fn set_max_width(&mut self, node: NodeId, value: Length) -> Result<(), TreeError> {
        self.update_style(node, |style| style.max_width = value)
    }

    pub(crate) fn set_max_height(&mut self, node: NodeId, value: Length) -> Result<(), TreeError> {
        self.update_style(node, |style| style.max_height = value)
    }

    pub(crate) fn set_gap(
        &mut self,
        node: NodeId,
        gap: StandaloneGap,
        value: Length,
    ) -> Result<(), TreeError> {
        self.update_style(node, |style| match gap {
            StandaloneGap::Column => style.column_gap = value,
            StandaloneGap::Row => style.row_gap = value,
            StandaloneGap::All => {
                style.column_gap = value;
                style.row_gap = value;
            }
        })
    }

    pub(crate) fn set_position(
        &mut self,
        node: NodeId,
        edge: StandaloneEdge,
        value: Length,
    ) -> Result<(), TreeError> {
        self.ensure_node(node)?;
        let rtl = self.nodes[node.0].style.direction == Direction::Rtl;
        apply_position_edge(&mut self.nodes[node.0].style, edge, value, rtl);
        Ok(())
    }

    pub(crate) fn set_margin(
        &mut self,
        node: NodeId,
        edge: StandaloneEdge,
        value: Length,
    ) -> Result<(), TreeError> {
        self.ensure_node(node)?;
        let rtl = self.nodes[node.0].style.direction == Direction::Rtl;
        apply_length_edge(&mut self.nodes[node.0].style.margin, edge, value, rtl);
        Ok(())
    }

    pub(crate) fn set_padding(
        &mut self,
        node: NodeId,
        edge: StandaloneEdge,
        value: Length,
    ) -> Result<(), TreeError> {
        self.ensure_node(node)?;
        let rtl = self.nodes[node.0].style.direction == Direction::Rtl;
        apply_length_edge(&mut self.nodes[node.0].style.padding, edge, value, rtl);
        Ok(())
    }

    pub(crate) fn set_border(
        &mut self,
        node: NodeId,
        edge: StandaloneEdge,
        value: f32,
    ) -> Result<(), TreeError> {
        self.ensure_node(node)?;
        let rtl = self.nodes[node.0].style.direction == Direction::Rtl;
        apply_f32_edge(&mut self.nodes[node.0].style.border, edge, value, rtl);
        Ok(())
    }

    fn assert_finite_subtree(&self, node: NodeId) -> Result<(), TreeError> {
        self.ensure_node(node)?;
        let layout = self.nodes[node.0].layout;
        for value in [
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
        ] {
            assert!(value.is_finite(), "standalone layout value must be finite");
        }
        assert!(layout.baseline.is_none_or(f32::is_finite));
        for &child in &self.nodes[node.0].children {
            self.assert_finite_subtree(child)?;
        }
        Ok(())
    }
}

fn apply_position_edge(style: &mut Style, edge: StandaloneEdge, value: Length, rtl: bool) {
    match edge {
        StandaloneEdge::Left => style.left = value,
        StandaloneEdge::Right => style.right = value,
        StandaloneEdge::Top => style.top = value,
        StandaloneEdge::Bottom => style.bottom = value,
        StandaloneEdge::Start => {
            if rtl {
                style.right = value;
            } else {
                style.left = value;
            }
        }
        StandaloneEdge::End => {
            if rtl {
                style.left = value;
            } else {
                style.right = value;
            }
        }
        StandaloneEdge::Horizontal => {
            style.left = value;
            style.right = value;
        }
        StandaloneEdge::Vertical => {
            style.top = value;
            style.bottom = value;
        }
        StandaloneEdge::All => {
            style.left = value;
            style.right = value;
            style.top = value;
            style.bottom = value;
        }
    }
}

fn apply_length_edge(edges: &mut Rect<Length>, edge: StandaloneEdge, value: Length, rtl: bool) {
    match edge {
        StandaloneEdge::Left => edges.left = value,
        StandaloneEdge::Right => edges.right = value,
        StandaloneEdge::Top => edges.top = value,
        StandaloneEdge::Bottom => edges.bottom = value,
        StandaloneEdge::Start => {
            if rtl {
                edges.right = value;
            } else {
                edges.left = value;
            }
        }
        StandaloneEdge::End => {
            if rtl {
                edges.left = value;
            } else {
                edges.right = value;
            }
        }
        StandaloneEdge::Horizontal => {
            edges.left = value;
            edges.right = value;
        }
        StandaloneEdge::Vertical => {
            edges.top = value;
            edges.bottom = value;
        }
        StandaloneEdge::All => *edges = Rect::all(value),
    }
}

fn apply_f32_edge(edges: &mut Rect<f32>, edge: StandaloneEdge, value: f32, rtl: bool) {
    match edge {
        StandaloneEdge::Left => edges.left = value,
        StandaloneEdge::Right => edges.right = value,
        StandaloneEdge::Top => edges.top = value,
        StandaloneEdge::Bottom => edges.bottom = value,
        StandaloneEdge::Start => {
            if rtl {
                edges.right = value;
            } else {
                edges.left = value;
            }
        }
        StandaloneEdge::End => {
            if rtl {
                edges.left = value;
            } else {
                edges.right = value;
            }
        }
        StandaloneEdge::Horizontal => {
            edges.left = value;
            edges.right = value;
        }
        StandaloneEdge::Vertical => {
            edges.top = value;
            edges.bottom = value;
        }
        StandaloneEdge::All => *edges = Rect::all(value),
    }
}

impl LayoutTree for StandaloneTree {
    type NodeId = NodeId;
    type Children<'a> = std::iter::Copied<std::slice::Iter<'a, NodeId>>;

    fn children(&self, node: Self::NodeId) -> Self::Children<'_> {
        self.nodes[node.0].children.iter().copied()
    }

    fn style(&self, node: Self::NodeId) -> &Style {
        &self.nodes[node.0].style
    }

    fn set_layout(&mut self, node: Self::NodeId, layout: LayoutResult) {
        self.nodes[node.0].layout = layout;
    }

    fn layout(&self, node: Self::NodeId) -> Option<LayoutResult> {
        Some(self.nodes[node.0].layout)
    }

    fn measure(&mut self, node: Self::NodeId, constraints: Constraints) -> Option<Size> {
        self.nodes[node.0].measure(constraints)
    }

    fn has_measure(&self, node: Self::NodeId) -> bool {
        self.nodes[node.0].measurement.is_some()
    }

    fn measure_func(&self, node: Self::NodeId) -> Option<StandaloneMeasureFunc> {
        match self.nodes[node.0].measurement {
            Some(StandaloneMeasurement::Callback(measure)) => Some(measure),
            Some(StandaloneMeasurement::Static(_)) | None => None,
        }
    }

    fn baseline(&self, node: Self::NodeId, _content_size: Size) -> Option<f32> {
        self.nodes[node.0].baseline
    }
}

impl RoundState for StandaloneTree {
    fn unrounded_layout(&self, node: NeutronNodeId) -> NeutronLayout {
        let layout = self.nodes[usize::from(node)].layout;
        let mut rounded_input = NeutronLayout::default();
        rounded_input.location = NeutronPoint::new(layout.offset.x, layout.offset.y);
        rounded_input.size = NeutronSize::new(layout.size.width, layout.size.height);
        rounded_input.content_size = NeutronSize::new(layout.size.width, layout.size.height);
        rounded_input.border = NeutronEdges {
            left: layout.border.left,
            right: layout.border.right,
            top: layout.border.top,
            bottom: layout.border.bottom,
        };
        rounded_input.padding = NeutronEdges {
            left: layout.padding.left,
            right: layout.padding.right,
            top: layout.padding.top,
            bottom: layout.padding.bottom,
        };
        rounded_input.margin = NeutronEdges {
            left: layout.margin.left,
            right: layout.margin.right,
            top: layout.margin.top,
            bottom: layout.margin.bottom,
        };
        rounded_input
    }

    fn set_final_layout(&mut self, node: NeutronNodeId, layout: &NeutronLayout) {
        let result = &mut self.nodes[usize::from(node)].layout;
        result.offset = crate::pr25_support::Point::new(layout.location.x, layout.location.y);
        result.size = Size::new(layout.size.width, layout.size.height);
        result.border = Rect::new(
            layout.border.left,
            layout.border.right,
            layout.border.top,
            layout.border.bottom,
        );
        result.padding = Rect::new(
            layout.padding.left,
            layout.padding.right,
            layout.padding.top,
            layout.padding.bottom,
        );
        result.margin = Rect::new(
            layout.margin.left,
            layout.margin.right,
            layout.margin.top,
            layout.margin.bottom,
        );
    }
}

fn layout_standalone_tree(
    tree: &mut StandaloneTree,
    root: NodeId,
    constraints: Constraints,
) -> Result<Size, TreeError> {
    tree.ensure_node(root)?;
    let size = LayoutEngine::new().layout_with_owner_constraints(tree, root, constraints);
    let topology = StandaloneTopology::from(&*tree);
    let scale = tree.nodes[root.0].config.physical_pixels_per_layout_unit;
    let scale = if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    };
    round_layout(&topology, tree, NeutronNodeId::from(root.0), scale);
    tree.assert_finite_subtree(root)?;
    Ok(size)
}

/// Executes the migrated Rust half of a head-to-head fixture.
pub(crate) fn run_standalone_rust(
    tree: StandaloneTree,
    root: NodeId,
    constraints: Constraints,
) -> Result<(), TreeError> {
    let mut first = tree.clone();
    let mut second = tree;
    let first_size = layout_standalone_tree(&mut first, root, constraints)?;
    let second_size = layout_standalone_tree(&mut second, root, constraints)?;

    assert_eq!(
        first_size, second_size,
        "standalone layout is nondeterministic"
    );
    assert_eq!(first.nodes.len(), second.nodes.len());
    for (first, second) in first.nodes.iter().zip(&second.nodes) {
        assert_eq!(
            first.layout, second.layout,
            "standalone node layout is nondeterministic"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU16, Ordering};

    use super::*;
    use crate::pr25_support::{SimpleNode, SimpleTree, run_rust_layout};

    #[test]
    fn root_box_edges_are_exported_before_standalone_rounding() {
        let mut tree = StandaloneTree::new();
        let root = tree.create_default_node();
        tree.set_display(root, Display::Linear).unwrap();
        tree.set_width(root, Length::points(100.0)).unwrap();
        tree.set_height(root, Length::points(40.0)).unwrap();
        tree.set_padding(root, StandaloneEdge::Left, Length::points(2.0))
            .unwrap();
        tree.set_padding(root, StandaloneEdge::Right, Length::points(3.0))
            .unwrap();
        tree.set_padding(root, StandaloneEdge::Top, Length::points(4.0))
            .unwrap();
        tree.set_padding(root, StandaloneEdge::Bottom, Length::points(5.0))
            .unwrap();
        tree.set_border(root, StandaloneEdge::Left, 1.0).unwrap();
        tree.set_border(root, StandaloneEdge::Right, 2.0).unwrap();
        tree.set_border(root, StandaloneEdge::Top, 3.0).unwrap();
        tree.set_border(root, StandaloneEdge::Bottom, 4.0).unwrap();
        tree.set_margin(root, StandaloneEdge::Left, Length::points(6.0))
            .unwrap();
        tree.set_margin(root, StandaloneEdge::Right, Length::points(7.0))
            .unwrap();
        tree.set_margin(root, StandaloneEdge::Top, Length::points(8.0))
            .unwrap();
        tree.set_margin(root, StandaloneEdge::Bottom, Length::points(9.0))
            .unwrap();

        layout_standalone_tree(&mut tree, root, Constraints::definite(200.0, 100.0)).unwrap();

        let layout = tree.nodes[root.0].layout;
        assert_eq!(layout.padding, Rect::new(2.0, 3.0, 4.0, 5.0));
        assert_eq!(layout.border, Rect::new(1.0, 2.0, 3.0, 4.0));
        assert_eq!(layout.margin, Rect::new(6.0, 7.0, 8.0, 9.0));
    }

    #[test]
    fn null_measure_keeps_block_on_the_measured_path() {
        let mut tree = SimpleTree::default();
        let root = tree.push(SimpleNode::with_null_measure(Style {
            display: Display::Block,
            ..Style::default()
        }));
        let child = tree.push(SimpleNode::new(Style {
            display: Display::Block,
            width: Length::points(50.0),
            height: Length::points(10.0),
            ..Style::default()
        }));
        tree.append_child(root, child);

        let size = LayoutEngine::new().layout_with_owner_constraints(
            &mut tree,
            root,
            Constraints::indefinite(),
        );

        assert_eq!(size, Size::ZERO);
        assert_eq!(tree.nodes[root].layout.size, Size::ZERO);
        assert_eq!(tree.nodes[child].layout.size, Size::ZERO);
        assert_eq!(tree.nodes[child].layout.offset, NeutronPoint::default());
    }

    #[test]
    fn sticky_metadata_is_suppressed_inside_hidden_subtrees() {
        let mut tree = SimpleTree::default();
        let root = tree.push(SimpleNode::new(Style {
            display: Display::Linear,
            width: Length::points(100.0),
            height: Length::points(40.0),
            ..Style::default()
        }));
        let hidden = tree.push(SimpleNode::new(Style {
            display: Display::None,
            ..Style::default()
        }));
        let sticky = tree.push(SimpleNode::new(Style {
            position: PositionType::Sticky,
            width: Length::points(20.0),
            height: Length::points(10.0),
            left: Length::percent(10.0),
            top: Length::percent(25.0),
            ..Style::default()
        }));
        tree.append_child(root, hidden);
        tree.append_child(hidden, sticky);

        run_rust_layout(&mut tree, root, Constraints::definite(100.0, 40.0));

        assert_eq!(tree.nodes[hidden].layout, LayoutResult::default());
        assert_eq!(tree.nodes[sticky].layout, LayoutResult::default());
    }

    static NONDETERMINISTIC_MEASURE: AtomicU16 = AtomicU16::new(0);

    fn nondeterministic_measure(_constraints: Constraints) -> Size {
        let width = f32::from(NONDETERMINISTIC_MEASURE.fetch_add(1, Ordering::SeqCst)) + 1.0;
        Size::new(width, 10.0)
    }

    #[test]
    #[should_panic(expected = "standalone layout is nondeterministic")]
    fn standalone_runner_compares_two_complete_layouts() {
        NONDETERMINISTIC_MEASURE.store(0, Ordering::SeqCst);
        let mut tree = StandaloneTree::new();
        let root = tree.create_default_node();
        tree.set_measure_func(root, Some(nondeterministic_measure))
            .unwrap();

        run_standalone_rust(tree, root, Constraints::indefinite()).unwrap();
    }
}
