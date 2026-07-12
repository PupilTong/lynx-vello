//! Guards for the Rust-only Flex benchmark inventory migrated from
//! `PupilTong/lynx#25`.

#[path = "../benches/scenarios/flexbox.rs"]
mod scenarios;
mod support;

use std::collections::BTreeSet;

use neutron_star::prelude::{LayoutSource, NodeId, Size};
use neutron_star::style::{
    AlignContent, AlignItems, BoxSizing, Dimension, Direction, FlexDirection, FlexWrap,
    LengthPercentage, LengthPercentageAuto,
};
use scenarios::{BenchCase, Lowering, SCENARIOS, scenario_named};
use support::TestStyle;

const SOURCE_FLEX_SCENARIOS: &[&str] = &[
    "flex_grow_row",
    "flex_wrap_gaps",
    "flex_at_most_root",
    "at_most_owner_matrix",
    "standalone_owner_direction_inheritance",
    "flex_axis_alignment_matrix",
    "flex_distribution_matrix",
    "flex_wrap_alignment_matrix",
    "flex_baseline_measured",
    "baseline_propagation_matrix",
    "measured_callback_matrix",
    "absolute_children",
    "nested_column_flex",
    "in_flow_order_matrix",
    "full_value_spacing_matrix",
    "box_sizing_matrix",
    "fit_content_subtrees",
    "mixed_display_none",
];

const FLEX_SLICE_SCENARIOS: &[&str] = &[
    "at_most_owner_matrix",
    "baseline_propagation_matrix",
    "measured_callback_matrix",
    "in_flow_order_matrix",
    "full_value_spacing_matrix",
    "box_sizing_matrix",
    "fit_content_subtrees",
    "mixed_display_none",
];

#[test]
fn scenario_inventory_matches_every_flex_tagged_source_benchmark() {
    assert_eq!(SCENARIOS.len(), SOURCE_FLEX_SCENARIOS.len());

    let mut names = BTreeSet::new();
    for (scenario, expected_name) in SCENARIOS.iter().zip(SOURCE_FLEX_SCENARIOS) {
        assert_eq!(scenario.name, *expected_name);
        assert!(names.insert(scenario.name), "duplicate {}", scenario.name);
        let expected_lowering = if FLEX_SLICE_SCENARIOS.contains(&scenario.name) {
            Lowering::FlexSlice
        } else {
            Lowering::Direct
        };
        assert_eq!(scenario.lowering, expected_lowering, "{}", scenario.name);
    }
}

#[test]
fn every_scenario_builds_and_runs_through_rust_only_dispatch() {
    for scenario in SCENARIOS {
        let mut case = (scenario.build)(8);
        assert!(
            case.node_count() >= 9,
            "{} built too few nodes",
            scenario.name
        );
        let output = case.run();
        for value in [
            output.size.width,
            output.size.height,
            output.content_size.width,
            output.content_size.height,
        ] {
            assert!(value.is_finite(), "{} returned {value}", scenario.name);
            assert!(value >= 0.0, "{} returned {value}", scenario.name);
        }
    }
}

#[test]
fn mixed_flex_slices_keep_their_exact_linear_node_scale() {
    const INPUT: usize = 6;
    for (name, expected_nodes) in [
        ("at_most_owner_matrix", 1 + 4 * INPUT),
        // The three retained baseline sources contribute 4, 6, and 6
        // nodes per row (including the row container), then repeat.
        ("baseline_propagation_matrix", 1 + 2 * (4 + 6 + 6)),
        ("measured_callback_matrix", 1 + 5 * INPUT),
        ("in_flow_order_matrix", 1 + 6 * INPUT),
        ("full_value_spacing_matrix", 1 + 5 * INPUT),
        ("box_sizing_matrix", 1 + 2 * INPUT),
        ("fit_content_subtrees", 1 + 2 * INPUT),
        ("mixed_display_none", 1 + 4 * INPUT),
    ] {
        let case = (scenario_named(name).build)(INPUT);
        assert_eq!(case.node_count(), expected_nodes, "{name}");
    }
}

fn root_child_styles(case: &BenchCase) -> Vec<&TestStyle> {
    case.tree.source.nodes[usize::from(case.root)]
        .children
        .iter()
        .map(|child| &case.tree.source.nodes[usize::from(*child)].style)
        .collect()
}

fn case_child(case: &BenchCase, parent: NodeId, index: usize) -> NodeId {
    case.tree.source.nodes[usize::from(parent)].children[index]
}

fn assert_calc_at_basis(case: &BenchCase, value: Dimension, basis: f32, expected: f32) {
    let Dimension::FitContent(LengthPercentage::Calc(calc)) = value else {
        panic!("expected fit-content(calc()), got {value:?}");
    };
    let actual = case.tree.source.resolve_calc(calc, basis);
    assert!(
        (actual - expected).abs() <= 0.001,
        "expected calc() to resolve to {expected}, got {actual}"
    );
}

fn assert_dimension_percent(value: Dimension, expected: f32) {
    let Dimension::Percent(actual) = value else {
        panic!("expected a percentage dimension, got {value:?}");
    };
    assert!((actual - expected).abs() <= 0.0001);
}

fn assert_length_percentage_percent(value: LengthPercentage, expected: f32) {
    let LengthPercentage::Percent(actual) = value else {
        panic!("expected a percentage length, got {value:?}");
    };
    assert!((actual - expected).abs() <= 0.0001);
}

fn assert_auto_calc_at_basis(
    case: &BenchCase,
    value: LengthPercentageAuto,
    basis: f32,
    expected: f32,
) {
    let LengthPercentageAuto::Calc(calc) = value else {
        panic!("expected calc() edge, got {value:?}");
    };
    let actual = case.tree.source.resolve_calc(calc, basis);
    assert!((actual - expected).abs() <= 0.001);
}

#[test]
fn mixed_flex_slices_derive_styles_from_the_source_raw_indices() {
    let at_most = (scenario_named("at_most_owner_matrix").build)(3);
    let at_most_children = root_child_styles(&at_most);
    // Source Flex rows are raw indices 1, 6, 11, ... in the five-display
    // matrix. Index 1 retains its 19px + 45% fit-content width; index 6 is
    // the fixed 43% branch because every selected raw index is 1 mod 5.
    assert_calc_at_basis(&at_most, at_most_children[0].size.width, 100.0, 64.0);
    assert_dimension_percent(at_most_children[1].size.width, 0.43);
    assert_calc_at_basis(&at_most, at_most_children[2].size.width, 100.0, 66.0);
    assert_eq!(at_most.known_dimensions, Size::NONE);
    assert_calc_at_basis(
        &at_most,
        at_most.tree.source.nodes[usize::from(at_most.root)]
            .style
            .size
            .width,
        100.0,
        92.0,
    );

    let measured = (scenario_named("measured_callback_matrix").build)(4);
    let measured_children = root_child_styles(&measured);
    assert_eq!(
        measured_children[0].size,
        Size::new(Dimension::Length(136.0), Dimension::Length(58.0))
    );
    assert_eq!(
        measured_children[0].border.left,
        LengthPercentage::Length(0.5)
    );
    assert!(matches!(
        measured_children[1].size.width,
        Dimension::FitContent(LengthPercentage::Length(126.0))
    ));
    assert_eq!(measured_children[1].border.left, LengthPercentage::ZERO);
    assert!(matches!(
        measured_children[3].size.height,
        Dimension::FitContent(LengthPercentage::Length(44.0))
    ));

    let in_flow = (scenario_named("in_flow_order_matrix").build)(4);
    for style in root_child_styles(&in_flow) {
        assert_eq!(style.direction, Direction::Rtl);
        assert_eq!(style.flex_direction, FlexDirection::RowReverse);
    }

    let spacing = (scenario_named("full_value_spacing_matrix").build)(4);
    for style in root_child_styles(&spacing) {
        assert_eq!(style.direction, Direction::Rtl);
        assert_eq!(style.flex_direction, FlexDirection::RowReverse);
        assert_eq!(style.border.left, LengthPercentage::Length(2.0));
        assert_eq!(style.border.top, LengthPercentage::Length(1.5));
        assert_eq!(style.border.bottom, LengthPercentage::Length(0.25));
    }
    let first_spacing = root_child_styles(&spacing)[0];
    assert_length_percentage_percent(first_spacing.padding.left, 0.05);
    assert_eq!(first_spacing.padding.top, LengthPercentage::ZERO);
    assert_eq!(first_spacing.padding.bottom, LengthPercentage::ZERO);
    let first_spacing_container = case_child(&spacing, spacing.root, 0);
    let first_spacing_leaf = case_child(&spacing, first_spacing_container, 0);
    let first_spacing_leaf_style =
        &spacing.tree.source.nodes[usize::from(first_spacing_leaf)].style;
    let LengthPercentageAuto::Percent(inset_left) = first_spacing_leaf_style.inset.left else {
        panic!("raw spacing index 1 must retain its percentage inset");
    };
    assert!((inset_left - 0.05).abs() <= 0.0001);
    assert_auto_calc_at_basis(&spacing, first_spacing_leaf_style.inset.top, 100.0, 8.0);
    assert_eq!(
        first_spacing_leaf_style.margin.left,
        LengthPercentageAuto::Auto
    );

    let box_sizing = (scenario_named("box_sizing_matrix").build)(3);
    let box_children = root_child_styles(&box_sizing);
    assert_eq!(box_children[0].box_sizing, BoxSizing::BorderBox);
    assert_eq!(box_children[0].flex_direction, FlexDirection::Column);
    assert_eq!(box_children[0].min_size.width, Dimension::Length(25.0));
    assert_eq!(box_children[1].box_sizing, BoxSizing::ContentBox);
    assert_eq!(box_children[1].flex_direction, FlexDirection::Row);
    assert_eq!(box_children[1].min_size.width, Dimension::Length(25.0));
    assert_eq!(box_sizing.known_dimensions.width, Some(3.0));
    let box_root = &box_sizing.tree.source.nodes[usize::from(box_sizing.root)].style;
    assert_eq!(box_root.size.width, Dimension::Length(360.0));
    assert_eq!(box_root.padding.left, LengthPercentage::Length(2.0));
    assert_eq!(box_root.border.left, LengthPercentage::Length(1.0));

    let fit_content = (scenario_named("fit_content_subtrees").build)(2);
    let fit_children = root_child_styles(&fit_content);
    assert_eq!(fit_children[0].flex_direction, FlexDirection::Column);
    assert_calc_at_basis(&fit_content, fit_children[0].size.width, 100.0, 48.0);
    assert_calc_at_basis(&fit_content, fit_children[0].size.height, 100.0, 32.0);
    assert_eq!(fit_children[1].flex_direction, FlexDirection::Row);
    assert_calc_at_basis(&fit_content, fit_children[1].size.width, 100.0, 47.0);
    assert_calc_at_basis(&fit_content, fit_children[1].size.height, 100.0, 35.0);
}

#[test]
fn flex_axis_alignment_matrix_keeps_its_complete_period() {
    let case = (scenario_named("flex_axis_alignment_matrix").build)(252);
    let mut directions = [false; 4];
    let mut justify = [false; 9];
    let mut align = [false; 7];

    for node in &case.tree.source.nodes {
        if node.style.size != Size::new(Dimension::Length(120.0), Dimension::Length(80.0)) {
            continue;
        }
        directions[flex_direction_index(node.style.flex_direction)] = true;
        justify[content_index(node.style.justify_content.expect("matrix justify-content"))] = true;
        align[items_index(node.style.align_items.expect("matrix align-items"))] = true;
    }

    assert!(directions.into_iter().all(std::convert::identity));
    assert!(justify.into_iter().all(std::convert::identity));
    assert!(align.into_iter().all(std::convert::identity));
}

#[test]
fn flex_distribution_matrix_keeps_grow_shrink_basis_order_and_minmax() {
    let case = (scenario_named("flex_distribution_matrix").build)(48);
    let mut directions = [false; 4];
    let mut saw_grow = false;
    let mut saw_custom_shrink = false;
    let mut saw_percent_basis = false;
    let mut saw_negative_order = false;
    let mut saw_positive_order = false;
    let mut saw_percent_minmax = false;

    for node in &case.tree.source.nodes {
        let style = &node.style;
        let is_matrix_container = [style.size.width, style.size.height]
            .into_iter()
            .any(|dimension| matches!(dimension, Dimension::Length(178.0 | 94.0)));
        if is_matrix_container {
            directions[flex_direction_index(style.flex_direction)] = true;
        }
        saw_grow |= style.flex_grow > 0.0;
        saw_custom_shrink |= (style.flex_shrink - 1.0).abs() > f32::EPSILON;
        saw_percent_basis |= matches!(style.flex_basis, Dimension::Percent(_));
        saw_negative_order |= style.order < 0;
        saw_positive_order |= style.order > 0;
        saw_percent_minmax |= [
            style.min_size.width,
            style.min_size.height,
            style.max_size.width,
            style.max_size.height,
        ]
        .into_iter()
        .any(|dimension| matches!(dimension, Dimension::Percent(_)));
    }

    assert!(directions.into_iter().all(std::convert::identity));
    assert!(saw_grow);
    assert!(saw_custom_shrink);
    assert!(saw_percent_basis);
    assert!(saw_negative_order);
    assert!(saw_positive_order);
    assert!(saw_percent_minmax);
}

#[test]
fn flex_wrap_alignment_matrix_keeps_its_complete_period() {
    let case = (scenario_named("flex_wrap_alignment_matrix").build)(252);
    let mut directions = [false; 4];
    let mut wraps = [false; 3];
    let mut content = [false; 9];
    let mut items = [false; 7];

    for node in &case.tree.source.nodes {
        if node.style.size != Size::new(Dimension::Length(76.0), Dimension::Length(64.0)) {
            continue;
        }
        directions[flex_direction_index(node.style.flex_direction)] = true;
        wraps[match node.style.flex_wrap {
            FlexWrap::NoWrap => 0,
            FlexWrap::Wrap => 1,
            FlexWrap::WrapReverse => 2,
        }] = true;
        content[content_index(node.style.align_content.expect("matrix align-content"))] = true;
        items[items_index(node.style.align_items.expect("matrix align-items"))] = true;
    }

    assert!(directions.into_iter().all(std::convert::identity));
    assert!(wraps.into_iter().all(std::convert::identity));
    assert!(content.into_iter().all(std::convert::identity));
    assert!(items.into_iter().all(std::convert::identity));
}

fn flex_direction_index(value: FlexDirection) -> usize {
    match value {
        FlexDirection::Row => 0,
        FlexDirection::RowReverse => 1,
        FlexDirection::Column => 2,
        FlexDirection::ColumnReverse => 3,
    }
}

fn content_index(value: AlignContent) -> usize {
    match value {
        AlignContent::Stretch => 0,
        AlignContent::FlexStart => 1,
        AlignContent::Start => 2,
        AlignContent::Center => 3,
        AlignContent::FlexEnd => 4,
        AlignContent::End => 5,
        AlignContent::SpaceBetween => 6,
        AlignContent::SpaceAround => 7,
        AlignContent::SpaceEvenly => 8,
    }
}

fn items_index(value: AlignItems) -> usize {
    match value {
        AlignItems::Stretch => 0,
        AlignItems::FlexStart => 1,
        AlignItems::Start => 2,
        AlignItems::Center => 3,
        AlignItems::FlexEnd => 4,
        AlignItems::End => 5,
        AlignItems::Baseline => 6,
    }
}
