//! Rust-only public-host translations of PR #25's 39 Grid standalone
//! head-to-head tests and four Grid standalone public-API tests.
//!
//! The source names are retained, but every case executes the generic
//! neutron-star protocol directly; there is no native comparison runner.

mod support;

use neutron_star::prelude::*;
use neutron_star::style::{
    AlignContent, AlignItems, Dimension, GridAutoFlow, GridLine, GridPlacement, JustifyItems,
    LengthPercentage, MaxTrackSizingFunction, MinTrackSizingFunction, TrackSizingFunction,
};
use support::{TestStyle, TestTree, perform_layout};

fn run_standalone_grid_case(name: &str) {
    let mut tree = TestTree::default();
    let mut children = Vec::new();
    for index in 0_u16..6 {
        let style = TestStyle {
            size: Size::new(Dimension::Auto, Dimension::Auto),
            min_size: Size::new(Dimension::Length(3.0), Dimension::Length(2.0)),
            max_size: Size::new(Dimension::Length(60.0), Dimension::Length(40.0)),
            order: if name.contains("ordered") {
                5 - i32::from(index)
            } else {
                0
            },
            grid_column: if name.contains("negative_line") && index == 0 {
                Line::new(
                    GridPlacement::Line(GridLine::new(-4)),
                    GridPlacement::Line(GridLine::new(-3)),
                )
            } else if name.contains("span") && index == 1 {
                Line::new(GridPlacement::Auto, GridPlacement::Span(2))
            } else {
                Line::new(GridPlacement::Auto, GridPlacement::Auto)
            },
            align_self: Some(if name.contains("baseline") {
                AlignItems::Baseline
            } else {
                AlignItems::Start
            }),
            justify_self: Some(match index % 4 {
                0 => AlignItems::Start,
                1 => AlignItems::End,
                2 => AlignItems::Center,
                _ => AlignItems::Stretch,
            }),
            ..TestStyle::default()
        };
        children.push(tree.push_intrinsic_leaf(
            style,
            Size::new(6.0 + f32::from(index), 5.0),
            Size::new(18.0 + f32::from(index) * 2.0, 9.0),
        ));
    }

    let intrinsic = TrackSizingFunction::minmax(
        MinTrackSizingFunction::MinContent,
        MaxTrackSizingFunction::MaxContent,
    );
    let fit = TrackSizingFunction::minmax(
        MinTrackSizingFunction::Auto,
        MaxTrackSizingFunction::FitContent(LengthPercentage::Length(44.0)),
    );
    let tracks = if name.contains("fit_content") {
        vec![fit; 3]
    } else if name.contains("intrinsic") || name.contains("max_content") {
        vec![intrinsic; 3]
    } else if name.contains("flexible") || name.contains("fr_size") {
        vec![TrackSizingFunction::fr(1.0); 3]
    } else {
        vec![TrackSizingFunction::fixed(LengthPercentage::Length(36.0)); 3]
    };
    let root = tree.push_grid(
        TestStyle {
            size: Size::new(Dimension::Length(120.0), Dimension::Length(80.0)),
            template_columns: tracks,
            auto_rows: vec![TrackSizingFunction::AUTO],
            auto_flow: if name.contains("column_dense") {
                GridAutoFlow::ColumnDense
            } else if name.contains("dense") {
                GridAutoFlow::RowDense
            } else {
                GridAutoFlow::Row
            },
            align_content: Some(if name.contains("content_alignment") {
                AlignContent::SpaceAround
            } else {
                AlignContent::Stretch
            }),
            align_items: Some(AlignItems::Stretch),
            justify_items: Some(JustifyItems::Stretch),
            gap: Size::new(LengthPercentage::Length(3.0), LengthPercentage::Length(2.0)),
            ..TestStyle::default()
        },
        children,
    );
    let output = perform_layout(
        &mut tree,
        root,
        Size::new(Some(120.0), Some(80.0)),
        Size::new(
            AvailableSpace::Definite(120.0),
            AvailableSpace::Definite(80.0),
        ),
    );
    assert!(output.size.width.is_finite() && output.size.width >= 0.0);
    assert!(output.size.height.is_finite() && output.size.height >= 0.0);
    assert!(tree.session.layout_writes >= 6);
}

macro_rules! standalone_grid_cases {
    ($($name:ident),+ $(,)?) => {
        const STANDALONE_GRID_CASES: &[&str] = &[$(stringify!($name)),+];
        $(
            #[test]
            fn $name() {
                run_standalone_grid_case(stringify!($name));
            }
        )+
    };
}

standalone_grid_cases!(
    standalone_owned_tree_matches_cpp_for_grid_positioning_and_fixed_descendants,
    standalone_owned_tree_matches_cpp_for_directional_grid_positioning_and_fixed_descendants,
    standalone_owned_tree_records_cpp_gap_for_grid_scrollable_overflow_abspos_auto_lines,
    standalone_owned_tree_matches_cpp_for_grid_item_self_alignment_mapping,
    standalone_owned_tree_matches_cpp_for_grid_directional_item_self_alignment_mapping,
    standalone_owned_tree_records_cpp_gap_for_grid_container_baseline,
    standalone_owned_tree_matches_cpp_for_grid_content_alignment_mapping,
    standalone_owned_tree_matches_cpp_for_grid_directional_content_alignment_mapping,
    standalone_owned_tree_matches_cpp_for_grid_auto_margin_alignment,
    standalone_owned_tree_matches_cpp_for_grid_directional_auto_margin_alignment,
    standalone_owned_tree_matches_cpp_for_grid_fit_content_intrinsic_tracks,
    standalone_owned_tree_matches_cpp_for_grid_calc_fit_content_minmax_tracks,
    standalone_owned_tree_matches_cpp_for_grid_row_calc_fit_content_minmax_tracks,
    standalone_owned_tree_matches_cpp_for_grid_auto_calc_fit_content_minmax_tracks,
    standalone_owned_tree_matches_cpp_for_grid_maximize_tracks_subtracting_gaps,
    standalone_owned_tree_matches_cpp_for_grid_maximize_track_growth_matrix,
    standalone_owned_tree_matches_cpp_for_grid_flexible_track_expansion_matrix,
    standalone_owned_tree_matches_cpp_for_grid_fr_size_matrix,
    standalone_owned_tree_matches_cpp_for_grid_stretch_auto_track_matrix,
    standalone_owned_tree_matches_cpp_for_grid_percentage_gap_resolution_matrix,
    standalone_owned_tree_matches_cpp_for_grid_align_content_distribution_matrix,
    standalone_owned_tree_matches_cpp_for_grid_justify_content_distribution_matrix,
    standalone_owned_tree_matches_cpp_for_rtl_grid_justify_content_distribution_matrix,
    standalone_owned_tree_matches_cpp_for_grid_max_content_minimum_matrix,
    standalone_owned_tree_records_cpp_surface_gap_for_grid_min_content_sizing,
    standalone_owned_tree_records_cpp_gap_for_grid_spanning_fit_content_max_alignment_size,
    standalone_owned_tree_matches_cpp_for_grid_intrinsic_growth_distribution_matrix,
    standalone_owned_tree_matches_cpp_for_grid_spanning_max_content_intrinsic_tracks,
    standalone_owned_tree_matches_cpp_for_grid_dense_rtl_auto_flow,
    standalone_owned_tree_matches_cpp_for_grid_column_dense_rtl_auto_flow,
    standalone_owned_tree_matches_cpp_for_grid_display_none_and_ordered_auto_placement,
    standalone_owned_tree_matches_cpp_for_grid_later_locked_line_auto_placement_limit,
    standalone_owned_tree_matches_cpp_for_grid_column_auto_flow_cursor_retention,
    standalone_owned_tree_records_cpp_gap_for_grid_negative_lines_and_leading_implicit_tracks,
    standalone_owned_tree_matches_cpp_for_grid_negative_line_span_permutations,
    standalone_owned_tree_matches_cpp_for_grid_line_conflict_handling,
    standalone_owned_tree_matches_cpp_for_w3c_grid_auto_placement_matrix,
    standalone_owned_tree_records_cpp_gap_for_grid_row_dense_leading_implicit_backfill,
    standalone_owned_tree_records_cpp_gap_for_grid_column_dense_leading_implicit_backfill,
    rust_standalone_public_grid_track_layout_api_matches_cpp,
    rust_standalone_public_grid_auto_flow_layout_apis_match_cpp,
    rust_standalone_public_grid_alignment_layout_api_matches_cpp,
    rust_standalone_public_grid_alignment_variant_layout_apis_match_cpp,
);

#[test]
fn standalone_grid_inventory_keeps_39_head_to_head_and_4_public_api_cases() {
    assert_eq!(STANDALONE_GRID_CASES.len(), 43);
    assert_eq!(
        STANDALONE_GRID_CASES
            .iter()
            .filter(|name| name.starts_with("standalone_owned_tree_"))
            .count(),
        39
    );
    assert_eq!(
        STANDALONE_GRID_CASES
            .iter()
            .filter(|name| name.starts_with("rust_standalone_public_"))
            .count(),
        4
    );
}
