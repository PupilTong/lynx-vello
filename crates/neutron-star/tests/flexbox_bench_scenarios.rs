//! Guards for the Rust-only Flex benchmark inventory migrated from
//! `PupilTong/lynx#25`.

#[path = "../benches/scenarios/flexbox.rs"]
mod scenarios;
mod support;

use std::collections::BTreeSet;

use neutron_star::prelude::Size;
use neutron_star::style::{AlignContent, AlignItems, Dimension, FlexDirection, FlexWrap};
use scenarios::{Lowering, SCENARIOS, scenario_named};

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
