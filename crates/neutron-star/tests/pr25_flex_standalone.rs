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
        "standalone_flex_auto_margin_and_align_self_preserves_both_effects",
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
        "standalone_flex_display_none_grow_and_order_preserves_the_combination",
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
        "standalone_align_items_mapping_runs_all_14_source_cases",
    ),
    (
        "standalone_owned_tree_matches_cpp_for_flex_align_self_mapping",
        "standalone_align_self_mapping_runs_all_14_source_cases",
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
        "standalone_align_content_mapping_runs_all_36_source_cases",
    ),
    (
        "standalone_owned_tree_matches_cpp_for_flex_direction_mapping",
        "standalone_direction_mapping_runs_all_8_source_cases",
    ),
    (
        "standalone_owned_tree_matches_cpp_for_flexible_lengths_direction_mapping",
        "flexible_lengths_direction_matrix_places_resolved_main_sizes",
    ),
    (
        "standalone_owned_tree_matches_cpp_for_flex_min_max_freeze_distribution",
        "standalone_min_max_freeze_inventory_maps_all_33_source_cases",
    ),
    (
        "standalone_owned_tree_matches_cpp_for_definite_indefinite_flex_size_matrix",
        "standalone_definite_indefinite_inventory_maps_all_5_source_cases",
    ),
];

/// The standalone test invokes the same 33 scenario builders as the canonical
/// Flex target. Keep the mapping one-to-one instead of treating one freeze
/// regression as representative of the whole matrix.
const STANDALONE_MIN_MAX_FREEZE_MAPPINGS: [(&str, &str); 33] = [
    (
        "flex_min_width_shrink_freeze_tree",
        "min_width_freezes_item_during_flex_shrink",
    ),
    (
        "flex_percent_min_width_shrink_freeze_tree",
        "percent_min_width_freezes_item_during_flex_shrink",
    ),
    (
        "flex_max_width_grow_redistribution_tree",
        "max_width_freezes_item_and_redistributes_flex_grow_space",
    ),
    (
        "flex_partial_grow_remaining_space_tree",
        "flex_grow_sum_below_one_leaves_remaining_space_for_justify_content",
    ),
    (
        "flex_partial_shrink_negative_space_tree",
        "flex_shrink_sum_below_one_leaves_negative_space_for_justify_content",
    ),
    (
        "flex_zero_grow_freezes_before_distribution_tree",
        "zero_flex_grow_freezes_item_before_distributing_positive_free_space",
    ),
    (
        "flex_all_zero_grow_leaves_space_for_justify_content_tree",
        "all_zero_flex_grow_items_freeze_and_leave_space_for_justify_content",
    ),
    (
        "flex_min_width_grow_violation_restarts_distribution_tree",
        "min_width_violation_freezes_item_during_flex_grow_and_restarts_distribution",
    ),
    (
        "flex_min_width_above_basis_initial_shrink_freeze_tree",
        "min_width_above_flex_basis_freezes_shrinking_item_to_hypothetical_main_size",
    ),
    (
        "flex_multiple_min_width_shrink_violations_tree",
        "multiple_min_width_violations_freeze_before_redistributing_flex_shrink_space",
    ),
    (
        "flex_max_width_below_basis_initial_grow_freeze_tree",
        "max_width_below_flex_basis_freezes_growing_item_to_hypothetical_main_size",
    ),
    (
        "flex_main_axis_gap_reduces_free_space_before_grow_tree",
        "main_axis_gap_reduces_free_space_before_flex_grow_distribution",
    ),
    (
        "flex_shrink_distribution_scaled_by_base_size_tree",
        "flex_shrink_distribution_is_scaled_by_flex_base_size",
    ),
    (
        "flex_shrink_negative_inner_size_floored_after_margins_tree",
        "flex_shrink_negative_inner_size_is_floored_after_outer_margins",
    ),
    (
        "flex_multiple_max_width_grow_violations_tree",
        "multiple_max_width_violations_freeze_before_redistributing_flex_grow_space",
    ),
    (
        "flex_percent_max_width_grow_redistribution_tree",
        "percent_max_width_freezes_item_and_redistributes_flex_grow_space",
    ),
    (
        "flex_zero_shrink_freezes_before_distribution_tree",
        "zero_flex_shrink_freezes_item_before_distributing_negative_free_space",
    ),
    (
        "flex_max_width_shrink_violation_restarts_distribution_tree",
        "max_width_violation_freezes_item_during_flex_shrink_and_restarts_distribution",
    ),
    (
        "flex_fit_content_max_width_grow_redistribution_tree",
        "fit_content_max_width_freezes_item_and_redistributes_flex_grow_space",
    ),
    (
        "flex_fit_content_max_width_without_argument_tree",
        "fit_content_max_width_without_argument_does_not_cap_flex_grow_space",
    ),
    (
        "flex_column_percent_min_height_shrink_freeze_tree",
        "column_percent_min_height_freezes_item_during_flex_shrink",
    ),
    (
        "flex_column_fit_content_min_height_shrink_freeze_tree",
        "column_fit_content_min_height_freezes_item_during_flex_shrink",
    ),
    (
        "flex_column_fit_content_min_height_without_argument_tree",
        "column_fit_content_min_height_without_argument_does_not_freeze_item",
    ),
    (
        "flex_column_percent_max_height_grow_redistribution_tree",
        "column_percent_max_height_freezes_item_and_redistributes_flex_grow_space",
    ),
    (
        "flex_column_fit_content_max_height_grow_redistribution_tree",
        "column_fit_content_max_height_freezes_item_and_redistributes_flex_grow_space",
    ),
    (
        "flex_column_max_content_max_height_grow_no_cap_tree",
        "column_max_content_max_height_does_not_cap_flex_grow_space",
    ),
    (
        "flex_row_reverse_grow_freeze_tree",
        "row_reverse_flex_grow_freeze_places_flexed_items_from_right_edge",
    ),
    (
        "flex_column_reverse_shrink_freeze_tree",
        "column_reverse_flex_shrink_freeze_places_flexed_items_from_bottom_edge",
    ),
    (
        "flex_wrapped_lines_resolve_flexible_lengths_independently_tree",
        "flexible_lengths_resolve_independently_per_wrapped_line",
    ),
    (
        "flex_measured_basis_grow_max_width_violation_tree",
        "measured_flex_basis_grow_max_width_violation_restarts_distribution",
    ),
    (
        "flex_measured_basis_shrink_min_width_violation_tree",
        "measured_flex_basis_shrink_min_width_violation_restarts_distribution",
    ),
    (
        "flex_nested_intrinsic_basis_grow_max_width_violation_tree",
        "nested_intrinsic_flex_basis_grow_max_width_violation_restarts_distribution",
    ),
    (
        "flex_nested_intrinsic_basis_shrink_min_width_violation_tree",
        "nested_intrinsic_flex_basis_shrink_min_width_violation_restarts_distribution",
    ),
];

/// Exact `case_name` strings passed by the source aggregate test, including
/// the label returned by its standalone column/aspect-ratio builder.
const STANDALONE_DEFINITE_INDEFINITE_MAPPINGS: [(&str, &str); 5] = [
    (
        "column flex item percent cross size and aspect ratio define main basis",
        "column_flex_item_percent_cross_size_and_aspect_ratio_define_main_basis",
    ),
    (
        "root flex fit-content percent argument caps final width",
        "root_flex_fit_content_percent_argument_caps_final_width",
    ),
    (
        "root flex fit-content calc argument caps final width",
        "root_flex_fit_content_calc_argument_caps_final_width",
    ),
    (
        "root column flex fit-content percent argument caps final height",
        "root_column_flex_fit_content_percent_argument_caps_final_height",
    ),
    (
        "root column flex fit-content calc argument caps final height",
        "root_column_flex_fit_content_calc_argument_caps_final_height",
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
    let standalone = include_str!("pr25_flex_standalone.rs");

    assert_eq!(STANDALONE_DEDICATED_MAPPINGS.len(), 30);
    for (source, target) in STANDALONE_DEDICATED_MAPPINGS {
        assert!(source.starts_with("standalone_owned_tree_matches_cpp_for_"));
        let needle = format!("fn {target}(");
        assert!(
            dedicated.contains(&needle) || public.contains(&needle) || standalone.contains(&needle),
            "standalone source case {source} must map to existing Rust target {target}"
        );
    }
}

#[test]
fn standalone_min_max_freeze_inventory_maps_all_33_source_cases() {
    let canonical = include_str!("pr25_flex_layout.rs");

    assert_eq!(STANDALONE_MIN_MAX_FREEZE_MAPPINGS.len(), 33);
    for (source_builder, target) in STANDALONE_MIN_MAX_FREEZE_MAPPINGS {
        assert!(source_builder.ends_with("_tree"));
        assert!(
            canonical.contains(&format!("fn {target}(")),
            "standalone source builder {source_builder} must map to the exact canonical Rust test {target}"
        );
    }
}

#[test]
fn standalone_definite_indefinite_inventory_maps_all_5_source_cases() {
    let canonical = include_str!("pr25_flex_layout.rs");

    assert_eq!(STANDALONE_DEFINITE_INDEFINITE_MAPPINGS.len(), 5);
    for (source_case, target) in STANDALONE_DEFINITE_INDEFINITE_MAPPINGS {
        assert!(
            canonical.contains(&format!("fn {target}(")),
            "standalone definite/indefinite source case {source_case} must map to the exact canonical Rust test {target}"
        );
    }
}

#[test]
fn standalone_flex_auto_margin_and_align_self_preserves_both_effects() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(Style {
        display: Display::Flex,
        width: Length::points(100.0),
        height: Length::points(30.0),
        align_items: AlignItems::FlexStart,
        ..Style::default()
    }));
    let auto_margin = tree.push(SimpleNode::new(Style {
        display: Display::Flex,
        flex_basis: Length::points(20.0),
        height: Length::points(10.0),
        margin: Rect::new(Length::Auto, Length::ZERO, Length::ZERO, Length::ZERO),
        align_self: Some(AlignItems::Center),
        ..Style::default()
    }));
    let start_aligned = tree.push(SimpleNode::new(Style {
        display: Display::Flex,
        flex_basis: Length::points(10.0),
        height: Length::points(10.0),
        ..Style::default()
    }));
    tree.append_child(root, auto_margin);
    tree.append_child(root, start_aligned);

    run_rust_layout(&mut tree, root, Constraints::definite(100.0, 30.0));

    // The single main-start auto margin absorbs all 70px of free space,
    // while align-self:center independently centers the same item in the
    // 30px cross axis.
    assert_close(tree.nodes[auto_margin].layout.offset.x, 70.0);
    assert_close(tree.nodes[auto_margin].layout.offset.y, 10.0);
    assert_close(tree.nodes[start_aligned].layout.offset.x, 90.0);
    assert_close(tree.nodes[start_aligned].layout.offset.y, 0.0);
}

#[test]
fn standalone_flex_display_none_grow_and_order_preserves_the_combination() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(Style {
        display: Display::Flex,
        width: Length::points(120.0),
        height: Length::points(20.0),
        align_items: AlignItems::FlexStart,
        ..Style::default()
    }));
    let later = tree.push(SimpleNode::new(Style {
        display: Display::Flex,
        flex_basis: Length::points(20.0),
        flex_grow: 1.0,
        height: Length::points(10.0),
        order: 2,
        ..Style::default()
    }));
    let hidden = tree.push(SimpleNode::new(Style {
        display: Display::None,
        flex_basis: Length::points(50.0),
        flex_grow: 10.0,
        height: Length::points(10.0),
        ..Style::default()
    }));
    let earlier = tree.push(SimpleNode::new(Style {
        display: Display::Flex,
        flex_basis: Length::points(20.0),
        flex_grow: 3.0,
        height: Length::points(10.0),
        order: 1,
        ..Style::default()
    }));
    for child in [later, hidden, earlier] {
        tree.append_child(root, child);
    }

    run_rust_layout(&mut tree, root, Constraints::definite(120.0, 20.0));

    // The hidden item's large factor is excluded. The two visible items are
    // sorted by `order` and divide 80px of free space in a 3:1 ratio.
    assert_eq!(tree.nodes[hidden].layout.size, Size::ZERO);
    assert_close(tree.nodes[earlier].layout.offset.x, 0.0);
    assert_close(tree.nodes[earlier].layout.size.width, 80.0);
    assert_close(tree.nodes[later].layout.offset.x, 80.0);
    assert_close(tree.nodes[later].layout.size.width, 40.0);
}

const STANDALONE_ALIGN_ITEMS_VALUES: [AlignItems; 7] = [
    AlignItems::Stretch,
    AlignItems::FlexStart,
    AlignItems::Start,
    AlignItems::Center,
    AlignItems::FlexEnd,
    AlignItems::End,
    AlignItems::Baseline,
];

const STANDALONE_ALIGN_CONTENT_VALUES: [AlignContent; 9] = [
    AlignContent::FlexStart,
    AlignContent::Start,
    AlignContent::Center,
    AlignContent::FlexEnd,
    AlignContent::End,
    AlignContent::SpaceBetween,
    AlignContent::SpaceAround,
    AlignContent::SpaceEvenly,
    AlignContent::Stretch,
];

fn standalone_expected_line_geometry(
    align_content: AlignContent,
    wrap: FlexWrap,
    container_cross_size: f32,
    natural_line_sizes: [f32; 2],
    gap: f32,
) -> ([f32; 2], [f32; 2]) {
    let reversed = wrap == FlexWrap::WrapReverse;
    let natural_total = natural_line_sizes[0] + gap + natural_line_sizes[1];
    let free = container_cross_size - natural_total;
    let mut line_sizes = natural_line_sizes;

    // Work in distance from the flex cross-start. `start`/`end` instead
    // target the writing-mode edge, so wrap-reverse swaps which packing
    // offset corresponds to that physical edge.
    let logical_offsets = match align_content {
        AlignContent::FlexStart => [0.0, natural_line_sizes[0] + gap],
        AlignContent::FlexEnd => [free, free + natural_line_sizes[0] + gap],
        AlignContent::Start => {
            let offset = if reversed { free } else { 0.0 };
            [offset, offset + natural_line_sizes[0] + gap]
        }
        AlignContent::End => {
            let offset = if reversed { 0.0 } else { free };
            [offset, offset + natural_line_sizes[0] + gap]
        }
        AlignContent::Center => [free / 2.0, free / 2.0 + natural_line_sizes[0] + gap],
        AlignContent::SpaceBetween => [0.0, natural_line_sizes[0] + gap + free],
        AlignContent::SpaceAround => [
            free / 4.0,
            free / 4.0 + natural_line_sizes[0] + gap + free / 2.0,
        ],
        AlignContent::SpaceEvenly => [
            free / 3.0,
            free / 3.0 + natural_line_sizes[0] + gap + free / 3.0,
        ],
        AlignContent::Stretch => {
            line_sizes[0] += free / 2.0;
            line_sizes[1] += free / 2.0;
            [0.0, line_sizes[0] + gap]
        }
    };

    let offsets = if reversed {
        [
            container_cross_size - logical_offsets[0] - line_sizes[0],
            container_cross_size - logical_offsets[1] - line_sizes[1],
        ]
    } else {
        logical_offsets
    };
    (offsets, line_sizes)
}

fn standalone_alignment_mapping_tree(
    flex_direction: FlexDirection,
    align_items: AlignItems,
    middle_align_self: Option<AlignItems>,
) -> (SimpleTree, [usize; 3]) {
    let is_row = flex_direction.is_row();
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(Style {
        display: Display::Flex,
        flex_direction,
        width: Length::points(120.0),
        height: Length::points(80.0),
        align_items,
        ..Style::default()
    }));
    let mut children = [0; 3];
    for (index, (main, cross)) in [(18.0, 8.0), (24.0, 12.0), (30.0, 16.0)]
        .into_iter()
        .enumerate()
    {
        let auto_cross = index == 1;
        let child = tree.push(SimpleNode::new(Style {
            display: Display::Flex,
            width: if is_row {
                Length::points(main)
            } else if auto_cross {
                Length::Auto
            } else {
                Length::points(cross)
            },
            height: if is_row {
                if auto_cross {
                    Length::Auto
                } else {
                    Length::points(cross)
                }
            } else {
                Length::points(main)
            },
            flex_basis: Length::points(main),
            align_self: (index == 1).then_some(middle_align_self).flatten(),
            ..Style::default()
        }));
        children[index] = child;
        tree.append_child(root, child);
    }
    run_rust_layout(&mut tree, root, Constraints::definite(120.0, 80.0));
    (tree, children)
}

#[test]
fn standalone_align_items_mapping_runs_all_14_source_cases() {
    let mut cases = 0;
    for flex_direction in [FlexDirection::Row, FlexDirection::Column] {
        for align_items in STANDALONE_ALIGN_ITEMS_VALUES {
            let (tree, [first, middle, third]) =
                standalone_alignment_mapping_tree(flex_direction, align_items, None);
            if flex_direction.is_row() {
                assert_close(tree.nodes[first].layout.offset.x, 0.0);
                assert_close(tree.nodes[middle].layout.offset.x, 18.0);
                assert_close(tree.nodes[third].layout.offset.x, 42.0);
                let expected = match align_items {
                    AlignItems::Stretch | AlignItems::FlexStart | AlignItems::Start => {
                        [0.0, 0.0, 0.0]
                    }
                    AlignItems::Center => [36.0, 40.0, 32.0],
                    AlignItems::FlexEnd | AlignItems::End => [72.0, 80.0, 64.0],
                    AlignItems::Baseline => [8.0, 16.0, 0.0],
                };
                for (child, expected_y) in [first, middle, third].into_iter().zip(expected) {
                    assert_close(tree.nodes[child].layout.offset.y, expected_y);
                }
                assert_close(
                    tree.nodes[middle].layout.size.height,
                    if align_items == AlignItems::Stretch {
                        80.0
                    } else {
                        0.0
                    },
                );
            } else {
                assert_close(tree.nodes[first].layout.offset.y, 0.0);
                assert_close(tree.nodes[middle].layout.offset.y, 18.0);
                assert_close(tree.nodes[third].layout.offset.y, 42.0);
                let expected = match align_items {
                    AlignItems::Stretch
                    | AlignItems::FlexStart
                    | AlignItems::Start
                    | AlignItems::Baseline => [0.0, 0.0, 0.0],
                    AlignItems::Center => [56.0, 60.0, 52.0],
                    AlignItems::FlexEnd | AlignItems::End => [112.0, 120.0, 104.0],
                };
                for (child, expected_x) in [first, middle, third].into_iter().zip(expected) {
                    assert_close(tree.nodes[child].layout.offset.x, expected_x);
                }
                assert_close(
                    tree.nodes[middle].layout.size.width,
                    if align_items == AlignItems::Stretch {
                        120.0
                    } else {
                        0.0
                    },
                );
            }
            cases += 1;
        }
    }
    assert_eq!(cases, 14);
}

#[test]
fn standalone_align_self_mapping_runs_all_14_source_cases() {
    let mut cases = 0;
    for flex_direction in [FlexDirection::Row, FlexDirection::Column] {
        for align_self in STANDALONE_ALIGN_ITEMS_VALUES {
            let (tree, [first, middle, third]) = standalone_alignment_mapping_tree(
                flex_direction,
                AlignItems::FlexStart,
                Some(align_self),
            );
            if flex_direction.is_row() {
                assert_close(tree.nodes[first].layout.offset.y, 0.0);
                assert_close(tree.nodes[third].layout.offset.y, 0.0);
                let expected_y = match align_self {
                    AlignItems::Stretch
                    | AlignItems::FlexStart
                    | AlignItems::Start
                    | AlignItems::Baseline => 0.0,
                    AlignItems::Center => 40.0,
                    AlignItems::FlexEnd | AlignItems::End => 80.0,
                };
                assert_close(tree.nodes[middle].layout.offset.y, expected_y);
                assert_close(
                    tree.nodes[middle].layout.size.height,
                    if align_self == AlignItems::Stretch {
                        80.0
                    } else {
                        0.0
                    },
                );
            } else {
                assert_close(tree.nodes[first].layout.offset.x, 0.0);
                assert_close(tree.nodes[third].layout.offset.x, 0.0);
                let expected_x = match align_self {
                    AlignItems::Stretch
                    | AlignItems::FlexStart
                    | AlignItems::Start
                    | AlignItems::Baseline => 0.0,
                    AlignItems::Center => 60.0,
                    AlignItems::FlexEnd | AlignItems::End => 120.0,
                };
                assert_close(tree.nodes[middle].layout.offset.x, expected_x);
                assert_close(
                    tree.nodes[middle].layout.size.width,
                    if align_self == AlignItems::Stretch {
                        120.0
                    } else {
                        0.0
                    },
                );
            }
            cases += 1;
        }
    }
    assert_eq!(cases, 14);
}

#[test]
#[allow(clippy::too_many_lines)] // Keep the 36 source-shaped cases and spec expectations together.
fn standalone_align_content_mapping_runs_all_36_source_cases() {
    let mut cases = 0;
    for flex_direction in [FlexDirection::Row, FlexDirection::Column] {
        for flex_wrap in [FlexWrap::Wrap, FlexWrap::WrapReverse] {
            for align_content in STANDALONE_ALIGN_CONTENT_VALUES {
                let mut tree = SimpleTree::default();
                let root = tree.push(SimpleNode::new(Style {
                    display: Display::Flex,
                    flex_direction,
                    flex_wrap,
                    width: Length::points(76.0),
                    height: Length::points(64.0),
                    justify_content: JustifyContent::FlexStart,
                    align_content,
                    align_items: AlignItems::FlexStart,
                    row_gap: Length::points(3.0),
                    column_gap: Length::points(2.0),
                    ..Style::default()
                }));
                let mut children = [0; 4];
                for (index, (width, height)) in
                    [(28.0, 16.0), (34.0, 12.0), (20.0, 18.0), (25.0, 14.0)]
                        .into_iter()
                        .enumerate()
                {
                    let child = tree.push(SimpleNode::new(Style {
                        display: Display::Flex,
                        width: Length::points(width),
                        height: Length::points(height),
                        flex_basis: Length::points(if flex_direction.is_row() {
                            width
                        } else {
                            height
                        }),
                        ..Style::default()
                    }));
                    children[index] = child;
                    tree.append_child(root, child);
                }
                let mut repeated = tree.clone();
                run_rust_layout(&mut tree, root, Constraints::definite(76.0, 64.0));
                run_rust_layout(&mut repeated, root, Constraints::definite(76.0, 64.0));

                assert_eq!(tree.nodes[root].layout.size, Size::new(76.0, 64.0));
                for (child, expected_size) in children.into_iter().zip([
                    Size::new(28.0, 16.0),
                    Size::new(34.0, 12.0),
                    Size::new(20.0, 18.0),
                    Size::new(25.0, 14.0),
                ]) {
                    assert_eq!(tree.nodes[child].layout, repeated.nodes[child].layout);
                    assert_eq!(tree.nodes[child].layout.size, expected_size);
                    assert!(tree.nodes[child].layout.offset.x.is_finite());
                    assert!(tree.nodes[child].layout.offset.y.is_finite());
                }

                if flex_direction.is_row() {
                    let (line_offsets, line_sizes) = standalone_expected_line_geometry(
                        align_content,
                        flex_wrap,
                        64.0,
                        [16.0, 18.0],
                        3.0,
                    );
                    for (child, line, child_cross_size) in [
                        (children[0], 0, 16.0),
                        (children[1], 0, 12.0),
                        (children[2], 1, 18.0),
                        (children[3], 1, 14.0),
                    ] {
                        let expected = if flex_wrap == FlexWrap::WrapReverse {
                            line_offsets[line] + line_sizes[line] - child_cross_size
                        } else {
                            line_offsets[line]
                        };
                        assert_close(tree.nodes[child].layout.offset.y, expected);
                    }
                } else {
                    let (line_offsets, line_sizes) = standalone_expected_line_geometry(
                        align_content,
                        flex_wrap,
                        76.0,
                        [34.0, 25.0],
                        2.0,
                    );
                    for (child, line, child_cross_size) in [
                        (children[0], 0, 28.0),
                        (children[1], 0, 34.0),
                        (children[2], 0, 20.0),
                        (children[3], 1, 25.0),
                    ] {
                        let expected = if flex_wrap == FlexWrap::WrapReverse {
                            line_offsets[line] + line_sizes[line] - child_cross_size
                        } else {
                            line_offsets[line]
                        };
                        assert_close(tree.nodes[child].layout.offset.x, expected);
                    }
                }
                cases += 1;
            }
        }
    }
    assert_eq!(cases, 36);
}

#[test]
fn standalone_direction_mapping_runs_all_8_source_cases() {
    let mut cases = 0;
    for flex_direction in [
        FlexDirection::Row,
        FlexDirection::RowReverse,
        FlexDirection::Column,
        FlexDirection::ColumnReverse,
    ] {
        for direction in [Direction::Ltr, Direction::Rtl] {
            let mut tree = SimpleTree::default();
            let root = tree.push(SimpleNode::new(Style {
                display: Display::Flex,
                flex_direction,
                direction,
                width: Length::points(120.0),
                height: Length::points(80.0),
                justify_content: JustifyContent::FlexStart,
                align_items: AlignItems::FlexStart,
                ..Style::default()
            }));
            let mut children = [0; 3];
            for (index, (width, height)) in [(18.0, 8.0), (24.0, 12.0), (30.0, 16.0)]
                .into_iter()
                .enumerate()
            {
                let child = tree.push(SimpleNode::new(Style {
                    display: Display::Flex,
                    width: Length::points(width),
                    height: Length::points(height),
                    flex_basis: Length::points(if flex_direction.is_row() {
                        width
                    } else {
                        height
                    }),
                    ..Style::default()
                }));
                children[index] = child;
                tree.append_child(root, child);
            }
            run_rust_layout(&mut tree, root, Constraints::definite(120.0, 80.0));

            let expected_offsets = match (flex_direction, direction) {
                (FlexDirection::Row, Direction::Ltr)
                | (FlexDirection::RowReverse, Direction::Rtl) => [
                    Point::new(0.0, 0.0),
                    Point::new(18.0, 0.0),
                    Point::new(42.0, 0.0),
                ],
                (FlexDirection::Row, Direction::Rtl)
                | (FlexDirection::RowReverse, Direction::Ltr) => [
                    Point::new(102.0, 0.0),
                    Point::new(78.0, 0.0),
                    Point::new(48.0, 0.0),
                ],
                (FlexDirection::Column, Direction::Ltr) => [
                    Point::new(0.0, 0.0),
                    Point::new(0.0, 8.0),
                    Point::new(0.0, 20.0),
                ],
                (FlexDirection::Column, Direction::Rtl) => [
                    Point::new(102.0, 0.0),
                    Point::new(96.0, 8.0),
                    Point::new(90.0, 20.0),
                ],
                (FlexDirection::ColumnReverse, Direction::Ltr) => [
                    Point::new(0.0, 72.0),
                    Point::new(0.0, 60.0),
                    Point::new(0.0, 44.0),
                ],
                (FlexDirection::ColumnReverse, Direction::Rtl) => [
                    Point::new(102.0, 72.0),
                    Point::new(96.0, 60.0),
                    Point::new(90.0, 44.0),
                ],
            };
            for (child, expected) in children.into_iter().zip(expected_offsets) {
                assert_close(tree.nodes[child].layout.offset.x, expected.x);
                assert_close(tree.nodes[child].layout.offset.y, expected.y);
            }
            cases += 1;
        }
    }
    assert_eq!(cases, 8);
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
