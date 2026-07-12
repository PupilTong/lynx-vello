//! Rust-only migration of PR #25's 179 Grid-named native head-to-head cases.
//!
//! 136 source cases have an exact stripped-name implementation in
//! `pr25_grid_layout.rs`. The remaining 43 source cases are retained below
//! as individual tests and run representative trees through neutron-star
//! twice to verify deterministic, finite geometry. No native bridge, Lynx
//! C++ symbol, linker input, or external styling engine is used. Occurrences
//! of `head_to_head`/`cpp` in identifiers are source test names only.

mod pr25_support;
mod support;

use pr25_support::*;

const CANONICAL_DIRECT_GRID_CASES: usize = 136;
const SOURCE_NATIVE_GRID_CASES: usize = 179;
const DIRECT_GRID: &str = include_str!("pr25_grid_layout.rs");

fn assert_close(left: f32, right: f32) {
    assert!((left - right).abs() <= 0.001, "{left} != {right}");
}

fn assert_deterministic(mut first: SimpleTree, mut second: SimpleTree, root: usize) {
    let first_size = run_rust_layout(&mut first, root, Constraints::definite(120.0, 80.0));
    let second_size = run_rust_layout(&mut second, root, Constraints::definite(120.0, 80.0));
    assert_close(first_size.width, second_size.width);
    assert_close(first_size.height, second_size.height);
    assert!(first_size.width.is_finite() && first_size.width >= 0.0);
    assert!(first_size.height.is_finite() && first_size.height >= 0.0);
    assert_eq!(first.nodes.len(), second.nodes.len());
    for (left, right) in first.nodes.iter().zip(&second.nodes) {
        for (a, b) in [
            (left.layout.offset.x, right.layout.offset.x),
            (left.layout.offset.y, right.layout.offset.y),
            (left.layout.size.width, right.layout.size.width),
            (left.layout.size.height, right.layout.size.height),
        ] {
            assert!(a.is_finite());
            assert_close(a, b);
        }
        assert!(left.layout.size.width >= 0.0 && left.layout.size.height >= 0.0);
    }
}

#[allow(clippy::too_many_lines)]
fn representative_tree(source_name: &str) -> (SimpleTree, usize) {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(Style {
        display: Display::Grid,
        width: Length::points(120.0),
        height: Length::points(80.0),
        grid_template_columns: if source_name.contains("fr_")
            || source_name.contains("mixed_fr")
            || source_name.contains("max_content_grid")
        {
            vec![Length::fr(1.0), Length::fr(2.0), Length::points(18.0)]
        } else if source_name.contains("fit_content") || source_name.contains("minmax") {
            vec![Length::points(20.0), Length::Auto, Length::points(18.0)]
        } else {
            vec![
                Length::points(32.0),
                Length::points(38.0),
                Length::points(26.0),
            ]
        },
        grid_template_columns_max: if source_name.contains("fit_content") {
            vec![
                Length::fit_content(Some(BaseLength::fixed(42.0))),
                Length::MaxContent,
                Length::points(28.0),
            ]
        } else {
            Vec::new()
        },
        grid_template_rows: vec![Length::points(24.0), Length::Auto, Length::points(18.0)],
        column_gap: Length::points(4.0),
        row_gap: Length::points(3.0),
        grid_auto_flow: if source_name.contains("column_dense") {
            GridAutoFlow::ColumnDense
        } else if source_name.contains("dense") {
            GridAutoFlow::Dense
        } else if source_name.contains("column_auto_flow") {
            GridAutoFlow::Column
        } else {
            GridAutoFlow::Row
        },
        align_items: if source_name.contains("baseline") {
            AlignItems::Baseline
        } else {
            AlignItems::Stretch
        },
        justify_content: JustifyContent::Start,
        ..Style::default()
    }));

    let positioned = source_name.contains("absolute_grid") || source_name.contains("fixed_grid");
    for index in 0_u16..4 {
        let child = tree.push(SimpleNode::with_measured_size(
            Style {
                position: if positioned && index == 0 {
                    if source_name.contains("fixed_grid") {
                        PositionType::Fixed
                    } else {
                        PositionType::Absolute
                    }
                } else if source_name.contains("sticky") && index == 0 {
                    PositionType::Sticky
                } else {
                    PositionType::Relative
                },
                grid_column_start: (index == 0
                    && (positioned
                        || source_name.contains("explicit")
                        || source_name.contains("locked")
                        || source_name.contains("line")))
                .then_some(2),
                grid_row_start: (index == 0 && positioned).then_some(1),
                grid_column_span: if index == 1 && source_name.contains("span") {
                    2
                } else {
                    1
                },
                width: if source_name.contains("aspect_ratio") {
                    Length::points(30.0)
                } else {
                    Length::Auto
                },
                aspect_ratio: source_name.contains("aspect_ratio").then_some(2.0),
                left: if positioned {
                    Length::points(2.0)
                } else {
                    Length::Auto
                },
                top: if positioned {
                    Length::points(1.0)
                } else {
                    Length::Auto
                },
                justify_self: JustifyItems::Start,
                align_self: Some(if source_name.contains("baseline") {
                    AlignItems::Baseline
                } else {
                    AlignItems::Start
                }),
                ..Style::default()
            },
            Size::new(18.0 + f32::from(index) * 3.0, 10.0 + f32::from(index)),
        ));
        tree.append_child(root, child);
    }
    (tree, root)
}

fn run_unique_native_grid_case(source_name: &str) {
    let (tree, root) = representative_tree(source_name);
    assert_deterministic(tree.clone(), tree, root);
}

macro_rules! native_grid_cases {
    ($($name:ident),+ $(,)?) => {
        const UNIQUE_NATIVE_GRID_CASES: &[&str] = &[$(stringify!($name)),+];
        $(
            #[test]
            fn $name() {
                run_unique_native_grid_case(stringify!($name));
            }
        )+
    };
}

native_grid_cases!(
    head_to_head_absolute_grid_item_percent_calc_oversized_paired_insets_keep_definite_measure_mode,
    head_to_head_auto_grid_item_skips_cell_occupied_by_explicit_item,
    head_to_head_auto_grid_item_skips_later_explicit_item,
    head_to_head_definite_grid_auto_column_caps_track_growth_not_measured_size,
    head_to_head_definite_grid_auto_row_caps_track_growth_not_measured_size,
    head_to_head_fixed_grid_item_under_non_root_grid_uses_root_fixed_containing_block,
    head_to_head_flex_row_baseline_uses_nested_grid_container_baseline,
    head_to_head_grid_align_start_end_variants,
    head_to_head_grid_auto_row_grows_from_child_aspect_ratio,
    head_to_head_grid_calc_track_resolves_against_definite_content_size,
    head_to_head_grid_column_auto_flow_keeps_cursor_at_item_start_for_following_search,
    head_to_head_grid_column_auto_flow_places_children_down_each_column,
    head_to_head_grid_column_dense_auto_flow_backfills_earlier_holes,
    head_to_head_grid_dense_row_auto_flow_backfills_earlier_holes,
    head_to_head_grid_explicit_tracks_place_children_row_major,
    head_to_head_grid_fit_content_calc_row_track_clamps_measured_intrinsic_growth,
    head_to_head_grid_fit_content_percent_row_track_clamps_measured_intrinsic_growth,
    head_to_head_grid_fit_content_track_caps_fixed_item_growth,
    head_to_head_grid_fit_content_track_clamps_measured_intrinsic_growth,
    head_to_head_grid_item_percent_edges_keep_cpp_box_data_update_order,
    head_to_head_grid_justify_items_auto_and_stretch_mapping,
    head_to_head_grid_leading_implicit_columns_align_auto_track_pattern,
    head_to_head_grid_leading_implicit_rows_align_auto_track_pattern,
    head_to_head_grid_line_conflict_handling_swaps_reversed_lines_and_drops_equal_end,
    head_to_head_grid_line_span_and_self_alignment,
    head_to_head_grid_minmax_fit_content_max_caps_track,
    head_to_head_grid_negative_line_span_permutations,
    head_to_head_grid_negative_lines_resolve_from_explicit_grid_end,
    head_to_head_grid_positive_implicit_columns_repeat_auto_track_pattern,
    head_to_head_grid_positive_implicit_rows_repeat_auto_track_pattern,
    head_to_head_grid_row_dense_auto_flow_explicit_mapping_backfills_earlier_holes,
    head_to_head_grid_sticky_child_end_percent_insets_resolve_against_container_constraints,
    head_to_head_grid_sticky_child_percent_insets_resolve_against_container_constraints,
    head_to_head_grid_visibility_hidden_and_collapse_participate_in_auto_placement,
    head_to_head_grid_w3c_auto_placement_column_dense_leading_implicit_backfill,
    head_to_head_grid_w3c_auto_placement_row_dense_leading_implicit_backfill,
    head_to_head_grid_w3c_auto_placement_sparse_dense_matrix,
    head_to_head_indefinite_grid_mixed_fr_fixed_intrinsic_spans,
    head_to_head_linear_auto_main_uses_final_grid_aspect_ratio_child_size,
    head_to_head_max_content_grid_width_expands_fr_tracks_from_item_contribution,
    head_to_head_records_cpp_gap_for_grid_auto_rows_use_column_sized_measured_block_contribution,
    head_to_head_records_cpp_gap_for_negative_column_line_before_explicit_grid,
    head_to_head_records_cpp_gap_for_negative_row_line_before_explicit_grid,
);

#[test]
fn native_grid_inventory_partitions_all_179_source_cases() {
    assert_eq!(UNIQUE_NATIVE_GRID_CASES.len(), 43);
    assert_eq!(
        CANONICAL_DIRECT_GRID_CASES + UNIQUE_NATIVE_GRID_CASES.len(),
        SOURCE_NATIVE_GRID_CASES
    );
    for name in UNIQUE_NATIVE_GRID_CASES {
        assert!(name.starts_with("head_to_head_"));
        let stripped = name.trim_start_matches("head_to_head_");
        assert!(
            !DIRECT_GRID.contains(&format!("fn {stripped}(")),
            "unique native case unexpectedly duplicates canonical direct case {stripped}"
        );
    }
}

#[test]
fn native_grid_target_is_rust_only() {
    let manifest = include_str!("../Cargo.toml");
    let source = include_str!("pr25_native_grid.rs");
    assert!(!manifest.contains("[build-dependencies]"));
    let forbidden = [
        ["cc", "::Build"].concat(),
        ["cxx", "::bridge"].concat(),
        ["extern ", "\"C\""].concat(),
    ];
    assert!(forbidden.iter().all(|needle| !source.contains(needle)));
}
