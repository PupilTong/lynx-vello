//! Compatibility facade for the Rust Flex fixtures migrated from PupilTong/lynx#25.
//!
//! The source PR used a monolithic `SimpleTree` API. This facade preserves its
//! fixture vocabulary while lowering every run into neutron-star's immutable
//! `TestSource` plus mutable `TestSession`; no C++ baseline or styling engine is
//! involved.

#![allow(dead_code)]

use std::collections::BTreeMap;

use neutron_star::geometry::Size as NSize;
use neutron_star::prelude::*;
pub(crate) use neutron_star::style::{
    AlignContent, AlignItems, BoxSizing, Direction, FlexDirection, FlexWrap, JustifyContent,
};
use neutron_star::style::{
    BoxGenerationMode, CalcHandle, Dimension, LengthPercentage, LengthPercentageAuto, Position,
};

use crate::support::{
    TestConstraints, TestDisplay, TestMeasure, TestMeasureMode, TestSideConstraint, TestSourceNode,
    TestStyle, TestTree,
};
pub(crate) use crate::support::{
    TestConstraints as Constraints, TestMeasureMode as MeasureMode,
    TestSideConstraint as SideConstraint,
};
pub(crate) type Point = neutron_star::geometry::Point<f32>;
pub(crate) type Size = neutron_star::geometry::Size<f32>;

pub(crate) trait DirectionExt {
    fn is_rtl(&self) -> bool;
}

impl DirectionExt for Direction {
    fn is_rtl(&self) -> bool {
        *self == Self::Rtl
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum Display {
    None,
    #[default]
    Block,
    Flex,
    Linear,
    Relative,
    Grid,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum PositionType {
    #[default]
    Static,
    Relative,
    Absolute,
    Fixed,
    Sticky,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum Visibility {
    #[default]
    Visible,
    Hidden,
    Collapse,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum LinearOrientation {
    Horizontal,
    HorizontalReverse,
    #[default]
    Vertical,
    VerticalReverse,
    Row,
    RowReverse,
    Column,
    ColumnReverse,
}

impl LinearOrientation {
    fn flex_direction(self) -> FlexDirection {
        match self {
            Self::Horizontal | Self::Row => FlexDirection::Row,
            Self::HorizontalReverse | Self::RowReverse => FlexDirection::RowReverse,
            Self::Vertical | Self::Column => FlexDirection::Column,
            Self::VerticalReverse | Self::ColumnReverse => FlexDirection::ColumnReverse,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct BaseLength {
    fixed: f32,
    percentage: f32,
    has_percentage: bool,
}

impl BaseLength {
    pub(crate) const fn fixed(fixed: f32) -> Self {
        Self {
            fixed,
            percentage: 0.0,
            has_percentage: false,
        }
    }

    pub(crate) const fn fixed_and_percent(fixed: f32, percentage: f32) -> Self {
        Self {
            fixed,
            percentage,
            has_percentage: true,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) enum Length {
    #[default]
    Auto,
    Points(f32),
    Percent(f32),
    Calc {
        fixed: f32,
        percent: f32,
    },
    Fr(f32),
    MinContent,
    MaxContent,
    FitContent(Option<BaseLength>),
}

impl Length {
    pub(crate) const ZERO: Self = Self::Points(0.0);

    pub(crate) const fn points(value: f32) -> Self {
        Self::Points(value)
    }

    pub(crate) const fn percent(value: f32) -> Self {
        Self::Percent(value)
    }

    pub(crate) const fn calc(fixed: f32, percent: f32) -> Self {
        Self::Calc { fixed, percent }
    }

    pub(crate) const fn fit_content(base: Option<BaseLength>) -> Self {
        Self::FitContent(base)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Rect<T> {
    pub(crate) left: T,
    pub(crate) right: T,
    pub(crate) top: T,
    pub(crate) bottom: T,
}

impl<T: Copy> Rect<T> {
    pub(crate) const fn all(value: T) -> Self {
        Self {
            left: value,
            right: value,
            top: value,
            bottom: value,
        }
    }

    pub(crate) const fn new(left: T, right: T, top: T, bottom: T) -> Self {
        Self {
            left,
            right,
            top,
            bottom,
        }
    }
}

impl<T: Default + Copy> Default for Rect<T> {
    fn default() -> Self {
        Self::all(T::default())
    }
}

pub(crate) type Edges = Rect<f32>;

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Style {
    pub(crate) display: Display,
    pub(crate) position: PositionType,
    pub(crate) box_sizing: BoxSizing,
    pub(crate) direction: Direction,
    pub(crate) visibility: Visibility,
    pub(crate) width: Length,
    pub(crate) height: Length,
    pub(crate) min_width: Length,
    pub(crate) min_height: Length,
    pub(crate) max_width: Length,
    pub(crate) max_height: Length,
    pub(crate) aspect_ratio: Option<f32>,
    pub(crate) left: Length,
    pub(crate) right: Length,
    pub(crate) top: Length,
    pub(crate) bottom: Length,
    pub(crate) margin: Rect<Length>,
    pub(crate) padding: Rect<Length>,
    pub(crate) border: Rect<f32>,
    pub(crate) flex_direction: FlexDirection,
    pub(crate) flex_wrap: FlexWrap,
    pub(crate) justify_content: JustifyContent,
    pub(crate) align_items: AlignItems,
    pub(crate) align_self: Option<AlignItems>,
    pub(crate) align_content: AlignContent,
    pub(crate) flex_grow: f32,
    pub(crate) flex_shrink: f32,
    pub(crate) flex_basis: Length,
    pub(crate) order: i32,
    pub(crate) row_gap: Length,
    pub(crate) column_gap: Length,
    pub(crate) linear_orientation: LinearOrientation,
    pub(crate) grid_template_columns: Vec<Length>,
    pub(crate) grid_template_rows: Vec<Length>,
}

impl Default for Style {
    fn default() -> Self {
        Self {
            display: Display::Block,
            position: PositionType::Static,
            box_sizing: BoxSizing::ContentBox,
            direction: Direction::Ltr,
            visibility: Visibility::Visible,
            width: Length::Auto,
            height: Length::Auto,
            min_width: Length::Auto,
            min_height: Length::Auto,
            max_width: Length::Auto,
            max_height: Length::Auto,
            aspect_ratio: None,
            left: Length::Auto,
            right: Length::Auto,
            top: Length::Auto,
            bottom: Length::Auto,
            margin: Rect::all(Length::ZERO),
            padding: Rect::all(Length::ZERO),
            border: Rect::all(0.0),
            flex_direction: FlexDirection::Row,
            flex_wrap: FlexWrap::NoWrap,
            justify_content: JustifyContent::Stretch,
            align_items: AlignItems::Stretch,
            align_self: None,
            align_content: AlignContent::Stretch,
            flex_grow: 0.0,
            flex_shrink: 1.0,
            flex_basis: Length::Auto,
            order: 0,
            row_gap: Length::ZERO,
            column_gap: Length::ZERO,
            linear_orientation: LinearOrientation::Vertical,
            grid_template_columns: Vec::new(),
            grid_template_rows: Vec::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct LayoutResult {
    pub(crate) offset: Point,
    pub(crate) size: Size,
    pub(crate) baseline: Option<f32>,
    pub(crate) padding: Edges,
    pub(crate) border: Edges,
    pub(crate) margin: Edges,
}

pub(crate) type SimpleMeasureFunc = fn(Constraints) -> Size;

#[derive(Clone, Debug)]
pub(crate) struct SimpleNode {
    pub(crate) style: Style,
    pub(crate) layout: LayoutResult,
    pub(crate) children: Vec<usize>,
    measured_size: Option<Size>,
    measure_func: Option<SimpleMeasureFunc>,
    baseline: Option<f32>,
}

impl SimpleNode {
    pub(crate) fn new(style: Style) -> Self {
        Self {
            style,
            layout: LayoutResult::default(),
            children: Vec::new(),
            measured_size: None,
            measure_func: None,
            baseline: None,
        }
    }

    pub(crate) fn with_measured_size(style: Style, measured_size: Size) -> Self {
        Self {
            measured_size: Some(measured_size),
            ..Self::new(style)
        }
    }

    pub(crate) fn with_measure_func(style: Style, measure_func: SimpleMeasureFunc) -> Self {
        Self {
            measure_func: Some(measure_func),
            ..Self::new(style)
        }
    }

    pub(crate) fn with_measured_size_and_baseline(
        style: Style,
        measured_size: Size,
        baseline: f32,
    ) -> Self {
        Self {
            measured_size: Some(measured_size),
            baseline: Some(baseline),
            ..Self::new(style)
        }
    }
}

#[derive(Clone, Debug, Default)]
pub(crate) struct SimpleTree {
    pub(crate) nodes: Vec<SimpleNode>,
}

impl SimpleTree {
    pub(crate) fn push(&mut self, node: SimpleNode) -> usize {
        let id = self.nodes.len();
        self.nodes.push(node);
        id
    }

    pub(crate) fn append_child(&mut self, parent: usize, child: usize) {
        self.nodes[parent].children.push(child);
    }
}

pub(crate) trait LayoutTree {
    type NodeId: Copy + Ord;
    type Children<'a>: Iterator<Item = Self::NodeId>
    where
        Self: 'a;

    fn children(&self, node: Self::NodeId) -> Self::Children<'_>;
    fn style(&self, node: Self::NodeId) -> &Style;
    fn set_layout(&mut self, node: Self::NodeId, layout: LayoutResult);
    fn layout(&self, node: Self::NodeId) -> Option<LayoutResult>;

    fn measure(&mut self, _node: Self::NodeId, _constraints: Constraints) -> Option<Size> {
        None
    }

    fn has_measure(&self, _node: Self::NodeId) -> bool {
        false
    }

    fn measure_func(&self, _node: Self::NodeId) -> Option<SimpleMeasureFunc> {
        None
    }

    fn baseline(&self, _node: Self::NodeId, _content_size: Size) -> Option<f32> {
        None
    }
}

impl LayoutTree for SimpleTree {
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
        let node = &self.nodes[node];
        node.measured_size
            .or_else(|| node.measure_func.map(|measure| measure(constraints)))
    }

    fn has_measure(&self, node: Self::NodeId) -> bool {
        self.nodes[node].measured_size.is_some() || self.nodes[node].measure_func.is_some()
    }

    fn measure_func(&self, node: Self::NodeId) -> Option<SimpleMeasureFunc> {
        self.nodes[node].measure_func
    }

    fn baseline(&self, node: Self::NodeId, _content_size: Size) -> Option<f32> {
        self.nodes[node].baseline
    }
}

fn dimension(tree: &mut TestTree, value: Length) -> Dimension {
    match value {
        // `fr` outside grid tracks is a Lynx raw-value extension. The host
        // lowering used by these CSS-focused fixtures treats it as `auto`.
        Length::Auto | Length::Fr(_) => Dimension::Auto,
        Length::Points(value) => Dimension::Length(value),
        Length::Percent(value) => Dimension::Percent(value / 100.0),
        Length::Calc { fixed, percent } => Dimension::Calc(tree.push_calc(fixed, percent / 100.0)),
        Length::MinContent => Dimension::MinContent,
        // The legacy argument-less keyword behaves as an uncapped intrinsic
        // size in the source Flex fixtures.
        Length::MaxContent | Length::FitContent(None) => Dimension::MaxContent,
        Length::FitContent(Some(base)) => {
            Dimension::FitContent(length_percentage_from_base(tree, base))
        }
    }
}

fn minimum_dimension(tree: &mut TestTree, value: Length) -> Dimension {
    if value == Length::FitContent(None) {
        Dimension::Auto
    } else {
        dimension(tree, value)
    }
}

fn maximum_dimension(tree: &mut TestTree, value: Length) -> Dimension {
    if value == Length::FitContent(None) {
        Dimension::Auto
    } else {
        dimension(tree, value)
    }
}

fn length_percentage_from_base(tree: &mut TestTree, base: BaseLength) -> LengthPercentage {
    if base.has_percentage {
        LengthPercentage::Calc(tree.push_calc(base.fixed, base.percentage / 100.0))
    } else {
        LengthPercentage::Length(base.fixed)
    }
}

fn length_percentage(tree: &mut TestTree, value: Length) -> LengthPercentage {
    match value {
        Length::Points(value) => LengthPercentage::Length(value),
        Length::Percent(value) => LengthPercentage::Percent(value / 100.0),
        Length::Calc { fixed, percent } => {
            LengthPercentage::Calc(tree.push_calc(fixed, percent / 100.0))
        }
        Length::Auto
        | Length::Fr(_)
        | Length::MinContent
        | Length::MaxContent
        | Length::FitContent(_) => LengthPercentage::ZERO,
    }
}

fn length_percentage_auto(tree: &mut TestTree, value: Length) -> LengthPercentageAuto {
    match value {
        Length::Auto => LengthPercentageAuto::Auto,
        Length::Points(value) => LengthPercentageAuto::Length(value),
        Length::Percent(value) => LengthPercentageAuto::Percent(value / 100.0),
        Length::Calc { fixed, percent } => {
            LengthPercentageAuto::Calc(tree.push_calc(fixed, percent / 100.0))
        }
        Length::Fr(_) | Length::MinContent | Length::MaxContent | Length::FitContent(_) => {
            LengthPercentageAuto::ZERO
        }
    }
}

fn convert_style(tree: &mut TestTree, style: &Style, has_children: bool) -> TestStyle {
    let display_mode = match style.display {
        Display::None => BoxGenerationMode::None,
        _ => BoxGenerationMode::Normal,
    };
    let position = match style.position {
        PositionType::Absolute => Position::Absolute,
        PositionType::Fixed => Position::AbsoluteHoisted,
        PositionType::Static | PositionType::Relative | PositionType::Sticky => Position::Relative,
    };
    let flex_direction = match style.display {
        Display::Linear => style.linear_orientation.flex_direction(),
        Display::Block | Display::Relative | Display::Grid if has_children => FlexDirection::Column,
        _ => style.flex_direction,
    };
    TestStyle {
        box_generation_mode: display_mode,
        visibility: match style.visibility {
            Visibility::Visible => neutron_star::style::Visibility::Visible,
            Visibility::Hidden => neutron_star::style::Visibility::Hidden,
            Visibility::Collapse => neutron_star::style::Visibility::Collapse,
        },
        position,
        inset: neutron_star::geometry::Edges {
            left: length_percentage_auto(tree, style.left),
            right: length_percentage_auto(tree, style.right),
            top: length_percentage_auto(tree, style.top),
            bottom: length_percentage_auto(tree, style.bottom),
        },
        size: NSize::new(dimension(tree, style.width), dimension(tree, style.height)),
        min_size: NSize::new(
            minimum_dimension(tree, style.min_width),
            minimum_dimension(tree, style.min_height),
        ),
        max_size: NSize::new(
            maximum_dimension(tree, style.max_width),
            maximum_dimension(tree, style.max_height),
        ),
        aspect_ratio: style.aspect_ratio,
        margin: neutron_star::geometry::Edges {
            left: length_percentage_auto(tree, style.margin.left),
            right: length_percentage_auto(tree, style.margin.right),
            top: length_percentage_auto(tree, style.margin.top),
            bottom: length_percentage_auto(tree, style.margin.bottom),
        },
        padding: neutron_star::geometry::Edges {
            left: length_percentage(tree, style.padding.left),
            right: length_percentage(tree, style.padding.right),
            top: length_percentage(tree, style.padding.top),
            bottom: length_percentage(tree, style.padding.bottom),
        },
        border: neutron_star::geometry::Edges {
            left: LengthPercentage::Length(style.border.left),
            right: LengthPercentage::Length(style.border.right),
            top: LengthPercentage::Length(style.border.top),
            bottom: LengthPercentage::Length(style.border.bottom),
        },
        box_sizing: style.box_sizing,
        direction: style.direction,
        flex_direction,
        flex_wrap: style.flex_wrap,
        gap: NSize::new(
            length_percentage(tree, style.column_gap),
            length_percentage(tree, style.row_gap),
        ),
        align_content: Some(style.align_content),
        // The source fixtures model Lynx computed defaults (`overflow:
        // hidden`) rather than CSS parser initials. Supplying that value at
        // the host boundary keeps automatic flex minimums faithful without
        // baking a Lynx default into neutron-star.
        overflow: neutron_star::geometry::Point::new(
            neutron_star::style::Overflow::Hidden,
            neutron_star::style::Overflow::Hidden,
        ),
        align_items: Some(style.align_items),
        justify_content: Some(style.justify_content),
        flex_basis: dimension(tree, style.flex_basis),
        flex_grow: style.flex_grow,
        flex_shrink: style.flex_shrink,
        align_self: style.align_self,
        order: style.order,
        ..TestStyle::default()
    }
}

fn lower_tree(tree: &SimpleTree) -> TestTree {
    let mut lowered = TestTree::default();
    for node in &tree.nodes {
        let style = convert_style(&mut lowered, &node.style, !node.children.is_empty());
        let display = if node.style.display == Display::Flex
            || (node.style.display != Display::None && !node.children.is_empty())
        {
            TestDisplay::Flex
        } else {
            TestDisplay::Leaf
        };
        let measure = if let Some(measure) = node.measure_func {
            TestMeasure::ConstraintFunction {
                measure,
                baseline: None,
            }
        } else {
            let intrinsic = node.measured_size.unwrap_or(Size::ZERO);
            TestMeasure::Intrinsic {
                min_content_size: intrinsic,
                max_content_size: intrinsic,
                first_baseline: node.baseline,
            }
        };
        lowered.push(TestSourceNode {
            display,
            style,
            children: Vec::new(),
            measure,
            // Starlight's horizontal linear container exports the last
            // participating child's baseline. This is host-dispatch output,
            // not Flex behavior; preserve it as a foreign-algorithm result.
            first_baseline_override: (node.style.display == Display::Linear)
                .then(|| {
                    node.children
                        .iter()
                        .filter_map(|&child| tree.nodes[child].baseline)
                        .next_back()
                })
                .flatten(),
        });
    }
    for (index, node) in tree.nodes.iter().enumerate() {
        lowered.source.nodes[index].children =
            node.children.iter().copied().map(NodeId::from).collect();
    }
    lowered
}

fn constraint_space(side: SideConstraint) -> AvailableSpace {
    match side.mode {
        MeasureMode::Indefinite => AvailableSpace::MaxContent,
        MeasureMode::Definite | MeasureMode::AtMost => AvailableSpace::Definite(side.size),
    }
}

fn known_dimensions(constraints: Constraints, owner_constraints: bool) -> NSize<Option<f32>> {
    if owner_constraints {
        return NSize::NONE;
    }
    NSize::new(
        (constraints.width.mode == MeasureMode::Definite).then_some(constraints.width.size),
        (constraints.height.mode == MeasureMode::Definite).then_some(constraints.height.size),
    )
}

fn run_simple_layout(
    tree: &mut SimpleTree,
    root: usize,
    constraints: Constraints,
    owner_constraints: bool,
) -> Size {
    let mut lowered = lower_tree(tree);
    let available = NSize::new(
        constraint_space(constraints.width),
        constraint_space(constraints.height),
    );
    let root_id = NodeId::from(root);
    let output = lowered.session.compute_child_layout(
        &lowered.source,
        root_id,
        LayoutInput::perform_layout(
            known_dimensions(constraints, owner_constraints),
            available.into_options(),
            available,
        ),
    );

    for (index, node) in tree.nodes.iter_mut().enumerate() {
        let session = &lowered.session.nodes[index];
        node.layout = LayoutResult {
            offset: session.layout.location,
            size: session.layout.size,
            baseline: session
                .output
                .first_baselines
                .y
                .or(Some(session.layout.size.height)),
            padding: Rect::new(
                session.layout.padding.left,
                session.layout.padding.right,
                session.layout.padding.top,
                session.layout.padding.bottom,
            ),
            border: Rect::new(
                session.layout.border.left,
                session.layout.border.right,
                session.layout.border.top,
                session.layout.border.bottom,
            ),
            margin: Rect::new(
                session.layout.margin.left,
                session.layout.margin.right,
                session.layout.margin.top,
                session.layout.margin.bottom,
            ),
        };
    }
    tree.nodes[root].layout.size = output.size;
    tree.nodes[root].layout.baseline = output.first_baselines.y;
    output.size
}

pub(crate) fn run_rust_layout<T: LayoutTree>(
    tree: &mut T,
    root: T::NodeId,
    constraints: Constraints,
) -> Size {
    run_layout_tree(tree, root, constraints, false)
}

#[derive(Debug, Default)]
pub(crate) struct LayoutEngine;

impl LayoutEngine {
    pub(crate) const fn new() -> Self {
        Self
    }

    #[allow(clippy::unused_self)] // Mirrors the source fixture's engine-shaped API.
    pub(crate) fn layout_with_owner_constraints<T: LayoutTree>(
        &mut self,
        tree: &mut T,
        root: T::NodeId,
        constraints: Constraints,
    ) -> Size {
        run_layout_tree(tree, root, constraints, true)
    }
}

fn run_layout_tree<T: LayoutTree>(
    tree: &mut T,
    root: T::NodeId,
    constraints: Constraints,
    owner_constraints: bool,
) -> Size {
    fn copy_subtree<T: LayoutTree>(
        tree: &mut T,
        node: T::NodeId,
        snapshot: &mut SimpleTree,
        node_map: &mut BTreeMap<T::NodeId, usize>,
    ) -> usize {
        if let Some(&mapped) = node_map.get(&node) {
            return mapped;
        }

        let style = tree.style(node).clone();
        let children = tree.children(node).collect::<Vec<_>>();
        let has_measure = tree.has_measure(node);
        let measure_func = tree.measure_func(node);
        let measured_size = if has_measure && measure_func.is_none() {
            tree.measure(node, Constraints::indefinite())
        } else {
            None
        };
        let baseline = measured_size.and_then(|size| tree.baseline(node, size));
        let mapped = snapshot.push(SimpleNode {
            style,
            layout: LayoutResult::default(),
            children: Vec::new(),
            measured_size,
            measure_func,
            baseline,
        });
        node_map.insert(node, mapped);

        for child in children {
            let mapped_child = copy_subtree(tree, child, snapshot, node_map);
            snapshot.append_child(mapped, mapped_child);
        }
        mapped
    }

    let mut snapshot = SimpleTree::default();
    let mut node_map = BTreeMap::new();
    let mapped_root = copy_subtree(tree, root, &mut snapshot, &mut node_map);
    let size = run_simple_layout(&mut snapshot, mapped_root, constraints, owner_constraints);
    for (node, mapped) in node_map {
        tree.set_layout(node, snapshot.nodes[mapped].layout);
    }
    size
}

// Keep the imported names visible so the compatibility type aliases are
// checked against the shared host's exact constraint representation.
const _: Option<(
    TestConstraints,
    TestSideConstraint,
    TestMeasureMode,
    CalcHandle,
)> = None;
