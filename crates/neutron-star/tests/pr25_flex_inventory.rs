//! Repository-local replacement for PR #25's ten Flex algorithm inventory
//! meta-tests.

const MIGRATION: &str = include_str!("../../../docs/pr25-flex-migration.md");
const ARCHITECTURE: &str = include_str!("../../../docs/layout-architecture.md");
const FLEXBOX: &str = include_str!("../src/compute/flexbox.rs");
const BASELINE_TESTS: &str = include_str!("flexbox.rs");
const CANONICAL_TESTS: &str = include_str!("pr25_flex_layout.rs");
const ADDITIONAL_TESTS: &str = include_str!("pr25_flex_additional.rs");
const INTERNAL_TESTS: &str = include_str!("pr25_flex_internal.rs");
const PROTOCOL_TESTS: &str = include_str!("protocol.rs");
const PUBLIC_TESTS: &str = include_str!("pr25_flex_public.rs");
const STANDALONE_TESTS: &str = include_str!("pr25_flex_standalone.rs");

const CROSS_FILE_FLEX_DIRECT_MAPPINGS: [(&str, &str); 11] = [
    (
        "flex_item_derives_cross_size_from_main_size_and_aspect_ratio",
        "flex_item_derives_cross_size_from_main_size_and_aspect_ratio",
    ),
    (
        "vertical_percentage_padding_and_margin_use_width_percent_base",
        "vertical_percentage_padding_and_margin_use_width_percent_base",
    ),
    (
        "absolute_position_can_use_right_bottom_insets",
        "absolute_position_can_use_right_and_bottom_insets_in_a_flex_container",
    ),
    (
        "absolute_flex_child_without_insets_uses_container_alignment",
        "absolute_flex_child_without_insets_uses_container_alignment",
    ),
    (
        "absolute_flex_child_center_alignment_uses_negative_free_space_when_overflowing",
        "absolute_flex_child_center_alignment_keeps_negative_free_space",
    ),
    (
        "absolute_flex_child_wrap_reverse_reverses_cross_axis_initial_position",
        "absolute_flex_child_wrap_reverse_reverses_cross_axis_static_alignment",
    ),
    (
        "absolute_rtl_flex_child_without_insets_uses_rtl_fronts",
        "absolute_rtl_flex_child_without_insets_uses_physical_fronts",
    ),
    (
        "flex_relative_child_percent_offsets_use_container_constraints",
        "flex_relative_child_percent_offsets_use_container_size",
    ),
    (
        "grid_placement_properties_do_not_affect_flex_items",
        "host_grid_placement_metadata_does_not_affect_flex_items",
    ),
    (
        "layout_engine_runs_over_custom_external_tree_ids_and_writeback",
        "external_host_measurement_baseline_and_writeback_survive_split_storage",
    ),
    (
        "layout_engine_runs_with_minimal_write_only_external_tree_adapter",
        "flex_layout_runs_with_a_minimal_write_only_session",
    ),
];

// Sticky offset clamping is deliberately not a neutron-star algorithm. These
// two source cases split into real in-flow Flex geometry plus an executable
// host-contract fixture; do not present the latter as a production layout API.
const CROSS_FILE_FLEX_HOST_CONTRACT_MAPPINGS: [(&str, &str); 2] = [
    (
        "flex_sticky_child_percent_insets_resolve_against_container_constraints",
        "flex_sticky_start_percent_insets_lower_at_the_host_boundary",
    ),
    (
        "flex_sticky_child_end_percent_insets_resolve_against_container_constraints",
        "flex_sticky_end_percent_insets_lower_at_the_host_boundary",
    ),
];

const STANDALONE_WRAPPER_GEOMETRY_MAPPINGS: [(&str, &str); 7] = [
    (
        "standalone_tree_layouts_owned_nodes_with_owner_constraints",
        "external_host_measurement_baseline_and_writeback_survive_split_storage",
    ),
    (
        "standalone_tree_measurement_api_tracks_dirty_state_and_baseline",
        "flex_layout_uses_external_text_layout_trait_for_content_size_and_baseline",
    ),
    (
        "standalone_tree_measure_func_receives_constraints_and_can_be_replaced",
        "compute_leaf_layout_uses_the_host_measurement",
    ),
    (
        "standalone_tree_baseline_func_receives_content_size_and_can_be_replaced",
        "flex_row_baseline_uses_measured_content_baseline",
    ),
    (
        "standalone_tree_node_mut_style_changes_dirty_ancestors_and_next_layout",
        "min_width_freezes_item_during_flex_shrink",
    ),
    (
        "standalone_tree_owner_direction_reaches_unset_descendants_only_during_layout",
        "standalone_direction_mapping_runs_all_8_source_cases",
    ),
    (
        "standalone_tree_clear_direction_restores_owner_direction_inheritance",
        "standalone_direction_mapping_runs_all_8_source_cases",
    ),
];

const STANDALONE_WRAPPER_HOST_EXCLUSIONS: [&str; 7] = [
    "standalone_tree_edge_style_setters_match_public_standalone_edges",
    "standalone_tree_dimension_style_setters_match_public_standalone_lengths",
    "standalone_tree_enum_scalar_and_vector_style_setters_update_style",
    "standalone_tree_exposes_layout_getters_with_edge_resolution",
    "standalone_tree_dirty_state_tracks_mutations_and_layout",
    "standalone_tree_reset_node_clears_children_layout_style_and_measurement",
    "standalone_tree_reset_attached_child_preserves_clean_parent_behavior",
];

const STANDALONE_WRAPPER_ROUNDING_EXCLUSIONS: [&str; 1] =
    ["standalone_tree_measured_layout_ceil_uses_node_physical_pixels_per_layout_unit"];

fn any_test_source_contains(needle: &str) -> bool {
    [
        BASELINE_TESTS,
        CANONICAL_TESTS,
        ADDITIONAL_TESTS,
        INTERNAL_TESTS,
        PROTOCOL_TESTS,
        PUBLIC_TESTS,
        STANDALONE_TESTS,
    ]
    .iter()
    .any(|source| source.contains(needle))
}

#[test]
fn standalone_wrapper_flex_inventory_maps_seven_geometry_slices_and_eight_exclusions() {
    assert_eq!(STANDALONE_WRAPPER_GEOMETRY_MAPPINGS.len(), 7);
    assert_eq!(STANDALONE_WRAPPER_HOST_EXCLUSIONS.len(), 7);
    assert_eq!(STANDALONE_WRAPPER_ROUNDING_EXCLUSIONS.len(), 1);

    for (source, target) in STANDALONE_WRAPPER_GEOMETRY_MAPPINGS {
        assert!(
            any_test_source_contains(&format!("fn {target}(")),
            "standalone wrapper source test {source} must map to existing geometry target {target}"
        );
    }
    for source in STANDALONE_WRAPPER_HOST_EXCLUSIONS {
        assert!(
            MIGRATION.contains(source),
            "host-wrapper exclusion {source} must remain documented by exact source name"
        );
    }
    for source in STANDALONE_WRAPPER_ROUNDING_EXCLUSIONS {
        assert!(
            MIGRATION.contains(source),
            "rounding exclusion {source} must remain documented by exact source name"
        );
    }
}

#[test]
fn cross_file_flex_inventory_maps_eleven_direct_tests_and_two_host_contracts() {
    assert_eq!(CROSS_FILE_FLEX_DIRECT_MAPPINGS.len(), 11);
    assert_eq!(CROSS_FILE_FLEX_HOST_CONTRACT_MAPPINGS.len(), 2);
    for (source, target) in CROSS_FILE_FLEX_DIRECT_MAPPINGS
        .into_iter()
        .chain(CROSS_FILE_FLEX_HOST_CONTRACT_MAPPINGS)
    {
        let needle = format!("fn {target}(");
        assert!(
            ADDITIONAL_TESTS.contains(&needle),
            "cross-file source test {source} must map to existing Rust target {target}"
        );
    }
}

#[test]
fn flexbox_layout_algorithm_inventory_tracks_every_w3c_step() {
    for step in [
        "Setup",
        "Available space & flex base sizes",
        "Line breaking",
        "Resolving flexible lengths",
        "Cross sizing",
        "Main-axis alignment",
        "Cross-axis alignment",
        "Out-of-flow children",
        "Finalize",
    ] {
        assert!(
            ARCHITECTURE.contains(step),
            "missing documented Flex pass {step}"
        );
    }
}

#[test]
fn flexbox_inventory_records_9_4_stretch_aspect_ratio_decision() {
    assert!(any_test_source_contains(
        "aspect_ratio_does_not_disable_cross_axis_stretch"
    ));
    assert!(any_test_source_contains(
        "stretched_flex_item_with_aspect_ratio_keeps_flexed_main_size_and_uses_line_cross_size"
    ));
}

#[test]
fn flexbox_alignment_inventory_traces_every_w3c_alignment_clause() {
    for family in [
        "ALIGN_CONTENT_VALUES",
        "ALIGN_ITEMS_VALUES",
        "ALIGN_SELF_VALUES",
        "JUSTIFY_CONTENT_VALUES",
    ] {
        assert!(PUBLIC_TESTS.contains(family));
    }
    for fallback in [
        "justify_content_negative_free_space_direction_matrix_uses_w3c_fallbacks",
        "overflowing_cross_axis_auto_margins_place_overflow_at_cross_end",
        "align_content_space_evenly_uses_negative_space_when_lines_overflow",
    ] {
        assert!(any_test_source_contains(fallback));
    }
}

#[test]
fn flexbox_initial_setup_inventory_traces_every_w3c_clause() {
    for target in [
        "order_is_stable_and_layout_order_is_the_sorted_index",
        "display_none_child_is_laid_out_as_zero_and_skipped_by_flex",
        "flex_visibility_collapse_restarts_line_collection_with_zero_main_size",
        "flex_initial_setup_skips_out_of_flow_children_but_positions_static_rect",
    ] {
        assert!(
            any_test_source_contains(target),
            "missing initial-setup target {target}"
        );
    }
}

#[test]
fn flexbox_line_length_inventory_traces_every_w3c_clause() {
    for target in [
        "flex_wrap_collects_zero_sized_item_after_exact_fit_on_same_line",
        "flex_main_size_wrap_collects_oversized_first_item_alone",
        "flex_main_size_line_collection_uses_outer_hypothetical_main_with_negative_margin",
        "flexible_lengths_resolve_independently_per_wrapped_line",
    ] {
        assert!(
            any_test_source_contains(target),
            "missing line-collection target {target}"
        );
    }
}

#[test]
fn flexbox_main_size_inventory_traces_every_w3c_clause() {
    for target in [
        "flex_factor_selection_uses_hypothetical_sizes_not_flex_base_sum",
        "initial_free_space_uses_frozen_targets_outer_margins_and_gap",
        "flex_shrink_distribution_is_scaled_by_flex_base_size",
        "multiple_max_width_violations_freeze_before_redistributing_flex_grow_space",
        "multiple_min_width_violations_freeze_before_redistributing_flex_shrink_space",
    ] {
        assert!(
            any_test_source_contains(target),
            "missing main-size target {target}"
        );
    }
}

#[test]
fn flexbox_cross_size_inventory_traces_every_w3c_clause() {
    for target in [
        "flex_cross_size_hypothetical_cross_layout_uses_used_main_size",
        "flex_cross_size_baseline_line_size_uses_largest_baseline_distances",
        "align_content_stretch_expands_wrapped_line_cross_sizes",
        "stretched_flex_item_cross_size_respects_min_max_constraints",
        "flex_column_container_baseline_uses_first_item_baseline_after_main_axis_alignment",
    ] {
        assert!(
            any_test_source_contains(target),
            "missing cross-size target {target}"
        );
    }
}

#[test]
fn flexbox_layout_algorithm_inventory_targets_existing_symbols() {
    for symbol in [
        "determine_flex_base_sizes",
        "collect_flex_lines",
        "resolve_flexible_lengths",
        "calculate_line_cross_sizes",
        "distribute_main_axis",
        "align_lines",
        "align_items_cross_axis",
        "perform_in_flow_layout",
        "perform_absolute_children",
    ] {
        assert!(
            FLEXBOX.contains(&format!("fn {symbol}")),
            "documented Flex target symbol {symbol} must exist"
        );
    }
}

#[test]
fn flexbox_migration_boundaries_prevent_false_cpp_parity_status() {
    for boundary in [
        "No C++ engine",
        "No styling engine",
        "not claim parity for the foreign algorithm",
        "not a compatibility promise with the C++ Starlight engine",
    ] {
        assert!(
            MIGRATION.contains(boundary),
            "missing boundary statement {boundary}"
        );
    }
}

#[test]
fn known_flexbox_migration_debt_markers_are_tracked() {
    for marker in [
        "`fr` outside Grid",
        "Sticky positioning is a host post-pass",
        "`display: block`, `linear`, `relative`, and `grid`",
        "cache-parity tests",
        "linear_cross_axis_cache_guard_covers_ignored_stretch_and_auto_children",
    ] {
        assert!(
            MIGRATION.contains(marker),
            "missing tracked boundary/debt marker {marker}"
        );
    }
}
