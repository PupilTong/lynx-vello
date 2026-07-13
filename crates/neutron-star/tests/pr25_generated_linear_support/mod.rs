//! Rust-side generated-tree vocabulary copied from PupilTong/lynx#25.

#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_precision_loss,
    clippy::range_plus_one,
    clippy::too_many_lines
)]

use crate::pr25_support::*;

#[derive(Clone, Copy, Debug)]
pub(crate) enum GeneratedContainer {
    Block,
    FlexRow,
    FlexColumnRtl,
    LinearRow,
    LinearColumnRtl,
    Relative,
    Grid,
}

pub(crate) const GENERATED_CONTAINERS: [GeneratedContainer; 7] = [
    GeneratedContainer::Block,
    GeneratedContainer::FlexRow,
    GeneratedContainer::FlexColumnRtl,
    GeneratedContainer::LinearRow,
    GeneratedContainer::LinearColumnRtl,
    GeneratedContainer::Relative,
    GeneratedContainer::Grid,
];

#[derive(Clone, Copy, Debug)]
pub(crate) enum OutOfFlowInset {
    None,
    Start,
    End,
    Both,
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum OutOfFlowSizingVariant {
    PercentCalc,
    FillAvailable,
    OversizedFillAvailableMeasured,
    MinMaxMeasuredClamp,
    FitContentMeasured,
    AspectBorderBoxMeasured,
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum FixedDescendantVariant {
    PercentStart,
    CalcEnd,
    FillAvailable,
    MeasuredAspect,
    FitContentSubtree,
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum StickyInsetLength {
    Points,
    Percent,
    Calc,
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum StickySizingVariant {
    PercentCalc,
    AutoMeasured,
    MinMaxMeasuredClamp,
    FitContentMeasured,
    AspectBorderBoxMeasured,
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum MeasuredVariant {
    Plain,
    Baseline,
    MinMax,
    AspectBorderBox,
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum BaselineConstraintMode {
    DefiniteRoot,
    AtMostOwner,
    IndefiniteOwner,
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum BaselineTrigger {
    ContainerAlignItems,
    ChildAlignSelf,
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum BaselineSource {
    MeasuredLeaf,
    NestedFlex,
    NestedFlexColumn,
    NestedFlexColumnReverse,
    NestedLinear,
    NestedLinearVertical,
    NestedLinearVerticalReverse,
    NestedGridFallback,
    NestedRelativeFallback,
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum SizingVariant {
    PercentCalcRoot,
    FitContentRoot,
    FitContentSubtree,
    PercentMinMaxRoot,
    BorderBoxPercentMinMaxRoot,
    ContentBoxAspectRoot,
    BorderBoxAspectRoot,
    IntrinsicMeasuredChild,
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum LinearConstraintMode {
    DefiniteRoot,
    AtMostOwner,
    IndefiniteOwner,
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum LinearEdgePattern {
    WeightedMinMax,
    WeightSumMainGravity,
    LayoutGravityOverride,
    CrossAutoMarginBaseline,
}

pub(crate) const fn all_linear_orientations() -> [LinearOrientation; 8] {
    [
        LinearOrientation::Horizontal,
        LinearOrientation::HorizontalReverse,
        LinearOrientation::Vertical,
        LinearOrientation::VerticalReverse,
        LinearOrientation::Row,
        LinearOrientation::RowReverse,
        LinearOrientation::Column,
        LinearOrientation::ColumnReverse,
    ]
}

pub(crate) fn tree_contains_linear(tree: &SimpleTree) -> bool {
    tree.nodes
        .iter()
        .any(|node| node.style.display == Display::Linear)
}

fn flex_style(style: Style) -> Style {
    Style {
        display: Display::Flex,
        box_sizing: BoxSizing::ContentBox,
        ..style
    }
}

fn grid_style(style: Style) -> Style {
    Style {
        display: Display::Grid,
        box_sizing: BoxSizing::ContentBox,
        ..style
    }
}

fn linear_style(style: Style) -> Style {
    Style {
        display: Display::Linear,
        box_sizing: BoxSizing::ContentBox,
        ..style
    }
}

fn relative_style(style: Style) -> Style {
    Style {
        display: Display::Relative,
        box_sizing: BoxSizing::ContentBox,
        ..style
    }
}

fn block_style(style: Style) -> Style {
    Style {
        display: Display::Block,
        box_sizing: BoxSizing::ContentBox,
        ..style
    }
}

pub(crate) fn measured_callback_tree(
    container: GeneratedContainer,
    variant: MeasuredVariant,
) -> (SimpleTree, usize) {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(measured_container_style(
        container, variant,
    )));
    for index in 0..2 {
        let style = measured_child_style(variant, index);
        let measured_size = measured_child_size(variant, index);
        let child = if matches!(variant, MeasuredVariant::Baseline) {
            SimpleNode::with_measured_size_and_baseline(
                style,
                measured_size,
                if index == 0 { 9.0 } else { 14.0 },
            )
        } else {
            SimpleNode::with_measured_size(style, measured_size)
        };
        let child = tree.push(child);
        tree.append_child(root, child);
    }
    (tree, root)
}

fn measured_container_style(container: GeneratedContainer, variant: MeasuredVariant) -> Style {
    let base = Style {
        width: Length::points(132.0),
        height: Length::points(84.0),
        padding: Rect::new(
            Length::points(2.0),
            Length::points(4.0),
            Length::points(3.0),
            Length::points(5.0),
        ),
        border: Rect::new(1.0, 2.0, 1.0, 3.0),
        justify_content: JustifyContent::Center,
        align_items: if matches!(variant, MeasuredVariant::Baseline) {
            AlignItems::Baseline
        } else {
            AlignItems::Center
        },
        align_content: AlignContent::Stretch,
        ..Style::default()
    };
    container_style_from_base(container, base)
}

fn measured_child_style(variant: MeasuredVariant, index: usize) -> Style {
    let mut style = block_style(Style {
        margin: Rect::new(
            Length::points((index + 1) as f32),
            Length::points((2 - index) as f32),
            Length::points((index % 2) as f32 + 1.0),
            Length::points(((index + 1) % 2) as f32 + 1.0),
        ),
        padding: Rect::new(
            Length::points(1.0),
            Length::points(index as f32 + 1.0),
            Length::points(2.0),
            Length::points(1.0),
        ),
        border: Rect::new(1.0, index as f32, 1.0, 2.0),
        ..Style::default()
    });
    match variant {
        MeasuredVariant::Plain | MeasuredVariant::Baseline => {}
        MeasuredVariant::MinMax => {
            style.min_width = Length::points(if index == 0 { 24.0 } else { 10.0 });
            style.max_width = Length::points(if index == 0 { 40.0 } else { 22.0 });
            style.min_height = Length::points(if index == 0 { 8.0 } else { 18.0 });
            style.max_height = Length::points(if index == 0 { 16.0 } else { 30.0 });
        }
        MeasuredVariant::AspectBorderBox => {
            style.box_sizing = BoxSizing::BorderBox;
            style.width = Length::points(if index == 0 { 36.0 } else { 28.0 });
            style.aspect_ratio = Some(if index == 0 { 2.0 } else { 1.25 });
        }
    }
    style
}

fn measured_child_size(variant: MeasuredVariant, index: usize) -> Size {
    match variant {
        MeasuredVariant::Plain => [Size::new(21.0, 13.0), Size::new(17.0, 19.0)][index],
        MeasuredVariant::Baseline => [Size::new(20.0, 18.0), Size::new(24.0, 16.0)][index],
        MeasuredVariant::MinMax => [Size::new(12.0, 12.0), Size::new(34.0, 12.0)][index],
        MeasuredVariant::AspectBorderBox => [Size::new(10.0, 40.0), Size::new(30.0, 12.0)][index],
    }
}

fn container_style_from_base(container: GeneratedContainer, base: Style) -> Style {
    match container {
        GeneratedContainer::Block => block_style(base),
        GeneratedContainer::FlexRow => flex_style(base),
        GeneratedContainer::FlexColumnRtl => flex_style(Style {
            direction: Direction::Rtl,
            flex_direction: FlexDirection::Column,
            ..base
        }),
        GeneratedContainer::LinearRow => linear_style(Style {
            linear_orientation: LinearOrientation::Horizontal,
            ..base
        }),
        GeneratedContainer::LinearColumnRtl => linear_style(Style {
            direction: Direction::Rtl,
            linear_orientation: LinearOrientation::Vertical,
            ..base
        }),
        GeneratedContainer::Relative => relative_style(base),
        GeneratedContainer::Grid => grid_style(Style {
            grid_template_columns: vec![Length::points(38.0), Length::points(36.0)],
            grid_template_rows: vec![Length::points(26.0), Length::points(24.0)],
            column_gap: Length::points(4.0),
            row_gap: Length::points(3.0),
            ..base
        }),
    }
}

pub(crate) fn flex_baseline_propagation_tree(
    constraint_mode: BaselineConstraintMode,
    trigger: BaselineTrigger,
    source: BaselineSource,
) -> (SimpleTree, usize, Constraints) {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(flex_baseline_root_style(
        constraint_mode,
        trigger,
    )));
    let reference = tree.push(SimpleNode::with_measured_size_and_baseline(
        block_style(Style {
            margin: Rect::new(
                Length::points(1.0),
                Length::points(2.0),
                Length::points(2.0),
                Length::points(1.0),
            ),
            ..baseline_trigger_style(trigger)
        }),
        Size::new(11.0, 38.0),
        31.0,
    ));
    let candidate = append_baseline_source(&mut tree, source, trigger);
    let trailing = tree.push(SimpleNode::with_measured_size(
        block_style(Style {
            width: Length::points(9.0),
            height: Length::points(13.0),
            margin: Rect::new(
                Length::ZERO,
                Length::points(1.0),
                Length::points(1.0),
                Length::points(3.0),
            ),
            ..Style::default()
        }),
        Size::new(9.0, 13.0),
    ));
    for child in [reference, candidate, trailing] {
        tree.append_child(root, child);
    }
    (tree, root, flex_baseline_constraints(constraint_mode))
}

fn flex_baseline_root_style(
    constraint_mode: BaselineConstraintMode,
    trigger: BaselineTrigger,
) -> Style {
    let mut style = Style {
        flex_direction: FlexDirection::Row,
        align_items: match trigger {
            BaselineTrigger::ContainerAlignItems => AlignItems::Baseline,
            BaselineTrigger::ChildAlignSelf => AlignItems::FlexStart,
        },
        justify_content: JustifyContent::FlexStart,
        padding: Rect::new(
            Length::points(2.0),
            Length::points(3.0),
            Length::points(4.0),
            Length::points(5.0),
        ),
        border: Rect::new(1.0, 2.0, 1.0, 2.0),
        ..Style::default()
    };
    if matches!(constraint_mode, BaselineConstraintMode::DefiniteRoot) {
        style.width = Length::points(118.0);
        style.height = Length::points(76.0);
    }
    flex_style(style)
}

fn flex_baseline_constraints(mode: BaselineConstraintMode) -> Constraints {
    match mode {
        BaselineConstraintMode::DefiniteRoot => Constraints::definite(118.0, 76.0),
        BaselineConstraintMode::AtMostOwner => Constraints::new(
            SideConstraint::at_most(118.0),
            SideConstraint::at_most(76.0),
        ),
        BaselineConstraintMode::IndefiniteOwner => Constraints::indefinite(),
    }
}

fn baseline_trigger_style(trigger: BaselineTrigger) -> Style {
    Style {
        align_self: match trigger {
            BaselineTrigger::ContainerAlignItems => None,
            BaselineTrigger::ChildAlignSelf => Some(AlignItems::Baseline),
        },
        ..Style::default()
    }
}

fn append_baseline_source(
    tree: &mut SimpleTree,
    source: BaselineSource,
    trigger: BaselineTrigger,
) -> usize {
    match source {
        BaselineSource::MeasuredLeaf => tree.push(SimpleNode::with_measured_size_and_baseline(
            block_style(Style {
                margin: Rect::new(
                    Length::points(2.0),
                    Length::points(1.0),
                    Length::points(1.0),
                    Length::points(2.0),
                ),
                ..baseline_trigger_style(trigger)
            }),
            Size::new(18.0, 24.0),
            17.0,
        )),
        BaselineSource::NestedFlex => {
            let nested = tree.push(SimpleNode::new(flex_style(Style {
                align_items: AlignItems::Baseline,
                margin: Rect::new(
                    Length::points(1.0),
                    Length::points(1.0),
                    Length::points(2.0),
                    Length::points(2.0),
                ),
                ..baseline_trigger_style(trigger)
            })));
            append_nested_baseline_children(tree, nested, 6.0, 19.0);
            nested
        }
        BaselineSource::NestedFlexColumn | BaselineSource::NestedFlexColumnReverse => {
            let nested = tree.push(SimpleNode::new(flex_style(Style {
                flex_direction: match source {
                    BaselineSource::NestedFlexColumn => FlexDirection::Column,
                    BaselineSource::NestedFlexColumnReverse => FlexDirection::ColumnReverse,
                    _ => unreachable!(),
                },
                justify_content: JustifyContent::Center,
                width: Length::points(26.0),
                height: Length::points(48.0),
                align_items: AlignItems::FlexStart,
                margin: Rect::new(
                    Length::points(2.0),
                    Length::points(1.0),
                    Length::points(2.0),
                    Length::points(1.0),
                ),
                ..baseline_trigger_style(trigger)
            })));
            append_nested_baseline_children(tree, nested, 7.0, 16.0);
            nested
        }
        BaselineSource::NestedLinear => {
            let nested = tree.push(SimpleNode::new(linear_style(Style {
                linear_orientation: LinearOrientation::Horizontal,
                margin: Rect::new(
                    Length::points(2.0),
                    Length::points(1.0),
                    Length::points(1.0),
                    Length::points(3.0),
                ),
                ..baseline_trigger_style(trigger)
            })));
            append_nested_baseline_children(tree, nested, 8.0, 21.0);
            nested
        }
        BaselineSource::NestedLinearVertical | BaselineSource::NestedLinearVerticalReverse => {
            let nested = tree.push(SimpleNode::new(linear_style(Style {
                linear_orientation: match source {
                    BaselineSource::NestedLinearVertical => LinearOrientation::Vertical,
                    BaselineSource::NestedLinearVerticalReverse => {
                        LinearOrientation::VerticalReverse
                    }
                    _ => unreachable!(),
                },
                linear_gravity: LinearGravity::CenterVertical,
                width: Length::points(25.0),
                height: Length::points(52.0),
                margin: Rect::new(
                    Length::points(1.0),
                    Length::points(3.0),
                    Length::points(2.0),
                    Length::points(1.0),
                ),
                ..baseline_trigger_style(trigger)
            })));
            append_nested_baseline_children(tree, nested, 9.0, 18.0);
            nested
        }
        BaselineSource::NestedGridFallback => {
            let nested = tree.push(SimpleNode::new(grid_style(Style {
                width: Length::points(24.0),
                height: Length::points(18.0),
                grid_template_columns: vec![Length::points(24.0)],
                grid_template_rows: vec![Length::points(18.0)],
                align_items: AlignItems::Baseline,
                margin: Rect::new(
                    Length::points(1.0),
                    Length::points(2.0),
                    Length::points(3.0),
                    Length::points(1.0),
                ),
                ..baseline_trigger_style(trigger)
            })));
            let child = tree.push(SimpleNode::with_measured_size_and_baseline(
                block_style(Style {
                    width: Length::points(10.0),
                    height: Length::points(8.0),
                    grid_column_start: Some(1),
                    grid_row_start: Some(1),
                    ..Style::default()
                }),
                Size::new(10.0, 8.0),
                5.0,
            ));
            tree.append_child(nested, child);
            nested
        }
        BaselineSource::NestedRelativeFallback => {
            let nested = tree.push(SimpleNode::new(relative_style(Style {
                width: Length::points(22.0),
                height: Length::points(16.0),
                margin: Rect::new(
                    Length::points(2.0),
                    Length::points(2.0),
                    Length::points(2.0),
                    Length::points(1.0),
                ),
                ..baseline_trigger_style(trigger)
            })));
            let child = tree.push(SimpleNode::with_measured_size(
                block_style(Style {
                    relative_align_left: RELATIVE_ALIGN_PARENT,
                    relative_align_top: RELATIVE_ALIGN_PARENT,
                    ..Style::default()
                }),
                Size::new(12.0, 9.0),
            ));
            tree.append_child(nested, child);
            nested
        }
    }
}

fn append_nested_baseline_children(
    tree: &mut SimpleTree,
    nested: usize,
    first_baseline: f32,
    second_baseline: f32,
) {
    let first = tree.push(SimpleNode::with_measured_size_and_baseline(
        block_style(Style {
            margin: Rect::new(
                Length::points(1.0),
                Length::ZERO,
                Length::points(1.0),
                Length::points(2.0),
            ),
            ..Style::default()
        }),
        Size::new(10.0, 18.0),
        first_baseline,
    ));
    let second = tree.push(SimpleNode::with_measured_size_and_baseline(
        block_style(Style {
            margin: Rect::new(
                Length::ZERO,
                Length::points(1.0),
                Length::points(2.0),
                Length::points(1.0),
            ),
            ..Style::default()
        }),
        Size::new(12.0, 24.0),
        second_baseline,
    ));
    tree.append_child(nested, first);
    tree.append_child(nested, second);
}

fn append_linear_matrix_children(tree: &mut SimpleTree, root: usize) {
    for (index, (width, height)) in [(16.0, 10.0), (22.0, 12.0), (18.0, 14.0)]
        .into_iter()
        .enumerate()
    {
        let child = tree.push(SimpleNode::new(block_style(Style {
            width: Length::points(width),
            height: Length::points(height),
            margin: Rect::new(
                Length::points(index as f32),
                Length::points((2 - index) as f32),
                Length::points((index % 2) as f32),
                Length::ZERO,
            ),
            ..Style::default()
        })));
        tree.append_child(root, child);
    }
}

pub(crate) fn linear_orientation_tree(
    linear_orientation: LinearOrientation,
    direction: Direction,
    justify_content: JustifyContent,
) -> (SimpleTree, usize) {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(linear_style(Style {
        direction,
        linear_orientation,
        justify_content,
        width: Length::points(120.0),
        height: Length::points(90.0),
        padding: Rect::new(
            Length::points(2.0),
            Length::points(3.0),
            Length::points(5.0),
            Length::points(7.0),
        ),
        ..Style::default()
    })));
    append_linear_matrix_children(&mut tree, root);
    (tree, root)
}

pub(crate) fn linear_gravity_tree(
    linear_orientation: LinearOrientation,
    direction: Direction,
    linear_gravity: LinearGravity,
) -> (SimpleTree, usize) {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(linear_style(Style {
        direction,
        linear_orientation,
        linear_gravity,
        justify_content: JustifyContent::FlexEnd,
        width: Length::points(120.0),
        height: Length::points(90.0),
        padding: Rect::new(
            Length::points(2.0),
            Length::points(3.0),
            Length::points(5.0),
            Length::points(7.0),
        ),
        ..Style::default()
    })));
    append_linear_matrix_children(&mut tree, root);
    (tree, root)
}

pub(crate) fn linear_layout_gravity_tree(
    linear_orientation: LinearOrientation,
    direction: Direction,
    linear_layout_gravity: LinearLayoutGravity,
) -> (SimpleTree, usize) {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(linear_style(Style {
        direction,
        linear_orientation,
        align_items: AlignItems::Stretch,
        linear_cross_gravity: LinearCrossGravity::Center,
        width: Length::points(120.0),
        height: Length::points(90.0),
        padding: Rect::new(
            Length::points(2.0),
            Length::points(3.0),
            Length::points(5.0),
            Length::points(7.0),
        ),
        ..Style::default()
    })));
    for (index, (width, height)) in [(16.0, 10.0), (22.0, 12.0), (18.0, 14.0)]
        .into_iter()
        .enumerate()
    {
        let child = tree.push(SimpleNode::new(block_style(Style {
            width: Length::points(width),
            height: Length::points(height),
            linear_layout_gravity: if index == 1 {
                linear_layout_gravity
            } else {
                LinearLayoutGravity::None
            },
            margin: Rect::new(
                Length::points(index as f32),
                Length::points((2 - index) as f32),
                Length::points((index % 2) as f32),
                Length::ZERO,
            ),
            ..Style::default()
        })));
        tree.append_child(root, child);
    }
    (tree, root)
}

pub(crate) fn linear_cross_gravity_tree(
    linear_orientation: LinearOrientation,
    direction: Direction,
    linear_cross_gravity: LinearCrossGravity,
) -> (SimpleTree, usize) {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(linear_style(Style {
        direction,
        linear_orientation,
        align_items: AlignItems::FlexStart,
        linear_cross_gravity,
        width: Length::points(120.0),
        height: Length::points(90.0),
        padding: Rect::new(
            Length::points(2.0),
            Length::points(3.0),
            Length::points(5.0),
            Length::points(7.0),
        ),
        ..Style::default()
    })));
    append_linear_matrix_children(&mut tree, root);
    (tree, root)
}

pub(crate) fn linear_css_alignment_tree(
    linear_orientation: LinearOrientation,
    direction: Direction,
    align_items: AlignItems,
    align_self: Option<AlignItems>,
) -> (SimpleTree, usize) {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(linear_style(Style {
        direction,
        linear_orientation,
        align_items,
        linear_cross_gravity: LinearCrossGravity::None,
        width: Length::points(120.0),
        height: Length::points(90.0),
        padding: Rect::new(
            Length::points(2.0),
            Length::points(3.0),
            Length::points(5.0),
            Length::points(7.0),
        ),
        border: Rect::new(1.0, 2.0, 1.0, 2.0),
        ..Style::default()
    })));
    for (index, (main, cross)) in [(16.0, 10.0), (22.0, 0.0), (18.0, 14.0)]
        .into_iter()
        .enumerate()
    {
        let child_style = linear_axis_child_style(
            linear_orientation,
            Length::points(main),
            if cross == 0.0 {
                Length::Auto
            } else {
                Length::points(cross)
            },
            Style {
                align_self: (index == 1).then_some(align_self).flatten(),
                margin: linear_cross_margin(
                    linear_orientation,
                    Length::points(index as f32),
                    Length::points((2 - index) as f32),
                ),
                padding: Rect::all(Length::points(1.0)),
                border: Rect::all(1.0),
                ..Style::default()
            },
        );
        let child = tree.push(SimpleNode::new(child_style));
        tree.append_child(root, child);
    }
    (tree, root)
}

fn linear_axis_child_style(
    linear_orientation: LinearOrientation,
    main_size: Length,
    cross_size: Length,
    style: Style,
) -> Style {
    let mut style = block_style(style);
    if linear_orientation.is_row() {
        style.width = main_size;
        style.height = cross_size;
    } else {
        style.width = cross_size;
        style.height = main_size;
    }
    style
}

fn linear_axis_min_max_style(
    linear_orientation: LinearOrientation,
    mut style: Style,
    min_main: Length,
    max_main: Length,
) -> Style {
    if linear_orientation.is_row() {
        style.min_width = min_main;
        style.max_width = max_main;
    } else {
        style.min_height = min_main;
        style.max_height = max_main;
    }
    style
}

fn linear_main_margin(orientation: LinearOrientation, start: f32, end: f32) -> Rect<Length> {
    if orientation.is_row() {
        Rect::new(
            Length::points(start),
            Length::points(end),
            Length::ZERO,
            Length::ZERO,
        )
    } else {
        Rect::new(
            Length::ZERO,
            Length::ZERO,
            Length::points(start),
            Length::points(end),
        )
    }
}

fn linear_cross_margin(orientation: LinearOrientation, start: Length, end: Length) -> Rect<Length> {
    if orientation.is_row() {
        Rect::new(Length::ZERO, Length::ZERO, start, end)
    } else {
        Rect::new(start, end, Length::ZERO, Length::ZERO)
    }
}

pub(crate) fn linear_edge_case_tree(
    orientation: LinearOrientation,
    direction: Direction,
    mode: LinearConstraintMode,
    pattern: LinearEdgePattern,
) -> (SimpleTree, usize, Constraints) {
    let mut tree = SimpleTree::default();
    let (mut root_style, constraints) = linear_edge_root_style(orientation, direction, mode);
    match pattern {
        LinearEdgePattern::WeightedMinMax => {
            root_style.linear_cross_gravity = LinearCrossGravity::Stretch;
        }
        LinearEdgePattern::WeightSumMainGravity => {
            root_style.linear_gravity = LinearGravity::End;
            root_style.linear_weight_sum = 4.0;
        }
        LinearEdgePattern::LayoutGravityOverride => {
            root_style.align_items = AlignItems::Stretch;
            root_style.linear_cross_gravity = LinearCrossGravity::End;
        }
        LinearEdgePattern::CrossAutoMarginBaseline => {
            root_style.align_items = AlignItems::FlexStart;
            root_style.linear_cross_gravity = LinearCrossGravity::End;
        }
    }
    let root = tree.push(SimpleNode::new(linear_style(root_style)));
    match pattern {
        LinearEdgePattern::WeightedMinMax => {
            append_linear_weighted_minmax_children(&mut tree, root, orientation);
        }
        LinearEdgePattern::WeightSumMainGravity => {
            append_linear_weight_sum_children(&mut tree, root, orientation);
        }
        LinearEdgePattern::LayoutGravityOverride => {
            append_linear_layout_gravity_children(&mut tree, root, orientation);
        }
        LinearEdgePattern::CrossAutoMarginBaseline => {
            append_linear_auto_margin_baseline_children(&mut tree, root, orientation);
        }
    }
    (tree, root, constraints)
}

fn linear_edge_root_style(
    orientation: LinearOrientation,
    direction: Direction,
    mode: LinearConstraintMode,
) -> (Style, Constraints) {
    let mut style = Style {
        direction,
        linear_orientation: orientation,
        width: Length::Auto,
        height: Length::Auto,
        padding: Rect::new(
            Length::points(3.0),
            Length::points(2.0),
            Length::points(4.0),
            Length::points(1.0),
        ),
        border: Rect::new(1.0, 2.0, 1.0, 2.0),
        ..Style::default()
    };
    let constraints = match mode {
        LinearConstraintMode::DefiniteRoot => {
            style.width = Length::points(142.0);
            style.height = Length::points(96.0);
            Constraints::definite(142.0, 96.0)
        }
        LinearConstraintMode::AtMostOwner => Constraints::new(
            SideConstraint::at_most(142.0),
            SideConstraint::at_most(96.0),
        ),
        LinearConstraintMode::IndefiniteOwner => Constraints::indefinite(),
    };
    (style, constraints)
}

fn append_linear_weighted_minmax_children(
    tree: &mut SimpleTree,
    root: usize,
    orientation: LinearOrientation,
) {
    let fixed = tree.push(SimpleNode::new(linear_axis_child_style(
        orientation,
        Length::points(18.0),
        Length::points(13.0),
        Style {
            margin: linear_main_margin(orientation, 1.0, 2.0),
            ..Style::default()
        },
    )));
    let capped = tree.push(SimpleNode::new(linear_axis_child_style(
        orientation,
        Length::Auto,
        Length::Auto,
        linear_axis_min_max_style(
            orientation,
            Style {
                linear_weight: 1.0,
                margin: linear_main_margin(orientation, 2.0, 1.0),
                ..Style::default()
            },
            Length::Auto,
            Length::percent(30.0),
        ),
    )));
    let floored = tree.push(SimpleNode::new(linear_axis_child_style(
        orientation,
        Length::Auto,
        Length::points(9.0),
        linear_axis_min_max_style(
            orientation,
            Style {
                linear_weight: 2.0,
                margin: linear_main_margin(orientation, 0.0, 3.0),
                ..Style::default()
            },
            Length::points(34.0),
            Length::Auto,
        ),
    )));
    for child in [fixed, capped, floored] {
        tree.append_child(root, child);
    }
}

fn append_linear_weight_sum_children(
    tree: &mut SimpleTree,
    root: usize,
    orientation: LinearOrientation,
) {
    let leading = tree.push(SimpleNode::new(linear_axis_child_style(
        orientation,
        Length::points(12.0),
        Length::points(11.0),
        Style {
            order: 1,
            ..Style::default()
        },
    )));
    let weighted_a = tree.push(SimpleNode::new(linear_axis_child_style(
        orientation,
        Length::Auto,
        Length::points(10.0),
        Style {
            linear_weight: 1.0,
            order: 0,
            ..Style::default()
        },
    )));
    let weighted_b = tree.push(SimpleNode::new(linear_axis_child_style(
        orientation,
        Length::Auto,
        Length::points(14.0),
        Style {
            linear_weight: 1.0,
            order: 2,
            ..Style::default()
        },
    )));
    for child in [leading, weighted_a, weighted_b] {
        tree.append_child(root, child);
    }
}

fn append_linear_layout_gravity_children(
    tree: &mut SimpleTree,
    root: usize,
    orientation: LinearOrientation,
) {
    let start = tree.push(SimpleNode::new(linear_axis_child_style(
        orientation,
        Length::points(15.0),
        Length::points(10.0),
        Style {
            linear_layout_gravity: LinearLayoutGravity::Start,
            ..Style::default()
        },
    )));
    let center = tree.push(SimpleNode::new(linear_axis_child_style(
        orientation,
        Length::points(17.0),
        Length::points(12.0),
        Style {
            linear_layout_gravity: LinearLayoutGravity::Center,
            ..Style::default()
        },
    )));
    let fill = tree.push(SimpleNode::new(linear_axis_child_style(
        orientation,
        Length::points(11.0),
        Length::Auto,
        Style {
            linear_layout_gravity: if orientation.is_row() {
                LinearLayoutGravity::FillVertical
            } else {
                LinearLayoutGravity::FillHorizontal
            },
            margin: linear_cross_margin(orientation, Length::points(1.0), Length::points(2.0)),
            ..Style::default()
        },
    )));
    for child in [start, center, fill] {
        tree.append_child(root, child);
    }
}

fn append_linear_auto_margin_baseline_children(
    tree: &mut SimpleTree,
    root: usize,
    orientation: LinearOrientation,
) {
    let measured = tree.push(SimpleNode::with_measured_size_and_baseline(
        linear_axis_child_style(
            orientation,
            Length::Auto,
            Length::Auto,
            Style {
                margin: linear_cross_margin(orientation, Length::Auto, Length::Auto),
                ..Style::default()
            },
        ),
        if orientation.is_row() {
            Size::new(22.0, 12.0)
        } else {
            Size::new(12.0, 22.0)
        },
        5.0,
    ));
    let fixed = tree.push(SimpleNode::new(linear_axis_child_style(
        orientation,
        Length::points(13.0),
        Length::points(9.0),
        Style::default(),
    )));
    for child in [measured, fixed] {
        tree.append_child(root, child);
    }
}

pub(crate) fn linear_composite_feature_tree(
    orientation: LinearOrientation,
    direction: Direction,
    mode: LinearConstraintMode,
) -> (SimpleTree, usize, Constraints) {
    let mut tree = SimpleTree::default();
    let (mut root_style, constraints) = linear_edge_root_style(orientation, direction, mode);
    root_style.linear_gravity = LinearGravity::SpaceBetween;
    root_style.linear_cross_gravity = LinearCrossGravity::Center;
    root_style.align_items = AlignItems::FlexStart;
    root_style.justify_content = JustifyContent::Center;
    root_style.min_width = Length::points(42.0);
    root_style.min_height = Length::points(36.0);
    let root = tree.push(SimpleNode::new(linear_style(root_style)));

    let measured = tree.push(SimpleNode::with_measured_size_and_baseline(
        linear_axis_child_style(
            orientation,
            Length::Auto,
            Length::Auto,
            Style {
                order: 2,
                margin: linear_main_margin(orientation, 1.0, 2.0),
                padding: Rect::all(Length::points(1.0)),
                border: Rect::all(1.0),
                ..Style::default()
            },
        ),
        if orientation.is_row() {
            Size::new(21.0, 13.0)
        } else {
            Size::new(13.0, 21.0)
        },
        7.0,
    ));
    let weighted = tree.push(SimpleNode::new(linear_axis_child_style(
        orientation,
        Length::Auto,
        Length::percent(42.0),
        linear_axis_min_max_style(
            orientation,
            Style {
                order: 0,
                linear_weight: 1.0,
                margin: linear_main_margin(orientation, 2.0, 1.0),
                padding: Rect::new(
                    Length::points(1.0),
                    Length::ZERO,
                    Length::points(2.0),
                    Length::ZERO,
                ),
                border: Rect::all(1.0),
                ..Style::default()
            },
            Length::points(18.0),
            Length::calc(16.0, 45.0),
        ),
    )));
    let aspect = tree.push(SimpleNode::new(linear_axis_child_style(
        orientation,
        Length::points(24.0),
        Length::Auto,
        Style {
            order: 1,
            box_sizing: BoxSizing::BorderBox,
            aspect_ratio: Some(1.5),
            linear_layout_gravity: if orientation.is_row() {
                LinearLayoutGravity::FillVertical
            } else {
                LinearLayoutGravity::FillHorizontal
            },
            margin: linear_cross_margin(orientation, Length::points(1.0), Length::points(2.0)),
            padding: Rect::all(Length::points(1.0)),
            border: Rect::all(1.0),
            ..Style::default()
        },
    )));
    let hidden = tree.push(SimpleNode::new(Style {
        display: Display::None,
        ..Style::default()
    }));
    let hidden_descendant = tree.push(SimpleNode::new(block_style(Style {
        width: Length::points(19.0),
        height: Length::points(7.0),
        ..Style::default()
    })));
    let absolute = tree.push(SimpleNode::new(block_style(Style {
        position: PositionType::Absolute,
        left: Length::percent(12.0),
        right: Length::calc(3.0, 10.0),
        top: Length::points(4.0),
        bottom: Length::Auto,
        width: Length::fit_content(Some(BaseLength::fixed_and_percent(5.0, 40.0))),
        height: Length::points(14.0),
        margin: Rect::all(Length::points(1.0)),
        padding: Rect::all(Length::points(1.0)),
        border: Rect::all(1.0),
        ..Style::default()
    })));
    for child in [measured, weighted, aspect, hidden, absolute] {
        tree.append_child(root, child);
    }
    tree.append_child(hidden, hidden_descendant);
    (tree, root, constraints)
}

pub(crate) fn sizing_minmax_aspect_tree(
    container: GeneratedContainer,
    variant: SizingVariant,
) -> (SimpleTree, usize) {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(sizing_container_style(container, variant)));
    append_sizing_children(&mut tree, root, variant);
    (tree, root)
}

fn sizing_container_style(container: GeneratedContainer, variant: SizingVariant) -> Style {
    let mut base = Style {
        width: Length::points(112.0),
        height: Length::points(78.0),
        padding: Rect::new(
            Length::points(3.0),
            Length::points(4.0),
            Length::points(5.0),
            Length::points(2.0),
        ),
        border: Rect::new(1.0, 2.0, 1.0, 3.0),
        justify_content: JustifyContent::Center,
        align_items: AlignItems::Center,
        ..Style::default()
    };
    match variant {
        SizingVariant::PercentCalcRoot => {
            base.width = Length::percent(72.0);
            base.height = Length::calc(8.0, 54.0);
        }
        SizingVariant::FitContentRoot => {
            base.width = Length::fit_content(Some(BaseLength::fixed_and_percent(6.0, 55.0)));
            base.height = Length::fit_content(Some(BaseLength::fixed_and_percent(4.0, 45.0)));
        }
        SizingVariant::FitContentSubtree | SizingVariant::IntrinsicMeasuredChild => {}
        SizingVariant::PercentMinMaxRoot | SizingVariant::BorderBoxPercentMinMaxRoot => {
            if matches!(variant, SizingVariant::BorderBoxPercentMinMaxRoot) {
                base.box_sizing = BoxSizing::BorderBox;
            }
            base.width = Length::Auto;
            base.height = Length::Auto;
            base.min_width = Length::percent(45.0);
            base.max_width = Length::calc(16.0, 70.0);
            base.min_height = Length::percent(35.0);
            base.max_height = Length::calc(10.0, 80.0);
        }
        SizingVariant::ContentBoxAspectRoot | SizingVariant::BorderBoxAspectRoot => {
            if matches!(variant, SizingVariant::BorderBoxAspectRoot) {
                base.box_sizing = BoxSizing::BorderBox;
            }
            base.width = Length::points(92.0);
            base.height = Length::Auto;
            base.aspect_ratio = Some(1.6);
        }
    }
    match container {
        GeneratedContainer::Block => block_style(base),
        GeneratedContainer::FlexRow => flex_style(base),
        GeneratedContainer::FlexColumnRtl => flex_style(Style {
            direction: Direction::Rtl,
            flex_direction: FlexDirection::Column,
            ..base
        }),
        GeneratedContainer::LinearRow => linear_style(Style {
            linear_orientation: LinearOrientation::Horizontal,
            ..base
        }),
        GeneratedContainer::LinearColumnRtl => linear_style(Style {
            direction: Direction::Rtl,
            linear_orientation: LinearOrientation::Vertical,
            ..base
        }),
        GeneratedContainer::Relative => relative_style(base),
        GeneratedContainer::Grid => grid_style(Style {
            grid_template_columns: vec![Length::points(34.0), Length::Auto],
            grid_template_rows: vec![Length::points(22.0), Length::Auto],
            column_gap: Length::points(3.0),
            row_gap: Length::points(4.0),
            ..base
        }),
    }
}

fn append_sizing_children(tree: &mut SimpleTree, root: usize, variant: SizingVariant) {
    if matches!(variant, SizingVariant::FitContentSubtree) {
        let child = tree.push(SimpleNode::new(block_style(Style {
            width: Length::fit_content(Some(BaseLength::fixed_and_percent(5.0, 50.0))),
            height: Length::fit_content(Some(BaseLength::fixed_and_percent(3.0, 40.0))),
            margin: Rect::new(
                Length::points(2.0),
                Length::points(1.0),
                Length::points(3.0),
                Length::points(2.0),
            ),
            ..Style::default()
        })));
        let grandchild = tree.push(SimpleNode::new(block_style(Style {
            width: Length::points(74.0),
            height: Length::points(26.0),
            padding: Rect::all(Length::points(1.0)),
            border: Rect::all(1.0),
            ..Style::default()
        })));
        let sibling = tree.push(SimpleNode::new(block_style(Style {
            width: Length::points(18.0),
            height: Length::points(12.0),
            ..Style::default()
        })));
        tree.append_child(root, child);
        tree.append_child(child, grandchild);
        tree.append_child(root, sibling);
        return;
    }
    if matches!(variant, SizingVariant::IntrinsicMeasuredChild) {
        let first = tree.push(SimpleNode::with_measured_size(
            block_style(Style {
                width: Length::max_content(),
                height: Length::fit_content(Some(BaseLength::fixed_and_percent(2.0, 40.0))),
                min_width: Length::points(18.0),
                max_width: Length::points(42.0),
                margin: Rect::new(
                    Length::points(1.0),
                    Length::points(2.0),
                    Length::points(1.0),
                    Length::points(2.0),
                ),
                ..Style::default()
            }),
            Size::new(35.0, 24.0),
        ));
        let second = tree.push(SimpleNode::with_measured_size(
            block_style(Style {
                width: Length::fit_content(Some(BaseLength::fixed_and_percent(3.0, 50.0))),
                height: Length::max_content(),
                min_height: Length::points(12.0),
                max_height: Length::points(30.0),
                ..Style::default()
            }),
            Size::new(26.0, 18.0),
        ));
        tree.append_child(root, first);
        tree.append_child(root, second);
        return;
    }
    let first = tree.push(SimpleNode::new(block_style(Style {
        width: Length::calc(4.0, 28.0),
        height: Length::percent(25.0),
        margin: Rect::new(
            Length::points(1.0),
            Length::points(2.0),
            Length::points(1.0),
            Length::ZERO,
        ),
        ..Style::default()
    })));
    let second = tree.push(SimpleNode::new(block_style(Style {
        width: Length::points(32.0),
        height: Length::points(18.0),
        box_sizing: if matches!(
            variant,
            SizingVariant::BorderBoxPercentMinMaxRoot | SizingVariant::BorderBoxAspectRoot
        ) {
            BoxSizing::BorderBox
        } else {
            BoxSizing::ContentBox
        },
        padding: Rect::all(Length::points(2.0)),
        border: Rect::all(1.0),
        ..Style::default()
    })));
    tree.append_child(root, first);
    tree.append_child(root, second);
}

pub(crate) fn display_none_origin_tree(container: GeneratedContainer) -> (SimpleTree, usize) {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(display_none_origin_container_style(
        container,
    )));
    let first = tree.push(SimpleNode::new(block_style(Style {
        width: Length::points(18.0),
        height: Length::points(10.0),
        margin: Rect::new(
            Length::points(1.0),
            Length::points(2.0),
            Length::points(1.0),
            Length::ZERO,
        ),
        ..Style::default()
    })));
    let hidden = tree.push(SimpleNode::new(Style {
        display: Display::None,
        ..Style::default()
    }));
    let hidden_descendant = tree.push(SimpleNode::new(block_style(Style {
        width: Length::points(40.0),
        height: Length::points(16.0),
        ..Style::default()
    })));
    let second = tree.push(SimpleNode::new(block_style(Style {
        width: Length::points(14.0),
        height: Length::points(12.0),
        margin: Rect::new(
            Length::points(2.0),
            Length::points(1.0),
            Length::ZERO,
            Length::points(1.0),
        ),
        ..Style::default()
    })));
    tree.append_child(root, first);
    tree.append_child(root, hidden);
    tree.append_child(hidden, hidden_descendant);
    tree.append_child(root, second);
    (tree, root)
}

fn display_none_origin_container_style(container: GeneratedContainer) -> Style {
    let base = Style {
        width: Length::points(118.0),
        height: Length::points(76.0),
        padding: Rect::new(
            Length::points(3.0),
            Length::points(4.0),
            Length::points(5.0),
            Length::points(6.0),
        ),
        border: Rect::new(2.0, 3.0, 4.0, 5.0),
        align_items: AlignItems::FlexStart,
        justify_content: JustifyContent::FlexStart,
        ..Style::default()
    };
    match container {
        GeneratedContainer::Grid => grid_style(Style {
            grid_template_columns: vec![Length::points(26.0), Length::points(24.0)],
            grid_template_rows: vec![Length::points(18.0), Length::points(16.0)],
            column_gap: Length::points(3.0),
            row_gap: Length::points(2.0),
            ..base
        }),
        _ => container_style_from_base(container, base),
    }
}

pub(crate) fn out_of_flow_position_tree(
    container: GeneratedContainer,
    position: PositionType,
    horizontal: OutOfFlowInset,
    vertical: OutOfFlowInset,
) -> (SimpleTree, usize) {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(generated_container_style(container)));
    let out_of_flow = tree.push(SimpleNode::new(out_of_flow_child_style(
        position, horizontal, vertical,
    )));
    tree.append_child(root, out_of_flow);
    (tree, root)
}

pub(crate) fn out_of_flow_sizing_tree(
    container: GeneratedContainer,
    position: PositionType,
    variant: OutOfFlowSizingVariant,
) -> (SimpleTree, usize) {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(generated_container_style(container)));
    let out_of_flow = tree.push(out_of_flow_sizing_child_node(position, variant));
    let sibling = tree.push(SimpleNode::new(block_style(Style {
        width: Length::points(13.0),
        height: Length::points(9.0),
        margin: Rect::new(
            Length::points(1.0),
            Length::points(2.0),
            Length::ZERO,
            Length::points(1.0),
        ),
        ..Style::default()
    })));
    tree.append_child(root, out_of_flow);
    tree.append_child(root, sibling);
    (tree, root)
}

fn out_of_flow_child_style(
    position: PositionType,
    horizontal: OutOfFlowInset,
    vertical: OutOfFlowInset,
) -> Style {
    let mut style = block_style(Style {
        position,
        width: if matches!(horizontal, OutOfFlowInset::Both) {
            Length::Auto
        } else {
            Length::points(18.0)
        },
        height: if matches!(vertical, OutOfFlowInset::Both) {
            Length::Auto
        } else {
            Length::points(12.0)
        },
        margin: Rect::new(
            Length::points(1.0),
            Length::points(2.0),
            Length::points(3.0),
            Length::points(4.0),
        ),
        padding: Rect::all(Length::points(1.0)),
        border: Rect::all(1.0),
        ..Style::default()
    });
    apply_horizontal_inset(&mut style, horizontal);
    apply_vertical_inset(&mut style, vertical);
    style
}

fn apply_horizontal_inset(style: &mut Style, inset: OutOfFlowInset) {
    match inset {
        OutOfFlowInset::None => {}
        OutOfFlowInset::Start => style.left = Length::points(9.0),
        OutOfFlowInset::End => style.right = Length::points(13.0),
        OutOfFlowInset::Both => {
            style.left = Length::points(9.0);
            style.right = Length::points(13.0);
        }
    }
}

fn apply_vertical_inset(style: &mut Style, inset: OutOfFlowInset) {
    match inset {
        OutOfFlowInset::None => {}
        OutOfFlowInset::Start => style.top = Length::points(7.0),
        OutOfFlowInset::End => style.bottom = Length::points(11.0),
        OutOfFlowInset::Both => {
            style.top = Length::points(7.0);
            style.bottom = Length::points(11.0);
        }
    }
}

fn out_of_flow_sizing_child_node(
    position: PositionType,
    variant: OutOfFlowSizingVariant,
) -> SimpleNode {
    let style = out_of_flow_sizing_child_style(position, variant);
    match variant {
        OutOfFlowSizingVariant::MinMaxMeasuredClamp => {
            SimpleNode::with_measured_size(style, Size::new(80.0, 10.0))
        }
        OutOfFlowSizingVariant::FitContentMeasured => {
            SimpleNode::with_measured_size(style, Size::new(92.0, 64.0))
        }
        OutOfFlowSizingVariant::AspectBorderBoxMeasured => {
            SimpleNode::with_measured_size(style, Size::new(50.0, 18.0))
        }
        OutOfFlowSizingVariant::OversizedFillAvailableMeasured => {
            SimpleNode::with_measure_func(style, generated_width_mode_sensitive_height_measure)
        }
        OutOfFlowSizingVariant::PercentCalc | OutOfFlowSizingVariant::FillAvailable => {
            SimpleNode::new(style)
        }
    }
}

fn generated_width_mode_sensitive_height_measure(constraints: Constraints) -> Size {
    let height = if constraints.width.is_definite() {
        17.0
    } else if constraints.width.mode == MeasureMode::AtMost {
        31.0
    } else {
        43.0
    };
    Size::new(11.0, height)
}

fn out_of_flow_sizing_child_style(
    position: PositionType,
    variant: OutOfFlowSizingVariant,
) -> Style {
    let mut style = block_style(Style {
        position,
        width: Length::points(18.0),
        height: Length::points(12.0),
        margin: Rect::new(
            Length::points(1.0),
            Length::points(2.0),
            Length::points(3.0),
            Length::points(4.0),
        ),
        padding: Rect::all(Length::points(1.0)),
        border: Rect::all(1.0),
        ..Style::default()
    });
    match variant {
        OutOfFlowSizingVariant::PercentCalc => {
            style.width = Length::calc(8.0, 45.0);
            style.height = Length::percent(40.0);
            style.left = Length::percent(10.0);
            style.top = Length::calc(2.0, 15.0);
        }
        OutOfFlowSizingVariant::FillAvailable => {
            style.width = Length::Auto;
            style.height = Length::Auto;
            style.left = Length::percent(10.0);
            style.right = Length::calc(3.0, 20.0);
            style.top = Length::calc(2.0, 15.0);
            style.bottom = Length::percent(25.0);
        }
        OutOfFlowSizingVariant::OversizedFillAvailableMeasured => {
            style.width = Length::Auto;
            style.height = Length::Auto;
            style.left = Length::percent(90.0);
            style.right = Length::calc(30.0, 80.0);
            style.top = Length::points(5.0);
        }
        OutOfFlowSizingVariant::MinMaxMeasuredClamp => {
            style.width = Length::Auto;
            style.height = Length::Auto;
            style.min_width = Length::percent(30.0);
            style.max_width = Length::calc(10.0, 40.0);
            style.min_height = Length::calc(4.0, 20.0);
            style.max_height = Length::percent(65.0);
            style.left = Length::points(7.0);
            style.top = Length::points(5.0);
        }
        OutOfFlowSizingVariant::FitContentMeasured => {
            style.width = Length::fit_content(Some(BaseLength::fixed_and_percent(5.0, 50.0)));
            style.height = Length::fit_content(Some(BaseLength::fixed_and_percent(4.0, 45.0)));
            style.right = Length::percent(12.0);
            style.bottom = Length::calc(1.0, 18.0);
        }
        OutOfFlowSizingVariant::AspectBorderBoxMeasured => {
            style.box_sizing = BoxSizing::BorderBox;
            style.width = Length::percent(42.0);
            style.height = Length::Auto;
            style.aspect_ratio = Some(1.6);
            style.left = Length::calc(3.0, 8.0);
            style.top = Length::percent(10.0);
        }
    }
    style
}

fn generated_container_style(container: GeneratedContainer) -> Style {
    let base = Style {
        width: Length::points(126.0),
        height: Length::points(92.0),
        padding: Rect::new(
            Length::points(3.0),
            Length::points(5.0),
            Length::points(7.0),
            Length::points(11.0),
        ),
        border: Rect::new(1.0, 2.0, 3.0, 4.0),
        justify_content: JustifyContent::Center,
        align_items: AlignItems::FlexEnd,
        ..Style::default()
    };
    match container {
        GeneratedContainer::Block => block_style(base),
        GeneratedContainer::FlexRow => flex_style(base),
        GeneratedContainer::FlexColumnRtl => flex_style(Style {
            direction: Direction::Rtl,
            flex_direction: FlexDirection::Column,
            flex_wrap: FlexWrap::WrapReverse,
            ..base
        }),
        GeneratedContainer::LinearRow => linear_style(Style {
            linear_orientation: LinearOrientation::Horizontal,
            ..base
        }),
        GeneratedContainer::LinearColumnRtl => linear_style(Style {
            direction: Direction::Rtl,
            linear_orientation: LinearOrientation::Vertical,
            ..base
        }),
        GeneratedContainer::Relative => relative_style(base),
        GeneratedContainer::Grid => grid_style(Style {
            grid_template_columns: vec![Length::points(50.0), Length::points(40.0)],
            grid_template_rows: vec![Length::points(30.0), Length::points(28.0)],
            column_gap: Length::points(4.0),
            row_gap: Length::points(6.0),
            ..base
        }),
    }
}

pub(crate) fn fixed_descendant_tree(
    root_container: GeneratedContainer,
    nested_container: GeneratedContainer,
    variant: FixedDescendantVariant,
) -> (SimpleTree, usize) {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(generated_container_style(root_container)));
    let nested = tree.push(SimpleNode::new(fixed_descendant_nested_style(
        nested_container,
    )));
    let wrapper = tree.push(SimpleNode::new(block_style(Style {
        width: Length::points(27.0),
        height: Length::points(22.0),
        margin: Rect::new(
            Length::points(1.0),
            Length::points(2.0),
            Length::points(1.0),
            Length::points(2.0),
        ),
        padding: Rect::all(Length::points(1.0)),
        ..Style::default()
    })));
    let fixed = tree.push(fixed_descendant_node(variant));
    let fixed_child = tree.push(SimpleNode::new(block_style(Style {
        width: Length::points(54.0),
        height: Length::points(18.0),
        margin: Rect::new(
            Length::points(1.0),
            Length::points(2.0),
            Length::points(1.0),
            Length::ZERO,
        ),
        ..Style::default()
    })));
    let nested_sibling = tree.push(SimpleNode::new(block_style(Style {
        width: Length::points(16.0),
        height: Length::points(12.0),
        margin: Rect::new(
            Length::points(1.0),
            Length::points(2.0),
            Length::points(3.0),
            Length::points(1.0),
        ),
        ..Style::default()
    })));
    let root_sibling = tree.push(SimpleNode::new(block_style(Style {
        width: Length::points(18.0),
        height: Length::points(10.0),
        margin: Rect::new(
            Length::points(2.0),
            Length::points(1.0),
            Length::ZERO,
            Length::points(2.0),
        ),
        ..Style::default()
    })));
    tree.append_child(root, nested);
    tree.append_child(root, root_sibling);
    tree.append_child(nested, wrapper);
    tree.append_child(nested, nested_sibling);
    tree.append_child(wrapper, fixed);
    if !matches!(variant, FixedDescendantVariant::MeasuredAspect) {
        tree.append_child(fixed, fixed_child);
    }
    (tree, root)
}

fn fixed_descendant_nested_style(container: GeneratedContainer) -> Style {
    let base = Style {
        width: Length::points(58.0),
        height: Length::points(42.0),
        padding: Rect::new(
            Length::points(2.0),
            Length::points(3.0),
            Length::points(4.0),
            Length::points(5.0),
        ),
        border: Rect::new(1.0, 2.0, 1.0, 2.0),
        justify_content: JustifyContent::FlexStart,
        align_items: AlignItems::FlexStart,
        ..Style::default()
    };
    match container {
        GeneratedContainer::Grid => grid_style(Style {
            grid_template_columns: vec![Length::points(24.0), Length::points(20.0)],
            grid_template_rows: vec![Length::points(16.0), Length::points(18.0)],
            column_gap: Length::points(3.0),
            row_gap: Length::points(2.0),
            ..base
        }),
        _ => container_style_from_base(container, base),
    }
}

fn fixed_descendant_node(variant: FixedDescendantVariant) -> SimpleNode {
    let style = fixed_descendant_style(variant);
    if matches!(variant, FixedDescendantVariant::MeasuredAspect) {
        SimpleNode::with_measured_size(style, Size::new(72.0, 31.0))
    } else {
        SimpleNode::new(style)
    }
}

fn fixed_descendant_style(variant: FixedDescendantVariant) -> Style {
    let mut style = block_style(Style {
        position: PositionType::Fixed,
        width: Length::points(18.0),
        height: Length::points(12.0),
        margin: Rect::new(
            Length::points(2.0),
            Length::points(3.0),
            Length::points(1.0),
            Length::points(4.0),
        ),
        padding: Rect::all(Length::points(1.0)),
        border: Rect::all(1.0),
        ..Style::default()
    });
    match variant {
        FixedDescendantVariant::PercentStart => {
            style.width = Length::percent(32.0);
            style.height = Length::percent(28.0);
            style.left = Length::percent(10.0);
            style.top = Length::percent(15.0);
        }
        FixedDescendantVariant::CalcEnd => {
            style.right = Length::calc(4.0, 7.0);
            style.bottom = Length::calc(3.0, 11.0);
        }
        FixedDescendantVariant::FillAvailable => {
            style.width = Length::Auto;
            style.height = Length::Auto;
            style.left = Length::percent(8.0);
            style.right = Length::calc(3.0, 12.0);
            style.top = Length::calc(2.0, 10.0);
            style.bottom = Length::percent(18.0);
        }
        FixedDescendantVariant::MeasuredAspect => {
            style.box_sizing = BoxSizing::BorderBox;
            style.width = Length::percent(40.0);
            style.height = Length::Auto;
            style.aspect_ratio = Some(2.0);
            style.left = Length::points(9.0);
            style.top = Length::points(6.0);
        }
        FixedDescendantVariant::FitContentSubtree => {
            style.width = Length::fit_content(Some(BaseLength::fixed(60.0)));
            style.height = Length::fit_content(Some(BaseLength::fixed(20.0)));
            style.left = Length::points(7.0);
            style.top = Length::points(9.0);
        }
    }
    style
}

pub(crate) fn sticky_position_tree(
    container: GeneratedContainer,
    inset_length: StickyInsetLength,
    horizontal: OutOfFlowInset,
    vertical: OutOfFlowInset,
) -> (SimpleTree, usize) {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(generated_container_style(container)));
    let sticky = tree.push(SimpleNode::new(sticky_child_style(
        inset_length,
        horizontal,
        vertical,
    )));
    let follower = tree.push(SimpleNode::new(block_style(Style {
        width: Length::points(11.0),
        height: Length::points(9.0),
        margin: Rect::new(
            Length::points(2.0),
            Length::points(1.0),
            Length::points(3.0),
            Length::points(4.0),
        ),
        ..Style::default()
    })));
    tree.append_child(root, sticky);
    tree.append_child(root, follower);
    (tree, root)
}

pub(crate) fn sticky_sizing_tree(
    container: GeneratedContainer,
    variant: StickySizingVariant,
) -> (SimpleTree, usize) {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(generated_container_style(container)));
    let sticky = tree.push(sticky_sizing_child_node(variant));
    let follower = tree.push(SimpleNode::new(block_style(Style {
        width: Length::points(13.0),
        height: Length::points(10.0),
        margin: Rect::new(
            Length::points(2.0),
            Length::points(1.0),
            Length::points(3.0),
            Length::points(2.0),
        ),
        ..Style::default()
    })));
    tree.append_child(root, sticky);
    tree.append_child(root, follower);
    (tree, root)
}

fn sticky_child_style(
    inset_length: StickyInsetLength,
    horizontal: OutOfFlowInset,
    vertical: OutOfFlowInset,
) -> Style {
    let mut style = block_style(Style {
        position: PositionType::Sticky,
        width: Length::points(18.0),
        height: Length::points(12.0),
        margin: Rect::new(
            Length::points(1.0),
            Length::points(2.0),
            Length::points(3.0),
            Length::points(4.0),
        ),
        padding: Rect::all(Length::points(1.0)),
        border: Rect::all(1.0),
        ..Style::default()
    });
    apply_horizontal_sticky_inset(&mut style, inset_length, horizontal);
    apply_vertical_sticky_inset(&mut style, inset_length, vertical);
    style
}

fn sticky_sizing_child_node(variant: StickySizingVariant) -> SimpleNode {
    let style = sticky_sizing_child_style(variant);
    match variant {
        StickySizingVariant::AutoMeasured => {
            SimpleNode::with_measured_size(style, Size::new(24.0, 16.0))
        }
        StickySizingVariant::MinMaxMeasuredClamp => {
            SimpleNode::with_measured_size(style, Size::new(80.0, 9.0))
        }
        StickySizingVariant::FitContentMeasured => {
            SimpleNode::with_measured_size(style, Size::new(74.0, 36.0))
        }
        StickySizingVariant::AspectBorderBoxMeasured => {
            SimpleNode::with_measured_size(style, Size::new(44.0, 18.0))
        }
        StickySizingVariant::PercentCalc => SimpleNode::new(style),
    }
}

fn sticky_sizing_child_style(variant: StickySizingVariant) -> Style {
    let mut style = block_style(Style {
        position: PositionType::Sticky,
        width: Length::points(18.0),
        height: Length::points(12.0),
        left: Length::calc(3.0, 10.0),
        top: Length::percent(20.0),
        margin: Rect::new(
            Length::points(1.0),
            Length::points(2.0),
            Length::points(2.0),
            Length::points(1.0),
        ),
        padding: Rect::all(Length::points(1.0)),
        border: Rect::all(1.0),
        ..Style::default()
    });
    match variant {
        StickySizingVariant::PercentCalc => {
            style.width = Length::calc(8.0, 35.0);
            style.height = Length::percent(32.0);
            style.right = Length::percent(12.0);
            style.bottom = Length::calc(1.0, 15.0);
        }
        StickySizingVariant::AutoMeasured => {
            style.width = Length::Auto;
            style.height = Length::Auto;
            style.right = Length::points(5.0);
            style.bottom = Length::points(4.0);
        }
        StickySizingVariant::MinMaxMeasuredClamp => {
            style.width = Length::Auto;
            style.height = Length::Auto;
            style.min_width = Length::percent(25.0);
            style.max_width = Length::calc(8.0, 35.0);
            style.min_height = Length::calc(3.0, 18.0);
            style.max_height = Length::percent(70.0);
        }
        StickySizingVariant::FitContentMeasured => {
            style.width = Length::fit_content(Some(BaseLength::fixed_and_percent(6.0, 45.0)));
            style.height = Length::fit_content(Some(BaseLength::fixed_and_percent(5.0, 35.0)));
            style.right = Length::percent(15.0);
            style.bottom = Length::calc(2.0, 12.0);
        }
        StickySizingVariant::AspectBorderBoxMeasured => {
            style.box_sizing = BoxSizing::BorderBox;
            style.width = Length::percent(38.0);
            style.height = Length::Auto;
            style.aspect_ratio = Some(1.5);
        }
    }
    style
}

fn apply_horizontal_sticky_inset(
    style: &mut Style,
    length: StickyInsetLength,
    inset: OutOfFlowInset,
) {
    match inset {
        OutOfFlowInset::None => {}
        OutOfFlowInset::Start => style.left = sticky_inset_length(length, 6.0, 10.0, 3.0, 10.0),
        OutOfFlowInset::End => style.right = sticky_inset_length(length, 8.0, 20.0, 4.0, 5.0),
        OutOfFlowInset::Both => {
            style.left = sticky_inset_length(length, 6.0, 10.0, 3.0, 10.0);
            style.right = sticky_inset_length(length, 8.0, 20.0, 4.0, 5.0);
        }
    }
}

fn apply_vertical_sticky_inset(
    style: &mut Style,
    length: StickyInsetLength,
    inset: OutOfFlowInset,
) {
    match inset {
        OutOfFlowInset::None => {}
        OutOfFlowInset::Start => style.top = sticky_inset_length(length, 7.0, 25.0, 2.0, 25.0),
        OutOfFlowInset::End => {
            style.bottom = sticky_inset_length(length, 11.0, 50.0, 1.0, 50.0);
        }
        OutOfFlowInset::Both => {
            style.top = sticky_inset_length(length, 7.0, 25.0, 2.0, 25.0);
            style.bottom = sticky_inset_length(length, 11.0, 50.0, 1.0, 50.0);
        }
    }
}

fn sticky_inset_length(
    variant: StickyInsetLength,
    points: f32,
    percent: f32,
    calc_fixed: f32,
    calc_percent: f32,
) -> Length {
    match variant {
        StickyInsetLength::Points => Length::points(points),
        StickyInsetLength::Percent => Length::percent(percent),
        StickyInsetLength::Calc => Length::calc(calc_fixed, calc_percent),
    }
}

#[derive(Clone, Debug)]
pub(crate) struct DeterministicRng {
    state: u64,
}

impl DeterministicRng {
    pub(crate) const fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    fn next_u32(&mut self) -> u32 {
        self.state = self
            .state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (self.state >> 32) as u32
    }

    fn range(&mut self, upper: usize) -> usize {
        debug_assert!(upper > 0);
        self.next_u32() as usize % upper
    }

    fn bool(&mut self) -> bool {
        self.range(2) == 0
    }

    fn points(&mut self, min: f32, step: f32, count: usize) -> f32 {
        min + step * self.range(count) as f32
    }
}

pub(crate) fn deterministic_supported_tree(
    rng: &mut DeterministicRng,
    case_index: usize,
) -> (SimpleTree, usize, Constraints) {
    let mut tree = SimpleTree::default();
    let root_display = match case_index % 3 {
        0 => Display::Block,
        1 => Display::Flex,
        _ => Display::Linear,
    };
    let root = tree.push(SimpleNode::new(random_container_style(
        rng,
        root_display,
        case_index,
    )));
    let child_count = 3 + rng.range(3);
    for child_index in 0..child_count {
        let child_display = random_child_display(rng, child_index);
        let child = tree.push(SimpleNode::new(random_child_style(
            rng,
            child_display,
            child_index,
        )));
        tree.append_child(root, child);
        if matches!(
            child_display,
            Display::Block | Display::Flex | Display::Linear
        ) && rng.range(4) == 0
        {
            append_random_grandchildren(rng, &mut tree, child, child_index);
        }
    }
    let constraints = match case_index % 3 {
        0 => Constraints::definite(160.0, 120.0),
        1 => Constraints::new(
            SideConstraint::at_most(180.0),
            SideConstraint::at_most(140.0),
        ),
        _ => Constraints::indefinite(),
    };
    (tree, root, constraints)
}

fn append_random_grandchildren(
    rng: &mut DeterministicRng,
    tree: &mut SimpleTree,
    parent: usize,
    child_index: usize,
) {
    for grandchild_index in 0..(1 + rng.range(2)) {
        let mut style = random_child_style(rng, Display::Block, grandchild_index);
        style.position = PositionType::Static;
        style.order = grandchild_index as i32;
        if child_index.is_multiple_of(2) {
            style.width = Length::points(rng.points(8.0, 3.0, 5));
        }
        let node = tree.push(SimpleNode::new(style));
        tree.append_child(parent, node);
    }
}

fn random_container_style(
    rng: &mut DeterministicRng,
    display: Display,
    _case_index: usize,
) -> Style {
    let mut style = random_base_style(rng, display);
    style.width = random_axis_length(rng, true);
    style.height = random_axis_length(rng, true);
    style.min_width = Length::points(20.0);
    style.min_height = Length::points(16.0);
    style.padding = random_edge_lengths(rng, false);
    style.border = Rect::new(
        rng.range(2) as f32,
        rng.range(2) as f32,
        rng.range(2) as f32,
        rng.range(2) as f32,
    );
    match display {
        Display::Flex => {
            style.flex_direction = [
                FlexDirection::Row,
                FlexDirection::Column,
                FlexDirection::RowReverse,
                FlexDirection::ColumnReverse,
            ][rng.range(4)];
            style.flex_wrap =
                [FlexWrap::NoWrap, FlexWrap::Wrap, FlexWrap::WrapReverse][rng.range(3)];
            style.align_items = random_align_items(rng);
            style.align_content = random_align_content(rng);
            style.justify_content = random_justify_content(rng);
        }
        Display::Linear => {
            style.linear_orientation = [
                LinearOrientation::Horizontal,
                LinearOrientation::Vertical,
                LinearOrientation::HorizontalReverse,
                LinearOrientation::VerticalReverse,
            ][rng.range(4)];
            style.linear_gravity = [
                LinearGravity::None,
                LinearGravity::Center,
                LinearGravity::SpaceBetween,
                LinearGravity::Start,
                LinearGravity::End,
            ][rng.range(5)];
            style.linear_layout_gravity = [
                LinearLayoutGravity::None,
                LinearLayoutGravity::Center,
                LinearLayoutGravity::Stretch,
                LinearLayoutGravity::Start,
                LinearLayoutGravity::End,
            ][rng.range(5)];
        }
        Display::None | Display::Block | Display::Relative | Display::Grid => {}
    }
    style
}

fn random_child_style(rng: &mut DeterministicRng, display: Display, child_index: usize) -> Style {
    let mut style = random_base_style(rng, display);
    style.width = random_axis_length(rng, false);
    style.height = random_axis_length(rng, false);
    let (min_width, max_width) = random_coherent_minmax_lengths(rng);
    let (min_height, max_height) = random_coherent_minmax_lengths(rng);
    style.min_width = min_width;
    style.min_height = min_height;
    style.max_width = max_width;
    style.max_height = max_height;
    style.margin = random_edge_lengths(rng, true);
    style.padding = random_edge_lengths(rng, false);
    style.border = Rect::all(rng.range(2) as f32);
    style.order = child_index as i32 - 1;
    style.flex_basis = random_axis_length(rng, false);
    style.flex_grow = if rng.bool() { 1.0 } else { 0.0 };
    style.flex_shrink = if rng.bool() { 1.0 } else { 0.0 };
    style.linear_weight = 0.0;
    style.align_self = if rng.range(3) == 0 {
        Some(random_align_items(rng))
    } else {
        None
    };
    style.justify_self = random_justify_items(rng);
    style
}

fn random_base_style(rng: &mut DeterministicRng, display: Display) -> Style {
    Style {
        display,
        box_sizing: if rng.bool() {
            BoxSizing::ContentBox
        } else {
            BoxSizing::BorderBox
        },
        direction: [Direction::Ltr, Direction::Rtl][rng.range(2)],
        row_gap: random_gap_length(rng),
        column_gap: random_gap_length(rng),
        ..Style::default()
    }
}

fn random_child_display(rng: &mut DeterministicRng, child_index: usize) -> Display {
    if child_index == 1 && rng.range(5) == 0 {
        return Display::None;
    }
    [Display::Block, Display::Flex, Display::Linear][rng.range(3)]
}

const RANDOM_AXIS_LENGTH_VARIANT_COUNT: usize = 4;

fn random_axis_length(rng: &mut DeterministicRng, _prefer_definite: bool) -> Length {
    let variant = rng.range(RANDOM_AXIS_LENGTH_VARIANT_COUNT);
    random_axis_length_for_variant(rng, variant)
}

fn random_axis_length_for_variant(rng: &mut DeterministicRng, variant: usize) -> Length {
    match variant {
        0 => Length::Auto,
        1 => Length::points(rng.points(18.0, 6.0, 10)),
        2 => Length::percent(rng.points(20.0, 10.0, 6)),
        3 => Length::fr(rng.points(1.0, 1.0, 4)),
        _ => unreachable!(),
    }
}

const RANDOM_MINMAX_LENGTH_VARIANT_COUNT: usize = 6;

fn random_coherent_minmax_lengths(rng: &mut DeterministicRng) -> (Length, Length) {
    let variant = rng.range(RANDOM_MINMAX_LENGTH_VARIANT_COUNT);
    random_coherent_minmax_lengths_for_variant(rng, variant)
}

fn random_coherent_minmax_lengths_for_variant(
    rng: &mut DeterministicRng,
    variant: usize,
) -> (Length, Length) {
    match variant {
        0 => (Length::Auto, Length::Auto),
        1 => (Length::points(rng.points(8.0, 4.0, 4)), Length::Auto),
        2 => {
            let min = rng.points(8.0, 4.0, 4);
            (
                Length::points(min),
                Length::points(min + rng.points(16.0, 4.0, 4)),
            )
        }
        3 => (Length::Auto, Length::points(rng.points(32.0, 8.0, 4))),
        4 => (Length::fr(rng.points(4.0, 2.0, 4)), Length::Auto),
        5 => {
            let min = rng.points(4.0, 2.0, 4);
            (Length::fr(min), Length::fr(min + rng.points(12.0, 2.0, 4)))
        }
        _ => unreachable!(),
    }
}

fn random_edge_lengths(rng: &mut DeterministicRng, allow_auto: bool) -> Rect<Length> {
    fn edge(rng: &mut DeterministicRng, allow_auto: bool) -> Length {
        let _ = allow_auto;
        match rng.range(2) {
            0 => Length::ZERO,
            _ => Length::points(rng.points(1.0, 2.0, 4)),
        }
    }
    Rect::new(
        edge(rng, allow_auto),
        edge(rng, allow_auto),
        edge(rng, allow_auto),
        edge(rng, allow_auto),
    )
}

fn random_gap_length(rng: &mut DeterministicRng) -> Length {
    match rng.range(2) {
        0 => Length::ZERO,
        _ => Length::points(rng.points(1.0, 2.0, 4)),
    }
}

fn random_justify_content(rng: &mut DeterministicRng) -> JustifyContent {
    [
        JustifyContent::FlexStart,
        JustifyContent::Center,
        JustifyContent::FlexEnd,
        JustifyContent::SpaceBetween,
        JustifyContent::SpaceAround,
        JustifyContent::SpaceEvenly,
        JustifyContent::Start,
        JustifyContent::End,
    ][rng.range(8)]
}

fn random_align_items(rng: &mut DeterministicRng) -> AlignItems {
    [
        AlignItems::Stretch,
        AlignItems::FlexStart,
        AlignItems::Center,
        AlignItems::FlexEnd,
        AlignItems::Start,
        AlignItems::End,
    ][rng.range(6)]
}

fn random_align_content(rng: &mut DeterministicRng) -> AlignContent {
    [
        AlignContent::FlexStart,
        AlignContent::Center,
        AlignContent::FlexEnd,
        AlignContent::SpaceBetween,
        AlignContent::SpaceAround,
        AlignContent::Stretch,
    ][rng.range(6)]
}

fn random_justify_items(rng: &mut DeterministicRng) -> JustifyItems {
    [
        JustifyItems::Auto,
        JustifyItems::Stretch,
        JustifyItems::Start,
        JustifyItems::Center,
        JustifyItems::End,
    ][rng.range(5)]
}
