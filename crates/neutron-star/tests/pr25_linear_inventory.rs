//! Repository-local replacement for PR #25's four combined
//! Linear/Relative algorithm-inventory tests, plus migration guards.

const MIGRATION: &str = include_str!("../../../docs/pr25-linear-migration.md");
const BENCHMARK_MIGRATION: &str = include_str!("../../../docs/pr25-linear-benchmarks.md");
const SPEC: &str = include_str!("../../../docs/starlight-linear-layout.md");
const ARCHITECTURE: &str = include_str!("../../../docs/layout-architecture.md");
const TRACKING: &str = include_str!("../../../docs/tracking/css-layout.md");
const LINEAR: &str = include_str!("../src/compute/linear.rs");
const LINEAR_STYLE: &str = include_str!("../src/style/linear.rs");
const CACHE: &str = include_str!("../src/cache.rs");
const TREE: &str = include_str!("../src/tree/mod.rs");
const DIRECT: &str = include_str!("pr25_linear_layout.rs");
const ADDITIONAL: &str = include_str!("pr25_linear_additional.rs");
const BLOCK_AS_LINEAR: &str = include_str!("pr25_block_as_linear.rs");
const EXTERNAL_CALLBACK: &str = include_str!("pr25_linear_external_callback.rs");
const NATIVE: &str = include_str!("pr25_native_linear.rs");
const NATIVE_EXACT: &str = include_str!("pr25_native_linear_exact.rs");
const NATIVE_INVENTORY: &str = include_str!("pr25_native_linear_inventory.txt");
const GENERATED: &str = include_str!("pr25_generated_linear.rs");
const GENERATED_SUPPORT: &str = include_str!("pr25_generated_linear_support/mod.rs");
const STANDALONE: &str = include_str!("pr25_linear_standalone.rs");
const STANDALONE_SUPPORT: &str = include_str!("pr25_linear_standalone_support/mod.rs");
const STANDALONE_INVENTORY: &str = include_str!("pr25_linear_standalone_inventory.txt");
const PUBLIC: &str = include_str!("pr25_linear_public.rs");
const FLEX_DIRECT: &str = include_str!("pr25_flex_layout.rs");
const FLEX_INTERNAL: &str = include_str!("pr25_flex_internal.rs");
const SUPPORT: &str = include_str!("pr25_support/mod.rs");
const COMMON_SUPPORT: &str = include_str!("support/mod.rs");
const BENCH: &str = include_str!("../benches/linear_pr25.rs");
const BENCH_SCENARIOS: &str = include_str!("../benches/scenarios/linear_pr25.rs");
const BENCH_TESTS: &str = include_str!("pr25_linear_bench_scenarios.rs");
const MANIFEST: &str = include_str!("../Cargo.toml");

fn literal_test_count(source: &str) -> usize {
    source
        .lines()
        .filter(|line| line.trim() == "#[test]")
        .count()
}

#[test]
fn linear_relative_algorithm_inventory_tracks_starlight_spec_sections() {
    for section in [
        "## Linear Items",
        "### 1. Initial Setup",
        "### 2. Item Measurement",
        "### 3. Weighted Main-Size Resolution",
        "### 4. Container Size Determination",
        "### 5. Main-Axis Alignment",
        "### 6. Cross-Axis Alignment",
        "### 7. Baseline",
        "### 8. Out-of-Flow Children",
    ] {
        assert!(
            SPEC.contains(section),
            "missing Linear spec section {section}"
        );
    }
    assert!(MIGRATION.contains("eight numbered spec"));
}

#[test]
fn linear_relative_inventory_targets_existing_symbols() {
    for symbol in [
        "fn resolve_item<",
        "fn refresh_item_edges<",
        "fn child_measurement<",
        "fn resolve_intrinsic_sizes<",
        "fn measure_item<",
        "fn distribute_weighted_items(",
        "fn size_items<",
        "fn natural_content_size(",
        "fn main_axis_distribution(",
        "fn position_items(",
        "fn absolute_static_position(",
        "fn container_baseline(",
        "fn commit_in_flow<",
        "fn commit_non_in_flow_children<",
        "pub fn compute_linear_layout<",
    ] {
        assert!(LINEAR.contains(symbol), "missing Linear symbol {symbol}");
    }
    for symbol in ["LinearContainerStyle", "LinearItemStyle"] {
        assert!(
            LINEAR_STYLE.contains(symbol),
            "missing style symbol {symbol}"
        );
    }
    assert!(TREE.contains("pub trait LinearSource: LayoutSource"));
    assert!(ARCHITECTURE.contains("LinearSource"));
    assert!(TRACKING.contains("display: linear"));
}

#[test]
fn linear_relative_inventory_traces_current_module_helpers() {
    for helper in [
        "LinearAxes",
        "computed_cross_gravity",
        "computed_main_gravity",
        "initial_item_flags",
        "intrinsic_measurement",
        "apply_border_box_ratio",
        "outer_main",
        "outer_cross",
        "cross_alignment_offset",
        "measure_absolute_static_box",
        "item_location",
        "relative_offset",
        "hide_subtree",
    ] {
        assert!(LINEAR.contains(helper), "untracked Linear helper {helper}");
    }
    assert!(CACHE.contains("The key is the **complete [`LayoutInput`]**"));
    assert!(FLEX_INTERNAL.contains("flex_cache_reuse_helpers_cover_mode_matrix_and_guard_paths"));
    assert!(
        EXTERNAL_CALLBACK
            .contains("fn linear_cross_axis_cache_guard_covers_ignored_stretch_and_auto_children(")
    );
}

#[test]
fn linear_relative_inventory_records_layout_resolver_and_visibility_boundaries() {
    for boundary in [
        "resolve_container_box",
        "resolve_item_box",
        "BoxGenerationMode::None",
        "hide_subtree",
        "Position::AbsoluteHoisted",
    ] {
        assert!(
            LINEAR.contains(boundary),
            "missing resolver boundary {boundary}"
        );
    }
    for test in [
        "visibility_hidden_and_collapse_linear_children_participate_in_layout",
        "display_none_child_is_laid_out_as_zero_and_skipped_by_linear_stack",
        "display_none_parent_clears_descendant_layouts",
    ] {
        assert!(
            DIRECT.contains(&format!("fn {test}(")),
            "missing boundary test {test}"
        );
    }
    assert!(MIGRATION.contains("Hidden subtrees"));
    assert!(SPEC.contains("visibility: hidden"));
    assert!(SPEC.contains("display: none"));
}

#[test]
fn linear_migration_counts_every_test_surface() {
    assert_eq!(literal_test_count(DIRECT), 119);
    assert_eq!(literal_test_count(ADDITIONAL), 4);
    assert_eq!(literal_test_count(BLOCK_AS_LINEAR), 9);
    assert_eq!(literal_test_count(EXTERNAL_CALLBACK), 6);
    for source_name in [
        "external_callback_display_none_child_is_zero_and_skipped_by_linear_stack",
        "external_callback_sticky_percent_insets_are_exported_for_container_children",
        "external_callback_flex_uses_nested_linear_container_baseline",
        "external_callback_linear_baseline_keeps_unresolved_start_auto_margin",
        "external_callback_linear_baseline_uses_gravity_before_paired_auto_margins_resolve",
        "linear_cross_axis_cache_guard_covers_ignored_stretch_and_auto_children",
    ] {
        assert!(
            EXTERNAL_CALLBACK.contains(&format!("fn {source_name}(")),
            "missing callback/cache fixture {source_name}"
        );
    }
    assert!(FLEX_DIRECT.contains("fn flex_row_baseline_uses_nested_linear_container_baseline("));

    // Two source functions have non-Linear names but instantiate Linear, and
    // two additional lines are source helpers rather than tests.
    assert_eq!(NATIVE_INVENTORY.lines().count(), 140);
    for hidden_name in [
        "head_to_head_flex_column_stretch_with_fr_sibling_preserves_percent_basis",
        "head_to_head_owner_definite_height_without_root_height_uses_root_at_most_height",
    ] {
        assert!(NATIVE_INVENTORY.lines().any(|name| name == hidden_name));
        assert!(NATIVE.contains(hidden_name));
    }
    assert!(NATIVE.contains("SOURCE_TEST_COUNT: usize = 138"));
    assert!(NATIVE.contains("OVERLAP_EXECUTION_COUNT: usize = 146"));
    assert!(NATIVE.contains("491\n    );"));
    assert!(NATIVE.contains("NATIVE_LINEAR_EXACT_SOURCE"));
    assert_eq!(literal_test_count(NATIVE_EXACT), 106);
    assert!(NATIVE_EXACT.contains("native_direct_overlap_inventory_has_exact_builders_and_rows"));
    assert!(NATIVE_EXACT.contains("assert_eq!(names.len(), 105)"));
    assert!(NATIVE_EXACT.contains("146\n    );"));
    assert!(NATIVE_EXACT.contains("display: Display::Flex"));
    assert!(NATIVE.contains("block-as-Linear source test has no executable Rust port"));

    assert_eq!(literal_test_count(GENERATED), 24);
    for count in ["27_794", "257"] {
        assert!(GENERATED.contains(count), "missing generated count {count}");
    }
    for count in ["1,758", "29,826"] {
        assert!(MIGRATION.contains(count), "missing generated total {count}");
    }
    assert!(GENERATED_SUPPORT.contains("fn deterministic_supported_tree("));

    assert_eq!(STANDALONE_INVENTORY.lines().count(), 458);
    assert_eq!(literal_test_count(STANDALONE), 459);
    assert!(
        STANDALONE.contains("standalone_linear_source_inventory_is_exact_and_executes_all_rows")
    );
    assert!(STANDALONE.contains("543"));
    assert!(
        STANDALONE.contains("standalone_owned_tree_matches_cpp_for_out_of_flow_intrinsic_sizing")
    );
    for name in STANDALONE_INVENTORY.lines() {
        assert!(
            STANDALONE.contains(&format!("fn {name}")),
            "missing source-shaped standalone fixture {name}"
        );
    }
    assert!(STANDALONE_SUPPORT.contains("run_standalone_rust"));
    assert!(STANDALONE_SUPPORT.contains("round_layout(&topology"));
    assert!(!STANDALONE.contains("run_standalone_head_to_head"));
    assert!(PUBLIC.contains("snapshots, 72"));
    assert!(
        PUBLIC.contains("fn standalone_public_display_layout_matrix_retains_the_linear_slice(")
    );
    assert!(
        PUBLIC.contains("fn standalone_public_list_gap_layout_matrix_runs_all_seven_rust_rows(")
    );
    assert!(PUBLIC.contains("const PUBLIC_LIST_GAP_VARIANTS: [PublicListGapVariant; 7]"));
    assert!(PUBLIC.contains("const SOURCE_PUBLIC_DISPLAY_VARIANTS: [Display; 6]"));
    assert!(PUBLIC.contains("standalone_public_linear_list_fixture_is_host_owned"));
    assert!(MIGRATION.contains("Adapter-only source tests"));
    for classification in [
        "flex_cache_reuse_helpers_cover_mode_matrix_and_guard_paths",
        "public_header_links_and_runs_c_smoke_when_ffi_library_is_available",
        "block_layout_runs_on_external_tree",
        "out_of_flow_intrinsic",
    ] {
        assert!(
            MIGRATION.contains(classification),
            "missing source-boundary classification {classification}"
        );
    }
}

#[test]
fn linear_migration_counts_all_fourteen_benchmarks() {
    let source_names = [
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
    for name in source_names {
        assert!(BENCH.contains(name), "missing benchmark function {name}");
        assert!(
            BENCH_TESTS.contains(name),
            "missing benchmark inventory {name}"
        );
    }
    assert!(BENCH_SCENARIOS.contains("HostListProtocolElided"));
    assert!(BENCHMARK_MIGRATION.contains("fourteen Linear-tagged workloads"));
    assert!(MANIFEST.contains("name = \"linear_pr25\""));
}

#[test]
fn linear_migration_is_rust_only_and_uses_real_linear_dispatch() {
    assert!(SUPPORT.contains("Display::Linear | Display::Block => TestDisplay::Linear"));
    assert!(COMMON_SUPPORT.contains("compute_linear_layout(source, session, child, input)"));

    let migrated_rust = [
        DIRECT,
        ADDITIONAL,
        BLOCK_AS_LINEAR,
        EXTERNAL_CALLBACK,
        NATIVE,
        NATIVE_EXACT,
        GENERATED,
        GENERATED_SUPPORT,
        STANDALONE,
        STANDALONE_SUPPORT,
        PUBLIC,
        BENCH,
        BENCH_SCENARIOS,
    ]
    .join("\n");
    let forbidden = [
        ["use starlight_", "cpp"].concat(),
        ["CppStarlight", "Engine"].concat(),
        ["extern ", "\"C\""].concat(),
        ["run_", "head_to_head"].concat(),
        ["cxx", "::bridge"].concat(),
    ];
    assert!(
        forbidden
            .iter()
            .all(|needle| !migrated_rust.contains(needle))
    );
}
