//! Repository-local replacement for PR #25's ten Flex algorithm inventory
//! meta-tests.

const MIGRATION: &str = include_str!("../../../docs/pr25-flex-migration.md");
const ARCHITECTURE: &str = include_str!("../../../docs/layout-architecture.md");
const FLEXBOX: &str = include_str!("../src/compute/flexbox.rs");
const BASELINE_TESTS: &str = include_str!("flexbox.rs");
const CANONICAL_TESTS: &str = include_str!("pr25_flex_layout.rs");
const ADDITIONAL_TESTS: &str = include_str!("pr25_flex_additional.rs");
const INTERNAL_TESTS: &str = include_str!("pr25_flex_internal.rs");
const PUBLIC_TESTS: &str = include_str!("pr25_flex_public.rs");
const STANDALONE_TESTS: &str = include_str!("pr25_flex_standalone.rs");

fn any_test_source_contains(needle: &str) -> bool {
    [
        BASELINE_TESTS,
        CANONICAL_TESTS,
        ADDITIONAL_TESTS,
        INTERNAL_TESTS,
        PUBLIC_TESTS,
        STANDALONE_TESTS,
    ]
    .iter()
    .any(|source| source.contains(needle))
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
