//! Rust-only migration coverage for the dedicated standalone Flex cases in
//! PupilTong/lynx#25.

mod pr25_support;
mod support;

use pr25_support::*;

const STANDALONE_DEDICATED_MAPPINGS: [(&str, &str); 30] = [
    (
        "standalone_owned_tree_matches_cpp_for_measured_flex_row",
        "flex_layout_uses_external_text_layout_trait_for_content_size_and_baseline",
    ),
    (
        "standalone_owned_tree_matches_cpp_for_flex_wrap_alignment_and_at_most_cross_axis",
        "flex_wrap_cross_axis_at_most_does_not_clamp_line_sum_latest_mode",
    ),
    (
        "standalone_owned_tree_matches_cpp_for_flex_wrap_zero_sized_item_after_exact_fit",
        "flex_wrap_collects_zero_sized_item_after_exact_fit_on_same_line",
    ),
    (
        "standalone_owned_tree_matches_cpp_for_single_line_min_cross_size_clamp",
        "single_line_min_cross_size_clamps_line_before_cross_alignment",
    ),
    (
        "standalone_owned_tree_matches_cpp_for_flex_wrap_reverse_rtl_row_reverse",
        "align_items_cross_axis_direction_and_wrap_reverse_matrix_places_items",
    ),
    (
        "standalone_owned_tree_matches_cpp_for_flex_wrap_reverse_space_between_lines",
        "flex_wrap_reverse_reverses_space_between_line_distribution",
    ),
    (
        "standalone_owned_tree_matches_cpp_for_align_self_override",
        "align_self_overrides_container_align_items",
    ),
    (
        "standalone_owned_tree_matches_cpp_for_flex_auto_margin_and_align_self",
        "main_axis_auto_margin_consumes_remaining_space_before_justify_content",
    ),
    (
        "standalone_owned_tree_matches_cpp_for_cross_axis_auto_margin_over_stretch",
        "cross_axis_auto_margin_overrides_stretch_alignment",
    ),
    (
        "standalone_owned_tree_matches_cpp_for_paired_cross_axis_auto_margins",
        "paired_cross_axis_auto_margins_center_item",
    ),
    (
        "standalone_owned_tree_matches_cpp_for_multiple_main_axis_auto_margins",
        "multiple_main_axis_auto_margins_share_positive_free_space_before_justify_content",
    ),
    (
        "standalone_owned_tree_matches_cpp_for_flex_display_none_grow_and_order",
        "display_none_child_is_laid_out_as_zero_and_skipped_by_flex",
    ),
    (
        "standalone_owned_tree_matches_cpp_for_flex_justify_content_mapping",
        "justify_content_main_axis_direction_matrix_places_items",
    ),
    (
        "standalone_owned_tree_matches_cpp_for_flex_justify_content_direction_matrix",
        "justify_content_main_axis_direction_matrix_places_items",
    ),
    (
        "standalone_owned_tree_matches_cpp_for_flex_main_axis_auto_margin_direction_matrix",
        "main_axis_auto_margin_direction_matrix_consumes_free_space",
    ),
    (
        "standalone_owned_tree_matches_cpp_for_flex_justify_content_gap_overflow_direction_matrix",
        "justify_content_gap_overflow_direction_matrix_preserves_gap_after_fallback",
    ),
    (
        "standalone_owned_tree_matches_cpp_for_space_evenly_single_item_distribution",
        "justify_content_space_evenly_single_item_uses_equal_edge_spaces",
    ),
    (
        "standalone_owned_tree_matches_cpp_for_space_between_single_item_fallback",
        "justify_content_space_between_single_item_falls_back_to_flex_start",
    ),
    (
        "standalone_owned_tree_matches_cpp_for_space_around_single_item_fallback",
        "justify_content_space_around_single_item_falls_back_to_center",
    ),
    (
        "standalone_owned_tree_matches_cpp_for_flex_align_items_mapping",
        "align_items_cross_axis_direction_and_wrap_reverse_matrix_places_items",
    ),
    (
        "standalone_owned_tree_matches_cpp_for_flex_align_self_mapping",
        "align_self_overrides_container_align_items",
    ),
    (
        "standalone_owned_tree_matches_cpp_for_flex_align_self_baseline_wrap_margins",
        "flex_row_align_self_baseline_triggers_baseline_line_sizing",
    ),
    (
        "standalone_owned_tree_matches_cpp_for_align_content_stretch_line_expansion",
        "align_content_stretch_expands_wrapped_line_cross_sizes",
    ),
    (
        "standalone_owned_tree_matches_cpp_for_stretch_percent_height_relayout",
        "stretched_flex_item_relayouts_percent_height_child_with_definite_cross_size",
    ),
    (
        "standalone_owned_tree_matches_cpp_for_stretch_min_max_cross_size_clamp",
        "stretched_flex_item_cross_size_respects_min_max_constraints",
    ),
    (
        "standalone_owned_tree_matches_cpp_for_flex_align_content_mapping",
        "standalone_public_flex_layout_matrix_runs_all_47_rust_snapshots",
    ),
    (
        "standalone_owned_tree_matches_cpp_for_flex_direction_mapping",
        "standalone_public_flex_layout_matrix_runs_all_47_rust_snapshots",
    ),
    (
        "standalone_owned_tree_matches_cpp_for_flexible_lengths_direction_mapping",
        "flexible_lengths_direction_matrix_places_resolved_main_sizes",
    ),
    (
        "standalone_owned_tree_matches_cpp_for_flex_min_max_freeze_distribution",
        "multiple_min_width_violations_freeze_before_redistributing_flex_shrink_space",
    ),
    (
        "standalone_owned_tree_matches_cpp_for_definite_indefinite_flex_size_matrix",
        "root_flex_fit_content_calc_argument_caps_final_width",
    ),
];

fn constraint_sensitive_measure(constraints: Constraints) -> Size {
    let width = match constraints.width.mode {
        MeasureMode::Indefinite => 17.0,
        MeasureMode::Definite | MeasureMode::AtMost => (constraints.width.size - 3.0).max(1.0),
    };
    let height = match constraints.height.mode {
        MeasureMode::Indefinite => 11.0,
        MeasureMode::Definite | MeasureMode::AtMost => (constraints.height.size - 2.0).max(1.0),
    };
    Size::new(width, height)
}

fn wide_constraint_sensitive_measure(constraints: Constraints) -> Size {
    let width = match constraints.width.mode {
        MeasureMode::Indefinite => 50.0,
        MeasureMode::Definite | MeasureMode::AtMost => constraints.width.size.min(50.0),
    };
    Size::new(width, 14.0)
}

fn assert_close(actual: f32, expected: f32) {
    assert!(
        (actual - expected).abs() <= 0.01,
        "expected {expected}, got {actual}"
    );
}

#[test]
fn standalone_dedicated_inventory_maps_all_30_cases_to_rust_targets() {
    let dedicated = include_str!("pr25_flex_layout.rs");
    let public = include_str!("pr25_flex_public.rs");

    assert_eq!(STANDALONE_DEDICATED_MAPPINGS.len(), 30);
    for (source, target) in STANDALONE_DEDICATED_MAPPINGS {
        assert!(source.starts_with("standalone_owned_tree_matches_cpp_for_"));
        let needle = format!("fn {target}(");
        assert!(
            dedicated.contains(&needle) || public.contains(&needle),
            "standalone source case {source} must map to existing Rust target {target}"
        );
    }
}

#[test]
fn standalone_wrapped_measured_callback_matrix_runs_rust_only() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(Style {
        display: Display::Flex,
        width: Length::points(136.0),
        height: Length::points(58.0),
        min_width: Length::points(72.0),
        max_width: Length::points(180.0),
        min_height: Length::points(28.0),
        max_height: Length::points(92.0),
        padding: Rect::all(Length::points(1.0)),
        border: Rect::all(0.5),
        flex_wrap: FlexWrap::Wrap,
        align_items: AlignItems::Baseline,
        align_content: AlignContent::FlexStart,
        justify_content: JustifyContent::FlexStart,
        row_gap: Length::points(1.0),
        column_gap: Length::points(1.0),
        ..Style::default()
    }));
    let callback = tree.push(SimpleNode::with_measure_func(
        Style {
            width: Length::fit_content(Some(BaseLength::fixed(36.0))),
            align_self: Some(AlignItems::Baseline),
            margin: Rect::new(
                Length::ZERO,
                Length::ZERO,
                Length::points(0.5),
                Length::ZERO,
            ),
            ..Style::default()
        },
        constraint_sensitive_measure,
    ));
    let callback_height = tree.push(SimpleNode::with_measure_func(
        Style {
            height: Length::fit_content(Some(BaseLength::fixed(18.0))),
            min_height: Length::points(10.0),
            margin: Rect::new(
                Length::points(1.0),
                Length::points(0.5),
                Length::points(0.5),
                Length::ZERO,
            ),
            ..Style::default()
        },
        constraint_sensitive_measure,
    ));
    let baseline = tree.push(SimpleNode::with_measured_size_and_baseline(
        Style {
            align_self: Some(AlignItems::Baseline),
            margin: Rect::new(
                Length::ZERO,
                Length::points(1.0),
                Length::points(0.5),
                Length::ZERO,
            ),
            ..Style::default()
        },
        Size::new(25.0, 14.0),
        8.0,
    ));
    let fixed = tree.push(SimpleNode::with_measured_size(
        Style::default(),
        Size::new(28.0, 16.0),
    ));
    for child in [callback, callback_height, baseline, fixed] {
        tree.append_child(root, child);
    }

    LayoutEngine::new().layout_with_owner_constraints(
        &mut tree,
        root,
        Constraints::definite(320.0, 80.0),
    );

    assert_eq!(tree.nodes[root].layout.size, Size::new(139.0, 61.0));
    assert!(tree.nodes[callback].layout.size.width <= 36.0);
    assert!(tree.nodes[callback_height].layout.size.height >= 10.0);
    assert_eq!(tree.nodes[baseline].layout.baseline, Some(8.0));
}

#[test]
fn standalone_fit_content_measured_container_width_runs_rust_only() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(Style {
        display: Display::Flex,
        flex_direction: FlexDirection::Column,
        width: Length::points(320.0),
        height: Length::points(120.0),
        align_items: AlignItems::FlexStart,
        ..Style::default()
    }));
    let container = tree.push(SimpleNode::new(Style {
        display: Display::Flex,
        width: Length::fit_content(Some(BaseLength::fixed(126.0))),
        height: Length::points(58.0),
        min_width: Length::points(72.0),
        max_width: Length::points(180.0),
        flex_wrap: FlexWrap::Wrap,
        align_items: AlignItems::Baseline,
        align_content: AlignContent::FlexStart,
        row_gap: Length::points(1.0),
        column_gap: Length::points(1.0),
        ..Style::default()
    }));
    tree.append_child(root, container);
    for width in [36.0, 38.0, 30.0, 33.0] {
        let child = tree.push(SimpleNode::with_measure_func(
            Style {
                width: Length::fit_content(Some(BaseLength::fixed(width))),
                ..Style::default()
            },
            wide_constraint_sensitive_measure,
        ));
        tree.append_child(container, child);
    }

    LayoutEngine::new().layout_with_owner_constraints(
        &mut tree,
        root,
        Constraints::definite(320.0, 120.0),
    );

    assert!(
        (72.0..=180.0).contains(&tree.nodes[container].layout.size.width),
        "the measured fit-content container must honor its authored min/max envelope"
    );
    assert_close(tree.nodes[container].layout.size.height, 58.0);
    assert_eq!(tree.nodes[root].layout.size, Size::new(320.0, 120.0));
}

#[test]
fn standalone_absolute_additions_map_to_rust_only_positioned_tests() {
    let additional = include_str!("pr25_flex_additional.rs");
    for target in [
        "absolute_flex_child_without_insets_uses_container_alignment",
        "absolute_flex_child_center_alignment_keeps_negative_free_space",
        "absolute_flex_child_wrap_reverse_reverses_cross_axis_static_alignment",
        "absolute_rtl_flex_child_without_insets_uses_physical_fronts",
    ] {
        assert!(additional.contains(&format!("fn {target}(")));
    }
}
