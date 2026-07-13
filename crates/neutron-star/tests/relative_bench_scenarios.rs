//! Inventory and structure guards for the Rust-only Relative benchmark
//! workloads migrated from `PupilTong/lynx#25`.

#[path = "../benches/scenarios/relative.rs"]
mod scenarios;
mod support;

use std::collections::BTreeSet;

use neutron_star::prelude::*;
use neutron_star::style::{BoxGenerationMode, BoxSizing, RelativeCenter, RelativeReference};
use scenarios::{Lowering, SCENARIOS, scenario_named};
use support::TestDisplay;

const SOURCE_RELATIVE_SCENARIOS: &[&str] = &[
    "at_most_owner_matrix",
    "baseline_propagation_matrix",
    "measured_callback_matrix",
    "box_sizing_matrix",
    "fit_content_subtrees",
    "relative_dependency_graph",
    "relative_center_matrix",
    "sticky_percent_insets",
    "mixed_display_none",
];

const RELATIVE_SLICE_SCENARIOS: &[&str] = &[
    "at_most_owner_matrix",
    "baseline_propagation_matrix",
    "measured_callback_matrix",
    "box_sizing_matrix",
    "fit_content_subtrees",
    "sticky_percent_insets",
    "mixed_display_none",
];

#[test]
fn scenario_inventory_matches_every_display_relative_source_benchmark() {
    assert_eq!(SCENARIOS.len(), SOURCE_RELATIVE_SCENARIOS.len());
    let mut names = BTreeSet::new();
    for (scenario, expected_name) in SCENARIOS.iter().zip(SOURCE_RELATIVE_SCENARIOS) {
        assert_eq!(scenario.name, *expected_name);
        assert!(names.insert(scenario.name), "duplicate {}", scenario.name);
        assert_eq!(
            scenario.lowering,
            if RELATIVE_SLICE_SCENARIOS.contains(&scenario.name) {
                Lowering::RelativeSlice
            } else {
                Lowering::Direct
            },
            "{}",
            scenario.name
        );
    }

    assert!(!names.contains("position_type_matrix"));
    assert!(!names.contains("mixed_position_offsets"));
    assert!(!names.contains("full_value_spacing_matrix"));
}

#[test]
fn every_scenario_builds_and_runs_through_rust_only_dispatch() {
    for scenario in SCENARIOS {
        let mut case = (scenario.build)(24);
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
        assert!(
            case.tree.session.child_layout_calls > 0,
            "{}",
            scenario.name
        );
    }
}

#[test]
fn source_relative_slices_keep_their_exact_linear_node_scale() {
    const INPUT: usize = 6;
    for (name, expected_nodes) in [
        ("at_most_owner_matrix", 1 + 4 * INPUT),
        ("baseline_propagation_matrix", 1 + 5 * INPUT),
        ("measured_callback_matrix", 1 + 5 * INPUT),
        ("box_sizing_matrix", 1 + 2 * INPUT),
        ("fit_content_subtrees", 1 + 2 * INPUT),
        ("relative_dependency_graph", 1 + INPUT),
        ("relative_center_matrix", 1 + INPUT),
        ("sticky_percent_insets", 1 + 3 * INPUT),
        ("mixed_display_none", 1 + 4 * INPUT),
    ] {
        let case = (scenario_named(name).build)(INPUT);
        assert_eq!(case.node_count(), expected_nodes, "{name}");
    }
}

#[test]
fn all_relative_workloads_use_the_source_two_pass_default() {
    for scenario in SCENARIOS {
        let case = (scenario.build)(12);
        let relative_nodes = case
            .tree
            .source
            .nodes
            .iter()
            .filter(|node| node.display == TestDisplay::Relative)
            .collect::<Vec<_>>();
        assert!(!relative_nodes.is_empty(), "{}", scenario.name);
        assert!(
            relative_nodes
                .iter()
                .all(|node| !node.style.relative_layout_once),
            "{}",
            scenario.name
        );
    }
}

#[test]
fn direct_dependency_graph_keeps_duplicate_forward_two_axis_groups() {
    let case = (scenario_named("relative_dependency_graph").build)(4);
    assert_eq!(
        case.tree.source.nodes[usize::from(case.root)].display,
        TestDisplay::Relative
    );
    let children = &case.tree.source.nodes[usize::from(case.root)].children;
    assert_eq!(children.len(), 4);
    let styles = children
        .iter()
        .map(|child| &case.tree.source.nodes[usize::from(*child)].style)
        .collect::<Vec<_>>();

    let id = RelativeReference::new(1);
    assert_eq!(styles[0].relative_id, id);
    assert_eq!(styles[0].relative_align.right, RelativeReference::PARENT);
    assert_eq!(styles[0].relative_align.bottom, RelativeReference::PARENT);
    assert_eq!(styles[1].relative_adjacent.right, id);
    assert_eq!(styles[1].relative_adjacent.bottom, id);
    assert_eq!(styles[2].relative_id, id, "the later duplicate must win");
    assert_eq!(styles[3].relative_align.left, id);
    assert_eq!(styles[3].relative_align.bottom, id);
}

#[test]
fn direct_center_matrix_keeps_every_center_value_and_parent_edges() {
    let case = (scenario_named("relative_center_matrix").build)(4);
    let children = &case.tree.source.nodes[usize::from(case.root)].children;
    let mut saw = [false; 4];
    let mut saw_parent_edge = false;
    for child in children {
        let style = &case.tree.source.nodes[usize::from(*child)].style;
        saw[match style.relative_center {
            RelativeCenter::None => 0,
            RelativeCenter::Horizontal => 1,
            RelativeCenter::Vertical => 2,
            RelativeCenter::Both => 3,
        }] = true;
        saw_parent_edge |= style.relative_align.left == RelativeReference::PARENT
            || style.relative_align.right == RelativeReference::PARENT
            || style.relative_align.top == RelativeReference::PARENT
            || style.relative_align.bottom == RelativeReference::PARENT;
    }
    assert!(saw.iter().all(|value| *value));
    assert!(saw_parent_edge);
}

#[test]
fn mixed_display_none_keeps_hidden_duplicate_out_of_the_relative_item_set() {
    let case = (scenario_named("mixed_display_none").build)(1);
    let root = &case.tree.source.nodes[usize::from(case.root)];
    let container = &case.tree.source.nodes[usize::from(root.children[0])];
    assert_eq!(container.display, TestDisplay::Relative);
    assert_eq!(container.children.len(), 3);

    let visible = &case.tree.source.nodes[usize::from(container.children[0])].style;
    let follower = &case.tree.source.nodes[usize::from(container.children[1])].style;
    let hidden = &case.tree.source.nodes[usize::from(container.children[2])].style;
    assert_eq!(hidden.box_generation_mode, BoxGenerationMode::None);
    assert_eq!(hidden.relative_id, visible.relative_id);
    assert_eq!(follower.relative_adjacent.right, visible.relative_id);
    assert_eq!(follower.relative_adjacent.bottom, visible.relative_id);
}

#[test]
fn slice_constraints_and_style_matrices_are_preserved() {
    let at_most = (scenario_named("at_most_owner_matrix").build)(3);
    assert_eq!(at_most.known_dimensions, Size::NONE);
    assert_eq!(
        at_most.available_space,
        Size::new(
            AvailableSpace::Definite(320.0),
            AvailableSpace::Definite(220.0)
        )
    );

    let box_sizing = (scenario_named("box_sizing_matrix").build)(2);
    let saw_content_box = box_sizing.tree.source.nodes.iter().any(|node| {
        node.display == TestDisplay::Relative && node.style.box_sizing == BoxSizing::ContentBox
    });
    let saw_border_box = box_sizing.tree.source.nodes.iter().any(|node| {
        node.display == TestDisplay::Relative && node.style.box_sizing == BoxSizing::BorderBox
    });
    assert!(saw_content_box && saw_border_box);

    let sticky = (scenario_named("sticky_percent_insets").build)(1);
    let sticky_root = &sticky.tree.source.nodes[usize::from(sticky.root)];
    let relative = &sticky.tree.source.nodes[usize::from(sticky_root.children[0])];
    let sticky_child = &sticky.tree.source.nodes[usize::from(relative.children[0])];
    assert_eq!(
        sticky_child.style.inset.left,
        neutron_star::style::LengthPercentageAuto::Percent(0.10)
    );
    assert_eq!(
        sticky_child.style.inset.top,
        neutron_star::style::LengthPercentageAuto::Percent(0.25)
    );
}

#[test]
fn benchmark_target_has_no_native_bridge_or_external_engine() {
    let target = include_str!("../benches/relative_pr25.rs");
    let scenarios = include_str!("../benches/scenarios/relative.rs");
    let forbidden = [
        ["cxx", "::bridge"].concat(),
        ["extern ", "\"C\""].concat(),
        ["run_", "head_to_head"].concat(),
        ["starlight_", "cpp"].concat(),
    ];
    assert!(
        forbidden
            .iter()
            .all(|needle| !target.contains(needle) && !scenarios.contains(needle))
    );
}
