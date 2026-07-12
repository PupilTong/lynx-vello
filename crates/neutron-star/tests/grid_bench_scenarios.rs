//! Guards for the 18 Rust-only Grid benchmark scenarios migrated from
//! PupilTong/lynx#25.

#[path = "../benches/scenarios/grid.rs"]
mod scenarios;
mod support;

use std::collections::BTreeSet;

use scenarios::{Lowering, SCENARIOS, scenario_named};

const SOURCE_GRID_SCENARIOS: &[&str] = &[
    "at_most_owner_matrix",
    "baseline_propagation_matrix",
    "measured_callback_matrix",
    "in_flow_order_matrix",
    "full_value_spacing_matrix",
    "box_sizing_matrix",
    "fit_content_subtrees",
    "sticky_percent_insets",
    "mixed_display_none",
    "grid_out_of_flow_intrinsic",
    "grid_out_of_flow_areas",
    "grid_item_alignment_matrix",
    "grid_content_alignment_matrix",
    "grid_auto_flow_matrix",
    "grid_auto_margin_alignment",
    "grid_minmax_intrinsic_tracks",
    "grid_auto_fit_content_max_tracks",
    "grid_indefinite_auto_fit_content_max_tracks",
];

const GRID_SLICE_SCENARIOS: &[&str] = &[
    "at_most_owner_matrix",
    "baseline_propagation_matrix",
    "measured_callback_matrix",
    "in_flow_order_matrix",
    "full_value_spacing_matrix",
    "box_sizing_matrix",
    "fit_content_subtrees",
    "sticky_percent_insets",
    "mixed_display_none",
];

#[test]
fn scenario_inventory_matches_every_grid_tagged_source_benchmark() {
    assert_eq!(SCENARIOS.len(), 18);
    let mut names = BTreeSet::new();
    for (scenario, expected) in SCENARIOS.iter().zip(SOURCE_GRID_SCENARIOS) {
        assert_eq!(scenario.name, *expected);
        assert!(names.insert(scenario.name));
        assert_eq!(
            scenario.lowering,
            if GRID_SLICE_SCENARIOS.contains(&scenario.name) {
                Lowering::GridSlice
            } else {
                Lowering::Direct
            }
        );
    }
}

#[test]
fn every_grid_scenario_builds_and_runs_through_rust_only_dispatch() {
    for scenario in SCENARIOS {
        let mut case = (scenario.build)(24);
        assert_eq!(case.node_count(), 25, "{}", scenario.name);
        let output = case.run();
        for value in [
            output.size.width,
            output.size.height,
            output.content_size.width,
            output.content_size.height,
        ] {
            assert!(
                value.is_finite() && value >= 0.0,
                "{}: {value}",
                scenario.name
            );
        }
    }
}

#[test]
fn scenario_lookup_and_scaling_are_exact() {
    for name in SOURCE_GRID_SCENARIOS {
        let scenario = scenario_named(name);
        assert_eq!((scenario.build)(1).node_count(), 2, "{name}");
        assert_eq!((scenario.build)(64).node_count(), 65, "{name}");
    }
}

#[test]
fn benchmark_target_has_no_native_bridge_or_external_engine() {
    let target = include_str!("../benches/grid_pr25.rs");
    let scenarios = include_str!("../benches/scenarios/grid.rs");
    let forbidden = [
        ["cxx", "::bridge"].concat(),
        ["extern ", "\"C\""].concat(),
        ["run_", "head_to_head"].concat(),
    ];
    assert!(
        forbidden
            .iter()
            .all(|needle| !target.contains(needle) && !scenarios.contains(needle))
    );
}
