//! Inventory guards for the Linear Divan/CodSpeed workloads.

#[path = "../benches/scenarios/linear.rs"]
mod scenarios;
#[path = "linear_support/mod.rs"]
mod support;

use std::collections::{BTreeSet, HashSet};

use neutron_star::prelude::{AvailableSpace, Size};
use neutron_star::style::{Dimension, LengthPercentage, LengthPercentageAuto};
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
    "intrinsic_pure_length",
    "intrinsic_sparse_percentage",
    "intrinsic_dense_percentage",
    "intrinsic_dense_padding_percentage",
    "intrinsic_percentage_size_only",
    "intrinsic_percentage_min_max_only",
    "intrinsic_relative_inset_only",
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
        ("intrinsic_pure_length", 1 + INPUT),
        ("intrinsic_sparse_percentage", 1 + INPUT),
        ("intrinsic_dense_percentage", 1 + INPUT),
        ("intrinsic_dense_padding_percentage", 1 + INPUT),
        ("intrinsic_percentage_size_only", 1 + INPUT),
        ("intrinsic_percentage_min_max_only", 1 + INPUT),
        ("intrinsic_relative_inset_only", 1 + INPUT),
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

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct DependencyCounts {
    preferred_size: [usize; 2],
    min_size: [usize; 2],
    max_size: [usize; 2],
    margin: [usize; 4],
    padding: [usize; 4],
    border: [usize; 4],
    inset: [usize; 4],
}

fn length_depends_on_basis(value: LengthPercentage) -> bool {
    matches!(
        value,
        LengthPercentage::Percent(_) | LengthPercentage::Calc(_)
    )
}

fn auto_length_depends_on_basis(value: LengthPercentageAuto) -> bool {
    matches!(
        value,
        LengthPercentageAuto::Percent(_) | LengthPercentageAuto::Calc(_)
    )
}

fn dimension_depends_on_basis(value: Dimension) -> bool {
    matches!(value, Dimension::Percent(_) | Dimension::Calc(_))
        || matches!(value, Dimension::FitContent(limit) if length_depends_on_basis(limit))
}

fn dependency_counts(case: &scenarios::BenchCase) -> DependencyCounts {
    root_containers(case).fold(DependencyCounts::default(), |mut counts, style| {
        counts.preferred_size[0] += usize::from(dimension_depends_on_basis(style.size.width));
        counts.preferred_size[1] += usize::from(dimension_depends_on_basis(style.size.height));
        counts.min_size[0] += usize::from(dimension_depends_on_basis(style.min_size.width));
        counts.min_size[1] += usize::from(dimension_depends_on_basis(style.min_size.height));
        counts.max_size[0] += usize::from(dimension_depends_on_basis(style.max_size.width));
        counts.max_size[1] += usize::from(dimension_depends_on_basis(style.max_size.height));
        for (count, value) in counts.margin.iter_mut().zip([
            style.margin.left,
            style.margin.right,
            style.margin.top,
            style.margin.bottom,
        ]) {
            *count += usize::from(auto_length_depends_on_basis(value));
        }
        for (count, value) in counts.padding.iter_mut().zip([
            style.padding.left,
            style.padding.right,
            style.padding.top,
            style.padding.bottom,
        ]) {
            *count += usize::from(length_depends_on_basis(value));
        }
        for (count, value) in counts.border.iter_mut().zip([
            style.border.left,
            style.border.right,
            style.border.top,
            style.border.bottom,
        ]) {
            *count += usize::from(length_depends_on_basis(value));
        }
        for (count, value) in counts.inset.iter_mut().zip([
            style.inset.left,
            style.inset.right,
            style.inset.top,
            style.inset.bottom,
        ]) {
            *count += usize::from(auto_length_depends_on_basis(value));
        }
        counts
    })
}

#[test]
fn intrinsic_scenarios_pin_exact_dependency_signatures() {
    const INPUT: usize = 128;
    for (name, expected) in [
        ("intrinsic_pure_length", DependencyCounts::default()),
        (
            "intrinsic_sparse_percentage",
            DependencyCounts {
                margin: [1, 0, 0, 0],
                ..DependencyCounts::default()
            },
        ),
        (
            "intrinsic_dense_percentage",
            DependencyCounts {
                margin: [INPUT, 0, 0, 0],
                ..DependencyCounts::default()
            },
        ),
        (
            "intrinsic_dense_padding_percentage",
            DependencyCounts {
                padding: [INPUT, 0, 0, 0],
                ..DependencyCounts::default()
            },
        ),
        (
            "intrinsic_percentage_size_only",
            DependencyCounts {
                preferred_size: [INPUT, 0],
                ..DependencyCounts::default()
            },
        ),
        (
            "intrinsic_percentage_min_max_only",
            DependencyCounts {
                min_size: [INPUT, 0],
                ..DependencyCounts::default()
            },
        ),
        (
            "intrinsic_relative_inset_only",
            DependencyCounts {
                inset: [INPUT, 0, 0, 0],
                ..DependencyCounts::default()
            },
        ),
    ] {
        let case = (scenario_named(name).build)(INPUT);
        assert_eq!(case.known_dimensions, Size::new(None, Some(16.0)), "{name}");
        assert_eq!(
            case.available_space,
            Size::new(AvailableSpace::MaxContent, AvailableSpace::Definite(16.0)),
            "{name}"
        );
        assert_eq!(dependency_counts(&case), expected, "{name}");
    }
}

#[test]
fn intrinsic_scenarios_exercise_box_refresh_without_resizing_the_container() {
    const INPUT: usize = 128;
    const INPUT_F32: f32 = 128.0;
    let natural_width = INPUT_F32 * 8.0;
    for (name, expected_content_width) in [
        ("intrinsic_pure_length", natural_width),
        ("intrinsic_sparse_percentage", natural_width + 1.0),
        ("intrinsic_dense_percentage", natural_width + INPUT_F32),
    ] {
        let mut case = (scenario_named(name).build)(INPUT);
        let output = case.run();
        support::assert_close(output.size.width, natural_width);
        support::assert_close(output.content_size.width, expected_content_width);
    }
}

#[test]
fn intrinsic_padding_scenario_exports_refreshed_used_padding() {
    const INPUT: usize = 128;
    const INPUT_F32: f32 = 128.0;
    let natural_width = INPUT_F32 * 8.0;
    let mut case = (scenario_named("intrinsic_dense_padding_percentage").build)(INPUT);
    let children = case.tree.source.nodes[usize::from(case.root)]
        .children
        .clone();
    let output = case.run();
    support::assert_close(output.size.width, natural_width);
    for child in children {
        support::assert_close(case.tree.layout(child).padding.left, 1.0);
    }
}

#[test]
fn intrinsic_noop_refresh_scenarios_pin_size_minmax_and_inset_geometry() {
    const INPUT: usize = 128;
    const INPUT_F32: f32 = 128.0;
    let natural_width = INPUT_F32 * 8.0;

    let mut size_case = (scenario_named("intrinsic_percentage_size_only").build)(INPUT);
    let size_output = size_case.run();
    support::assert_close(size_output.size.width, natural_width);
    support::assert_close(size_output.content_size.width, natural_width);

    let mut min_max_case = (scenario_named("intrinsic_percentage_min_max_only").build)(INPUT);
    let min_max_output = min_max_case.run();
    support::assert_close(min_max_output.size.width, natural_width);
    support::assert_close(min_max_output.content_size.width, natural_width);

    let mut inset_case = (scenario_named("intrinsic_relative_inset_only").build)(INPUT);
    let inset_output = inset_case.run();
    support::assert_close(inset_output.size.width, natural_width);
    support::assert_close(inset_output.content_size.width, natural_width + 1.0);
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
