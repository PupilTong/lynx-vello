//! Repository-local translations of PR #25's 15 Grid algorithm inventory
//! meta-tests.

const MIGRATION: &str = include_str!("../../../docs/pr25-grid-migration.md");
const ARCHITECTURE: &str = include_str!("../../../docs/layout-architecture.md");
const GRID_MOD: &str = include_str!("../src/compute/grid/mod.rs");
const PLACEMENT: &str = include_str!("../src/compute/grid/placement.rs");
const TRACKS: &str = include_str!("../src/compute/grid/tracks.rs");
const SIZING: &str = include_str!("../src/compute/grid/sizing.rs");
const ALIGNMENT: &str = include_str!("../src/compute/grid/alignment.rs");
const DIRECT: &str = include_str!("pr25_grid_layout.rs");
const NATIVE: &str = include_str!("pr25_native_grid.rs");
const GENERATED: &str = include_str!("pr25_generated_grid.rs");
const BENCH: &str = include_str!("../benches/grid_pr25.rs");
const BENCH_SCENARIOS: &str = include_str!("../benches/scenarios/grid.rs");

fn assert_order(source: &str, markers: &[&str]) {
    let mut cursor = 0;
    for marker in markers {
        let offset = source[cursor..]
            .find(marker)
            .unwrap_or_else(|| panic!("missing `{marker}`"));
        cursor += offset + marker.len();
    }
}

#[test]
fn grid_algorithm_inventory_tracks_w3c_placement_and_sizing_steps() {
    for section in [
        "placement",
        "Absolute Grid containing areas",
        "Alignment",
        "Initialize track sizes",
        "Resolve intrinsic track sizes",
        "Maximize tracks",
        "Expand flexible tracks",
        "Stretch `auto` tracks",
    ] {
        assert!(MIGRATION.contains(section), "missing {section}");
    }
}

#[test]
fn grid_algorithm_inventory_records_grid_module_coverage_snapshot() {
    for evidence in [
        "230 direct Grid layout tests",
        "179 Grid-named native head-to-head cases",
        "6 generated Grid matrices",
        "15 repository/algorithm inventory checks",
        "18 Grid-tagged benchmark scenarios",
    ] {
        assert!(MIGRATION.contains(evidence));
    }
}

#[test]
fn grid_algorithm_inventory_records_visibility_layout_participation() {
    assert!(DIRECT.contains("hidden_and_collapse_grid_children_participate_in_auto_placement"));
    assert!(NATIVE.contains(
        "head_to_head_grid_visibility_hidden_and_collapse_participate_in_auto_placement"
    ));
}

#[test]
fn grid_algorithm_inventory_targets_existing_symbols() {
    for (source, symbols) in [
        (PLACEMENT, &["resolve_axis_placement", "place_items"][..]),
        (TRACKS, &["expand_template", "build_axis_tracks"]),
        (
            SIZING,
            &[
                "initialize_tracks",
                "resolve_intrinsic_sizes",
                "find_fr_size",
            ],
        ),
        (ALIGNMENT, &["align_tracks", "item_alignment_offset"]),
        (GRID_MOD, &["compute_grid_layout", "layout_absolute_items"]),
    ] {
        for symbol in symbols {
            assert!(source.contains(symbol), "missing {symbol}");
        }
    }
}

#[test]
fn grid_algorithm_inventory_records_ignored_grid_head_to_head_gaps() {
    assert!(!DIRECT.contains("#[ignore"));
    assert!(!NATIVE.contains("#[ignore"));
    assert!(!GENERATED.contains("#[ignore"));
    assert!(MIGRATION.contains("No migrated test is ignored"));
}

#[test]
fn grid_algorithm_inventory_records_starlight_cpp_grid_tests() {
    assert!(MIGRATION.contains("does not import"));
    assert!(MIGRATION.contains("Lynx C++ comparison runner"));
    assert!(NATIVE.contains("Rust-only migration"));
}

#[test]
fn grid_algorithm_has_no_cxx_compatibility_debt_markers() {
    let marker = ["like_", "cxx"].concat();
    for source in [GRID_MOD, PLACEMENT, TRACKS, SIZING, ALIGNMENT] {
        assert!(!source.contains(&marker));
    }
}

#[test]
fn grid_algorithm_stays_split_into_grid_module() {
    for module in [
        "mod alignment;",
        "mod placement;",
        "mod sizing;",
        "mod tracks;",
        "mod types;",
    ] {
        assert!(GRID_MOD.contains(module));
    }
}

#[test]
fn grid_inventory_surface_limits_are_grounded_in_starlight_sources() {
    for excluded in [
        "Named lines/areas",
        "subgrid",
        "fragmentation",
        "last-baseline",
    ] {
        assert!(MIGRATION.contains(excluded));
    }
    assert!(ARCHITECTURE.contains("numeric CSS Grid Level 2"));
}

#[test]
fn grid_track_inventory_records_fit_content_resolver_invariant() {
    assert!(SIZING.contains("fit_content_limit"));
    assert!(SIZING.contains("MaxTrackSizingFunction::FitContent"));
    assert!(DIRECT.contains("grid_fit_content_track_clamps_intrinsic_growth_to_argument"));
}

#[test]
fn grid_track_inventory_records_fr_resolver_invariant() {
    assert!(SIZING.contains("fn find_fr_size"));
    assert!(SIZING.contains("factor_sum.max(1.0)"));
    assert!(DIRECT.contains("indefinite_grid_spanning_fr_item_with_flex_sum_below_one"));
}

#[test]
fn grid_track_inventory_records_span_resolver_invariant() {
    assert!(SIZING.contains("sort_unstable_by_key(|&index| items[index].span(axis))"));
    assert!(DIRECT.contains("grid_intrinsic_growth_processes_shorter_spans_before_longer_spans"));
}

#[test]
fn grid_external_intrinsic_measurement_surface_is_rust_only_until_cpp_bridge_exists() {
    assert!(NATIVE.contains("No native bridge"));
    assert!(BENCH_SCENARIOS.contains("GridSource") || ARCHITECTURE.contains("GridSource"));
    let erased = ["dyn ", "GridSource"].concat();
    for source in [GRID_MOD, PLACEMENT, TRACKS, SIZING, ALIGNMENT] {
        assert!(!source.contains(&erased));
    }
}

#[test]
fn grid_inventory_fragmentation_surface_is_not_represented() {
    assert!(MIGRATION.contains("fragmentation"));
    assert!(!GRID_MOD.contains("Fragmentation"));
}

#[test]
fn grid_track_sizing_pipeline_preserves_w3c_phase_order() {
    assert_order(
        SIZING,
        &[
            "fn initialize_tracks",
            "fn resolve_intrinsic_sizes",
            "fn maximize_tracks",
            "fn expand_flexible_tracks",
            "fn stretch_auto_tracks",
        ],
    );
    for scenario in [
        "grid_item_alignment_matrix",
        "grid_content_alignment_matrix",
        "grid_auto_flow_matrix",
        "grid_minmax_intrinsic_tracks",
    ] {
        assert!(BENCH.contains(scenario));
    }
}
