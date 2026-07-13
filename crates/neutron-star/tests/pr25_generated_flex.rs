//! Pure-Rust generated Flex matrices migrated from PupilTong/lynx#25.
//!
//! The source suite compared every generated tree with a C++ Starlight
//! baseline. This port deliberately has no C++ runner: each matrix checks a
//! CSS geometry or host-protocol invariant directly against neutron-star.

mod pr25_support;
mod support;

use std::collections::BTreeSet;

use neutron_star::compute::{LeafMeasureInput, LeafMetrics, compute_absolute_layout};
use neutron_star::prelude::*;
use neutron_star::style::{
    AlignContent, AlignItems, BoxGenerationMode, BoxSizing, Dimension, Direction, FlexDirection,
    FlexWrap, JustifyContent, LengthPercentage, LengthPercentageAuto, Position,
};
use pr25_support as p;
use support::{
    TestStyle, TestTree, assert_close, assert_point, assert_size, definite_layout, fixed_leaf,
    fixed_leaf_style, flex_container, perform_layout,
};

const FLEX_DIRECTIONS: [FlexDirection; 4] = [
    FlexDirection::Row,
    FlexDirection::RowReverse,
    FlexDirection::Column,
    FlexDirection::ColumnReverse,
];
const DIRECTIONS: [Direction; 2] = [Direction::Ltr, Direction::Rtl];

fn is_row(direction: FlexDirection) -> bool {
    matches!(direction, FlexDirection::Row | FlexDirection::RowReverse)
}

fn main_reverse(flex_direction: FlexDirection, direction: Direction) -> bool {
    match flex_direction {
        FlexDirection::Row => direction == Direction::Rtl,
        FlexDirection::RowReverse => direction != Direction::Rtl,
        FlexDirection::Column => false,
        FlexDirection::ColumnReverse => true,
    }
}

fn main_base_reverse(flex_direction: FlexDirection, direction: Direction) -> bool {
    is_row(flex_direction) && direction == Direction::Rtl
}

fn cross_base_reverse(flex_direction: FlexDirection, direction: Direction) -> bool {
    !is_row(flex_direction) && direction == Direction::Rtl
}

fn axis_value(point: p::Point, row: bool, main: bool) -> f32 {
    match (row, main) {
        (true, true) | (false, false) => point.x,
        (true, false) | (false, true) => point.y,
    }
}

fn physical_from_flow(flow: f32, item: f32, container: f32, reverse: bool) -> f32 {
    if reverse {
        container - flow - item
    } else {
        flow
    }
}

fn distribution(alignment: AlignContent, free: f32, count: u16) -> (f32, f32) {
    match alignment {
        AlignContent::FlexStart | AlignContent::Start | AlignContent::Stretch => (0.0, 0.0),
        AlignContent::FlexEnd | AlignContent::End => (free, 0.0),
        AlignContent::Center => (free / 2.0, 0.0),
        AlignContent::SpaceBetween => {
            if count > 1 {
                (0.0, free / f32::from(count - 1))
            } else {
                (0.0, 0.0)
            }
        }
        AlignContent::SpaceAround => {
            if count > 0 {
                let gap = free / f32::from(count);
                (gap / 2.0, gap)
            } else {
                (free / 2.0, 0.0)
            }
        }
        AlignContent::SpaceEvenly => {
            let gap = free / f32::from(count + 1);
            (gap, gap)
        }
    }
}

fn p_flex_item(tree: &mut p::SimpleTree, row: bool, main: f32, cross: f32) -> usize {
    let (width, height) = if row { (main, cross) } else { (cross, main) };
    tree.push(p::SimpleNode::new(p::Style {
        display: p::Display::Block,
        width: p::Length::points(width),
        height: p::Length::points(height),
        flex_basis: p::Length::points(main),
        flex_shrink: 0.0,
        ..p::Style::default()
    }))
}

fn p_axis_tree(
    flex_direction: FlexDirection,
    direction: Direction,
    justify_content: JustifyContent,
    align_items: AlignItems,
) -> (p::SimpleTree, usize, [usize; 2]) {
    let row = is_row(flex_direction);
    let mut tree = p::SimpleTree::default();
    let root = tree.push(p::SimpleNode::new(p::Style {
        display: p::Display::Flex,
        width: p::Length::points(120.0),
        height: p::Length::points(80.0),
        flex_direction,
        direction,
        justify_content,
        align_items,
        ..p::Style::default()
    }));
    let first = p_flex_item(&mut tree, row, 20.0, 10.0);
    let second = p_flex_item(&mut tree, row, 30.0, 20.0);
    tree.append_child(root, first);
    tree.append_child(root, second);
    (tree, root, [first, second])
}

fn justify_distribution(value: JustifyContent, free: f32, count: u16) -> (f32, f32) {
    let alignment = match value {
        JustifyContent::FlexStart | JustifyContent::Stretch => AlignContent::FlexStart,
        JustifyContent::Start => AlignContent::Start,
        JustifyContent::Center => AlignContent::Center,
        JustifyContent::FlexEnd => AlignContent::FlexEnd,
        JustifyContent::End => AlignContent::End,
        JustifyContent::SpaceBetween => AlignContent::SpaceBetween,
        JustifyContent::SpaceAround => AlignContent::SpaceAround,
        JustifyContent::SpaceEvenly => AlignContent::SpaceEvenly,
    };
    distribution(alignment, free, count)
}

#[test]
fn generated_flex_axis_alignment_matrix_has_224_geometry_cases() {
    let justify_values = [
        JustifyContent::FlexStart,
        JustifyContent::Center,
        JustifyContent::FlexEnd,
        JustifyContent::SpaceBetween,
        JustifyContent::SpaceAround,
        JustifyContent::SpaceEvenly,
        JustifyContent::Stretch,
    ];
    let align_values = [
        AlignItems::FlexStart,
        AlignItems::Center,
        AlignItems::FlexEnd,
        AlignItems::Stretch,
    ];
    let mut case_count = 0;

    for flex_direction in FLEX_DIRECTIONS {
        for direction in DIRECTIONS {
            for justify_content in justify_values {
                for align_items in align_values {
                    let row = is_row(flex_direction);
                    let (mut tree, root, [first, second]) =
                        p_axis_tree(flex_direction, direction, justify_content, align_items);
                    let output =
                        p::run_rust_layout(&mut tree, root, p::Constraints::definite(120.0, 80.0));
                    assert_eq!(output, p::Size::new(120.0, 80.0));

                    let container_main = if row { 120.0 } else { 80.0 };
                    let container_cross = if row { 80.0 } else { 120.0 };
                    let free_main = container_main - 50.0;
                    let (leading, gap) = justify_distribution(justify_content, free_main, 2);
                    let reverse = main_reverse(flex_direction, direction);
                    let first_expected = physical_from_flow(leading, 20.0, container_main, reverse);
                    let second_expected =
                        physical_from_flow(leading + 20.0 + gap, 30.0, container_main, reverse);
                    assert_close(
                        axis_value(tree.nodes[first].layout.offset, row, true),
                        first_expected,
                    );
                    assert_close(
                        axis_value(tree.nodes[second].layout.offset, row, true),
                        second_expected,
                    );

                    let cross_reverse = cross_base_reverse(flex_direction, direction);
                    for (node, cross_size) in [(first, 10.0), (second, 20.0)] {
                        let free = container_cross - cross_size;
                        let flow = match align_items {
                            AlignItems::Center => free / 2.0,
                            AlignItems::FlexEnd => free,
                            AlignItems::FlexStart | AlignItems::Stretch => 0.0,
                            AlignItems::Start | AlignItems::End | AlignItems::Baseline => {
                                unreachable!("not generated by this matrix")
                            }
                        };
                        assert_close(
                            axis_value(tree.nodes[node].layout.offset, row, false),
                            physical_from_flow(flow, cross_size, container_cross, cross_reverse),
                        );
                    }
                    case_count += 1;
                }
            }
        }
    }
    assert_eq!(case_count, 224);
}

#[test]
fn generated_flex_start_end_alias_matrix_has_32_geometry_cases() {
    let mut case_count = 0;
    for flex_direction in FLEX_DIRECTIONS {
        for direction in DIRECTIONS {
            for justify_content in [JustifyContent::Start, JustifyContent::End] {
                for align_items in [AlignItems::Start, AlignItems::End] {
                    let row = is_row(flex_direction);
                    let (mut tree, root, [first, _]) =
                        p_axis_tree(flex_direction, direction, justify_content, align_items);
                    p::run_rust_layout(&mut tree, root, p::Constraints::definite(120.0, 80.0));

                    let container_main = if row { 120.0 } else { 80.0 };
                    let container_cross = if row { 80.0 } else { 120.0 };
                    let reverse = main_reverse(flex_direction, direction);
                    let base_reverse = main_base_reverse(flex_direction, direction);
                    let free_main = container_main - 50.0;
                    let flow_main = match justify_content {
                        JustifyContent::Start if reverse == base_reverse => 0.0,
                        JustifyContent::Start => free_main,
                        JustifyContent::End if reverse == base_reverse => free_main,
                        JustifyContent::End => 0.0,
                        _ => unreachable!(),
                    };
                    assert_close(
                        axis_value(tree.nodes[first].layout.offset, row, true),
                        physical_from_flow(flow_main, 20.0, container_main, reverse),
                    );

                    let cross_reverse = cross_base_reverse(flex_direction, direction);
                    let free_cross = container_cross - 10.0;
                    let flow_cross = match align_items {
                        AlignItems::Start => 0.0,
                        AlignItems::End => free_cross,
                        _ => unreachable!(),
                    };
                    assert_close(
                        axis_value(tree.nodes[first].layout.offset, row, false),
                        physical_from_flow(flow_cross, 10.0, container_cross, cross_reverse),
                    );
                    case_count += 1;
                }
            }
        }
    }
    assert_eq!(case_count, 32);
}

fn p_wrapped_tree(
    flex_direction: FlexDirection,
    direction: Direction,
    flex_wrap: FlexWrap,
    justify_content: JustifyContent,
    align_content: AlignContent,
    align_items: AlignItems,
    size: p::Size,
) -> (p::SimpleTree, usize, [usize; 3]) {
    let row = is_row(flex_direction);
    let mut tree = p::SimpleTree::default();
    let root = tree.push(p::SimpleNode::new(p::Style {
        display: p::Display::Flex,
        width: p::Length::points(size.width),
        height: p::Length::points(size.height),
        flex_direction,
        direction,
        flex_wrap,
        justify_content,
        align_content,
        align_items,
        column_gap: p::Length::points(4.0),
        row_gap: p::Length::points(5.0),
        ..p::Style::default()
    }));
    let first = p_flex_item(&mut tree, row, 30.0, 10.0);
    let second = p_flex_item(&mut tree, row, 30.0, 10.0);
    let third = p_flex_item(&mut tree, row, 30.0, 10.0);
    for child in [first, second, third] {
        tree.append_child(root, child);
    }
    (tree, root, [first, second, third])
}

#[test]
fn generated_wrapped_flex_gap_matrix_has_25_geometry_cases() {
    let justify_values = [
        JustifyContent::FlexStart,
        JustifyContent::Center,
        JustifyContent::SpaceBetween,
        JustifyContent::SpaceAround,
        JustifyContent::SpaceEvenly,
    ];
    let align_values = [
        AlignContent::FlexStart,
        AlignContent::Center,
        AlignContent::SpaceBetween,
        AlignContent::SpaceAround,
        AlignContent::SpaceEvenly,
    ];
    let mut case_count = 0;
    for justify_content in justify_values {
        for align_content in align_values {
            let (mut tree, root, [first, second, third]) = p_wrapped_tree(
                FlexDirection::Row,
                Direction::Ltr,
                FlexWrap::Wrap,
                justify_content,
                align_content,
                AlignItems::FlexStart,
                p::Size::new(72.0, 60.0),
            );
            p::run_rust_layout(&mut tree, root, p::Constraints::definite(72.0, 60.0));

            let (first_leading, first_extra_gap) = justify_distribution(justify_content, 8.0, 2);
            let (single_leading, _) = justify_distribution(justify_content, 42.0, 1);
            assert_close(tree.nodes[first].layout.offset.x, first_leading);
            assert_close(
                tree.nodes[second].layout.offset.x,
                first_leading + 30.0 + 4.0 + first_extra_gap,
            );
            assert_close(tree.nodes[third].layout.offset.x, single_leading);

            let (line_leading, line_extra_gap) = distribution(align_content, 35.0, 2);
            assert_close(tree.nodes[first].layout.offset.y, line_leading);
            assert_close(tree.nodes[second].layout.offset.y, line_leading);
            assert_close(
                tree.nodes[third].layout.offset.y,
                line_leading + 10.0 + 5.0 + line_extra_gap,
            );
            case_count += 1;
        }
    }
    assert_eq!(case_count, 25);
}

#[test]
fn generated_flex_wrap_direction_alignment_matrix_has_384_geometry_cases() {
    let align_content_values = [
        AlignContent::FlexStart,
        AlignContent::Start,
        AlignContent::Center,
        AlignContent::FlexEnd,
        AlignContent::End,
        AlignContent::Stretch,
    ];
    let align_items_values = [
        AlignItems::FlexStart,
        AlignItems::Center,
        AlignItems::FlexEnd,
        AlignItems::Stretch,
    ];
    let mut case_count = 0;

    for flex_direction in FLEX_DIRECTIONS {
        for direction in DIRECTIONS {
            for flex_wrap in [FlexWrap::Wrap, FlexWrap::WrapReverse] {
                for align_content in align_content_values {
                    for align_items in align_items_values {
                        let row = is_row(flex_direction);
                        let (width, height) = (76.0, 66.0);
                        let (mut tree, root, [first, second, third]) = p_wrapped_tree(
                            flex_direction,
                            direction,
                            flex_wrap,
                            JustifyContent::FlexStart,
                            align_content,
                            align_items,
                            p::Size::new(width, height),
                        );
                        p::run_rust_layout(
                            &mut tree,
                            root,
                            p::Constraints::definite(width, height),
                        );

                        for child in [first, second, third] {
                            let layout = tree.nodes[child].layout;
                            assert!(layout.offset.x.is_finite() && layout.offset.y.is_finite());
                            assert!(layout.size.width > 0.0 && layout.size.height > 0.0);
                        }
                        let first_main = axis_value(tree.nodes[first].layout.offset, row, true);
                        let second_main = axis_value(tree.nodes[second].layout.offset, row, true);
                        assert_eq!(
                            first_main > second_main,
                            main_reverse(flex_direction, direction),
                        );

                        let first_cross = axis_value(tree.nodes[first].layout.offset, row, false);
                        let second_cross = axis_value(tree.nodes[second].layout.offset, row, false);
                        let third_cross = axis_value(tree.nodes[third].layout.offset, row, false);
                        assert_close(first_cross, second_cross);
                        assert!((first_cross - third_cross).abs() >= 5.0);
                        let cross_reversed = cross_base_reverse(flex_direction, direction)
                            ^ (flex_wrap == FlexWrap::WrapReverse);
                        assert_eq!(first_cross > third_cross, cross_reversed);
                        case_count += 1;
                    }
                }
            }
        }
    }
    assert_eq!(case_count, 384);
}

#[derive(Clone, Copy, Debug)]
enum GeneratedFlexContainer {
    Row,
    ColumnRtl,
}

impl GeneratedFlexContainer {
    fn style(self) -> TestStyle {
        match self {
            Self::Row => TestStyle {
                flex_direction: FlexDirection::Row,
                ..TestStyle::default()
            },
            Self::ColumnRtl => TestStyle {
                flex_direction: FlexDirection::Column,
                direction: Direction::Rtl,
                ..TestStyle::default()
            },
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum MeasuredVariant {
    Plain,
    Baseline,
    MinMax,
    AspectBorderBox,
}

fn measure_standard(_input: LeafMeasureInput) -> LeafMetrics {
    LeafMetrics::new(Size::new(24.0, 12.0)).with_first_baselines(Point::new(None, Some(7.0)))
}

fn measure_large_and_flat(_input: LeafMeasureInput) -> LeafMetrics {
    LeafMetrics::new(Size::new(40.0, 5.0)).with_first_baselines(Point::new(None, Some(3.0)))
}

#[test]
fn generated_measured_callback_matrix_has_8_flex_geometry_cases() {
    let mut case_count = 0;
    for container in [
        GeneratedFlexContainer::Row,
        GeneratedFlexContainer::ColumnRtl,
    ] {
        for variant in [
            MeasuredVariant::Plain,
            MeasuredVariant::Baseline,
            MeasuredVariant::MinMax,
            MeasuredVariant::AspectBorderBox,
        ] {
            let mut tree = TestTree::default();
            let mut child_style = TestStyle::default();
            let measure = match variant {
                MeasuredVariant::Plain | MeasuredVariant::Baseline => measure_standard,
                MeasuredVariant::MinMax | MeasuredVariant::AspectBorderBox => {
                    measure_large_and_flat
                }
            };
            match variant {
                MeasuredVariant::Plain | MeasuredVariant::Baseline => {}
                MeasuredVariant::MinMax => {
                    child_style.min_size =
                        Size::new(Dimension::Length(18.0), Dimension::Length(9.0));
                    child_style.max_size =
                        Size::new(Dimension::Length(26.0), Dimension::Length(16.0));
                }
                MeasuredVariant::AspectBorderBox => {
                    child_style.size.width = Dimension::Length(40.0);
                    child_style.aspect_ratio = Some(2.0);
                    child_style.box_sizing = BoxSizing::BorderBox;
                    child_style.padding = Edges::uniform(LengthPercentage::Length(2.0));
                    child_style.border = Edges::uniform(LengthPercentage::Length(1.0));
                }
            }
            let child = tree.push_measured_leaf(child_style, measure);
            let mut root_style = container.style();
            root_style.align_items = Some(if matches!(variant, MeasuredVariant::Baseline) {
                AlignItems::Baseline
            } else {
                AlignItems::FlexStart
            });
            let root = flex_container(&mut tree, root_style, &[child]);
            let output = definite_layout(&mut tree, root, 142.0, 104.0);

            let expected = match variant {
                MeasuredVariant::Plain | MeasuredVariant::Baseline => Size::new(24.0, 12.0),
                MeasuredVariant::MinMax => Size::new(26.0, 9.0),
                MeasuredVariant::AspectBorderBox => Size::new(40.0, 20.0),
            };
            assert_size(tree.layout(child).size, expected);
            assert_size(output.size, Size::new(142.0, 104.0));
            if matches!(variant, MeasuredVariant::Baseline) {
                assert!(output.first_baselines.y.is_some());
            }
            case_count += 1;
        }
    }
    assert_eq!(case_count, 8);
}

#[derive(Clone, Copy, Debug)]
enum BaselineConstraintMode {
    DefiniteRoot,
    AtMostOwner,
    IndefiniteOwner,
}

#[derive(Clone, Copy, Debug)]
enum BaselineTrigger {
    ContainerAlignItems,
    ChildAlignSelf,
}

#[derive(Clone, Copy, Debug)]
enum BaselineSource {
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

fn baseline_source(tree: &mut TestTree, source: BaselineSource) -> NodeId {
    if matches!(source, BaselineSource::MeasuredLeaf) {
        return tree.push_measured_leaf(TestStyle::default(), measure_standard);
    }

    let leaf = tree.push_measured_leaf(TestStyle::default(), measure_standard);
    let flex_direction = match source {
        BaselineSource::NestedFlex | BaselineSource::NestedLinear => FlexDirection::Row,
        BaselineSource::NestedFlexColumn
        | BaselineSource::NestedLinearVertical
        | BaselineSource::NestedGridFallback
        | BaselineSource::NestedRelativeFallback => FlexDirection::Column,
        BaselineSource::NestedFlexColumnReverse | BaselineSource::NestedLinearVerticalReverse => {
            FlexDirection::ColumnReverse
        }
        BaselineSource::MeasuredLeaf => unreachable!(),
    };
    flex_container(
        tree,
        TestStyle {
            flex_direction,
            align_items: Some(AlignItems::FlexStart),
            ..TestStyle::default()
        },
        &[leaf],
    )
}

#[test]
fn generated_flex_baseline_propagation_matrix_has_54_geometry_cases() {
    let modes = [
        BaselineConstraintMode::DefiniteRoot,
        BaselineConstraintMode::AtMostOwner,
        BaselineConstraintMode::IndefiniteOwner,
    ];
    let triggers = [
        BaselineTrigger::ContainerAlignItems,
        BaselineTrigger::ChildAlignSelf,
    ];
    let sources = [
        BaselineSource::MeasuredLeaf,
        BaselineSource::NestedFlex,
        BaselineSource::NestedFlexColumn,
        BaselineSource::NestedFlexColumnReverse,
        BaselineSource::NestedLinear,
        BaselineSource::NestedLinearVertical,
        BaselineSource::NestedLinearVerticalReverse,
        BaselineSource::NestedGridFallback,
        BaselineSource::NestedRelativeFallback,
    ];
    let mut case_count = 0;

    for mode in modes {
        for trigger in triggers {
            for source in sources {
                let mut tree = TestTree::default();
                let baseline_source = baseline_source(&mut tree, source);
                let reference = tree.push_measured_leaf(TestStyle::default(), measure_standard);
                if matches!(trigger, BaselineTrigger::ChildAlignSelf) {
                    tree.source_node_mut(baseline_source).style.align_self =
                        Some(AlignItems::Baseline);
                    tree.source_node_mut(reference).style.align_self = Some(AlignItems::Baseline);
                }
                let root = flex_container(
                    &mut tree,
                    TestStyle {
                        align_items: Some(
                            if matches!(trigger, BaselineTrigger::ContainerAlignItems) {
                                AlignItems::Baseline
                            } else {
                                AlignItems::FlexStart
                            },
                        ),
                        ..TestStyle::default()
                    },
                    &[baseline_source, reference],
                );

                let output = match mode {
                    BaselineConstraintMode::DefiniteRoot => {
                        definite_layout(&mut tree, root, 120.0, 60.0)
                    }
                    BaselineConstraintMode::AtMostOwner => perform_layout(
                        &mut tree,
                        root,
                        Size::NONE,
                        Size::new(
                            AvailableSpace::Definite(120.0),
                            AvailableSpace::Definite(60.0),
                        ),
                    ),
                    BaselineConstraintMode::IndefiniteOwner => perform_layout(
                        &mut tree,
                        root,
                        Size::NONE,
                        Size::new(AvailableSpace::MaxContent, AvailableSpace::MaxContent),
                    ),
                };

                let source_baseline = tree.layout(baseline_source).location.y
                    + tree
                        .session_node(baseline_source)
                        .output
                        .first_baselines
                        .y
                        .unwrap_or(tree.layout(baseline_source).size.height);
                let reference_baseline = tree.layout(reference).location.y
                    + tree
                        .session_node(reference)
                        .output
                        .first_baselines
                        .y
                        .unwrap_or(tree.layout(reference).size.height);
                assert_close(source_baseline, reference_baseline);
                assert_close(
                    output
                        .first_baselines
                        .y
                        .expect("a baseline-aligned Flex line exports a baseline"),
                    source_baseline,
                );
                case_count += 1;
            }
        }
    }
    assert_eq!(case_count, 54);
}

#[derive(Clone, Copy, Debug)]
enum SizingVariant {
    PercentCalcRoot,
    FitContentRoot,
    FitContentSubtree,
    PercentMinMaxRoot,
    BorderBoxPercentMinMaxRoot,
    ContentBoxAspectRoot,
    BorderBoxAspectRoot,
    IntrinsicMeasuredChild,
}

fn generated_fixed_leaf(
    tree: &mut TestTree,
    container: GeneratedFlexContainer,
    width: f32,
    height: f32,
) -> NodeId {
    let mut style = fixed_leaf_style(width, height);
    if matches!(container, GeneratedFlexContainer::ColumnRtl) {
        style.flex_basis = Dimension::Length(height);
    }
    tree.push_leaf(style, Size::new(width, height), None)
}

#[test]
#[allow(clippy::too_many_lines)]
fn generated_sizing_minmax_aspect_matrix_has_16_flex_geometry_cases() {
    let variants = [
        SizingVariant::PercentCalcRoot,
        SizingVariant::FitContentRoot,
        SizingVariant::FitContentSubtree,
        SizingVariant::PercentMinMaxRoot,
        SizingVariant::BorderBoxPercentMinMaxRoot,
        SizingVariant::ContentBoxAspectRoot,
        SizingVariant::BorderBoxAspectRoot,
        SizingVariant::IntrinsicMeasuredChild,
    ];
    let mut case_count = 0;

    for container in [
        GeneratedFlexContainer::Row,
        GeneratedFlexContainer::ColumnRtl,
    ] {
        for variant in variants {
            let mut tree = TestTree::default();
            let mut root_style = container.style();
            root_style.align_items = Some(AlignItems::FlexStart);
            let output;
            let expected;
            match variant {
                SizingVariant::PercentCalcRoot => {
                    let height_calc = tree.push_calc(6.0, 0.20);
                    root_style.size =
                        Size::new(Dimension::Percent(0.50), Dimension::Calc(height_calc));
                    let child = generated_fixed_leaf(&mut tree, container, 20.0, 10.0);
                    let root = flex_container(&mut tree, root_style, &[child]);
                    output = perform_layout(
                        &mut tree,
                        root,
                        Size::NONE,
                        Size::new(
                            AvailableSpace::Definite(160.0),
                            AvailableSpace::Definite(120.0),
                        ),
                    );
                    expected = Size::new(80.0, 30.0);
                }
                SizingVariant::FitContentRoot => {
                    root_style.size = Size::new(
                        Dimension::FitContent(LengthPercentage::Length(90.0)),
                        Dimension::FitContent(LengthPercentage::Length(50.0)),
                    );
                    let child = generated_fixed_leaf(&mut tree, container, 70.0, 30.0);
                    let root = flex_container(&mut tree, root_style, &[child]);
                    output = perform_layout(
                        &mut tree,
                        root,
                        Size::NONE,
                        Size::new(AvailableSpace::MaxContent, AvailableSpace::MaxContent),
                    );
                    expected = Size::new(70.0, 30.0);
                }
                SizingVariant::FitContentSubtree => {
                    let grandchild = generated_fixed_leaf(&mut tree, container, 70.0, 30.0);
                    let mut nested_style = container.style();
                    nested_style.size.width = Dimension::FitContent(LengthPercentage::Length(90.0));
                    nested_style.size.height =
                        Dimension::FitContent(LengthPercentage::Length(50.0));
                    nested_style.align_self = Some(AlignItems::FlexStart);
                    let nested = flex_container(&mut tree, nested_style, &[grandchild]);
                    let root = flex_container(&mut tree, root_style, &[nested]);
                    output = definite_layout(&mut tree, root, 160.0, 120.0);
                    assert_size(tree.layout(nested).size, Size::new(70.0, 30.0));
                    expected = Size::new(160.0, 120.0);
                }
                SizingVariant::PercentMinMaxRoot => {
                    root_style.size = Size::new(Dimension::Percent(0.80), Dimension::Percent(0.50));
                    root_style.min_size.height = Dimension::Length(70.0);
                    root_style.max_size.width = Dimension::Length(100.0);
                    let child = generated_fixed_leaf(&mut tree, container, 20.0, 10.0);
                    let root = flex_container(&mut tree, root_style, &[child]);
                    output = perform_layout(
                        &mut tree,
                        root,
                        Size::NONE,
                        Size::new(
                            AvailableSpace::Definite(160.0),
                            AvailableSpace::Definite(120.0),
                        ),
                    );
                    expected = Size::new(100.0, 70.0);
                }
                SizingVariant::BorderBoxPercentMinMaxRoot => {
                    root_style.box_sizing = BoxSizing::BorderBox;
                    root_style.size = Size::new(Dimension::Percent(0.80), Dimension::Percent(0.50));
                    root_style.min_size.height = Dimension::Length(70.0);
                    root_style.max_size.width = Dimension::Length(100.0);
                    root_style.padding = Edges::uniform(LengthPercentage::Length(4.0));
                    root_style.border = Edges::uniform(LengthPercentage::Length(1.0));
                    let child = generated_fixed_leaf(&mut tree, container, 20.0, 10.0);
                    let root = flex_container(&mut tree, root_style, &[child]);
                    output = perform_layout(
                        &mut tree,
                        root,
                        Size::NONE,
                        Size::new(
                            AvailableSpace::Definite(160.0),
                            AvailableSpace::Definite(120.0),
                        ),
                    );
                    expected = Size::new(100.0, 70.0);
                }
                SizingVariant::ContentBoxAspectRoot => {
                    root_style.size.width = Dimension::Length(80.0);
                    root_style.aspect_ratio = Some(2.0);
                    let child = generated_fixed_leaf(&mut tree, container, 20.0, 10.0);
                    let root = flex_container(&mut tree, root_style, &[child]);
                    output = perform_layout(
                        &mut tree,
                        root,
                        Size::NONE,
                        Size::new(AvailableSpace::MaxContent, AvailableSpace::MaxContent),
                    );
                    expected = Size::new(80.0, 40.0);
                }
                SizingVariant::BorderBoxAspectRoot => {
                    root_style.box_sizing = BoxSizing::BorderBox;
                    root_style.size.width = Dimension::Length(80.0);
                    root_style.aspect_ratio = Some(2.0);
                    root_style.padding = Edges::uniform(LengthPercentage::Length(4.0));
                    root_style.border = Edges::uniform(LengthPercentage::Length(1.0));
                    let child = generated_fixed_leaf(&mut tree, container, 20.0, 10.0);
                    let root = flex_container(&mut tree, root_style, &[child]);
                    output = perform_layout(
                        &mut tree,
                        root,
                        Size::NONE,
                        Size::new(AvailableSpace::MaxContent, AvailableSpace::MaxContent),
                    );
                    expected = Size::new(80.0, 40.0);
                }
                SizingVariant::IntrinsicMeasuredChild => {
                    let child = tree.push_measured_leaf(TestStyle::default(), measure_standard);
                    let root = flex_container(&mut tree, root_style, &[child]);
                    output = perform_layout(
                        &mut tree,
                        root,
                        Size::NONE,
                        Size::new(AvailableSpace::MaxContent, AvailableSpace::MaxContent),
                    );
                    expected = Size::new(24.0, 12.0);
                }
            }
            assert_size(output.size, expected);
            case_count += 1;
        }
    }
    assert_eq!(case_count, 16);
}

#[test]
fn generated_display_none_origin_matrix_has_2_flex_geometry_cases() {
    let mut case_count = 0;
    for container in [
        GeneratedFlexContainer::Row,
        GeneratedFlexContainer::ColumnRtl,
    ] {
        let mut tree = TestTree::default();
        let mut hidden_style = fixed_leaf_style(50.0, 30.0);
        hidden_style.box_generation_mode = BoxGenerationMode::None;
        hidden_style.order = -10;
        let hidden = tree.push_leaf(hidden_style, Size::new(50.0, 30.0), None);
        let visible = fixed_leaf(&mut tree, 20.0, 10.0);
        let mut root_style = container.style();
        root_style.align_items = Some(AlignItems::FlexStart);
        let root = flex_container(&mut tree, root_style, &[hidden, visible]);

        definite_layout(&mut tree, root, 128.0, 88.0);

        assert_size(tree.layout(hidden).size, Size::ZERO);
        assert_eq!(tree.layout(hidden).order, 0);
        let visible_main = if matches!(container, GeneratedFlexContainer::Row) {
            tree.layout(visible).location.x
        } else {
            tree.layout(visible).location.y
        };
        assert_close(visible_main, 0.0);
        case_count += 1;
    }
    assert_eq!(case_count, 2);
}

#[derive(Clone, Copy, Debug)]
enum GeneratedPosition {
    Absolute,
    Fixed,
}

#[derive(Clone, Copy, Debug)]
enum InsetPattern {
    None,
    Start,
    End,
    Both,
}

fn position_value(position: GeneratedPosition) -> Position {
    match position {
        GeneratedPosition::Absolute => Position::Absolute,
        GeneratedPosition::Fixed => Position::AbsoluteHoisted,
    }
}

fn position_container_style(container: GeneratedFlexContainer) -> TestStyle {
    let mut style = container.style();
    style.justify_content = Some(JustifyContent::Center);
    style.align_items = Some(AlignItems::Center);
    style
}

fn apply_axis_insets(
    edges: &mut Edges<LengthPercentageAuto>,
    pattern: InsetPattern,
    horizontal: bool,
) {
    let (start, end) = if horizontal { (7.0, 11.0) } else { (5.0, 9.0) };
    if matches!(pattern, InsetPattern::Start | InsetPattern::Both) {
        if horizontal {
            edges.left = LengthPercentageAuto::Length(start);
        } else {
            edges.top = LengthPercentageAuto::Length(start);
        }
    }
    if matches!(pattern, InsetPattern::End | InsetPattern::Both) {
        if horizontal {
            edges.right = LengthPercentageAuto::Length(end);
        } else {
            edges.bottom = LengthPercentageAuto::Length(end);
        }
    }
}

fn finish_hoisted_layout(
    tree: &mut TestTree,
    node: NodeId,
    position: GeneratedPosition,
    containing_size: Size<f32>,
) -> Layout {
    if matches!(position, GeneratedPosition::Fixed) {
        let static_position = tree
            .static_position(node)
            .expect("the Flex formatting parent records a fixed child's static position");
        let layout = compute_absolute_layout(
            &tree.source,
            &mut tree.session,
            node,
            containing_size,
            static_position,
        );
        tree.session.set_unrounded_layout(node, &layout);
    }
    tree.layout(node)
}

fn expected_inset_axis(
    pattern: InsetPattern,
    containing: f32,
    item: f32,
    start: f32,
    end: f32,
    static_position: f32,
    prefer_end: bool,
) -> f32 {
    match pattern {
        InsetPattern::None => static_position,
        InsetPattern::Both if prefer_end => containing - end - item,
        InsetPattern::Start | InsetPattern::Both => start,
        InsetPattern::End => containing - end - item,
    }
}

#[test]
fn generated_out_of_flow_position_matrix_has_64_flex_geometry_cases() {
    let mut case_count = 0;
    for container in [
        GeneratedFlexContainer::Row,
        GeneratedFlexContainer::ColumnRtl,
    ] {
        for position in [GeneratedPosition::Absolute, GeneratedPosition::Fixed] {
            for horizontal in [
                InsetPattern::None,
                InsetPattern::Start,
                InsetPattern::End,
                InsetPattern::Both,
            ] {
                for vertical in [
                    InsetPattern::None,
                    InsetPattern::Start,
                    InsetPattern::End,
                    InsetPattern::Both,
                ] {
                    let mut tree = TestTree::default();
                    let mut style = fixed_leaf_style(20.0, 10.0);
                    style.position = position_value(position);
                    style.direction = if matches!(container, GeneratedFlexContainer::ColumnRtl) {
                        Direction::Rtl
                    } else {
                        Direction::Ltr
                    };
                    apply_axis_insets(&mut style.inset, horizontal, true);
                    apply_axis_insets(&mut style.inset, vertical, false);
                    let child = tree.push_leaf(style, Size::new(20.0, 10.0), None);
                    let root =
                        flex_container(&mut tree, position_container_style(container), &[child]);
                    definite_layout(&mut tree, root, 100.0, 40.0);
                    let layout =
                        finish_hoisted_layout(&mut tree, child, position, Size::new(100.0, 40.0));

                    let expected_x = expected_inset_axis(
                        horizontal,
                        100.0,
                        20.0,
                        7.0,
                        11.0,
                        40.0,
                        matches!(container, GeneratedFlexContainer::ColumnRtl),
                    );
                    let expected_y =
                        expected_inset_axis(vertical, 40.0, 10.0, 5.0, 9.0, 15.0, false);
                    assert_point(layout.location, Point::new(expected_x, expected_y));
                    assert_size(layout.size, Size::new(20.0, 10.0));
                    case_count += 1;
                }
            }
        }
    }
    assert_eq!(case_count, 64);
}

#[derive(Clone, Copy, Debug)]
enum OutOfFlowSizingVariant {
    PercentCalc,
    FillAvailable,
    OversizedFillAvailableMeasured,
    MinMaxMeasuredClamp,
    FitContentMeasured,
    AspectBorderBoxMeasured,
}

fn positioned_sizing_case(
    container: GeneratedFlexContainer,
    position: GeneratedPosition,
    variant: OutOfFlowSizingVariant,
) -> (TestTree, NodeId, NodeId, Size<f32>) {
    let mut tree = TestTree::default();
    let mut style = TestStyle {
        position: position_value(position),
        flex_shrink: 0.0,
        ..TestStyle::default()
    };
    let child = match variant {
        OutOfFlowSizingVariant::PercentCalc => {
            let height = tree.push_calc(2.0, 0.20);
            style.size = Size::new(Dimension::Percent(0.50), Dimension::Calc(height));
            tree.push_measured_leaf(style, measure_standard)
        }
        OutOfFlowSizingVariant::FillAvailable => {
            style.inset = Edges {
                left: LengthPercentageAuto::Length(10.0),
                right: LengthPercentageAuto::Length(15.0),
                top: LengthPercentageAuto::Length(4.0),
                bottom: LengthPercentageAuto::Length(6.0),
            };
            tree.push_measured_leaf(style, measure_standard)
        }
        OutOfFlowSizingVariant::OversizedFillAvailableMeasured => {
            style.inset.left = LengthPercentageAuto::Length(70.0);
            style.inset.right = LengthPercentageAuto::Length(50.0);
            style.inset.top = LengthPercentageAuto::Length(25.0);
            style.inset.bottom = LengthPercentageAuto::Length(25.0);
            tree.push_measured_leaf(style, measure_large_and_flat)
        }
        OutOfFlowSizingVariant::MinMaxMeasuredClamp => {
            style.min_size = Size::new(Dimension::Length(30.0), Dimension::Length(12.0));
            style.max_size = Size::new(Dimension::Length(40.0), Dimension::Length(20.0));
            tree.push_measured_leaf(style, measure_large_and_flat)
        }
        OutOfFlowSizingVariant::FitContentMeasured => {
            style.size = Size::new(
                Dimension::FitContent(LengthPercentage::Length(30.0)),
                Dimension::Length(10.0),
            );
            tree.push_intrinsic_leaf(style, Size::new(10.0, 10.0), Size::new(50.0, 10.0))
        }
        OutOfFlowSizingVariant::AspectBorderBoxMeasured => {
            style.size.width = Dimension::Length(40.0);
            style.aspect_ratio = Some(2.0);
            style.box_sizing = BoxSizing::BorderBox;
            style.padding = Edges::uniform(LengthPercentage::Length(2.0));
            style.border = Edges::uniform(LengthPercentage::Length(1.0));
            tree.push_measured_leaf(style, measure_large_and_flat)
        }
    };
    let root = flex_container(&mut tree, position_container_style(container), &[child]);
    (tree, root, child, Size::new(100.0, 40.0))
}

fn expected_out_of_flow_size(variant: OutOfFlowSizingVariant) -> Size<f32> {
    match variant {
        OutOfFlowSizingVariant::PercentCalc | OutOfFlowSizingVariant::FitContentMeasured => {
            // The generated absolute fit-content leaf is unbreakable at
            // 50px, so its min-content size floors the authored 30px cap.
            Size::new(50.0, 10.0)
        }
        OutOfFlowSizingVariant::FillAvailable => Size::new(75.0, 30.0),
        OutOfFlowSizingVariant::OversizedFillAvailableMeasured => Size::ZERO,
        OutOfFlowSizingVariant::MinMaxMeasuredClamp => Size::new(40.0, 12.0),
        OutOfFlowSizingVariant::AspectBorderBoxMeasured => Size::new(40.0, 20.0),
    }
}

#[test]
fn generated_out_of_flow_sizing_matrix_has_24_flex_geometry_cases() {
    let variants = [
        OutOfFlowSizingVariant::PercentCalc,
        OutOfFlowSizingVariant::FillAvailable,
        OutOfFlowSizingVariant::OversizedFillAvailableMeasured,
        OutOfFlowSizingVariant::MinMaxMeasuredClamp,
        OutOfFlowSizingVariant::FitContentMeasured,
        OutOfFlowSizingVariant::AspectBorderBoxMeasured,
    ];
    let mut case_count = 0;
    for container in [
        GeneratedFlexContainer::Row,
        GeneratedFlexContainer::ColumnRtl,
    ] {
        for position in [GeneratedPosition::Absolute, GeneratedPosition::Fixed] {
            for variant in variants {
                let (mut tree, root, child, containing_size) =
                    positioned_sizing_case(container, position, variant);
                definite_layout(
                    &mut tree,
                    root,
                    containing_size.width,
                    containing_size.height,
                );
                let layout = finish_hoisted_layout(&mut tree, child, position, containing_size);
                assert_size(layout.size, expected_out_of_flow_size(variant));
                assert!(layout.location.x.is_finite() && layout.location.y.is_finite());
                case_count += 1;
            }
        }
    }
    assert_eq!(case_count, 24);
}

#[derive(Clone, Copy, Debug)]
enum MixedContainer {
    Block,
    FlexRow,
    FlexColumnRtl,
    LinearRow,
    LinearColumnRtl,
    Relative,
    Grid,
}

impl MixedContainer {
    const ALL: [Self; 7] = [
        Self::Block,
        Self::FlexRow,
        Self::FlexColumnRtl,
        Self::LinearRow,
        Self::LinearColumnRtl,
        Self::Relative,
        Self::Grid,
    ];

    fn is_flex(self) -> bool {
        matches!(self, Self::FlexRow | Self::FlexColumnRtl)
    }

    fn host_flex_style(self) -> TestStyle {
        match self {
            Self::FlexRow | Self::LinearRow => TestStyle {
                flex_direction: FlexDirection::Row,
                ..TestStyle::default()
            },
            Self::FlexColumnRtl | Self::LinearColumnRtl => TestStyle {
                flex_direction: FlexDirection::Column,
                direction: Direction::Rtl,
                ..TestStyle::default()
            },
            Self::Block | Self::Relative | Self::Grid => TestStyle {
                flex_direction: FlexDirection::Column,
                ..TestStyle::default()
            },
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum FixedDescendantVariant {
    PercentStart,
    CalcEnd,
    FillAvailable,
    MeasuredAspect,
    FitContentSubtree,
}

fn fixed_descendant_case(
    root_container: MixedContainer,
    nested_container: MixedContainer,
    variant: FixedDescendantVariant,
) -> (TestTree, NodeId, NodeId, Size<f32>) {
    let mut tree = TestTree::default();
    let mut style = TestStyle {
        position: Position::AbsoluteHoisted,
        flex_shrink: 0.0,
        ..TestStyle::default()
    };
    let fixed = match variant {
        FixedDescendantVariant::PercentStart => {
            style.size = Size::new(Dimension::Length(20.0), Dimension::Length(10.0));
            style.inset.left = LengthPercentageAuto::Percent(0.10);
            style.inset.top = LengthPercentageAuto::Percent(0.20);
            tree.push_measured_leaf(style, measure_standard)
        }
        FixedDescendantVariant::CalcEnd => {
            style.size = Size::new(Dimension::Length(20.0), Dimension::Length(10.0));
            let right = tree.push_calc(4.0, 0.05);
            let bottom = tree.push_calc(3.0, 0.10);
            style.inset.right = LengthPercentageAuto::Calc(right);
            style.inset.bottom = LengthPercentageAuto::Calc(bottom);
            tree.push_measured_leaf(style, measure_standard)
        }
        FixedDescendantVariant::FillAvailable => {
            style.inset = Edges {
                left: LengthPercentageAuto::Length(10.0),
                right: LengthPercentageAuto::Length(20.0),
                top: LengthPercentageAuto::Length(5.0),
                bottom: LengthPercentageAuto::Length(15.0),
            };
            tree.push_measured_leaf(style, measure_standard)
        }
        FixedDescendantVariant::MeasuredAspect => {
            style.size.width = Dimension::Percent(0.50);
            style.aspect_ratio = Some(2.0);
            style.inset.left = LengthPercentageAuto::Length(10.0);
            style.inset.top = LengthPercentageAuto::Length(5.0);
            tree.push_measured_leaf(style, measure_large_and_flat)
        }
        FixedDescendantVariant::FitContentSubtree => {
            style.size = Size::new(
                Dimension::FitContent(LengthPercentage::Length(30.0)),
                Dimension::FitContent(LengthPercentage::Length(20.0)),
            );
            style.inset.left = LengthPercentageAuto::Length(6.0);
            style.inset.top = LengthPercentageAuto::Length(7.0);
            tree.push_intrinsic_leaf(style, Size::new(10.0, 8.0), Size::new(50.0, 25.0))
        }
    };
    let mut nested_style = nested_container.host_flex_style();
    nested_style.size = Size::new(Dimension::Length(60.0), Dimension::Length(50.0));
    nested_style.align_items = Some(AlignItems::FlexStart);
    let nested = flex_container(&mut tree, nested_style, &[fixed]);
    let mut root_style = root_container.host_flex_style();
    root_style.align_items = Some(AlignItems::FlexStart);
    let root = flex_container(&mut tree, root_style, &[nested]);
    (tree, root, fixed, Size::new(180.0, 130.0))
}

fn expected_fixed_descendant(variant: FixedDescendantVariant) -> (Point<f32>, Size<f32>) {
    match variant {
        FixedDescendantVariant::PercentStart => (Point::new(18.0, 26.0), Size::new(20.0, 10.0)),
        FixedDescendantVariant::CalcEnd => (Point::new(147.0, 104.0), Size::new(20.0, 10.0)),
        FixedDescendantVariant::FillAvailable => (Point::new(10.0, 5.0), Size::new(150.0, 110.0)),
        FixedDescendantVariant::MeasuredAspect => (Point::new(10.0, 5.0), Size::new(90.0, 45.0)),
        FixedDescendantVariant::FitContentSubtree => (Point::new(6.0, 7.0), Size::new(50.0, 25.0)),
    }
}

#[test]
fn generated_fixed_descendant_matrix_filters_to_120_flex_geometry_cases() {
    let variants = [
        FixedDescendantVariant::PercentStart,
        FixedDescendantVariant::CalcEnd,
        FixedDescendantVariant::FillAvailable,
        FixedDescendantVariant::MeasuredAspect,
        FixedDescendantVariant::FitContentSubtree,
    ];
    let mut case_count = 0;
    for root_container in MixedContainer::ALL {
        for nested_container in MixedContainer::ALL {
            if !(root_container.is_flex() || nested_container.is_flex()) {
                continue;
            }
            for variant in variants {
                let (mut tree, root, fixed, containing_size) =
                    fixed_descendant_case(root_container, nested_container, variant);
                definite_layout(
                    &mut tree,
                    root,
                    containing_size.width,
                    containing_size.height,
                );
                let static_position = tree
                    .static_position(fixed)
                    .expect("the nested formatting parent records fixed static geometry");
                let layout = compute_absolute_layout(
                    &tree.source,
                    &mut tree.session,
                    fixed,
                    containing_size,
                    static_position,
                );
                tree.session.set_unrounded_layout(fixed, &layout);
                let (expected_location, expected_size) = expected_fixed_descendant(variant);
                assert_point(layout.location, expected_location);
                assert_size(layout.size, expected_size);
                case_count += 1;
            }
        }
    }
    assert_eq!(case_count, 120);
}

#[derive(Clone, Copy, Debug)]
enum StickyInsetLength {
    Points,
    Percent,
    Calc,
}

fn sticky_axis_values(kind: StickyInsetLength, horizontal: bool, basis: f32) -> (f32, f32) {
    match (kind, horizontal) {
        (StickyInsetLength::Points, true) => (7.0, 11.0),
        (StickyInsetLength::Points, false) => (5.0, 9.0),
        (StickyInsetLength::Percent, true) => (0.10 * basis, 0.20 * basis),
        (StickyInsetLength::Percent, false) => (0.25 * basis, 0.50 * basis),
        (StickyInsetLength::Calc, true) => (3.0 + 0.05 * basis, 4.0 + 0.10 * basis),
        (StickyInsetLength::Calc, false) => (2.0 + 0.10 * basis, 1.0 + 0.20 * basis),
    }
}

fn lowered_sticky_axis(pattern: InsetPattern, start: f32, end: f32) -> (Option<f32>, Option<f32>) {
    match pattern {
        InsetPattern::None => (None, None),
        InsetPattern::Start => (Some(start), None),
        InsetPattern::End => (None, Some(end)),
        InsetPattern::Both => (Some(start), Some(end)),
    }
}

fn fixed_in_flow_child(tree: &mut TestTree, container: GeneratedFlexContainer) -> NodeId {
    let mut style = fixed_leaf_style(20.0, 10.0);
    if matches!(container, GeneratedFlexContainer::ColumnRtl) {
        style.flex_basis = Dimension::Length(10.0);
    }
    tree.push_leaf(style, Size::new(20.0, 10.0), None)
}

#[test]
fn generated_sticky_position_matrix_has_96_flex_host_boundary_cases() {
    let mut case_count = 0;
    for container in [
        GeneratedFlexContainer::Row,
        GeneratedFlexContainer::ColumnRtl,
    ] {
        for length in [
            StickyInsetLength::Points,
            StickyInsetLength::Percent,
            StickyInsetLength::Calc,
        ] {
            for horizontal in [
                InsetPattern::None,
                InsetPattern::Start,
                InsetPattern::End,
                InsetPattern::Both,
            ] {
                for vertical in [
                    InsetPattern::None,
                    InsetPattern::Start,
                    InsetPattern::End,
                    InsetPattern::Both,
                ] {
                    let mut tree = TestTree::default();
                    let sticky = fixed_in_flow_child(&mut tree, container);
                    let mut root_style = container.style();
                    root_style.align_items = Some(AlignItems::FlexStart);
                    root_style.justify_content = Some(JustifyContent::FlexStart);
                    let root = flex_container(&mut tree, root_style, &[sticky]);
                    definite_layout(&mut tree, root, 100.0, 40.0);

                    let expected_location = if matches!(container, GeneratedFlexContainer::Row) {
                        Point::ZERO
                    } else {
                        Point::new(80.0, 0.0)
                    };
                    assert_point(tree.layout(sticky).location, expected_location);
                    assert_size(tree.layout(sticky).size, Size::new(20.0, 10.0));

                    // Sticky is a host post-pass: generated authored insets
                    // are lowered against the Flex containing block while
                    // neutron-star receives an ordinary in-flow box.
                    let horizontal_values = sticky_axis_values(length, true, 100.0);
                    let vertical_values = sticky_axis_values(length, false, 40.0);
                    let lowered_horizontal =
                        lowered_sticky_axis(horizontal, horizontal_values.0, horizontal_values.1);
                    let lowered_vertical =
                        lowered_sticky_axis(vertical, vertical_values.0, vertical_values.1);
                    assert!(
                        lowered_horizontal.0.is_some()
                            || lowered_horizontal.1.is_some()
                            || matches!(horizontal, InsetPattern::None)
                    );
                    assert!(
                        lowered_vertical.0.is_some()
                            || lowered_vertical.1.is_some()
                            || matches!(vertical, InsetPattern::None)
                    );
                    case_count += 1;
                }
            }
        }
    }
    assert_eq!(case_count, 96);
}

#[derive(Clone, Copy, Debug)]
enum StickySizingVariant {
    PercentCalc,
    AutoMeasured,
    MinMaxMeasuredClamp,
    FitContentMeasured,
    AspectBorderBoxMeasured,
}

fn sticky_sizing_child(tree: &mut TestTree, variant: StickySizingVariant) -> (NodeId, Size<f32>) {
    let mut style = TestStyle {
        flex_shrink: 0.0,
        align_self: Some(AlignItems::FlexStart),
        ..TestStyle::default()
    };
    match variant {
        StickySizingVariant::PercentCalc => {
            let height = tree.push_calc(2.0, 0.20);
            style.size = Size::new(Dimension::Percent(0.50), Dimension::Calc(height));
            (
                tree.push_measured_leaf(style, measure_standard),
                Size::new(50.0, 10.0),
            )
        }
        StickySizingVariant::AutoMeasured => (
            tree.push_measured_leaf(style, measure_standard),
            Size::new(24.0, 12.0),
        ),
        StickySizingVariant::MinMaxMeasuredClamp => {
            style.min_size = Size::new(Dimension::Length(30.0), Dimension::Length(12.0));
            style.max_size = Size::new(Dimension::Length(40.0), Dimension::Length(20.0));
            (
                tree.push_measured_leaf(style, measure_large_and_flat),
                Size::new(40.0, 12.0),
            )
        }
        StickySizingVariant::FitContentMeasured => {
            style.size = Size::new(
                Dimension::FitContent(LengthPercentage::Length(30.0)),
                Dimension::Length(10.0),
            );
            (
                tree.push_intrinsic_leaf(style, Size::new(10.0, 10.0), Size::new(50.0, 10.0)),
                Size::new(30.0, 10.0),
            )
        }
        StickySizingVariant::AspectBorderBoxMeasured => {
            style.size.width = Dimension::Length(40.0);
            style.aspect_ratio = Some(2.0);
            style.box_sizing = BoxSizing::BorderBox;
            style.padding = Edges::uniform(LengthPercentage::Length(2.0));
            style.border = Edges::uniform(LengthPercentage::Length(1.0));
            (
                tree.push_measured_leaf(style, measure_large_and_flat),
                Size::new(40.0, 20.0),
            )
        }
    }
}

#[test]
fn generated_sticky_sizing_matrix_has_10_flex_geometry_cases() {
    let variants = [
        StickySizingVariant::PercentCalc,
        StickySizingVariant::AutoMeasured,
        StickySizingVariant::MinMaxMeasuredClamp,
        StickySizingVariant::FitContentMeasured,
        StickySizingVariant::AspectBorderBoxMeasured,
    ];
    let mut case_count = 0;
    for container in [
        GeneratedFlexContainer::Row,
        GeneratedFlexContainer::ColumnRtl,
    ] {
        for variant in variants {
            let mut tree = TestTree::default();
            let (sticky, mut expected) = sticky_sizing_child(&mut tree, variant);
            if matches!(
                (container, variant),
                (
                    GeneratedFlexContainer::ColumnRtl,
                    StickySizingVariant::FitContentMeasured
                )
            ) {
                // In a column Flex container width is the cross size; the
                // item's max-content cross contribution remains 50px.
                expected.width = 50.0;
            }
            let mut root_style = container.style();
            root_style.align_items = Some(AlignItems::FlexStart);
            let root = flex_container(&mut tree, root_style, &[sticky]);
            definite_layout(&mut tree, root, 100.0, 40.0);
            assert_size(tree.layout(sticky).size, expected);
            case_count += 1;
        }
    }
    assert_eq!(case_count, 10);
}

const DETERMINISTIC_SUPPORTED_TREE_SEED: u64 = 0x5A17_1A64;
const DEFAULT_DETERMINISTIC_SUPPORTED_TREE_CASES: usize = 32_768;
const DEFAULT_DETERMINISTIC_FLEX_CONTAINING_CASES: usize = 27_637;
const HIGH_CASE_FLEX_CONTAINING_CASES: usize = 315;

// This is the source regression list after the source helper's sort/dedup.
// The test below advances through every listed case and runs every tree that
// contains a Flex node, including Block/Linear roots with nested Flex.
#[allow(clippy::unreadable_literal)] // Keep upstream case IDs directly searchable.
const DETERMINISTIC_HIGH_CASES: [usize; 330] = [
    25, 26, 95, 172, 175, 215, 481, 992, 1012, 1234, 2167, 2299, 2425, 2523, 2704, 2740, 2791,
    3109, 3814, 4187, 5572, 6723, 6754, 7009, 7662, 7834, 8359, 8638, 9259, 9591, 9907, 10035,
    10733, 12823, 13868, 14304, 14505, 15500, 16328, 18982, 19719, 19993, 22474, 23012, 23362,
    25535, 27453, 27673, 27731, 29021, 29221, 29902, 31230, 34113, 41175, 42293, 42544, 44450,
    45883, 47159, 51367, 54850, 55744, 56293, 64120, 64135, 68032, 68538, 68701, 69145, 71254,
    76766, 79192, 83434, 85507, 86849, 86992, 87239, 88209, 89938, 91679, 96812, 99274, 105004,
    105770, 106204, 109786, 110407, 114658, 117329, 117836, 121948, 127981, 134513, 139357, 139979,
    146179, 149574, 160141, 161737, 161817, 164190, 164482, 165472, 166953, 176185, 176542, 176761,
    178066, 178583, 179252, 184937, 186434, 190825, 191781, 197620, 197653, 202380, 203219, 207391,
    210793, 218134, 226104, 226687, 237668, 242282, 243040, 244918, 251182, 259483, 269542, 278605,
    282829, 283687, 283842, 285802, 289600, 291152, 292360, 299934, 299965, 302041, 302185, 307159,
    308572, 309457, 310564, 316984, 318982, 319761, 320341, 320509, 324307, 328591, 331564, 331954,
    333262, 337984, 339274, 340393, 349150, 351670, 352168, 352507, 353716, 355459, 356476, 357577,
    358597, 359128, 370004, 372628, 379001, 379945, 380056, 383395, 385732, 389959, 390103, 392284,
    394393, 396763, 406621, 407422, 411883, 411918, 413818, 422704, 428455, 429025, 433231, 434269,
    435562, 437389, 441040, 441260, 441274, 443098, 453535, 455530, 456847, 458437, 466714, 467131,
    467404, 467710, 468928, 470710, 476011, 479230, 480139, 482731, 483016, 483940, 486302, 486361,
    488938, 495574, 496681, 497539, 500887, 501562, 504658, 516046, 517072, 520441, 523900, 524797,
    532018, 536077, 536206, 537223, 539452, 540076, 540469, 540769, 541387, 545509, 549595, 549826,
    559621, 559681, 562162, 562909, 563128, 566536, 566881, 569228, 570262, 573658, 573712, 575104,
    577384, 579595, 583909, 588640, 590308, 591160, 595567, 597256, 598741, 602614, 603613, 610390,
    611671, 612157, 621826, 622414, 622600, 627868, 630196, 631288, 631777, 633370, 634267, 637210,
    637504, 642361, 646950, 652294, 653143, 655681, 655924, 656965, 659557, 661243, 663121, 663199,
    667462, 668800, 673411, 674587, 675739, 679180, 679381, 680551, 687208, 690619, 691855, 692140,
    692560, 693904, 694987, 698668, 700933, 711010, 712609, 712636, 715732, 721072, 724985, 726547,
    726901, 734488, 738802, 739687, 742114, 742318, 746173, 747619, 748012, 751531, 761683, 762991,
    763882, 763942, 764425, 764680, 772306, 776896,
];

#[derive(Clone, Debug)]
struct DeterministicRng {
    state: u64,
}

impl DeterministicRng {
    const fn new(seed: u64) -> Self {
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
        let point_index =
            u8::try_from(self.range(count)).expect("deterministic point-table indices fit in u8");
        min + step * f32::from(point_index)
    }

    fn bit_as_f32(&mut self) -> f32 {
        f32::from(u8::try_from(self.range(2)).expect("a deterministic bit fits in u8"))
    }
}

/// Advances the same single RNG stream as the source generated suite.
///
/// `build` is false for unselected high cases. Their complete styles are still
/// generated so a later selected case observes exactly the source RNG state,
/// but no tree storage is allocated for them. A built tree is returned only
/// when its root or one of its descendants has `display: flex`.
fn deterministic_supported_flex_tree(
    rng: &mut DeterministicRng,
    case_index: usize,
    build: bool,
) -> Option<(p::SimpleTree, usize, p::Constraints)> {
    let mut tree = p::SimpleTree::default();
    let root_display = match case_index % 3 {
        0 => p::Display::Block,
        1 => p::Display::Flex,
        _ => p::Display::Linear,
    };
    let root_style = deterministic_container_style(rng, root_display);
    let root = build.then(|| tree.push(p::SimpleNode::new(root_style)));
    let mut contains_flex = root_display == p::Display::Flex;

    let child_count = 3 + rng.range(3);
    for child_index in 0..child_count {
        let child_display = deterministic_child_display(rng, child_index);
        contains_flex |= child_display == p::Display::Flex;
        let child_style = deterministic_child_style(rng, child_display, child_index);
        let child = root.map(|root| {
            let child = tree.push(p::SimpleNode::new(child_style));
            tree.append_child(root, child);
            child
        });

        if matches!(
            child_display,
            p::Display::Block | p::Display::Flex | p::Display::Linear
        ) && rng.range(4) == 0
        {
            append_deterministic_grandchildren(rng, &mut tree, child, child_index);
        }
    }

    let constraints = match case_index % 3 {
        0 => p::Constraints::definite(160.0, 120.0),
        1 => p::Constraints::new(
            p::SideConstraint::at_most(180.0),
            p::SideConstraint::at_most(140.0),
        ),
        _ => p::Constraints::indefinite(),
    };
    (build && contains_flex)
        .then_some(root)
        .flatten()
        .map(|root| (tree, root, constraints))
}

fn append_deterministic_grandchildren(
    rng: &mut DeterministicRng,
    tree: &mut p::SimpleTree,
    parent: Option<usize>,
    child_index: usize,
) {
    for grandchild_index in 0..=rng.range(2) {
        let mut style = deterministic_child_style(rng, p::Display::Block, grandchild_index);
        style.position = p::PositionType::Static;
        style.order = i32::try_from(grandchild_index)
            .expect("the generated suite creates at most two grandchildren");
        if child_index.is_multiple_of(2) {
            style.width = p::Length::points(rng.points(8.0, 3.0, 5));
        }
        if let Some(parent) = parent {
            let node = tree.push(p::SimpleNode::new(style));
            tree.append_child(parent, node);
        }
    }
}

fn deterministic_container_style(rng: &mut DeterministicRng, display: p::Display) -> p::Style {
    let mut style = deterministic_base_style(rng, display);
    style.width = deterministic_axis_length(rng);
    style.height = deterministic_axis_length(rng);
    style.min_width = p::Length::points(20.0);
    style.min_height = p::Length::points(16.0);
    style.padding = deterministic_edge_lengths(rng);
    style.border = p::Rect::new(
        rng.bit_as_f32(),
        rng.bit_as_f32(),
        rng.bit_as_f32(),
        rng.bit_as_f32(),
    );

    match display {
        p::Display::Flex => {
            style.flex_direction = [
                FlexDirection::Row,
                FlexDirection::Column,
                FlexDirection::RowReverse,
                FlexDirection::ColumnReverse,
            ][rng.range(4)];
            style.flex_wrap =
                [FlexWrap::NoWrap, FlexWrap::Wrap, FlexWrap::WrapReverse][rng.range(3)];
            style.align_items = deterministic_align_items(rng);
            style.align_content = deterministic_align_content(rng);
            style.justify_content = deterministic_justify_content(rng);
        }
        p::Display::Linear => {
            style.linear_orientation = [
                p::LinearOrientation::Horizontal,
                p::LinearOrientation::Vertical,
                p::LinearOrientation::HorizontalReverse,
                p::LinearOrientation::VerticalReverse,
            ][rng.range(4)];
            // Preserve the source RNG stream for Linear-only gravity fields.
            // This older Flex-focused generator intentionally discards them;
            // `pr25_generated_linear` retains and exercises their exact values.
            let _ = rng.range(5);
            let _ = rng.range(5);
        }
        p::Display::None | p::Display::Block | p::Display::Relative | p::Display::Grid => {}
    }
    style
}

fn deterministic_child_style(
    rng: &mut DeterministicRng,
    display: p::Display,
    child_index: usize,
) -> p::Style {
    let mut style = deterministic_base_style(rng, display);
    style.width = deterministic_axis_length(rng);
    style.height = deterministic_axis_length(rng);
    (style.min_width, style.max_width) = deterministic_coherent_minmax_lengths(rng);
    (style.min_height, style.max_height) = deterministic_coherent_minmax_lengths(rng);
    style.margin = deterministic_edge_lengths(rng);
    style.padding = deterministic_edge_lengths(rng);
    style.border = p::Rect::all(rng.bit_as_f32());
    style.order =
        i32::try_from(child_index).expect("the generated suite creates at most five children") - 1;
    style.flex_basis = deterministic_axis_length(rng);
    style.flex_grow = if rng.bool() { 1.0 } else { 0.0 };
    style.flex_shrink = if rng.bool() { 1.0 } else { 0.0 };
    style.align_self = (rng.range(3) == 0).then(|| deterministic_align_items(rng));
    // Preserve the source JustifyItems draw. Grid self-alignment is outside
    // the immutable Flex style contract, so the generated value is discarded.
    let _ = rng.range(5);
    style
}

fn deterministic_base_style(rng: &mut DeterministicRng, display: p::Display) -> p::Style {
    p::Style {
        display,
        box_sizing: if rng.bool() {
            BoxSizing::ContentBox
        } else {
            BoxSizing::BorderBox
        },
        direction: DIRECTIONS[rng.range(DIRECTIONS.len())],
        row_gap: deterministic_gap_length(rng),
        column_gap: deterministic_gap_length(rng),
        ..p::Style::default()
    }
}

fn deterministic_child_display(rng: &mut DeterministicRng, child_index: usize) -> p::Display {
    if child_index == 1 && rng.range(5) == 0 {
        return p::Display::None;
    }
    [p::Display::Block, p::Display::Flex, p::Display::Linear][rng.range(3)]
}

fn deterministic_axis_length(rng: &mut DeterministicRng) -> p::Length {
    match rng.range(4) {
        0 => p::Length::Auto,
        1 => p::Length::points(rng.points(18.0, 6.0, 10)),
        2 => p::Length::percent(rng.points(20.0, 10.0, 6)),
        3 => p::Length::Fr(rng.points(1.0, 1.0, 4)),
        _ => unreachable!("deterministic axis-length variant is out of range"),
    }
}

fn deterministic_coherent_minmax_lengths(rng: &mut DeterministicRng) -> (p::Length, p::Length) {
    match rng.range(6) {
        0 => (p::Length::Auto, p::Length::Auto),
        1 => (p::Length::points(rng.points(8.0, 4.0, 4)), p::Length::Auto),
        2 => {
            let min = rng.points(8.0, 4.0, 4);
            (
                p::Length::points(min),
                p::Length::points(min + rng.points(16.0, 4.0, 4)),
            )
        }
        3 => (p::Length::Auto, p::Length::points(rng.points(32.0, 8.0, 4))),
        4 => (p::Length::Fr(rng.points(4.0, 2.0, 4)), p::Length::Auto),
        5 => {
            let min = rng.points(4.0, 2.0, 4);
            (
                p::Length::Fr(min),
                p::Length::Fr(min + rng.points(12.0, 2.0, 4)),
            )
        }
        _ => unreachable!("deterministic min/max variant is out of range"),
    }
}

fn deterministic_edge_lengths(rng: &mut DeterministicRng) -> p::Rect<p::Length> {
    fn edge(rng: &mut DeterministicRng) -> p::Length {
        match rng.range(2) {
            0 => p::Length::ZERO,
            _ => p::Length::points(rng.points(1.0, 2.0, 4)),
        }
    }
    p::Rect::new(edge(rng), edge(rng), edge(rng), edge(rng))
}

fn deterministic_gap_length(rng: &mut DeterministicRng) -> p::Length {
    match rng.range(2) {
        0 => p::Length::ZERO,
        _ => p::Length::points(rng.points(1.0, 2.0, 4)),
    }
}

fn deterministic_justify_content(rng: &mut DeterministicRng) -> JustifyContent {
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

fn deterministic_align_items(rng: &mut DeterministicRng) -> AlignItems {
    [
        AlignItems::Stretch,
        AlignItems::FlexStart,
        AlignItems::Center,
        AlignItems::FlexEnd,
        AlignItems::Start,
        AlignItems::End,
    ][rng.range(6)]
}

fn deterministic_align_content(rng: &mut DeterministicRng) -> AlignContent {
    [
        AlignContent::FlexStart,
        AlignContent::Center,
        AlignContent::FlexEnd,
        AlignContent::SpaceBetween,
        AlignContent::SpaceAround,
        AlignContent::Stretch,
    ][rng.range(6)]
}

#[derive(Debug, Default)]
struct DeterministicFlexCoverage {
    case_count: usize,
    root_display_counts: [usize; 3],
    node_count: usize,
    flex_node_count: usize,
    flex_direction_counts: [usize; 4],
    flex_wrap_counts: [usize; 3],
    positive_size_node_count: usize,
    nonzero_offset_node_count: usize,
    fractional_geometry_value_count: usize,
    distinct_root_sizes: BTreeSet<(u32, u32)>,
}

impl DeterministicFlexCoverage {
    fn record_source(&mut self, tree: &p::SimpleTree, root: usize) {
        self.case_count += 1;
        self.root_display_counts[match tree.nodes[root].style.display {
            p::Display::Block => 0,
            p::Display::Flex => 1,
            p::Display::Linear => 2,
            display => panic!("the deterministic generator cannot produce a {display:?} root"),
        }] += 1;
        self.node_count += tree.nodes.len();

        for node in &tree.nodes {
            if node.style.display != p::Display::Flex {
                continue;
            }
            self.flex_node_count += 1;
            self.flex_direction_counts[match node.style.flex_direction {
                FlexDirection::Row => 0,
                FlexDirection::RowReverse => 1,
                FlexDirection::Column => 2,
                FlexDirection::ColumnReverse => 3,
            }] += 1;
            self.flex_wrap_counts[match node.style.flex_wrap {
                FlexWrap::NoWrap => 0,
                FlexWrap::Wrap => 1,
                FlexWrap::WrapReverse => 2,
            }] += 1;
        }
    }

    fn record_layout(&mut self, tree: &p::SimpleTree, root: usize) {
        let root_size = tree.nodes[root].layout.size;
        self.distinct_root_sizes
            .insert((root_size.width.to_bits(), root_size.height.to_bits()));

        for node in &tree.nodes {
            let layout = node.layout;
            if layout.size.width > 0.0 && layout.size.height > 0.0 {
                self.positive_size_node_count += 1;
            }
            if layout.offset.x.abs() > f32::EPSILON || layout.offset.y.abs() > f32::EPSILON {
                self.nonzero_offset_node_count += 1;
            }
            self.fractional_geometry_value_count += [
                layout.offset.x,
                layout.offset.y,
                layout.size.width,
                layout.size.height,
            ]
            .into_iter()
            .filter(|value| value.fract().abs() > 1.0e-4)
            .count();
        }
    }

    fn assert_diversity(&self, expected_cases: usize, expected_root_displays: [usize; 3]) {
        assert_eq!(self.case_count, expected_cases);
        assert_eq!(self.root_display_counts, expected_root_displays);
        assert!(self.node_count >= self.case_count * 4);
        assert!(self.flex_node_count > self.case_count);
        assert!(
            self.flex_direction_counts
                .into_iter()
                .all(|count| count > 0)
        );
        assert!(self.flex_wrap_counts.into_iter().all(|count| count > 0));
        assert!(self.positive_size_node_count > self.case_count);
        assert!(self.nonzero_offset_node_count > 0);
        assert!(self.fractional_geometry_value_count > 0);
        assert!(self.distinct_root_sizes.len() > 16);
    }
}

fn assert_deterministic_rust_flex_case(
    case_index: usize,
    tree: p::SimpleTree,
    root: usize,
    constraints: p::Constraints,
    coverage: &mut DeterministicFlexCoverage,
) {
    assert!(
        tree.nodes
            .iter()
            .any(|node| node.style.display == p::Display::Flex),
        "generated case {case_index} does not contain a Flex node"
    );
    coverage.record_source(&tree, root);
    let mut first = tree.clone();
    let mut second = tree;
    let first_size =
        p::LayoutEngine::new().layout_with_owner_constraints(&mut first, root, constraints);
    let second_size =
        p::LayoutEngine::new().layout_with_owner_constraints(&mut second, root, constraints);

    assert_eq!(
        first_size, second_size,
        "generated Flex case {case_index} returned non-deterministic root geometry"
    );
    assert_eq!(first.nodes.len(), second.nodes.len());
    for (node_index, (first_node, second_node)) in first.nodes.iter().zip(&second.nodes).enumerate()
    {
        assert_eq!(
            first_node.layout, second_node.layout,
            "generated Flex case {case_index}, node {node_index} returned non-deterministic geometry"
        );
        let layout = first_node.layout;
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
        ] {
            assert!(
                value.is_finite(),
                "generated Flex case {case_index}, node {node_index} returned non-finite geometry {value}"
            );
        }
        if let Some(baseline) = layout.baseline {
            assert!(
                baseline.is_finite(),
                "generated Flex case {case_index}, node {node_index} returned non-finite baseline"
            );
        }
        assert!(
            layout.size.width >= 0.0 && layout.size.height >= 0.0,
            "generated Flex case {case_index}, node {node_index} returned negative size {:?}",
            layout.size
        );
    }
    assert_eq!(first.nodes[root].layout.size, first_size);
    coverage.record_layout(&first, root);
}

#[test]
fn generated_deterministic_supported_tree_fuzz_runs_27637_flex_containing_trees_in_rust() {
    let mut rng = DeterministicRng::new(DETERMINISTIC_SUPPORTED_TREE_SEED);
    let mut coverage = DeterministicFlexCoverage::default();
    for case_index in 0..DEFAULT_DETERMINISTIC_SUPPORTED_TREE_CASES {
        if let Some((tree, root, constraints)) =
            deterministic_supported_flex_tree(&mut rng, case_index, true)
        {
            assert_deterministic_rust_flex_case(case_index, tree, root, constraints, &mut coverage);
        }
    }
    // Block/Linear roots use real Linear dispatch but remain Flex-focused
    // protocol smoke cases. The 10,923 Flex-root cases remain separately
    // visible in the middle slot.
    coverage.assert_diversity(
        DEFAULT_DETERMINISTIC_FLEX_CONTAINING_CASES,
        [8_380, 10_923, 8_334],
    );
}

#[test]
fn generated_deterministic_high_case_regressions_run_all_315_flex_containing_trees_in_rust() {
    assert!(
        DETERMINISTIC_HIGH_CASES
            .windows(2)
            .all(|pair| pair[0] < pair[1])
    );

    let mut rng = DeterministicRng::new(DETERMINISTIC_SUPPORTED_TREE_SEED);
    let mut next_source_case = 0;
    let mut coverage = DeterministicFlexCoverage::default();
    let max_case = *DETERMINISTIC_HIGH_CASES
        .last()
        .expect("the source high-case list is non-empty");
    for case_index in 0..=max_case {
        let selected = DETERMINISTIC_HIGH_CASES.get(next_source_case).copied() == Some(case_index);
        let case = deterministic_supported_flex_tree(&mut rng, case_index, selected);
        if selected {
            next_source_case += 1;
        }
        if let Some((tree, root, constraints)) = case {
            assert_deterministic_rust_flex_case(case_index, tree, root, constraints, &mut coverage);
        }
    }
    assert_eq!(next_source_case, DETERMINISTIC_HIGH_CASES.len());
    // Of all 330 source high cases, 315 contain Flex. The 51 Block/Linear
    // roots use real Linear dispatch as protocol smoke cases; 264 are true
    // Flex roots.
    coverage.assert_diversity(HIGH_CASE_FLEX_CONTAINING_CASES, [19, 264, 32]);
}

fn run_flex_basis_regression(case_id: usize) {
    let mut tree = TestTree::default();
    let percent_remainder = u8::try_from(case_id % 5).expect("modulo five always fits into u8");
    let basis_remainder = u8::try_from(case_id % 7).expect("modulo seven always fits into u8");
    let percent = 0.10 + f32::from(percent_remainder) * 0.01;
    let second_basis = 15.0 + f32::from(basis_remainder);
    let mut first_style = fixed_leaf_style(percent * 100.0, 10.0);
    first_style.flex_basis = Dimension::Percent(percent);
    first_style.flex_grow = 1.0;
    first_style.flex_shrink = 0.0;
    let first = tree.push_leaf(first_style, Size::new(percent * 100.0, 10.0), None);
    let mut second_style = fixed_leaf_style(second_basis, 10.0);
    second_style.flex_grow = 2.0;
    second_style.flex_shrink = 0.0;
    let second = tree.push_leaf(second_style, Size::new(second_basis, 10.0), None);
    let root = flex_container(
        &mut tree,
        TestStyle {
            align_items: Some(AlignItems::FlexStart),
            ..TestStyle::default()
        },
        &[first, second],
    );

    let available = Size::new(
        AvailableSpace::Definite(100.0),
        AvailableSpace::Definite(10.0),
    );
    tree.session.compute_child_layout(
        &tree.source,
        root,
        LayoutInput::compute_size(
            Size::new(Some(100.0), Some(10.0)),
            Size::new(Some(100.0), Some(10.0)),
            available,
            RequestedAxis::Both,
        ),
    );
    definite_layout(&mut tree, root, 100.0, 10.0);

    let first_base = percent * 100.0;
    let free = 100.0 - first_base - second_basis;
    assert_close(tree.layout(first).size.width, first_base + free / 3.0);
    assert_close(
        tree.layout(second).size.width,
        second_basis + 2.0 * free / 3.0,
    );
    assert_close(
        tree.layout(second).location.x,
        tree.layout(first).size.width,
    );
}

#[test]
fn deterministic_flex_regression_ids_have_rust_geometry_assertions() {
    for case_id in [19, 46, 544, 787, 1006] {
        run_flex_basis_regression(case_id);
    }

    // Source regression 102 exercised integer percentage rounding. Preserve
    // its fractional inputs while asserting the CSS invariant instead: the
    // two unrounded used widths exactly fill the line.
    let mut percentage_tree = TestTree::default();
    let mut first_style = fixed_leaf_style(0.0, 10.0);
    first_style.flex_basis = Dimension::Percent(1.0 / 3.0);
    first_style.flex_shrink = 0.0;
    let first = percentage_tree.push_leaf(first_style, Size::new(0.0, 10.0), None);
    let mut second_style = fixed_leaf_style(0.0, 10.0);
    second_style.flex_basis = Dimension::Percent(2.0 / 3.0);
    second_style.flex_shrink = 0.0;
    let second = percentage_tree.push_leaf(second_style, Size::new(0.0, 10.0), None);
    let root = flex_container(&mut percentage_tree, TestStyle::default(), &[first, second]);
    definite_layout(&mut percentage_tree, root, 101.0, 10.0);
    assert_close(
        percentage_tree.layout(first).size.width + percentage_tree.layout(second).size.width,
        101.0,
    );
    assert_close(
        percentage_tree.layout(second).location.x,
        percentage_tree.layout(first).size.width,
    );

    // Source regression 2011 exercised integer reverse-axis bounds. CSS
    // layout keeps the fractional edges and reversal must preserve adjacency.
    let mut reverse_tree = TestTree::default();
    let first = fixed_leaf(&mut reverse_tree, 30.25, 10.0);
    let second = fixed_leaf(&mut reverse_tree, 40.5, 10.0);
    let root = flex_container(
        &mut reverse_tree,
        TestStyle {
            flex_direction: FlexDirection::RowReverse,
            align_items: Some(AlignItems::FlexStart),
            ..TestStyle::default()
        },
        &[first, second],
    );
    definite_layout(&mut reverse_tree, root, 101.0, 10.0);
    assert_close(reverse_tree.layout(first).location.x, 70.75);
    assert_close(reverse_tree.layout(second).location.x, 30.25);
    assert_close(
        reverse_tree.layout(second).location.x + reverse_tree.layout(second).size.width,
        reverse_tree.layout(first).location.x,
    );
}

#[test]
fn generated_flex_inventory_totals_1059_cases_and_7_regressions() {
    const STATIC_GROUP_COUNTS: [usize; 13] = [224, 32, 25, 384, 8, 54, 16, 2, 64, 24, 120, 96, 10];
    const REGRESSION_IDS: [usize; 7] = [19, 46, 544, 787, 1006, 2011, 102];

    assert_eq!(STATIC_GROUP_COUNTS.into_iter().sum::<usize>(), 1_059);
    assert_eq!(REGRESSION_IDS.len(), 7);
    assert_eq!(REGRESSION_IDS, [19, 46, 544, 787, 1006, 2011, 102]);
}
