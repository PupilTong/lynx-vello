//! Inventory and structure guards for the Rust-only Linear benchmark
//! workloads migrated from `PupilTong/lynx#25`.

#[path = "../benches/scenarios/linear_pr25.rs"]
mod migrated_scenarios;
#[path = "support/mod.rs"]
mod support;

use std::collections::BTreeSet;

use migrated_scenarios::{
    Lowering, SCENARIOS, SourceLength, SourceListComponentType, scenario_named,
};
use neutron_star::prelude::{AvailableSpace, LayoutSource, Size};
use neutron_star::style::{
    BoxGenerationMode, BoxSizing, LengthPercentage, LengthPercentageAuto, LinearCrossGravity,
    LinearGravity, LinearLayoutGravity, LinearOrientation, Position,
};
use support::TestDisplay;

const SOURCE_LINEAR_SCENARIOS: &[&str] = &[
    "at_most_owner_matrix",
    "baseline_propagation_matrix",
    "measured_callback_matrix",
    "in_flow_order_matrix",
    "full_value_spacing_matrix",
    "staggered_linear_list",
    "staggered_linear_raw_list_gaps",
    "linear_gravity_matrix",
    "linear_layout_gravity_matrix",
    "linear_cross_gravity_matrix",
    "box_sizing_matrix",
    "fit_content_subtrees",
    "sticky_percent_insets",
    "mixed_display_none",
];

fn assert_close(actual: f32, expected: f32) {
    assert!(
        (actual - expected).abs() <= 1e-4,
        "expected {expected}, got {actual}"
    );
}

#[test]
fn scenario_inventory_matches_every_linear_tagged_source_benchmark() {
    assert_eq!(SOURCE_LINEAR_SCENARIOS.len(), 14);
    assert_eq!(SCENARIOS.len(), SOURCE_LINEAR_SCENARIOS.len());

    let mut names = BTreeSet::new();
    for (scenario, expected) in SCENARIOS.iter().zip(SOURCE_LINEAR_SCENARIOS) {
        assert_eq!(scenario.name, *expected);
        assert!(names.insert(scenario.name), "duplicate {}", scenario.name);
        assert_eq!(
            scenario.lowering,
            if matches!(
                scenario.name,
                "full_value_spacing_matrix"
                    | "staggered_linear_list"
                    | "staggered_linear_raw_list_gaps"
            ) {
                Lowering::HostListProtocolElided
            } else {
                Lowering::CompleteLogicalTopology
            },
            "{}",
            scenario.name
        );
    }

    assert_eq!(names, SOURCE_LINEAR_SCENARIOS.iter().copied().collect());
}

#[test]
fn every_new_scenario_builds_and_runs_through_rust_only_static_dispatch() {
    for scenario in SCENARIOS {
        let mut case = (scenario.build)(12);
        let output = case.run();
        for value in [
            output.size.width,
            output.size.height,
            output.content_size.width,
            output.content_size.height,
        ] {
            assert!(
                value.is_finite() && value >= 0.0,
                "{} returned {value}",
                scenario.name
            );
        }
        assert!(case.tree.session.layout_writes > 0, "{}", scenario.name);
    }
}

#[test]
fn measured_callback_keeps_source_indefinite_intrinsic_fallback() {
    for available_space in [AvailableSpace::MinContent, AvailableSpace::MaxContent] {
        let metrics =
            migrated_scenarios::callback_metrics(neutron_star::compute::LeafMeasureInput::new(
                Size::NONE,
                Size::new(available_space, available_space),
                neutron_star::prelude::LayoutGoal::Commit,
            ));
        assert_eq!(metrics.size, Size::new(24.0, 12.0));
        assert_eq!(metrics.first_baselines.y, Some(9.0));
    }
}

#[test]
fn migrated_scenarios_retain_exact_source_topology_cardinalities() {
    const INPUT: usize = 8;
    for (name, expected_nodes) in [
        ("at_most_owner_matrix", 1 + 4 * INPUT),
        // Source branch sizes for indices 0..8 are 4,6,6,6,5,5,4,6.
        ("baseline_propagation_matrix", 43),
        ("measured_callback_matrix", 1 + 5 * INPUT),
        ("in_flow_order_matrix", 1 + 6 * INPUT),
        ("full_value_spacing_matrix", 1 + 5 * INPUT),
        ("staggered_linear_list", 1 + INPUT),
        // ceil(sqrt(8)) = 3 nested Linear containers.
        ("staggered_linear_raw_list_gaps", 1 + 3 + INPUT),
        ("linear_gravity_matrix", 1 + 4 * INPUT),
        ("linear_layout_gravity_matrix", 1 + 4 * INPUT),
        ("linear_cross_gravity_matrix", 1 + 4 * INPUT),
        ("box_sizing_matrix", 1 + 2 * INPUT),
        ("fit_content_subtrees", 1 + 2 * INPUT),
        ("sticky_percent_insets", 1 + 3 * INPUT),
        ("mixed_display_none", 1 + 4 * INPUT),
    ] {
        assert_eq!(
            (scenario_named(name).build)(INPUT).node_count(),
            expected_nodes,
            "{name}"
        );
    }
}

#[test]
fn migrated_scenarios_allocate_parents_before_children_and_append_child_vectors() {
    const INPUT: usize = 5;

    for scenario in SCENARIOS {
        let case = (scenario.build)(INPUT);
        assert_eq!(
            usize::from(case.root),
            0,
            "{} must allocate the source root first",
            scenario.name
        );

        for (parent_index, parent) in case.tree.source.nodes.iter().enumerate() {
            for child in &parent.children {
                assert!(
                    parent_index < usize::from(*child),
                    "{} allocated child {child:?} before parent {parent_index}",
                    scenario.name
                );
            }
        }

        let root_children = &case.tree.source.nodes[usize::from(case.root)].children;
        assert!(!root_children.is_empty(), "{}", scenario.name);
        assert!(
            root_children.capacity() > root_children.len(),
            "{} must grow the root's initially-empty child Vec via append, not install an exact-capacity completed Vec",
            scenario.name
        );
    }
}

#[test]
fn migrated_scenarios_retain_source_owner_constraints() {
    const INPUT: usize = 8;
    let wrap = Size::new(AvailableSpace::Definite(320.0), AvailableSpace::MaxContent);
    for name in [
        "baseline_propagation_matrix",
        "measured_callback_matrix",
        "in_flow_order_matrix",
        "full_value_spacing_matrix",
        "staggered_linear_list",
        "staggered_linear_raw_list_gaps",
        "linear_gravity_matrix",
        "linear_layout_gravity_matrix",
        "linear_cross_gravity_matrix",
        "fit_content_subtrees",
        "mixed_display_none",
    ] {
        let case = (scenario_named(name).build)(INPUT);
        assert_eq!(case.known_dimensions, Size::NONE, "{name}");
        assert_eq!(case.available_space, wrap, "{name}");
    }

    let at_most = (scenario_named("at_most_owner_matrix").build)(INPUT);
    assert_eq!(
        at_most.available_space,
        Size::new(
            AvailableSpace::Definite(320.0),
            AvailableSpace::Definite(220.0),
        )
    );
    let box_sizing = (scenario_named("box_sizing_matrix").build)(INPUT);
    assert_eq!(
        box_sizing.available_space,
        Size::new(AvailableSpace::Definite(8.0), AvailableSpace::MaxContent,)
    );
    let sticky = (scenario_named("sticky_percent_insets").build)(INPUT);
    assert_eq!(
        sticky.available_space,
        Size::new(
            AvailableSpace::Definite(320.0),
            AvailableSpace::Definite(240.0),
        )
    );
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum SpacingSignature {
    Length(f32),
    Percent(f32),
    CalcAt100(f32),
    Auto,
}

fn lp_signature(tree: &mut support::TestTree, index: usize) -> SpacingSignature {
    match migrated_scenarios::spacing_lp(tree, index) {
        LengthPercentage::Length(value) => SpacingSignature::Length(value),
        LengthPercentage::Percent(value) => SpacingSignature::Percent(value),
        LengthPercentage::Calc(handle) => {
            SpacingSignature::CalcAt100(tree.source.resolve_calc(handle, 100.0))
        }
    }
}

fn lpa_signature(tree: &mut support::TestTree, index: usize) -> SpacingSignature {
    match migrated_scenarios::spacing_lpa(tree, index) {
        LengthPercentageAuto::Length(value) => SpacingSignature::Length(value),
        LengthPercentageAuto::Percent(value) => SpacingSignature::Percent(value),
        LengthPercentageAuto::Calc(handle) => {
            SpacingSignature::CalcAt100(tree.source.resolve_calc(handle, 100.0))
        }
        LengthPercentageAuto::Auto => SpacingSignature::Auto,
    }
}

#[test]
fn full_value_spacing_retains_all_nine_source_variants_and_property_lowerings() {
    assert_eq!(
        (0..9)
            .map(migrated_scenarios::source_spacing_length)
            .collect::<Vec<_>>(),
        vec![
            SourceLength::Points(2.0),
            SourceLength::Percent(0.05),
            SourceLength::Calc {
                length: 3.0,
                percentage: 0.05,
            },
            SourceLength::Auto,
            SourceLength::Fr(2.0),
            SourceLength::MaxContent,
            SourceLength::FitContentNone,
            SourceLength::FitContentPoints(4.0),
            SourceLength::FitContentCalc {
                length: 1.0,
                percentage: 0.11,
            },
        ]
    );

    let mut tree = support::TestTree::default();
    assert_eq!(
        (0..9)
            .map(|index| lp_signature(&mut tree, index))
            .collect::<Vec<_>>(),
        vec![
            SpacingSignature::Length(2.0),
            SpacingSignature::Percent(0.05),
            SpacingSignature::CalcAt100(8.0),
            SpacingSignature::Length(0.0),
            SpacingSignature::Length(2.0),
            SpacingSignature::Length(0.0),
            SpacingSignature::Length(0.0),
            SpacingSignature::Length(4.0),
            SpacingSignature::CalcAt100(12.0),
        ]
    );
    assert_eq!(
        (0..9)
            .map(|index| lpa_signature(&mut tree, index))
            .collect::<Vec<_>>(),
        vec![
            SpacingSignature::Length(2.0),
            SpacingSignature::Percent(0.05),
            SpacingSignature::CalcAt100(8.0),
            SpacingSignature::Auto,
            SpacingSignature::Length(2.0),
            SpacingSignature::Length(0.0),
            SpacingSignature::Length(0.0),
            SpacingSignature::Length(4.0),
            SpacingSignature::CalcAt100(12.0),
        ]
    );
}

fn root_children(case: &migrated_scenarios::BenchCase) -> Vec<&support::TestSourceNode> {
    case.tree.source.nodes[usize::from(case.root)]
        .children
        .iter()
        .map(|child| &case.tree.source.nodes[usize::from(*child)])
        .collect()
}

fn root_child_displays(case: &migrated_scenarios::BenchCase) -> Vec<TestDisplay> {
    root_children(case)
        .into_iter()
        .map(|node| node.display)
        .collect()
}

fn leaf_count(case: &migrated_scenarios::BenchCase) -> usize {
    case.tree
        .source
        .nodes
        .iter()
        .filter(|node| node.display == TestDisplay::Leaf)
        .count()
}

#[test]
fn source_block_constructor_classification_is_preserved() {
    assert_eq!(
        leaf_count(&(scenario_named("at_most_owner_matrix").build)(5)),
        15
    );
    assert_eq!(
        leaf_count(&(scenario_named("baseline_propagation_matrix").build)(6)),
        21
    );
    assert_eq!(
        leaf_count(&(scenario_named("measured_callback_matrix").build)(5)),
        20
    );

    for name in [
        "in_flow_order_matrix",
        "full_value_spacing_matrix",
        "staggered_linear_list",
        "staggered_linear_raw_list_gaps",
        "linear_gravity_matrix",
        "linear_layout_gravity_matrix",
        "linear_cross_gravity_matrix",
        "box_sizing_matrix",
        "fit_content_subtrees",
        "sticky_percent_insets",
        "mixed_display_none",
    ] {
        assert_eq!(
            leaf_count(&(scenario_named(name).build)(13)),
            0,
            "unmeasured childless source Block must be an empty Linear: {name}"
        );
    }
}

#[test]
fn five_way_mixed_workloads_keep_every_source_display_branch_and_index() {
    let at_most = (scenario_named("at_most_owner_matrix").build)(5);
    let at_most_containers = root_children(&at_most);
    assert_eq!(
        root_child_displays(&at_most),
        vec![
            // Source Block is host-dispatched to vertical Linear.
            TestDisplay::Linear,
            TestDisplay::Flex,
            TestDisplay::Linear,
            TestDisplay::Grid,
            TestDisplay::Relative,
        ]
    );
    assert!(
        at_most_containers
            .iter()
            .all(|node| node.children.len() == 3)
    );
    assert_eq!(
        at_most_containers[2].style.linear_orientation,
        LinearOrientation::Horizontal
    );
    assert_eq!(
        at_most_containers[2].style.linear_cross_gravity,
        LinearCrossGravity::Start
    );

    let measured = (scenario_named("measured_callback_matrix").build)(5);
    assert_eq!(
        root_child_displays(&measured),
        vec![
            TestDisplay::Linear,
            TestDisplay::Flex,
            TestDisplay::Linear,
            TestDisplay::Grid,
            TestDisplay::Relative,
        ]
    );
    assert!(
        root_children(&measured)
            .iter()
            .all(|node| node.children.len() == 4)
    );
}

#[test]
fn other_mixed_workloads_keep_every_source_display_branch_and_index() {
    let order = (scenario_named("in_flow_order_matrix").build)(4);
    let order_containers = root_children(&order);
    assert_eq!(
        root_child_displays(&order),
        vec![
            TestDisplay::Linear,
            TestDisplay::Flex,
            TestDisplay::Linear,
            TestDisplay::Grid,
        ]
    );
    let orders = order_containers
        .iter()
        .map(|container| {
            container
                .children
                .iter()
                .map(|child| order.tree.source.nodes[usize::from(*child)].style.order)
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    assert_eq!(
        orders,
        vec![
            vec![-3, 2, -1, 0, -2],
            vec![-2, 3, 0, 1, -1],
            vec![-1, 4, 1, 2, 0],
            vec![-3, 2, -1, 0, -2],
        ]
    );

    let box_sizing = (scenario_named("box_sizing_matrix").build)(5);
    assert_eq!(
        root_child_displays(&box_sizing),
        vec![
            TestDisplay::Linear,
            TestDisplay::Flex,
            TestDisplay::Linear,
            TestDisplay::Relative,
            TestDisplay::Grid,
        ]
    );
    assert_eq!(
        root_children(&box_sizing)
            .iter()
            .map(|node| node.style.box_sizing)
            .collect::<Vec<_>>(),
        vec![
            BoxSizing::ContentBox,
            BoxSizing::BorderBox,
            BoxSizing::ContentBox,
            BoxSizing::BorderBox,
            BoxSizing::ContentBox,
        ]
    );

    let sticky = (scenario_named("sticky_percent_insets").build)(4);
    assert_eq!(
        root_child_displays(&sticky),
        vec![
            TestDisplay::Flex,
            TestDisplay::Linear,
            TestDisplay::Grid,
            TestDisplay::Relative,
        ]
    );
}

#[test]
fn gravity_matrices_cover_exact_source_value_sets() {
    let main = (scenario_named("linear_gravity_matrix").build)(11);
    assert_eq!(
        root_children(&main)
            .iter()
            .map(|node| node.style.linear_gravity)
            .collect::<Vec<_>>(),
        vec![
            LinearGravity::None,
            LinearGravity::Top,
            LinearGravity::Bottom,
            LinearGravity::Left,
            LinearGravity::Right,
            LinearGravity::CenterVertical,
            LinearGravity::CenterHorizontal,
            LinearGravity::SpaceBetween,
            LinearGravity::Start,
            LinearGravity::End,
            LinearGravity::Center,
        ]
    );

    let item = (scenario_named("linear_layout_gravity_matrix").build)(13);
    assert_eq!(
        root_children(&item)
            .iter()
            .map(|container| {
                let child = container.children[1];
                item.tree.source.nodes[usize::from(child)]
                    .style
                    .linear_layout_gravity
            })
            .collect::<Vec<_>>(),
        vec![
            LinearLayoutGravity::None,
            LinearLayoutGravity::Top,
            LinearLayoutGravity::Bottom,
            LinearLayoutGravity::Left,
            LinearLayoutGravity::Right,
            LinearLayoutGravity::CenterVertical,
            LinearLayoutGravity::CenterHorizontal,
            LinearLayoutGravity::FillVertical,
            LinearLayoutGravity::FillHorizontal,
            LinearLayoutGravity::Center,
            LinearLayoutGravity::Stretch,
            LinearLayoutGravity::Start,
            LinearLayoutGravity::End,
        ]
    );

    let cross = (scenario_named("linear_cross_gravity_matrix").build)(5);
    assert_eq!(
        root_children(&cross)
            .iter()
            .map(|node| node.style.linear_cross_gravity)
            .collect::<Vec<_>>(),
        vec![
            LinearCrossGravity::None,
            LinearCrossGravity::Start,
            LinearCrossGravity::End,
            LinearCrossGravity::Center,
            LinearCrossGravity::Stretch,
        ]
    );
}

#[test]
fn staggered_list_preserves_column_gap_and_component_metadata() {
    let direct = (scenario_named("staggered_linear_list").build)(31);
    let root = &direct.tree.source.nodes[usize::from(direct.root)];
    assert_eq!(root.display, TestDisplay::Linear);
    assert_eq!(root.children.len(), 31);
    assert!(root.children.iter().all(|child| {
        direct.tree.source.nodes[usize::from(*child)].display == TestDisplay::Flex
    }));
    assert_eq!(direct.linear_list_metadata.len(), 1);
    let direct_container = direct.linear_list_metadata[0];
    assert_eq!(direct_container.node, direct.root);
    assert_eq!(direct_container.column_count, Some(4));
    assert_eq!(direct_container.main_axis_gap, None);
    assert_eq!(
        direct_container.cross_axis_gap,
        Some(SourceLength::Points(2.0))
    );
    assert_eq!(direct.list_item_metadata.len(), 31);
    assert!(
        direct
            .list_item_metadata
            .iter()
            .zip(&root.children)
            .all(|(metadata, child)| metadata.node == *child)
    );
    assert_eq!(
        direct
            .list_item_metadata
            .iter()
            .enumerate()
            .filter_map(|(index, metadata)| metadata.component_type.map(|kind| (index, kind)))
            .collect::<Vec<_>>(),
        vec![
            (0, SourceListComponentType::Header),
            (10, SourceListComponentType::Default),
            (15, SourceListComponentType::ListRow),
            (30, SourceListComponentType::Footer),
        ]
    );
}

#[test]
fn staggered_raw_list_preserves_every_gap_and_component_variant() {
    let nested = (scenario_named("staggered_linear_raw_list_gaps").build)(17);
    let root = &nested.tree.source.nodes[usize::from(nested.root)];
    assert_eq!(root.children.len(), 5, "ceil(sqrt(17)) nested containers");
    assert!(root.children.iter().all(|container| {
        let container = &nested.tree.source.nodes[usize::from(*container)];
        container.display == TestDisplay::Linear
            && container.style.linear_orientation == LinearOrientation::Vertical
    }));
    assert_eq!(nested.linear_list_metadata.len(), 5);
    assert_eq!(
        nested
            .linear_list_metadata
            .iter()
            .map(|metadata| metadata.column_count)
            .collect::<Vec<_>>(),
        vec![Some(2), Some(3), Some(4), Some(2), Some(3)]
    );
    assert_eq!(
        nested
            .linear_list_metadata
            .iter()
            .map(|metadata| metadata.cross_axis_gap)
            .collect::<Vec<_>>(),
        vec![
            Some(SourceLength::Auto),
            Some(SourceLength::Fr(4.0)),
            Some(SourceLength::MaxContent),
            Some(SourceLength::FitContentPoints(14.0)),
            Some(SourceLength::Auto),
        ]
    );

    let raw_components = (scenario_named("staggered_linear_raw_list_gaps").build)(289);
    for (kind, expected) in [
        (Some(SourceListComponentType::Header), 17),
        (Some(SourceListComponentType::ListRow), 17),
        (Some(SourceListComponentType::Footer), 17),
        (None, 238),
    ] {
        assert_eq!(
            raw_components
                .list_item_metadata
                .iter()
                .filter(|metadata| metadata.component_type == kind)
                .count(),
            expected
        );
    }
}

#[test]
fn full_spacing_preserves_list_metadata_without_expanding_the_protocol() {
    let spacing = (scenario_named("full_value_spacing_matrix").build)(36);
    assert_eq!(spacing.linear_list_metadata.len(), 9);
    let spacing_root = &spacing.tree.source.nodes[usize::from(spacing.root)];
    for (metadata_index, metadata) in spacing.linear_list_metadata.iter().enumerate() {
        let source_index = 2 + metadata_index * 4;
        assert_eq!(metadata.node, spacing_root.children[source_index]);
        assert_eq!(metadata.column_count, Some(2));
        assert_eq!(
            metadata.main_axis_gap,
            Some(migrated_scenarios::source_spacing_length(source_index + 6))
        );
        assert_eq!(
            metadata.cross_axis_gap,
            Some(migrated_scenarios::source_spacing_length(source_index + 7))
        );
    }

    // TestStyle still has only generic Linear L1 fields. The benchmark-host
    // vectors preserve construction cost and authored values without adding
    // list vocabulary to production traits.
    assert!(
        SCENARIOS
            .iter()
            .filter(|scenario| scenario.lowering == Lowering::HostListProtocolElided)
            .map(|scenario| scenario.name)
            .eq([
                "full_value_spacing_matrix",
                "staggered_linear_list",
                "staggered_linear_raw_list_gaps",
            ])
    );
}

#[test]
fn sticky_workload_keeps_insets_in_host_metadata_without_visual_offsets() {
    let mut case = (scenario_named("sticky_percent_insets").build)(4);
    assert_eq!(case.sticky_insets.len(), 4);

    for (index, metadata) in case.sticky_insets.iter().enumerate() {
        let sticky = &case.tree.source.nodes[usize::from(metadata.node)];
        assert_eq!(sticky.style.position, Position::Relative);
        assert_eq!(sticky.style.inset.left, LengthPercentageAuto::Auto);
        assert_eq!(sticky.style.inset.right, LengthPercentageAuto::Auto);
        assert_eq!(sticky.style.inset.top, LengthPercentageAuto::Auto);
        assert_eq!(sticky.style.inset.bottom, LengthPercentageAuto::Auto);

        assert_eq!(metadata.insets.left, LengthPercentageAuto::Percent(0.10));
        assert_eq!(metadata.insets.top, LengthPercentageAuto::Percent(0.25));
        assert_eq!(
            metadata.insets.right,
            if index.is_multiple_of(3) {
                LengthPercentageAuto::Percent(0.05)
            } else {
                LengthPercentageAuto::Auto
            }
        );
        assert_eq!(
            metadata.insets.bottom,
            if index.is_multiple_of(5) {
                LengthPercentageAuto::Percent(0.10)
            } else {
                LengthPercentageAuto::Auto
            }
        );
    }

    case.run();
    assert_eq!(case.sticky_positions.len(), 4);
    for (index, position) in case.sticky_positions.iter().enumerate() {
        assert_eq!(position.node, case.sticky_insets[index].node);
        assert_close(position.sticky_pos.left, 32.0);
        assert_close(position.sticky_pos.top, 10.0);
        assert_close(
            position.sticky_pos.right,
            if index.is_multiple_of(3) { 16.0 } else { -1e10 },
        );
        assert_close(
            position.sticky_pos.bottom,
            if index.is_multiple_of(5) { 4.0 } else { -1e10 },
        );
    }
}

#[test]
fn display_none_workload_keeps_every_mixed_branch_and_hidden_node() {
    let mut case = (scenario_named("mixed_display_none").build)(4);
    assert_eq!(
        root_child_displays(&case),
        vec![
            TestDisplay::Flex,
            TestDisplay::Linear,
            TestDisplay::Grid,
            TestDisplay::Relative,
        ]
    );
    let hidden = root_children(&case)
        .iter()
        .map(|container| {
            assert_eq!(container.children.len(), 3);
            let hidden = container
                .children
                .iter()
                .copied()
                .filter(|child| {
                    case.tree.source.nodes[usize::from(*child)]
                        .style
                        .box_generation_mode
                        == BoxGenerationMode::None
                })
                .collect::<Vec<_>>();
            assert_eq!(hidden.len(), 1);
            hidden[0]
        })
        .collect::<Vec<_>>();

    case.run();
    assert!(
        hidden
            .iter()
            .all(|node| case.tree.layout(*node).size == Size::ZERO)
    );
}

#[test]
fn benchmark_target_lists_all_fourteen_and_has_no_native_baseline() {
    let target = include_str!("../benches/linear_pr25.rs");
    let migrated = include_str!("../benches/scenarios/linear_pr25.rs");
    for name in SOURCE_LINEAR_SCENARIOS {
        assert!(target.contains(name), "benchmark target omitted {name}");
    }
    let build_batch = target
        .find("let mut cases = (0..SOURCE_ITERATIONS)")
        .expect("the timed closure must build the source batch");
    let layout_batch = target
        .find("for case in &mut cases")
        .expect("the timed closure must lay out the source batch");
    assert!(
        build_batch < layout_batch && target.contains("divan::black_box(case.run())"),
        "every source tree must be built before the batch's first layout"
    );
    assert!(
        migrated.contains("black_box((")
            && migrated.contains("&self.sticky_positions")
            && migrated.contains("&self.linear_list_metadata")
            && migrated.contains("&self.list_item_metadata"),
        "timed runs must consume every host-only output/metadata vector"
    );
    assert!(
        target.contains("const NODES: usize = 1_000;")
            && target.contains("const SOURCE_ITERATIONS: usize = 200;")
            && target.contains("ItemsCount::new(NODES * SOURCE_ITERATIONS)")
            && target.contains("sample_count = 1, sample_size = 1")
            && !target.contains(".with_inputs("),
        "the benchmark must preserve the source default N and B...B,L...L batch once per sample"
    );

    let forbidden = [
        ["cxx", "::bridge"].concat(),
        ["extern ", "\"C\""].concat(),
        ["run_", "head_to_head"].concat(),
        ["starlight_", "cpp"].concat(),
        ["speedup", "_ratio"].concat(),
    ];
    assert!(
        forbidden
            .iter()
            .all(|needle| !target.contains(needle) && !migrated.contains(needle))
    );
}
