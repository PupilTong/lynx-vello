//! Repository-local replacements for PR #25's four Relative algorithm
//! inventory tests, plus migration cardinality and Rust-only boundary guards.

const MIGRATION: &str = include_str!("../../../docs/pr25-relative-migration.md");
const SPEC: &str = include_str!("../../../docs/starlight-relative-layout.md");
const ARCHITECTURE: &str = include_str!("../../../docs/layout-architecture.md");
const TRACKING: &str = include_str!("../../../docs/tracking/css-layout.md");
const RELATIVE: &str = include_str!("../src/compute/relative.rs");
const RELATIVE_STYLE: &str = include_str!("../src/style/relative.rs");
const DIRECT: &str = include_str!("pr25_relative_layout.rs");
const NATIVE: &str = include_str!("pr25_native_relative.rs");
const GENERATED: &str = include_str!("pr25_generated_relative.rs");
const STANDALONE: &str = include_str!("pr25_relative_standalone.rs");
const ADDITIONAL: &str = include_str!("pr25_relative_additional.rs");
const PUBLIC: &str = include_str!("pr25_relative_public.rs");
const SUPPORT: &str = include_str!("pr25_support/mod.rs");
const BENCH: &str = include_str!("../benches/relative_pr25.rs");
const BENCH_SCENARIOS: &str = include_str!("../benches/scenarios/relative.rs");
const BENCH_TESTS: &str = include_str!("relative_bench_scenarios.rs");
const MANIFEST: &str = include_str!("../Cargo.toml");

const GENERATED_MATRICES: &[(&str, usize)] = &[
    (
        "generated_relative_center_parent_edge_matrix_matches_cpp",
        64,
    ),
    (
        "generated_relative_sibling_dependency_matrix_matches_cpp",
        32,
    ),
    (
        "generated_relative_missing_reference_matrix_matches_cpp",
        96,
    ),
    (
        "generated_relative_dependency_resolution_matrix_matches_cpp",
        30,
    ),
    (
        "generated_relative_measured_constraint_matrix_matches_cpp",
        20,
    ),
    ("generated_relative_composite_feature_matrix_matches_cpp", 6),
    ("generated_measured_callback_matrix_matches_cpp", 4),
    ("generated_flex_baseline_propagation_matrix_matches_cpp", 6),
    ("generated_sizing_minmax_aspect_matrix_matches_cpp", 8),
    ("generated_display_none_origin_matrix_matches_cpp", 1),
    ("generated_out_of_flow_position_matrix_matches_cpp", 32),
    ("generated_out_of_flow_sizing_matrix_matches_cpp", 12),
    ("generated_fixed_descendant_matrix_matches_cpp", 65),
    ("generated_sticky_position_matrix_matches_cpp", 48),
    ("generated_sticky_sizing_matrix_matches_cpp", 5),
];

const BENCHMARKS: &[&str] = &[
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

const STANDALONE_FALSE_FRIENDS: &[&str] = &[
    "standalone_owned_tree_matches_cpp_for_linear_relative_position_physical_pixel_rounding",
    "standalone_owned_tree_matches_cpp_for_relative_position_offsets",
    "standalone_owned_tree_matches_cpp_for_linear_relative_position_left_top_preserves_horizontal_flow",
    "standalone_owned_tree_matches_cpp_for_linear_relative_position_right_bottom_calc_preserves_vertical_flow",
    "standalone_owned_tree_matches_cpp_for_linear_relative_position_start_insets_win_over_end_insets",
    "standalone_owned_tree_matches_cpp_for_calc_and_out_of_flow_edge_behaviors",
];

fn section<'a>(source: &'a str, start: &str, end: &str) -> &'a str {
    let start = source
        .find(start)
        .unwrap_or_else(|| panic!("missing section start `{start}`"));
    let source = &source[start..];
    let end = source
        .find(end)
        .unwrap_or_else(|| panic!("missing section end `{end}`"));
    &source[..end]
}

fn lines_starting_with(source: &str, prefix: &str) -> usize {
    source
        .lines()
        .filter(|line| line.trim_start().starts_with(prefix))
        .count()
}

fn literal_test_count(source: &str) -> usize {
    source
        .lines()
        .filter(|line| line.trim() == "#[test]")
        .count()
}

#[test]
fn linear_relative_algorithm_inventory_tracks_starlight_spec_sections() {
    for section in [
        "## Relative Items",
        "## Reference Resolution",
        "## Dependency Ordering",
        "### 1. Initial Setup",
        "### 2. Initial Child Constraints",
        "### 3. Position Equation",
        "### 4. One-Pass Relative Layout",
        "### 5. Two-Pass Relative Layout",
        "### 6. Container Size Determination",
        "### 7. Final Item Placement",
        "### 8. Out-of-Flow Children",
    ] {
        assert!(
            SPEC.contains(section),
            "missing Relative spec section {section}"
        );
    }
    assert!(MIGRATION.contains("eight numbered algorithm sections"));
}

#[test]
fn linear_relative_inventory_targets_existing_symbols() {
    for symbol in [
        "fn resolve_item<",
        "fn dependency_order(",
        "fn axis_constraints(",
        "fn measure_item<",
        "fn position_axis(",
        "fn one_pass_layout<",
        "fn two_pass_layout<",
        "fn commit_in_flow<",
        "fn commit_out_of_flow<",
        "pub fn compute_relative_layout<",
    ] {
        assert!(
            RELATIVE.contains(symbol),
            "missing Relative symbol {symbol}"
        );
    }
    for symbol in ["RelativeContainerStyle", "RelativeItemStyle"] {
        assert!(
            RELATIVE_STYLE.contains(symbol),
            "missing style symbol {symbol}"
        );
    }
    assert!(ARCHITECTURE.contains("RelativeSource: LayoutSource"));
    assert!(TRACKING.contains("display: relative"));
}

#[test]
fn linear_relative_inventory_traces_current_module_helpers() {
    for helper in [
        "IdLookup",
        "Dependencies",
        "add_axis_dependencies",
        "reference_position",
        "all_constraints",
        "constrained_border_size",
        "fit_content_available",
        "prepare_intrinsic_sizes",
        "measurement_input",
        "refresh_item_bases",
        "final_outer_axis",
        "relative_offset",
    ] {
        assert!(
            RELATIVE.contains(helper),
            "untracked Relative helper {helper}"
        );
    }
    assert!(MIGRATION.contains("resolve_item"));
    assert!(MIGRATION.contains("commit_out_of_flow"));
}

#[test]
fn linear_relative_inventory_records_layout_resolver_and_visibility_boundaries() {
    for boundary in [
        "resolve_item_box_with_bases",
        "BoxGenerationMode::None",
        "hide_subtree",
        "Position::AbsoluteHoisted",
    ] {
        assert!(
            RELATIVE.contains(boundary),
            "missing resolver boundary {boundary}"
        );
    }
    for test in [
        "visibility_hidden_and_collapse_relative_children_participate_in_dependency_layout",
        "relative_display_skips_display_none_duplicate_id_for_dependency_lookup",
    ] {
        assert!(
            DIRECT.contains(&format!("fn {test}(")),
            "missing boundary test {test}"
        );
    }
    assert!(MIGRATION.contains("visibility: hidden"));
    assert!(MIGRATION.contains("display: none"));
}

#[test]
fn relative_migration_counts_direct_native_and_cross_file_surfaces() {
    assert_eq!(literal_test_count(DIRECT), 72);
    assert!(MIGRATION.contains("72 direct tests"));

    let false_friends = section(
        NATIVE,
        "const FALSE_FRIENDS",
        "const NATIVE_RELATIVE_INVENTORY",
    );
    let native = section(
        NATIVE,
        "const NATIVE_RELATIVE_INVENTORY",
        "const CANONICAL_RELATIVE_MAPPING",
    );
    let canonical = section(
        NATIVE,
        "const CANONICAL_RELATIVE_MAPPING",
        "fn assert_close",
    );
    let unique = section(
        NATIVE,
        "unique_native_relative_cases!(",
        "#[test]\nfn native_relative_inventory",
    );
    assert_eq!(lines_starting_with(false_friends, "\"head_to_head_"), 4);
    assert_eq!(lines_starting_with(native, "\"head_to_head_"), 72);
    assert_eq!(lines_starting_with(canonical, "\"head_to_head_"), 57);
    assert_eq!(lines_starting_with(unique, "head_to_head_"), 15);
    for marker in [
        "57 canonical",
        "15 unique",
        "four CSS-position false friends",
    ] {
        assert!(
            MIGRATION.contains(marker),
            "missing native partition `{marker}`"
        );
    }

    for sticky in [
        "relative_sticky_child_percent_insets_resolve_against_container_constraints",
        "relative_sticky_child_end_percent_insets_resolve_against_container_constraints",
    ] {
        assert!(ADDITIONAL.contains(&format!("fn {sticky}(")));
    }
    assert!(MIGRATION.contains("2 additional sticky cases"));

    for public_test in [
        "standalone_public_relative_layout_matrix_runs_both_rust_snapshots",
        "standalone_public_display_layout_matrix_retains_the_relative_slice",
        "standalone_public_relative_center_enum_slice_covers_every_value",
    ] {
        assert!(PUBLIC.contains(&format!("fn {public_test}(")));
    }
    assert!(MIGRATION.contains("2 dedicated Relative snapshots"));
    assert!(MIGRATION.contains("mixed display matrix"));
}

#[test]
fn relative_generated_and_standalone_inventories_keep_every_case() {
    assert_eq!(GENERATED_MATRICES.len(), 15);
    assert_eq!(
        GENERATED_MATRICES
            .iter()
            .map(|(_, count)| count)
            .sum::<usize>(),
        429
    );
    for (name, _) in GENERATED_MATRICES {
        assert!(
            GENERATED.contains(&format!("fn {name}(")),
            "missing matrix {name}"
        );
        assert!(MIGRATION.contains(name), "undocumented matrix {name}");
    }

    let standalone = section(STANDALONE, "standalone_relative_cases!(", "\n);");
    assert_eq!(
        lines_starting_with(standalone, "standalone_owned_tree_matches_cpp_for_"),
        401
    );
    assert_eq!(STANDALONE_FALSE_FRIENDS.len(), 6);
    for false_friend in STANDALONE_FALSE_FRIENDS {
        assert!(!standalone.contains(false_friend));
        assert!(MIGRATION.contains(false_friend));
    }
    assert!(MIGRATION.contains("401 Relative-container source"));
}

#[test]
fn relative_benchmark_inventory_keeps_all_nine_rust_workloads() {
    assert_eq!(BENCHMARKS.len(), 9);
    for name in BENCHMARKS {
        assert!(BENCH.contains(&format!("relative_bench!({name})")));
        assert!(BENCH_SCENARIOS.contains(&format!("\"{name}\"")));
        assert!(BENCH_TESTS.contains(&format!("\"{name}\"")));
        assert!(MIGRATION.contains(name));
    }
}

#[test]
fn relative_migration_is_rust_only_unignored_and_documents_translations() {
    for (name, source) in [
        ("direct", DIRECT),
        ("native", NATIVE),
        ("generated", GENERATED),
        ("standalone", STANDALONE),
        ("additional", ADDITIONAL),
        ("public API", PUBLIC),
        ("benchmark inventory", BENCH_TESTS),
    ] {
        assert!(
            !source.contains("#[ignore"),
            "ignored Relative case in {name}"
        );
        for forbidden in [
            "use starlight_cpp",
            "CppStarlightEngine",
            "run_head_to_head(",
            "run_standalone_head_to_head(",
            "extern \"C\"",
            "cxx::bridge",
            "#[link(",
            "native-standalone",
        ] {
            assert!(!source.contains(forbidden), "{name} imports `{forbidden}`");
        }
    }
    assert!(!MANIFEST.contains("starlight_cpp"));
    assert!(!MANIFEST.contains("starlight_ffi"));
    assert!(SUPPORT.contains("finish_relative_fixed_pass"));

    for family in [
        "Fractional rounding",
        "CSS fit-content min-content floor",
        "Caller constraint authority",
        "One-pass is not retroactive",
        "Fixed and Sticky are host boundaries",
    ] {
        assert!(
            MIGRATION.contains(family),
            "missing translation family {family}"
        );
    }
    for exclusion in [
        "FFI/C ABI layer",
        "style-storage tests",
        "GN wiring",
        "No C++ or FFI surface is fabricated",
    ] {
        assert!(
            MIGRATION.contains(exclusion),
            "missing exclusion {exclusion}"
        );
    }
}
