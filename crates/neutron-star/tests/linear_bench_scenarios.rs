//! Inventory guards for the Linear Divan/CodSpeed workloads.

#[path = "../benches/scenarios/linear.rs"]
mod scenarios;
#[path = "linear_support/mod.rs"]
mod support;

use std::collections::{BTreeSet, HashSet};

use scenarios::{
    CROSS_GRAVITIES, LAYOUT_GRAVITIES, MAIN_GRAVITIES, ORIENTATIONS, SCENARIOS, scenario_named,
};

const EXPECTED_SCENARIOS: &[&str] = &[
    "fixed_stack",
    "ordered_stack",
    "weighted_distribution",
    "weighted_freeze",
    "measured_stretch",
    "mixed_hidden_absolute",
    "linear_gravity_matrix",
    "linear_layout_gravity_matrix",
    "linear_cross_gravity_matrix",
];

#[test]
fn scenario_inventory_is_stable_and_unique() {
    assert_eq!(SCENARIOS.len(), EXPECTED_SCENARIOS.len());
    let mut names = BTreeSet::new();
    for (scenario, expected) in SCENARIOS.iter().zip(EXPECTED_SCENARIOS) {
        assert_eq!(scenario.name, *expected);
        assert!(names.insert(scenario.name), "duplicate {}", scenario.name);
    }
}

#[test]
fn every_scenario_builds_and_runs_through_rust_only_static_dispatch() {
    for scenario in SCENARIOS {
        let mut case = (scenario.build)(16);
        assert!(
            case.node_count() >= 17,
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
fn scaling_scenarios_retain_exact_node_counts() {
    const INPUT: usize = 8;
    for (name, expected) in [
        ("fixed_stack", 1 + INPUT),
        ("ordered_stack", 1 + INPUT),
        ("weighted_distribution", 1 + INPUT),
        ("weighted_freeze", 1 + INPUT),
        ("measured_stretch", 1 + INPUT),
        // Every sixth input beginning at index one adds a hidden descendant.
        ("mixed_hidden_absolute", 1 + INPUT + 2),
        ("linear_gravity_matrix", 1 + 4 * INPUT),
        ("linear_layout_gravity_matrix", 1 + 4 * INPUT),
        ("linear_cross_gravity_matrix", 1 + 4 * INPUT),
    ] {
        assert_eq!(
            (scenario_named(name).build)(INPUT).node_count(),
            expected,
            "{name}"
        );
    }
}

fn root_containers(case: &scenarios::BenchCase) -> impl Iterator<Item = &support::TestStyle> {
    case.tree.source.nodes[usize::from(case.root)]
        .children
        .iter()
        .map(|child| &case.tree.source.nodes[usize::from(*child)].style)
}

#[test]
fn main_gravity_matrix_covers_exact_eighty_eight_case_period() {
    let case = (scenario_named("linear_gravity_matrix").build)(88);
    let styles = root_containers(&case).collect::<Vec<_>>();
    assert_eq!(styles.len(), 88);
    assert_eq!(
        styles
            .iter()
            .map(|style| style.linear_orientation)
            .collect::<HashSet<_>>(),
        ORIENTATIONS.into_iter().collect()
    );
    assert_eq!(
        styles
            .iter()
            .map(|style| style.linear_gravity)
            .collect::<HashSet<_>>(),
        MAIN_GRAVITIES.into_iter().collect()
    );
}

#[test]
fn item_gravity_matrix_covers_exact_one_hundred_four_case_period() {
    let case = (scenario_named("linear_layout_gravity_matrix").build)(104);
    let root = &case.tree.source.nodes[usize::from(case.root)];
    let mut orientations = HashSet::new();
    let mut gravities = HashSet::new();
    for container in &root.children {
        let container = &case.tree.source.nodes[usize::from(*container)];
        orientations.insert(container.style.linear_orientation);
        let item = container.children[1];
        gravities.insert(
            case.tree.source.nodes[usize::from(item)]
                .style
                .linear_layout_gravity,
        );
    }
    assert_eq!(orientations, ORIENTATIONS.into_iter().collect());
    assert_eq!(gravities, LAYOUT_GRAVITIES.into_iter().collect());
}

#[test]
fn cross_gravity_matrix_covers_exact_forty_case_period() {
    let case = (scenario_named("linear_cross_gravity_matrix").build)(40);
    let styles = root_containers(&case).collect::<Vec<_>>();
    assert_eq!(styles.len(), 40);
    assert_eq!(
        styles
            .iter()
            .map(|style| style.linear_orientation)
            .collect::<HashSet<_>>(),
        ORIENTATIONS.into_iter().collect()
    );
    assert_eq!(
        styles
            .iter()
            .map(|style| style.linear_cross_gravity)
            .collect::<HashSet<_>>(),
        CROSS_GRAVITIES.into_iter().collect()
    );
}
