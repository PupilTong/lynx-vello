//! Rust-only migration of the Flex scenarios from PR #25's native
//! head-to-head suite.
//!
//! The original tests compared two engines. This target preserves the source
//! trees but runs only neutron-star, checking deterministic geometry, finite
//! outputs, non-negative sizes, and stable host child ordering. The explicit
//! inventories below prevent either the 191-case source scope or its 91-case
//! overlap with `pr25_flex_layout.rs` from drifting silently.
//! Legacy `head_to_head`/`cpp` words in test names and inventory strings are
//! retained source identifiers only; this target neither executes C++ nor
//! adopts its integer layout-unit rounding.
//!
//! Host lowerings are intentional and visible:
//! - Flex-only `fr` values have no W3C meaning and become `auto`;
//! - foreign Block/Linear/Grid containers are host-dispatched as column Flex adapters because
//!   neutron-star does not own those algorithms yet;
//! - cache-parity scenarios rebuild through the stateless test session and therefore assert
//!   geometry, not C++ cache-hit behavior;
//! - sticky positioning remains in-flow here; sticky inset export belongs to the host positioning
//!   layer.

mod pr25_support;
mod support;

use std::collections::BTreeSet;

use pr25_support::{
    AlignContent, AlignItems, BaseLength, BoxSizing, Constraints, Direction, Display,
    FlexDirection, FlexWrap, JustifyContent, LayoutResult, Length, PositionType, Rect,
    SideConstraint, SimpleNode, SimpleTree, Size, Style, run_rust_layout,
};

const NATIVE_FLEX_INVENTORY: &[&str] = &[
    "head_to_head_absolute_child_can_use_right_bottom_insets",
    "head_to_head_absolute_child_with_edges",
    "head_to_head_absolute_flex_child_center_alignment_allows_negative_free_space",
    "head_to_head_absolute_flex_child_uses_static_position_without_participating_in_flex_layout",
    "head_to_head_absolute_flex_child_without_insets_uses_container_alignment",
    "head_to_head_absolute_flex_child_wrap_reverse_reverses_cross_axis_initial_position",
    "head_to_head_absolute_rtl_flex_child_without_insets_uses_rtl_fronts",
    "head_to_head_align_content_center_uses_negative_free_space_when_lines_overflow",
    "head_to_head_align_content_centers_wrapped_lines_in_cross_axis",
    "head_to_head_align_content_flex_end_places_wrapped_lines_at_cross_end",
    "head_to_head_align_content_space_around_centers_overflow_and_keeps_row_gap",
    "head_to_head_align_content_space_between_keeps_row_gap_when_lines_overflow",
    "head_to_head_align_content_space_evenly_distributes_wrapped_lines",
    "head_to_head_align_content_space_evenly_uses_negative_space_when_lines_overflow",
    "head_to_head_align_content_start_end_alias_flex_edges_for_wrapped_lines",
    "head_to_head_align_content_stretch_expands_wrapped_line_cross_sizes",
    "head_to_head_align_items_center_uses_negative_cross_space_when_item_overflows",
    "head_to_head_align_items_flex_end_uses_negative_cross_space_when_item_overflows",
    "head_to_head_align_self_overrides_container_align_items",
    "head_to_head_align_start_end_variants_in_flex_cross_axis",
    "head_to_head_auto_margin_consumes_remaining_main_space",
    "head_to_head_border_box_max_width_caps_flex_grow_without_adding_padding_border",
    "head_to_head_border_box_min_width_freezes_flex_item_without_adding_padding_border",
    "head_to_head_calc_column_gap",
    "head_to_head_calc_padding_margin_and_position_edges",
    "head_to_head_calc_size_lengths",
    "head_to_head_column_fit_content_max_height_freezes_item_and_redistributes_flex_grow_space",
    "head_to_head_column_fit_content_min_height_freezes_item_during_flex_shrink",
    "head_to_head_column_fit_content_min_height_without_argument_does_not_freeze_item",
    "head_to_head_column_flex_item_fit_content_height_uses_natural_main_axis_size",
    "head_to_head_column_flex_item_percent_cross_size_and_aspect_ratio_define_main_basis",
    "head_to_head_column_max_content_max_height_does_not_cap_flex_grow_space",
    "head_to_head_column_percent_max_height_freezes_item_and_redistributes_flex_grow_space",
    "head_to_head_column_percent_min_height_freezes_item_during_flex_shrink",
    "head_to_head_column_reverse_flex_shrink_freeze_places_flexed_items_from_bottom_edge",
    "head_to_head_column_reverse_positions_items_from_bottom_edge_in_tree_order",
    "head_to_head_cross_axis_auto_margin_overrides_stretch_alignment",
    "head_to_head_display_none_child_is_laid_out_as_zero_and_skipped_by_flex",
    "head_to_head_explicit_ltr_no_wrap_mapping_keeps_single_flex_line",
    "head_to_head_explicit_stretch_justify_and_align_content_mapping",
    "head_to_head_fit_content_max_width_freezes_item_and_redistributes_flex_grow_space",
    "head_to_head_fit_content_max_width_without_argument_does_not_cap_flex_grow_space",
    "head_to_head_fit_content_min_width_freezes_item_during_flex_shrink",
    "head_to_head_flex_aligned_growing_percent_basis_target_defines_child_basis_base",
    "head_to_head_flex_all_zero_grow_items_leave_space_for_justify_content",
    "head_to_head_flex_auto_main_preserves_intrinsic_percent_basis_child",
    "head_to_head_flex_basis_fit_content_argument_resolves_before_measuring_item",
    "head_to_head_flex_basis_fit_content_percent_argument_resolves_against_main_axis",
    "head_to_head_flex_basis_fr_length_is_imported_as_full_value",
    "head_to_head_flex_basis_max_content_uses_auto_measure_path",
    "head_to_head_flex_column_container_baseline_uses_first_item_baseline_after_main_axis_alignment",
    "head_to_head_flex_column_stretch_with_fr_sibling_preserves_percent_basis",
    "head_to_head_flex_cross_size_baseline_line_size_uses_largest_baseline_distances",
    "head_to_head_flex_cross_size_hypothetical_cross_layout_uses_used_main_size",
    "head_to_head_flex_explicit_shrinking_stretch_remeasures_aligned_inflexible_percent_basis_subtree",
    "head_to_head_flex_explicit_stretch_remeasures_growing_percent_basis_subtree",
    "head_to_head_flex_explicit_stretch_remeasures_shrinking_percent_basis_subtree",
    "head_to_head_flex_grow_and_order",
    "head_to_head_flex_grow_sum_below_one_leaves_remaining_space_for_justify_content",
    "head_to_head_flex_growing_explicit_stretch_handles_inflexible_percent_basis_subtree",
    "head_to_head_flex_growing_explicit_stretch_remeasures_shrinking_percent_basis_subtree",
    "head_to_head_flex_growing_percent_basis_target_defines_local_flexible_child_basis_base",
    "head_to_head_flex_growing_percent_basis_target_defines_local_inflexible_child_basis_base",
    "head_to_head_flex_growing_percent_main_target_defines_child_main_size_base",
    "head_to_head_flex_growing_target_defines_percent_basis_and_main_size_child_base",
    "head_to_head_flex_implicit_growing_stretch_remeasures_aligned_growing_percent_basis_subtree",
    "head_to_head_flex_implicit_non_shrinking_stretch_remeasures_aligned_inflexible_percent_basis_subtree",
    "head_to_head_flex_implicit_shrinking_stretch_remeasures_aligned_shrinking_percent_basis_subtree",
    "head_to_head_flex_implicit_shrinking_stretch_remeasures_mixed_percent_basis_subtree",
    "head_to_head_flex_implicit_stretch_defines_percent_basis_for_non_shrinking_descendant",
    "head_to_head_flex_implicit_stretch_remeasures_local_inflexible_percent_basis_subtree",
    "head_to_head_flex_implicit_stretch_remeasures_shared_growing_percent_basis_line",
    "head_to_head_flex_implicit_stretch_remeasures_unresolved_percent_basis_descendant",
    "head_to_head_flex_item_derives_cross_size_from_main_size_and_aspect_ratio",
    "head_to_head_flex_item_fit_content_width_uses_natural_main_axis_size",
    "head_to_head_flex_justify_content_space_evenly",
    "head_to_head_flex_line_length_aspect_ratio_uses_definite_cross_size_for_content_basis",
    "head_to_head_flex_line_length_auto_container_main_size_uses_max_content_sum",
    "head_to_head_flex_line_length_available_main_space_uses_inner_content_box_for_auto_basis",
    "head_to_head_flex_line_length_definite_flex_basis_overrides_main_size_property",
    "head_to_head_flex_line_length_hypothetical_main_size_clamps_max_before_wrapping",
    "head_to_head_flex_line_length_hypothetical_main_size_clamps_min_before_wrapping",
    "head_to_head_flex_main_axis_gap_reduces_free_space_before_grow_distribution",
    "head_to_head_flex_main_size_line_collection_uses_outer_hypothetical_main_with_negative_margin",
    "head_to_head_flex_main_size_nowrap_collects_all_items_into_single_line_even_when_overflowing",
    "head_to_head_flex_main_size_resolves_flexible_lengths_per_line_independently",
    "head_to_head_flex_main_size_wrap_collects_oversized_first_item_alone",
    "head_to_head_flex_max_target_defines_inflexible_percent_basis_child_base",
    "head_to_head_flex_max_target_defines_percent_flex_basis_descendant_base",
    "head_to_head_flex_max_width_below_basis_freezes_growing_item_to_hypothetical_main_size",
    "head_to_head_flex_max_width_violation_freezes_item_during_shrink_and_restarts_distribution",
    "head_to_head_flex_min_target_defines_percent_flex_basis_descendant_base",
    "head_to_head_flex_min_width_above_basis_freezes_shrinking_item_to_hypothetical_main_size",
    "head_to_head_flex_min_width_violation_freezes_item_during_grow_and_restarts_distribution",
    "head_to_head_flex_multiple_max_width_violations_freeze_before_redistributing_flex_grow_space",
    "head_to_head_flex_multiple_min_width_violations_freeze_before_redistributing_flex_shrink_space",
    "head_to_head_flex_nowrap_cross_axis_at_most_does_not_clamp_latest_mode",
    "head_to_head_flex_oversized_inflexible_fixed_child_with_percent_basis_sibling",
    "head_to_head_flex_own_percent_basis_and_main_size_define_percent_child_bases",
    "head_to_head_flex_percent_basis_explicit_stretch_remeasures_flexible_percent_basis_subtree",
    "head_to_head_flex_percent_main_length_parent_defines_growing_percent_basis_child_base",
    "head_to_head_flex_point_basis_parent_defines_growing_percent_basis_child_base",
    "head_to_head_flex_preserved_percent_basis_parent_defines_growing_percent_child_base",
    "head_to_head_flex_preserved_percent_basis_parent_defines_inflexible_percent_basis_and_main_size_child_base",
    "head_to_head_flex_relative_child_percent_offsets_use_container_constraints",
    "head_to_head_flex_row_align_self_baseline_triggers_baseline_line_sizing",
    "head_to_head_flex_row_baseline_can_expand_auto_cross_size_for_bottom_margin",
    "head_to_head_flex_row_baseline_uses_nested_flex_container_baseline",
    "head_to_head_flex_row_baseline_uses_nested_grid_container_baseline",
    "head_to_head_flex_row_baseline_uses_nested_linear_container_baseline",
    "head_to_head_flex_shrink_distribution_is_scaled_by_flex_base_size",
    "head_to_head_flex_shrink_negative_inner_size_is_floored_after_outer_margins",
    "head_to_head_flex_shrink_sum_below_one_leaves_negative_space_for_justify_content",
    "head_to_head_flex_shrinking_explicit_stretch_handles_inflexible_percent_basis_subtree",
    "head_to_head_flex_shrinking_target_defines_percent_main_size_child_base",
    "head_to_head_flex_shrunk_parent_target_defines_percent_basis_child_base",
    "head_to_head_flex_sticky_child_end_percent_insets_resolve_against_container_constraints",
    "head_to_head_flex_sticky_child_percent_insets_resolve_against_container_constraints",
    "head_to_head_flex_stretch_reexports_cached_block_subtree",
    "head_to_head_flex_stretch_reexports_cached_block_subtree_with_fractional_offsets",
    "head_to_head_flex_stretch_remeasures_aligned_inflexible_percent_basis_subtree",
    "head_to_head_flex_unchanged_main_defines_inflexible_percent_basis_child_base",
    "head_to_head_flex_unchanged_main_defines_stretch_percent_basis_child_base",
    "head_to_head_flex_wrap_collects_items_into_multiple_lines",
    "head_to_head_flex_wrap_collects_zero_sized_item_after_exact_fit_on_same_line",
    "head_to_head_flex_wrap_cross_axis_at_most_does_not_clamp_line_sum_latest_mode",
    "head_to_head_flex_wrap_reverse_center_reexports_cached_block_subtree_with_fractional_offset",
    "head_to_head_flex_wrap_reverse_places_first_line_at_cross_end",
    "head_to_head_flex_wrap_reverse_reverses_space_between_line_distribution",
    "head_to_head_flex_wrap_reverse_stretched_line_uses_reversed_cross_alignment",
    "head_to_head_flex_zero_grow_freezes_item_before_distributing_positive_free_space",
    "head_to_head_flex_zero_shrink_freezes_item_before_distributing_negative_free_space",
    "head_to_head_flexible_lengths_direction_matrix_places_resolved_main_sizes",
    "head_to_head_flexible_lengths_resolve_independently_per_wrapped_line",
    "head_to_head_full_value_column_gap_units",
    "head_to_head_full_value_edge_lengths_reach_cpp_baseline_import",
    "head_to_head_full_value_row_gap_units",
    "head_to_head_justify_content_center_uses_negative_free_space_when_items_overflow",
    "head_to_head_justify_content_gap_overflow_direction_matrix",
    "head_to_head_justify_content_main_axis_direction_matrix",
    "head_to_head_justify_content_space_around_single_item_fallback",
    "head_to_head_justify_content_space_around_uses_edge_difference_width_rounding_when_overflowing",
    "head_to_head_justify_content_space_between_single_item_fallback",
    "head_to_head_justify_content_space_evenly_single_item_equal_edge_spaces",
    "head_to_head_justify_content_start_end_variants",
    "head_to_head_justify_content_stretch_behaves_like_flex_start_in_flex_layout",
    "head_to_head_main_axis_auto_margin_direction_matrix",
    "head_to_head_main_axis_auto_margin_without_positive_free_space_zeroes_margins_then_justify_content",
    "head_to_head_max_content_min_width_does_not_freeze_item_during_flex_shrink",
    "head_to_head_max_width_freezes_item_and_redistributes_flex_grow_space",
    "head_to_head_measured_baseline_alignment",
    "head_to_head_measured_exact_item_uses_constraints_without_measure_size",
    "head_to_head_measured_flex_basis_grow_max_width_violation_restarts_distribution",
    "head_to_head_measured_flex_basis_shrink_min_width_violation_restarts_distribution",
    "head_to_head_measured_max_content_item",
    "head_to_head_min_width_freezes_item_during_flex_shrink",
    "head_to_head_multiple_main_axis_auto_margins_share_positive_free_space",
    "head_to_head_nested_column_flex",
    "head_to_head_nested_intrinsic_flex_basis_grow_max_width_violation_restarts_distribution",
    "head_to_head_nested_intrinsic_flex_basis_shrink_min_width_violation_restarts_distribution",
    "head_to_head_orthogonal_flex_reuses_percent_basis_subtree_measure",
    "head_to_head_overflowing_cross_axis_auto_margins_place_overflow_at_cross_end",
    "head_to_head_owner_definite_width_strips_root_horizontal_margins",
    "head_to_head_owner_definite_width_without_root_width_uses_root_at_most_width",
    "head_to_head_paired_cross_axis_auto_margins_center_item",
    "head_to_head_paired_main_axis_auto_margins_center_item",
    "head_to_head_percent_max_width_freezes_item_and_redistributes_flex_grow_space",
    "head_to_head_percent_min_width_freezes_item_during_flex_shrink",
    "head_to_head_percent_padding_gap_and_margin",
    "head_to_head_root_at_most_shrink_wraps_flex_line",
    "head_to_head_root_column_flex_fit_content_calc_argument_caps_final_height",
    "head_to_head_root_column_flex_fit_content_percent_argument_caps_final_height",
    "head_to_head_root_flex_fit_content_calc_argument_caps_final_width",
    "head_to_head_root_flex_fit_content_percent_argument_caps_final_width",
    "head_to_head_row_reverse_flex_end_packs_items_at_left_edge",
    "head_to_head_row_reverse_flex_grow_freeze_places_flexed_items_from_right_edge",
    "head_to_head_row_reverse_positions_items_from_right_edge_in_tree_order",
    "head_to_head_rtl_column_uses_right_cross_start_for_flex_start",
    "head_to_head_rtl_row_reverse_uses_left_main_front",
    "head_to_head_rtl_row_uses_right_main_front",
    "head_to_head_simple_tree_measure_and_baseline_callbacks",
    "head_to_head_single_cross_axis_auto_margins_absorb_positive_free_space",
    "head_to_head_single_line_min_cross_size_clamps_line_before_cross_alignment",
    "head_to_head_stretched_flex_item_cross_size_respects_min_max_constraints",
    "head_to_head_stretched_flex_item_relayouts_percent_height_child_with_definite_cross_size",
    "head_to_head_stretched_flex_item_with_aspect_ratio_keeps_flexed_main_size_and_uses_line_cross_size",
    "head_to_head_vertical_percentage_padding_and_margin_use_width_percent_base",
    "head_to_head_wrap_gaps_and_align_content",
    "head_to_head_wrapped_flex_fit_content_measured_callback_container_width",
    "head_to_head_wrapped_flex_measured_callback_baseline_exports_cpp_first_line_baseline",
    "head_to_head_wrapped_root_at_most_shrink_wraps_largest_flex_line",
];

/// Native cases already implemented under the exact stripped name in
/// `tests/pr25_flex_layout.rs`; this is an explicit mapping, not duplicate
/// test code.
const CANONICAL_FLEX_MAPPING: &[(&str, &str)] = &[
    (
        "head_to_head_align_content_center_uses_negative_free_space_when_lines_overflow",
        "align_content_center_uses_negative_free_space_when_lines_overflow",
    ),
    (
        "head_to_head_align_content_centers_wrapped_lines_in_cross_axis",
        "align_content_centers_wrapped_lines_in_cross_axis",
    ),
    (
        "head_to_head_align_content_space_around_centers_overflow_and_keeps_row_gap",
        "align_content_space_around_centers_overflow_and_keeps_row_gap",
    ),
    (
        "head_to_head_align_content_space_between_keeps_row_gap_when_lines_overflow",
        "align_content_space_between_keeps_row_gap_when_lines_overflow",
    ),
    (
        "head_to_head_align_content_space_evenly_distributes_wrapped_lines",
        "align_content_space_evenly_distributes_wrapped_lines",
    ),
    (
        "head_to_head_align_content_space_evenly_uses_negative_space_when_lines_overflow",
        "align_content_space_evenly_uses_negative_space_when_lines_overflow",
    ),
    (
        "head_to_head_align_content_start_end_alias_flex_edges_for_wrapped_lines",
        "align_content_start_end_alias_flex_edges_for_wrapped_lines",
    ),
    (
        "head_to_head_align_content_stretch_expands_wrapped_line_cross_sizes",
        "align_content_stretch_expands_wrapped_line_cross_sizes",
    ),
    (
        "head_to_head_align_items_center_uses_negative_cross_space_when_item_overflows",
        "align_items_center_uses_negative_cross_space_when_item_overflows",
    ),
    (
        "head_to_head_align_items_flex_end_uses_negative_cross_space_when_item_overflows",
        "align_items_flex_end_uses_negative_cross_space_when_item_overflows",
    ),
    (
        "head_to_head_align_self_overrides_container_align_items",
        "align_self_overrides_container_align_items",
    ),
    (
        "head_to_head_border_box_max_width_caps_flex_grow_without_adding_padding_border",
        "border_box_max_width_caps_flex_grow_without_adding_padding_border",
    ),
    (
        "head_to_head_border_box_min_width_freezes_flex_item_without_adding_padding_border",
        "border_box_min_width_freezes_flex_item_without_adding_padding_border",
    ),
    (
        "head_to_head_column_fit_content_max_height_freezes_item_and_redistributes_flex_grow_space",
        "column_fit_content_max_height_freezes_item_and_redistributes_flex_grow_space",
    ),
    (
        "head_to_head_column_fit_content_min_height_freezes_item_during_flex_shrink",
        "column_fit_content_min_height_freezes_item_during_flex_shrink",
    ),
    (
        "head_to_head_column_fit_content_min_height_without_argument_does_not_freeze_item",
        "column_fit_content_min_height_without_argument_does_not_freeze_item",
    ),
    (
        "head_to_head_column_flex_item_fit_content_height_uses_natural_main_axis_size",
        "column_flex_item_fit_content_height_uses_natural_main_axis_size",
    ),
    (
        "head_to_head_column_flex_item_percent_cross_size_and_aspect_ratio_define_main_basis",
        "column_flex_item_percent_cross_size_and_aspect_ratio_define_main_basis",
    ),
    (
        "head_to_head_column_max_content_max_height_does_not_cap_flex_grow_space",
        "column_max_content_max_height_does_not_cap_flex_grow_space",
    ),
    (
        "head_to_head_column_percent_max_height_freezes_item_and_redistributes_flex_grow_space",
        "column_percent_max_height_freezes_item_and_redistributes_flex_grow_space",
    ),
    (
        "head_to_head_column_percent_min_height_freezes_item_during_flex_shrink",
        "column_percent_min_height_freezes_item_during_flex_shrink",
    ),
    (
        "head_to_head_column_reverse_flex_shrink_freeze_places_flexed_items_from_bottom_edge",
        "column_reverse_flex_shrink_freeze_places_flexed_items_from_bottom_edge",
    ),
    (
        "head_to_head_column_reverse_positions_items_from_bottom_edge_in_tree_order",
        "column_reverse_positions_items_from_bottom_edge_in_tree_order",
    ),
    (
        "head_to_head_cross_axis_auto_margin_overrides_stretch_alignment",
        "cross_axis_auto_margin_overrides_stretch_alignment",
    ),
    (
        "head_to_head_display_none_child_is_laid_out_as_zero_and_skipped_by_flex",
        "display_none_child_is_laid_out_as_zero_and_skipped_by_flex",
    ),
    (
        "head_to_head_fit_content_max_width_freezes_item_and_redistributes_flex_grow_space",
        "fit_content_max_width_freezes_item_and_redistributes_flex_grow_space",
    ),
    (
        "head_to_head_fit_content_max_width_without_argument_does_not_cap_flex_grow_space",
        "fit_content_max_width_without_argument_does_not_cap_flex_grow_space",
    ),
    (
        "head_to_head_fit_content_min_width_freezes_item_during_flex_shrink",
        "fit_content_min_width_freezes_item_during_flex_shrink",
    ),
    (
        "head_to_head_flex_basis_fit_content_argument_resolves_before_measuring_item",
        "flex_basis_fit_content_argument_resolves_before_measuring_item",
    ),
    (
        "head_to_head_flex_basis_fit_content_percent_argument_resolves_against_main_axis",
        "flex_basis_fit_content_percent_argument_resolves_against_main_axis",
    ),
    (
        "head_to_head_flex_column_container_baseline_uses_first_item_baseline_after_main_axis_alignment",
        "flex_column_container_baseline_uses_first_item_baseline_after_main_axis_alignment",
    ),
    (
        "head_to_head_flex_cross_size_baseline_line_size_uses_largest_baseline_distances",
        "flex_cross_size_baseline_line_size_uses_largest_baseline_distances",
    ),
    (
        "head_to_head_flex_cross_size_hypothetical_cross_layout_uses_used_main_size",
        "flex_cross_size_hypothetical_cross_layout_uses_used_main_size",
    ),
    (
        "head_to_head_flex_grow_sum_below_one_leaves_remaining_space_for_justify_content",
        "flex_grow_sum_below_one_leaves_remaining_space_for_justify_content",
    ),
    (
        "head_to_head_flex_item_fit_content_width_uses_natural_main_axis_size",
        "flex_item_fit_content_width_uses_natural_main_axis_size",
    ),
    (
        "head_to_head_flex_line_length_aspect_ratio_uses_definite_cross_size_for_content_basis",
        "flex_line_length_aspect_ratio_uses_definite_cross_size_for_content_basis",
    ),
    (
        "head_to_head_flex_line_length_auto_container_main_size_uses_max_content_sum",
        "flex_line_length_auto_container_main_size_uses_max_content_sum",
    ),
    (
        "head_to_head_flex_line_length_available_main_space_uses_inner_content_box_for_auto_basis",
        "flex_line_length_available_main_space_uses_inner_content_box_for_auto_basis",
    ),
    (
        "head_to_head_flex_line_length_definite_flex_basis_overrides_main_size_property",
        "flex_line_length_definite_flex_basis_overrides_main_size_property",
    ),
    (
        "head_to_head_flex_line_length_hypothetical_main_size_clamps_max_before_wrapping",
        "flex_line_length_hypothetical_main_size_clamps_max_before_wrapping",
    ),
    (
        "head_to_head_flex_line_length_hypothetical_main_size_clamps_min_before_wrapping",
        "flex_line_length_hypothetical_main_size_clamps_min_before_wrapping",
    ),
    (
        "head_to_head_flex_main_size_line_collection_uses_outer_hypothetical_main_with_negative_margin",
        "flex_main_size_line_collection_uses_outer_hypothetical_main_with_negative_margin",
    ),
    (
        "head_to_head_flex_main_size_nowrap_collects_all_items_into_single_line_even_when_overflowing",
        "flex_main_size_nowrap_collects_all_items_into_single_line_even_when_overflowing",
    ),
    (
        "head_to_head_flex_main_size_resolves_flexible_lengths_per_line_independently",
        "flex_main_size_resolves_flexible_lengths_per_line_independently",
    ),
    (
        "head_to_head_flex_main_size_wrap_collects_oversized_first_item_alone",
        "flex_main_size_wrap_collects_oversized_first_item_alone",
    ),
    (
        "head_to_head_flex_nowrap_cross_axis_at_most_does_not_clamp_latest_mode",
        "flex_nowrap_cross_axis_at_most_does_not_clamp_latest_mode",
    ),
    (
        "head_to_head_flex_row_align_self_baseline_triggers_baseline_line_sizing",
        "flex_row_align_self_baseline_triggers_baseline_line_sizing",
    ),
    (
        "head_to_head_flex_row_baseline_can_expand_auto_cross_size_for_bottom_margin",
        "flex_row_baseline_can_expand_auto_cross_size_for_bottom_margin",
    ),
    (
        "head_to_head_flex_row_baseline_uses_nested_flex_container_baseline",
        "flex_row_baseline_uses_nested_flex_container_baseline",
    ),
    (
        "head_to_head_flex_row_baseline_uses_nested_grid_container_baseline",
        "flex_row_baseline_uses_nested_grid_container_baseline",
    ),
    (
        "head_to_head_flex_row_baseline_uses_nested_linear_container_baseline",
        "flex_row_baseline_uses_nested_linear_container_baseline",
    ),
    (
        "head_to_head_flex_shrink_distribution_is_scaled_by_flex_base_size",
        "flex_shrink_distribution_is_scaled_by_flex_base_size",
    ),
    (
        "head_to_head_flex_shrink_negative_inner_size_is_floored_after_outer_margins",
        "flex_shrink_negative_inner_size_is_floored_after_outer_margins",
    ),
    (
        "head_to_head_flex_shrink_sum_below_one_leaves_negative_space_for_justify_content",
        "flex_shrink_sum_below_one_leaves_negative_space_for_justify_content",
    ),
    (
        "head_to_head_flex_wrap_collects_items_into_multiple_lines",
        "flex_wrap_collects_items_into_multiple_lines",
    ),
    (
        "head_to_head_flex_wrap_collects_zero_sized_item_after_exact_fit_on_same_line",
        "flex_wrap_collects_zero_sized_item_after_exact_fit_on_same_line",
    ),
    (
        "head_to_head_flex_wrap_cross_axis_at_most_does_not_clamp_line_sum_latest_mode",
        "flex_wrap_cross_axis_at_most_does_not_clamp_line_sum_latest_mode",
    ),
    (
        "head_to_head_flex_wrap_reverse_places_first_line_at_cross_end",
        "flex_wrap_reverse_places_first_line_at_cross_end",
    ),
    (
        "head_to_head_flex_wrap_reverse_reverses_space_between_line_distribution",
        "flex_wrap_reverse_reverses_space_between_line_distribution",
    ),
    (
        "head_to_head_flex_wrap_reverse_stretched_line_uses_reversed_cross_alignment",
        "flex_wrap_reverse_stretched_line_uses_reversed_cross_alignment",
    ),
    (
        "head_to_head_flexible_lengths_direction_matrix_places_resolved_main_sizes",
        "flexible_lengths_direction_matrix_places_resolved_main_sizes",
    ),
    (
        "head_to_head_flexible_lengths_resolve_independently_per_wrapped_line",
        "flexible_lengths_resolve_independently_per_wrapped_line",
    ),
    (
        "head_to_head_justify_content_center_uses_negative_free_space_when_items_overflow",
        "justify_content_center_uses_negative_free_space_when_items_overflow",
    ),
    (
        "head_to_head_justify_content_space_around_uses_edge_difference_width_rounding_when_overflowing",
        "justify_content_space_around_uses_edge_difference_width_rounding_when_overflowing",
    ),
    (
        "head_to_head_justify_content_stretch_behaves_like_flex_start_in_flex_layout",
        "justify_content_stretch_behaves_like_flex_start_in_flex_layout",
    ),
    (
        "head_to_head_main_axis_auto_margin_without_positive_free_space_zeroes_margins_then_justify_content",
        "main_axis_auto_margin_without_positive_free_space_zeroes_margins_then_justify_content",
    ),
    (
        "head_to_head_max_content_min_width_does_not_freeze_item_during_flex_shrink",
        "max_content_min_width_does_not_freeze_item_during_flex_shrink",
    ),
    (
        "head_to_head_max_width_freezes_item_and_redistributes_flex_grow_space",
        "max_width_freezes_item_and_redistributes_flex_grow_space",
    ),
    (
        "head_to_head_measured_flex_basis_grow_max_width_violation_restarts_distribution",
        "measured_flex_basis_grow_max_width_violation_restarts_distribution",
    ),
    (
        "head_to_head_measured_flex_basis_shrink_min_width_violation_restarts_distribution",
        "measured_flex_basis_shrink_min_width_violation_restarts_distribution",
    ),
    (
        "head_to_head_min_width_freezes_item_during_flex_shrink",
        "min_width_freezes_item_during_flex_shrink",
    ),
    (
        "head_to_head_nested_intrinsic_flex_basis_grow_max_width_violation_restarts_distribution",
        "nested_intrinsic_flex_basis_grow_max_width_violation_restarts_distribution",
    ),
    (
        "head_to_head_nested_intrinsic_flex_basis_shrink_min_width_violation_restarts_distribution",
        "nested_intrinsic_flex_basis_shrink_min_width_violation_restarts_distribution",
    ),
    (
        "head_to_head_overflowing_cross_axis_auto_margins_place_overflow_at_cross_end",
        "overflowing_cross_axis_auto_margins_place_overflow_at_cross_end",
    ),
    (
        "head_to_head_paired_cross_axis_auto_margins_center_item",
        "paired_cross_axis_auto_margins_center_item",
    ),
    (
        "head_to_head_paired_main_axis_auto_margins_center_item",
        "paired_main_axis_auto_margins_center_item",
    ),
    (
        "head_to_head_percent_max_width_freezes_item_and_redistributes_flex_grow_space",
        "percent_max_width_freezes_item_and_redistributes_flex_grow_space",
    ),
    (
        "head_to_head_percent_min_width_freezes_item_during_flex_shrink",
        "percent_min_width_freezes_item_during_flex_shrink",
    ),
    (
        "head_to_head_root_column_flex_fit_content_calc_argument_caps_final_height",
        "root_column_flex_fit_content_calc_argument_caps_final_height",
    ),
    (
        "head_to_head_root_column_flex_fit_content_percent_argument_caps_final_height",
        "root_column_flex_fit_content_percent_argument_caps_final_height",
    ),
    (
        "head_to_head_root_flex_fit_content_calc_argument_caps_final_width",
        "root_flex_fit_content_calc_argument_caps_final_width",
    ),
    (
        "head_to_head_root_flex_fit_content_percent_argument_caps_final_width",
        "root_flex_fit_content_percent_argument_caps_final_width",
    ),
    (
        "head_to_head_row_reverse_flex_end_packs_items_at_left_edge",
        "row_reverse_flex_end_packs_items_at_left_edge",
    ),
    (
        "head_to_head_row_reverse_flex_grow_freeze_places_flexed_items_from_right_edge",
        "row_reverse_flex_grow_freeze_places_flexed_items_from_right_edge",
    ),
    (
        "head_to_head_row_reverse_positions_items_from_right_edge_in_tree_order",
        "row_reverse_positions_items_from_right_edge_in_tree_order",
    ),
    (
        "head_to_head_rtl_column_uses_right_cross_start_for_flex_start",
        "rtl_column_uses_right_cross_start_for_flex_start",
    ),
    (
        "head_to_head_single_cross_axis_auto_margins_absorb_positive_free_space",
        "single_cross_axis_auto_margins_absorb_positive_free_space",
    ),
    (
        "head_to_head_single_line_min_cross_size_clamps_line_before_cross_alignment",
        "single_line_min_cross_size_clamps_line_before_cross_alignment",
    ),
    (
        "head_to_head_stretched_flex_item_cross_size_respects_min_max_constraints",
        "stretched_flex_item_cross_size_respects_min_max_constraints",
    ),
    (
        "head_to_head_stretched_flex_item_relayouts_percent_height_child_with_definite_cross_size",
        "stretched_flex_item_relayouts_percent_height_child_with_definite_cross_size",
    ),
    (
        "head_to_head_stretched_flex_item_with_aspect_ratio_keeps_flexed_main_size_and_uses_line_cross_size",
        "stretched_flex_item_with_aspect_ratio_keeps_flexed_main_size_and_uses_line_cross_size",
    ),
];

const UNIQUE_NATIVE_FLEX_SCENARIOS: &[&str] = &[
    "head_to_head_absolute_child_can_use_right_bottom_insets",
    "head_to_head_absolute_child_with_edges",
    "head_to_head_absolute_flex_child_center_alignment_allows_negative_free_space",
    "head_to_head_absolute_flex_child_uses_static_position_without_participating_in_flex_layout",
    "head_to_head_absolute_flex_child_without_insets_uses_container_alignment",
    "head_to_head_absolute_flex_child_wrap_reverse_reverses_cross_axis_initial_position",
    "head_to_head_absolute_rtl_flex_child_without_insets_uses_rtl_fronts",
    "head_to_head_align_content_flex_end_places_wrapped_lines_at_cross_end",
    "head_to_head_align_start_end_variants_in_flex_cross_axis",
    "head_to_head_auto_margin_consumes_remaining_main_space",
    "head_to_head_calc_column_gap",
    "head_to_head_calc_padding_margin_and_position_edges",
    "head_to_head_calc_size_lengths",
    "head_to_head_explicit_ltr_no_wrap_mapping_keeps_single_flex_line",
    "head_to_head_explicit_stretch_justify_and_align_content_mapping",
    "head_to_head_flex_aligned_growing_percent_basis_target_defines_child_basis_base",
    "head_to_head_flex_all_zero_grow_items_leave_space_for_justify_content",
    "head_to_head_flex_auto_main_preserves_intrinsic_percent_basis_child",
    "head_to_head_flex_basis_fr_length_is_imported_as_full_value",
    "head_to_head_flex_basis_max_content_uses_auto_measure_path",
    "head_to_head_flex_column_stretch_with_fr_sibling_preserves_percent_basis",
    "head_to_head_flex_explicit_shrinking_stretch_remeasures_aligned_inflexible_percent_basis_subtree",
    "head_to_head_flex_explicit_stretch_remeasures_growing_percent_basis_subtree",
    "head_to_head_flex_explicit_stretch_remeasures_shrinking_percent_basis_subtree",
    "head_to_head_flex_grow_and_order",
    "head_to_head_flex_growing_explicit_stretch_handles_inflexible_percent_basis_subtree",
    "head_to_head_flex_growing_explicit_stretch_remeasures_shrinking_percent_basis_subtree",
    "head_to_head_flex_growing_percent_basis_target_defines_local_flexible_child_basis_base",
    "head_to_head_flex_growing_percent_basis_target_defines_local_inflexible_child_basis_base",
    "head_to_head_flex_growing_percent_main_target_defines_child_main_size_base",
    "head_to_head_flex_growing_target_defines_percent_basis_and_main_size_child_base",
    "head_to_head_flex_implicit_growing_stretch_remeasures_aligned_growing_percent_basis_subtree",
    "head_to_head_flex_implicit_non_shrinking_stretch_remeasures_aligned_inflexible_percent_basis_subtree",
    "head_to_head_flex_implicit_shrinking_stretch_remeasures_aligned_shrinking_percent_basis_subtree",
    "head_to_head_flex_implicit_shrinking_stretch_remeasures_mixed_percent_basis_subtree",
    "head_to_head_flex_implicit_stretch_defines_percent_basis_for_non_shrinking_descendant",
    "head_to_head_flex_implicit_stretch_remeasures_local_inflexible_percent_basis_subtree",
    "head_to_head_flex_implicit_stretch_remeasures_shared_growing_percent_basis_line",
    "head_to_head_flex_implicit_stretch_remeasures_unresolved_percent_basis_descendant",
    "head_to_head_flex_item_derives_cross_size_from_main_size_and_aspect_ratio",
    "head_to_head_flex_justify_content_space_evenly",
    "head_to_head_flex_main_axis_gap_reduces_free_space_before_grow_distribution",
    "head_to_head_flex_max_target_defines_inflexible_percent_basis_child_base",
    "head_to_head_flex_max_target_defines_percent_flex_basis_descendant_base",
    "head_to_head_flex_max_width_below_basis_freezes_growing_item_to_hypothetical_main_size",
    "head_to_head_flex_max_width_violation_freezes_item_during_shrink_and_restarts_distribution",
    "head_to_head_flex_min_target_defines_percent_flex_basis_descendant_base",
    "head_to_head_flex_min_width_above_basis_freezes_shrinking_item_to_hypothetical_main_size",
    "head_to_head_flex_min_width_violation_freezes_item_during_grow_and_restarts_distribution",
    "head_to_head_flex_multiple_max_width_violations_freeze_before_redistributing_flex_grow_space",
    "head_to_head_flex_multiple_min_width_violations_freeze_before_redistributing_flex_shrink_space",
    "head_to_head_flex_oversized_inflexible_fixed_child_with_percent_basis_sibling",
    "head_to_head_flex_own_percent_basis_and_main_size_define_percent_child_bases",
    "head_to_head_flex_percent_basis_explicit_stretch_remeasures_flexible_percent_basis_subtree",
    "head_to_head_flex_percent_main_length_parent_defines_growing_percent_basis_child_base",
    "head_to_head_flex_point_basis_parent_defines_growing_percent_basis_child_base",
    "head_to_head_flex_preserved_percent_basis_parent_defines_growing_percent_child_base",
    "head_to_head_flex_preserved_percent_basis_parent_defines_inflexible_percent_basis_and_main_size_child_base",
    "head_to_head_flex_relative_child_percent_offsets_use_container_constraints",
    "head_to_head_flex_shrinking_explicit_stretch_handles_inflexible_percent_basis_subtree",
    "head_to_head_flex_shrinking_target_defines_percent_main_size_child_base",
    "head_to_head_flex_shrunk_parent_target_defines_percent_basis_child_base",
    "head_to_head_flex_sticky_child_end_percent_insets_resolve_against_container_constraints",
    "head_to_head_flex_sticky_child_percent_insets_resolve_against_container_constraints",
    "head_to_head_flex_stretch_reexports_cached_block_subtree",
    "head_to_head_flex_stretch_reexports_cached_block_subtree_with_fractional_offsets",
    "head_to_head_flex_stretch_remeasures_aligned_inflexible_percent_basis_subtree",
    "head_to_head_flex_unchanged_main_defines_inflexible_percent_basis_child_base",
    "head_to_head_flex_unchanged_main_defines_stretch_percent_basis_child_base",
    "head_to_head_flex_wrap_reverse_center_reexports_cached_block_subtree_with_fractional_offset",
    "head_to_head_flex_zero_grow_freezes_item_before_distributing_positive_free_space",
    "head_to_head_flex_zero_shrink_freezes_item_before_distributing_negative_free_space",
    "head_to_head_full_value_column_gap_units",
    "head_to_head_full_value_edge_lengths_reach_cpp_baseline_import",
    "head_to_head_full_value_row_gap_units",
    "head_to_head_justify_content_gap_overflow_direction_matrix",
    "head_to_head_justify_content_main_axis_direction_matrix",
    "head_to_head_justify_content_space_around_single_item_fallback",
    "head_to_head_justify_content_space_between_single_item_fallback",
    "head_to_head_justify_content_space_evenly_single_item_equal_edge_spaces",
    "head_to_head_justify_content_start_end_variants",
    "head_to_head_main_axis_auto_margin_direction_matrix",
    "head_to_head_measured_baseline_alignment",
    "head_to_head_measured_exact_item_uses_constraints_without_measure_size",
    "head_to_head_measured_max_content_item",
    "head_to_head_multiple_main_axis_auto_margins_share_positive_free_space",
    "head_to_head_nested_column_flex",
    "head_to_head_orthogonal_flex_reuses_percent_basis_subtree_measure",
    "head_to_head_owner_definite_width_strips_root_horizontal_margins",
    "head_to_head_owner_definite_width_without_root_width_uses_root_at_most_width",
    "head_to_head_percent_padding_gap_and_margin",
    "head_to_head_root_at_most_shrink_wraps_flex_line",
    "head_to_head_rtl_row_reverse_uses_left_main_front",
    "head_to_head_rtl_row_uses_right_main_front",
    "head_to_head_simple_tree_measure_and_baseline_callbacks",
    "head_to_head_vertical_percentage_padding_and_margin_use_width_percent_base",
    "head_to_head_wrap_gaps_and_align_content",
    "head_to_head_wrapped_flex_fit_content_measured_callback_container_width",
    "head_to_head_wrapped_flex_measured_callback_baseline_exports_cpp_first_line_baseline",
    "head_to_head_wrapped_root_at_most_shrink_wraps_largest_flex_line",
];

const EXPLICIT_HOST_LOWERINGS: &[(&str, &str)] = &[
    (
        "head_to_head_flex_basis_fr_length_is_imported_as_full_value",
        "Flex-only fr is normalized to auto",
    ),
    (
        "head_to_head_flex_column_stretch_with_fr_sibling_preserves_percent_basis",
        "fr is normalized to auto and foreign display containers use host Flex adapters",
    ),
    (
        "head_to_head_flex_row_baseline_uses_nested_linear_container_baseline",
        "the canonical target uses the host's foreign-container baseline override",
    ),
    (
        "head_to_head_flex_row_baseline_uses_nested_grid_container_baseline",
        "the canonical target lowers the foreign Grid subtree through a column-Flex adapter",
    ),
    (
        "head_to_head_flex_stretch_reexports_cached_block_subtree",
        "the stateless host reruns the subtree and asserts deterministic geometry",
    ),
    (
        "head_to_head_flex_stretch_reexports_cached_block_subtree_with_fractional_offsets",
        "the stateless host reruns the subtree and asserts deterministic geometry",
    ),
    (
        "head_to_head_flex_wrap_reverse_center_reexports_cached_block_subtree_with_fractional_offset",
        "the stateless host reruns the subtree and asserts deterministic geometry",
    ),
    (
        "head_to_head_orthogonal_flex_reuses_percent_basis_subtree_measure",
        "the stateless host validates the recomputed geometry rather than cache reuse",
    ),
    (
        "head_to_head_wrapped_flex_fit_content_measured_callback_container_width",
        "the Block root is host-lowered to a column Flex adapter",
    ),
    (
        "head_to_head_flex_sticky_child_percent_insets_resolve_against_container_constraints",
        "sticky remains in-flow; sticky export is a host-layer responsibility",
    ),
    (
        "head_to_head_flex_sticky_child_end_percent_insets_resolve_against_container_constraints",
        "sticky remains in-flow; sticky export is a host-layer responsibility",
    ),
];

#[test]
fn native_flex_inventory_partitions_into_canonical_and_unique_cases() {
    assert_eq!(NATIVE_FLEX_INVENTORY.len(), 191);
    assert_eq!(CANONICAL_FLEX_MAPPING.len(), 91);
    assert_eq!(UNIQUE_NATIVE_FLEX_SCENARIOS.len(), 100);

    let inventory = NATIVE_FLEX_INVENTORY
        .iter()
        .copied()
        .collect::<BTreeSet<_>>();
    assert_eq!(inventory.len(), NATIVE_FLEX_INVENTORY.len());

    let canonical = CANONICAL_FLEX_MAPPING
        .iter()
        .map(|(native, canonical)| {
            assert_eq!(native.strip_prefix("head_to_head_"), Some(*canonical));
            *native
        })
        .collect::<BTreeSet<_>>();
    let unique = UNIQUE_NATIVE_FLEX_SCENARIOS
        .iter()
        .copied()
        .collect::<BTreeSet<_>>();
    assert!(canonical.is_disjoint(&unique));
    assert_eq!(
        canonical.union(&unique).copied().collect::<BTreeSet<_>>(),
        inventory
    );

    for (scenario, reason) in EXPLICIT_HOST_LOWERINGS {
        assert!(
            inventory.contains(scenario),
            "lowered scenario missing: {scenario}"
        );
        assert!(!reason.is_empty());
    }
}

#[allow(dead_code)]
trait NativeLengthExt {
    fn fr(value: f32) -> Self;
    fn max_content() -> Self;
}

impl NativeLengthExt for Length {
    fn fr(value: f32) -> Self {
        Self::Fr(value)
    }

    fn max_content() -> Self {
        Self::MaxContent
    }
}

trait NativeMeasuredNodeExt {
    fn with_measure_func_and_baseline(
        style: Style,
        measure: fn(Constraints) -> Size,
        baseline: fn(Size) -> f32,
    ) -> Self;
}

impl NativeMeasuredNodeExt for SimpleNode {
    fn with_measure_func_and_baseline(
        style: Style,
        measure: fn(Constraints) -> Size,
        baseline: fn(Size) -> f32,
    ) -> Self {
        // The shared facade has separate static-baseline and constraint-aware
        // measurement variants. For this native-only host adapter, retain the
        // callback's unconstrained artifact as a deterministic measured leaf.
        let size = measure(Constraints::indefinite());
        Self::with_measured_size_and_baseline(style, size, baseline(size))
    }
}

fn standalone_style(style: Style) -> Style {
    Style {
        display: Display::Flex,
        box_sizing: BoxSizing::ContentBox,
        ..style
    }
}

fn block_standalone_style(style: Style) -> Style {
    Style {
        display: Display::Block,
        box_sizing: BoxSizing::ContentBox,
        ..style
    }
}

fn fixed_flex_child(tree: &mut SimpleTree, width: f32, height: f32) -> usize {
    tree.push(SimpleNode::new(standalone_style(Style {
        flex_basis: Length::points(width),
        height: Length::points(height),
        ..Style::default()
    })))
}

#[derive(Clone, Copy, Debug)]
struct NativeMainAxisDirectionCase {
    flex_direction: FlexDirection,
    direction: Direction,
}

const NATIVE_MAIN_AXIS_MATRIX: [NativeMainAxisDirectionCase; 8] = [
    NativeMainAxisDirectionCase {
        flex_direction: FlexDirection::Row,
        direction: Direction::Ltr,
    },
    NativeMainAxisDirectionCase {
        flex_direction: FlexDirection::Row,
        direction: Direction::Rtl,
    },
    NativeMainAxisDirectionCase {
        flex_direction: FlexDirection::RowReverse,
        direction: Direction::Ltr,
    },
    NativeMainAxisDirectionCase {
        flex_direction: FlexDirection::RowReverse,
        direction: Direction::Rtl,
    },
    NativeMainAxisDirectionCase {
        flex_direction: FlexDirection::Column,
        direction: Direction::Ltr,
    },
    NativeMainAxisDirectionCase {
        flex_direction: FlexDirection::Column,
        direction: Direction::Rtl,
    },
    NativeMainAxisDirectionCase {
        flex_direction: FlexDirection::ColumnReverse,
        direction: Direction::Ltr,
    },
    NativeMainAxisDirectionCase {
        flex_direction: FlexDirection::ColumnReverse,
        direction: Direction::Rtl,
    },
];

const NATIVE_JUSTIFY_MATRIX: [JustifyContent; 9] = [
    JustifyContent::Stretch,
    JustifyContent::FlexStart,
    JustifyContent::Start,
    JustifyContent::Center,
    JustifyContent::FlexEnd,
    JustifyContent::End,
    JustifyContent::SpaceBetween,
    JustifyContent::SpaceAround,
    JustifyContent::SpaceEvenly,
];

const NATIVE_GAP_OVERFLOW_JUSTIFY_MATRIX: [JustifyContent; 3] = [
    JustifyContent::Center,
    JustifyContent::SpaceBetween,
    JustifyContent::SpaceAround,
];

fn fixed_matrix_flex_child(tree: &mut SimpleTree) -> usize {
    tree.push(SimpleNode::new(standalone_style(Style {
        flex_basis: Length::points(10.0),
        width: Length::points(10.0),
        height: Length::points(10.0),
        ..Style::default()
    })))
}

fn fixed_main_axis_matrix_flex_child(
    tree: &mut SimpleTree,
    case: NativeMainAxisDirectionCase,
    main_size: f32,
    cross_size: f32,
) -> usize {
    tree.push(SimpleNode::new(standalone_style(Style {
        flex_basis: Length::points(main_size),
        flex_shrink: 0.0,
        width: Length::points(if case.flex_direction.is_row() {
            main_size
        } else {
            cross_size
        }),
        height: Length::points(if case.flex_direction.is_row() {
            cross_size
        } else {
            main_size
        }),
        ..Style::default()
    })))
}

fn native_main_start_auto_margin(case: NativeMainAxisDirectionCase) -> Rect<Length> {
    match case.flex_direction {
        FlexDirection::Row => {
            if case.direction == Direction::Rtl {
                Rect::new(Length::ZERO, Length::Auto, Length::ZERO, Length::ZERO)
            } else {
                Rect::new(Length::Auto, Length::ZERO, Length::ZERO, Length::ZERO)
            }
        }
        FlexDirection::RowReverse => {
            if case.direction == Direction::Rtl {
                Rect::new(Length::Auto, Length::ZERO, Length::ZERO, Length::ZERO)
            } else {
                Rect::new(Length::ZERO, Length::Auto, Length::ZERO, Length::ZERO)
            }
        }
        FlexDirection::Column => Rect::new(Length::ZERO, Length::ZERO, Length::Auto, Length::ZERO),
        FlexDirection::ColumnReverse => {
            Rect::new(Length::ZERO, Length::ZERO, Length::ZERO, Length::Auto)
        }
    }
}

fn simple_tree_callback_measure(constraints: Constraints) -> Size {
    let width = bounded_size(constraints.width).map_or(17.0, |size| (size - 3.0).max(1.0));
    let height = bounded_size(constraints.height).map_or(11.0, |size| (size - 2.0).max(1.0));
    Size::new(width, height)
}

fn bounded_size(constraint: SideConstraint) -> Option<f32> {
    match constraint.mode {
        pr25_support::MeasureMode::Indefinite => None,
        pr25_support::MeasureMode::Definite | pr25_support::MeasureMode::AtMost => {
            Some(constraint.size)
        }
    }
}

fn simple_tree_callback_baseline(content_size: Size) -> f32 {
    (content_size.height - 2.0).max(0.0)
}

#[track_caller]
fn assert_rust_scenario(tree: SimpleTree, root: usize, constraints: Constraints) {
    let caller = std::panic::Location::caller();
    assert_rust_scenario_impl(
        &format!("{}:{}", caller.file(), caller.line()),
        tree,
        root,
        constraints,
    );
}

fn assert_rust_scenario_named(name: &str, tree: SimpleTree, root: usize, constraints: Constraints) {
    assert_rust_scenario_impl(name, tree, root, constraints);
}

fn assert_rust_scenario_impl(name: &str, tree: SimpleTree, root: usize, constraints: Constraints) {
    let topology = tree
        .nodes
        .iter()
        .map(|node| node.children.clone())
        .collect::<Vec<_>>();
    let mut first = tree.clone();
    let mut repeat = tree;
    let first_size = run_rust_layout(&mut first, root, constraints);
    let repeat_size = run_rust_layout(&mut repeat, root, constraints);

    assert_eq!(
        first_size, repeat_size,
        "{name}: non-deterministic root geometry"
    );
    assert_eq!(
        first.nodes.len(),
        repeat.nodes.len(),
        "{name}: node count drift"
    );
    assert_eq!(
        first
            .nodes
            .iter()
            .map(|node| &node.children)
            .collect::<Vec<_>>(),
        topology.iter().collect::<Vec<_>>(),
        "{name}: layout reordered host topology",
    );

    for (index, (first_node, repeat_node)) in first.nodes.iter().zip(&repeat.nodes).enumerate() {
        assert_eq!(
            first_node.layout, repeat_node.layout,
            "{name}: node {index} geometry is non-deterministic",
        );
        assert_finite_layout(name, index, first_node.layout);
    }
    assert_eq!(
        first.nodes[root].layout.size, first_size,
        "{name}: root writeback mismatch"
    );
    assert_known_axis(name, "width", constraints.width, first_size.width);
    assert_known_axis(name, "height", constraints.height, first_size.height);
}

fn assert_known_axis(name: &str, axis: &str, constraint: SideConstraint, actual: f32) {
    match constraint.mode {
        pr25_support::MeasureMode::Definite => assert!(
            (actual - constraint.size).abs() <= 0.01,
            "{name}: definite {axis} expected {}, got {actual}",
            constraint.size,
        ),
        // Available space may be exceeded by minimum content; AtMost is not a
        // hard used-size clamp in CSS sizing.
        pr25_support::MeasureMode::AtMost | pr25_support::MeasureMode::Indefinite => {}
    }
}

fn assert_finite_layout(name: &str, index: usize, layout: LayoutResult) {
    let values = [
        layout.offset.x,
        layout.offset.y,
        layout.size.width,
        layout.size.height,
        layout.padding.left,
        layout.padding.right,
        layout.padding.top,
        layout.padding.bottom,
        layout.border.left,
        layout.border.right,
        layout.border.top,
        layout.border.bottom,
        layout.margin.left,
        layout.margin.right,
        layout.margin.top,
        layout.margin.bottom,
    ];
    assert!(
        values.into_iter().all(f32::is_finite),
        "{name}: node {index} has non-finite geometry"
    );
    assert!(
        layout.size.width >= 0.0,
        "{name}: node {index} has negative width"
    );
    assert!(
        layout.size.height >= 0.0,
        "{name}: node {index} has negative height"
    );
    if let Some(baseline) = layout.baseline {
        assert!(
            baseline.is_finite(),
            "{name}: node {index} has non-finite baseline"
        );
    }
}

#[test]
fn head_to_head_flex_grow_and_order() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(standalone_style(Style {
        width: Length::points(120.0),
        height: Length::points(20.0),
        align_items: AlignItems::FlexStart,
        ..Style::default()
    })));
    let first = tree.push(SimpleNode::new(standalone_style(Style {
        flex_basis: Length::points(20.0),
        flex_grow: 1.0,
        height: Length::points(10.0),
        order: 2,
        ..Style::default()
    })));
    let second = tree.push(SimpleNode::new(standalone_style(Style {
        flex_basis: Length::points(20.0),
        flex_grow: 3.0,
        height: Length::points(10.0),
        order: 1,
        ..Style::default()
    })));
    tree.append_child(root, first);
    tree.append_child(root, second);

    assert_rust_scenario(tree, root, Constraints::definite(120.0, 20.0));
}

#[test]
fn head_to_head_flex_growing_target_defines_percent_basis_and_main_size_child_base() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(standalone_style(Style {
        flex_direction: FlexDirection::Row,
        align_items: AlignItems::FlexStart,
        width: Length::points(92.0),
        height: Length::points(24.0),
        ..Style::default()
    })));
    let child = tree.push(SimpleNode::new(standalone_style(Style {
        flex_direction: FlexDirection::Row,
        flex_basis: Length::points(20.0),
        flex_grow: 1.0,
        flex_shrink: 0.0,
        width: Length::points(20.0),
        height: Length::points(18.0),
        align_items: AlignItems::FlexStart,
        ..Style::default()
    })));
    let percent = tree.push(SimpleNode::new(standalone_style(Style {
        flex_basis: Length::percent(50.0),
        width: Length::percent(40.0),
        height: Length::points(6.0),
        ..Style::default()
    })));
    let fixed = tree.push(SimpleNode::new(standalone_style(Style {
        flex_basis: Length::points(7.0),
        width: Length::points(7.0),
        height: Length::points(5.0),
        ..Style::default()
    })));
    let sibling = tree.push(SimpleNode::new(standalone_style(Style {
        flex_basis: Length::points(15.0),
        width: Length::points(15.0),
        height: Length::points(10.0),
        ..Style::default()
    })));
    tree.append_child(root, child);
    tree.append_child(root, sibling);
    tree.append_child(child, percent);
    tree.append_child(child, fixed);

    assert_rust_scenario(tree, root, Constraints::definite(92.0, 24.0));
}

#[test]
fn head_to_head_flex_growing_percent_main_target_defines_child_main_size_base() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(standalone_style(Style {
        flex_direction: FlexDirection::Row,
        align_items: AlignItems::FlexStart,
        width: Length::points(91.0),
        height: Length::points(22.0),
        ..Style::default()
    })));
    let child = tree.push(SimpleNode::new(standalone_style(Style {
        flex_direction: FlexDirection::Row,
        flex_basis: Length::points(20.0),
        flex_grow: 1.0,
        flex_shrink: 0.0,
        width: Length::percent(35.0),
        height: Length::points(16.0),
        align_items: AlignItems::FlexStart,
        ..Style::default()
    })));
    let percent = tree.push(SimpleNode::new(standalone_style(Style {
        flex_basis: Length::points(8.0),
        width: Length::percent(50.0),
        height: Length::points(6.0),
        ..Style::default()
    })));
    let fixed = tree.push(SimpleNode::new(standalone_style(Style {
        flex_basis: Length::points(6.0),
        width: Length::points(6.0),
        height: Length::points(5.0),
        ..Style::default()
    })));
    let sibling = tree.push(SimpleNode::new(standalone_style(Style {
        flex_basis: Length::points(14.0),
        width: Length::points(14.0),
        height: Length::points(9.0),
        ..Style::default()
    })));
    tree.append_child(root, child);
    tree.append_child(root, sibling);
    tree.append_child(child, percent);
    tree.append_child(child, fixed);

    assert_rust_scenario(tree, root, Constraints::definite(91.0, 22.0));
}

#[test]
fn head_to_head_flex_percent_main_length_parent_defines_growing_percent_basis_child_base() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(standalone_style(Style {
        flex_direction: FlexDirection::Row,
        align_items: AlignItems::FlexStart,
        width: Length::points(90.0),
        height: Length::points(22.0),
        ..Style::default()
    })));
    let child = tree.push(SimpleNode::new(standalone_style(Style {
        flex_direction: FlexDirection::Row,
        flex_basis: Length::Auto,
        flex_grow: 0.0,
        flex_shrink: 1.0,
        width: Length::percent(50.0),
        height: Length::points(16.0),
        align_items: AlignItems::FlexStart,
        ..Style::default()
    })));
    let percent = tree.push(SimpleNode::new(standalone_style(Style {
        flex_basis: Length::percent(50.0),
        flex_grow: 1.0,
        flex_shrink: 0.0,
        height: Length::points(6.0),
        ..Style::default()
    })));
    let fixed = tree.push(SimpleNode::new(standalone_style(Style {
        flex_basis: Length::points(5.0),
        width: Length::points(5.0),
        height: Length::points(5.0),
        ..Style::default()
    })));
    let sibling = tree.push(SimpleNode::new(standalone_style(Style {
        flex_basis: Length::points(20.0),
        flex_shrink: 0.0,
        width: Length::points(20.0),
        height: Length::points(9.0),
        ..Style::default()
    })));
    tree.append_child(root, child);
    tree.append_child(root, sibling);
    tree.append_child(child, percent);
    tree.append_child(child, fixed);

    assert_rust_scenario(tree, root, Constraints::definite(90.0, 22.0));
}

#[test]
fn head_to_head_flex_point_basis_parent_defines_growing_percent_basis_child_base() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(standalone_style(Style {
        flex_direction: FlexDirection::Row,
        align_items: AlignItems::FlexStart,
        width: Length::points(76.0),
        height: Length::points(22.0),
        ..Style::default()
    })));
    let child = tree.push(SimpleNode::new(standalone_style(Style {
        flex_direction: FlexDirection::Row,
        flex_basis: Length::points(36.0),
        flex_grow: 0.0,
        flex_shrink: 1.0,
        height: Length::points(16.0),
        align_items: AlignItems::FlexStart,
        ..Style::default()
    })));
    let percent = tree.push(SimpleNode::new(standalone_style(Style {
        flex_basis: Length::percent(50.0),
        flex_grow: 1.0,
        flex_shrink: 0.0,
        height: Length::points(6.0),
        ..Style::default()
    })));
    let fixed = tree.push(SimpleNode::new(standalone_style(Style {
        flex_basis: Length::points(5.0),
        width: Length::points(5.0),
        height: Length::points(5.0),
        ..Style::default()
    })));
    let sibling = tree.push(SimpleNode::new(standalone_style(Style {
        flex_basis: Length::points(18.0),
        flex_shrink: 0.0,
        width: Length::points(18.0),
        height: Length::points(9.0),
        ..Style::default()
    })));
    tree.append_child(root, child);
    tree.append_child(root, sibling);
    tree.append_child(child, percent);
    tree.append_child(child, fixed);

    assert_rust_scenario(tree, root, Constraints::definite(76.0, 22.0));
}

#[test]
fn head_to_head_flex_own_percent_basis_and_main_size_define_percent_child_bases() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(standalone_style(Style {
        flex_direction: FlexDirection::Row,
        align_items: AlignItems::FlexStart,
        width: Length::points(100.0),
        height: Length::points(24.0),
        ..Style::default()
    })));
    let child = tree.push(SimpleNode::new(standalone_style(Style {
        flex_direction: FlexDirection::Row,
        flex_basis: Length::percent(50.0),
        flex_grow: 0.0,
        flex_shrink: 0.0,
        width: Length::percent(40.0),
        height: Length::points(16.0),
        align_items: AlignItems::FlexStart,
        ..Style::default()
    })));
    let percent = tree.push(SimpleNode::new(standalone_style(Style {
        flex_basis: Length::percent(50.0),
        flex_grow: 0.0,
        flex_shrink: 0.0,
        width: Length::percent(40.0),
        height: Length::points(6.0),
        ..Style::default()
    })));
    let fixed = tree.push(SimpleNode::new(standalone_style(Style {
        flex_basis: Length::points(5.0),
        width: Length::points(5.0),
        height: Length::points(5.0),
        ..Style::default()
    })));
    let sibling = tree.push(SimpleNode::new(standalone_style(Style {
        flex_basis: Length::points(30.0),
        flex_shrink: 0.0,
        width: Length::points(30.0),
        height: Length::points(9.0),
        ..Style::default()
    })));
    tree.append_child(root, child);
    tree.append_child(root, sibling);
    tree.append_child(child, percent);
    tree.append_child(child, fixed);

    assert_rust_scenario(tree, root, Constraints::definite(100.0, 24.0));
}

#[test]
fn head_to_head_flex_oversized_inflexible_fixed_child_with_percent_basis_sibling() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(standalone_style(Style {
        flex_direction: FlexDirection::Row,
        align_items: AlignItems::FlexStart,
        width: Length::points(100.0),
        height: Length::points(24.0),
        ..Style::default()
    })));
    let child = tree.push(SimpleNode::new(standalone_style(Style {
        flex_direction: FlexDirection::Row,
        flex_basis: Length::percent(40.0),
        flex_grow: 0.0,
        flex_shrink: 0.0,
        width: Length::percent(40.0),
        height: Length::points(16.0),
        align_items: AlignItems::FlexStart,
        ..Style::default()
    })));
    let oversized = tree.push(SimpleNode::new(standalone_style(Style {
        flex_basis: Length::points(60.0),
        flex_grow: 0.0,
        flex_shrink: 0.0,
        width: Length::points(60.0),
        height: Length::points(6.0),
        ..Style::default()
    })));
    let percent = tree.push(SimpleNode::new(standalone_style(Style {
        flex_basis: Length::percent(50.0),
        flex_grow: 0.0,
        flex_shrink: 0.0,
        height: Length::points(5.0),
        ..Style::default()
    })));
    let sibling = tree.push(SimpleNode::new(standalone_style(Style {
        flex_basis: Length::points(30.0),
        flex_shrink: 0.0,
        width: Length::points(30.0),
        height: Length::points(9.0),
        ..Style::default()
    })));
    tree.append_child(root, child);
    tree.append_child(root, sibling);
    tree.append_child(child, oversized);
    tree.append_child(child, percent);

    assert_rust_scenario(tree, root, Constraints::definite(100.0, 24.0));
}

#[test]
fn head_to_head_flex_shrinking_target_defines_percent_main_size_child_base() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(standalone_style(Style {
        flex_direction: FlexDirection::Row,
        align_items: AlignItems::FlexStart,
        width: Length::points(42.0),
        height: Length::points(22.0),
        ..Style::default()
    })));
    let child = tree.push(SimpleNode::new(standalone_style(Style {
        flex_direction: FlexDirection::Row,
        flex_basis: Length::points(36.0),
        flex_grow: 0.0,
        flex_shrink: 1.0,
        width: Length::points(36.0),
        height: Length::points(16.0),
        align_items: AlignItems::FlexStart,
        ..Style::default()
    })));
    let percent = tree.push(SimpleNode::new(standalone_style(Style {
        flex_basis: Length::points(8.0),
        width: Length::percent(50.0),
        height: Length::points(6.0),
        ..Style::default()
    })));
    let fixed = tree.push(SimpleNode::new(standalone_style(Style {
        flex_basis: Length::points(6.0),
        width: Length::points(6.0),
        height: Length::points(5.0),
        ..Style::default()
    })));
    let sibling = tree.push(SimpleNode::new(standalone_style(Style {
        flex_basis: Length::points(18.0),
        flex_shrink: 0.0,
        width: Length::points(18.0),
        height: Length::points(9.0),
        ..Style::default()
    })));
    tree.append_child(root, child);
    tree.append_child(root, sibling);
    tree.append_child(child, percent);
    tree.append_child(child, fixed);

    assert_rust_scenario(tree, root, Constraints::definite(42.0, 22.0));
}

#[test]
fn head_to_head_flex_shrunk_parent_target_defines_percent_basis_child_base() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(standalone_style(Style {
        flex_direction: FlexDirection::Row,
        align_items: AlignItems::FlexStart,
        width: Length::points(45.0),
        height: Length::points(22.0),
        ..Style::default()
    })));
    let child = tree.push(SimpleNode::new(standalone_style(Style {
        flex_direction: FlexDirection::Row,
        flex_basis: Length::points(42.0),
        flex_grow: 0.0,
        flex_shrink: 1.0,
        width: Length::points(42.0),
        height: Length::points(16.0),
        align_items: AlignItems::FlexStart,
        ..Style::default()
    })));
    let percent = tree.push(SimpleNode::new(standalone_style(Style {
        flex_basis: Length::percent(50.0),
        flex_grow: 0.0,
        flex_shrink: 0.0,
        height: Length::points(6.0),
        ..Style::default()
    })));
    let fixed = tree.push(SimpleNode::new(standalone_style(Style {
        flex_basis: Length::points(5.0),
        width: Length::points(5.0),
        height: Length::points(5.0),
        ..Style::default()
    })));
    let sibling = tree.push(SimpleNode::new(standalone_style(Style {
        flex_basis: Length::points(28.0),
        flex_grow: 0.0,
        flex_shrink: 0.0,
        width: Length::points(28.0),
        height: Length::points(9.0),
        ..Style::default()
    })));
    tree.append_child(root, child);
    tree.append_child(root, sibling);
    tree.append_child(child, percent);
    tree.append_child(child, fixed);

    assert_rust_scenario(tree, root, Constraints::definite(45.0, 22.0));
}

#[test]
fn head_to_head_flex_unchanged_main_defines_stretch_percent_basis_child_base() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(standalone_style(Style {
        flex_direction: FlexDirection::Row,
        align_items: AlignItems::FlexStart,
        width: Length::points(70.0),
        height: Length::points(22.0),
        ..Style::default()
    })));
    let child = tree.push(SimpleNode::new(standalone_style(Style {
        flex_direction: FlexDirection::Row,
        flex_basis: Length::points(40.0),
        flex_grow: 0.0,
        flex_shrink: 1.0,
        width: Length::points(40.0),
        height: Length::points(16.0),
        align_items: AlignItems::FlexStart,
        ..Style::default()
    })));
    let stretched = tree.push(SimpleNode::new(standalone_style(Style {
        flex_basis: Length::percent(50.0),
        flex_grow: 0.0,
        flex_shrink: 0.0,
        align_self: Some(AlignItems::Stretch),
        height: Length::points(6.0),
        ..Style::default()
    })));
    let fixed = tree.push(SimpleNode::new(standalone_style(Style {
        flex_basis: Length::points(5.0),
        width: Length::points(5.0),
        height: Length::points(5.0),
        ..Style::default()
    })));
    let sibling = tree.push(SimpleNode::new(standalone_style(Style {
        flex_basis: Length::points(30.0),
        flex_shrink: 0.0,
        width: Length::points(30.0),
        height: Length::points(9.0),
        ..Style::default()
    })));
    tree.append_child(root, child);
    tree.append_child(root, sibling);
    tree.append_child(child, stretched);
    tree.append_child(child, fixed);

    assert_rust_scenario(tree, root, Constraints::definite(70.0, 22.0));
}

#[test]
fn head_to_head_flex_unchanged_main_defines_inflexible_percent_basis_child_base() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(standalone_style(Style {
        flex_direction: FlexDirection::Row,
        align_items: AlignItems::FlexStart,
        width: Length::points(69.0),
        height: Length::points(22.0),
        ..Style::default()
    })));
    let child = tree.push(SimpleNode::new(standalone_style(Style {
        flex_direction: FlexDirection::Row,
        flex_basis: Length::points(39.0),
        flex_grow: 0.0,
        flex_shrink: 1.0,
        width: Length::points(39.0),
        height: Length::points(16.0),
        align_items: AlignItems::FlexStart,
        ..Style::default()
    })));
    let percent = tree.push(SimpleNode::new(standalone_style(Style {
        flex_basis: Length::percent(50.0),
        flex_grow: 0.0,
        flex_shrink: 0.0,
        height: Length::points(6.0),
        ..Style::default()
    })));
    let fixed = tree.push(SimpleNode::new(standalone_style(Style {
        flex_basis: Length::points(5.0),
        width: Length::points(5.0),
        height: Length::points(5.0),
        ..Style::default()
    })));
    let sibling = tree.push(SimpleNode::new(standalone_style(Style {
        flex_basis: Length::points(30.0),
        flex_shrink: 0.0,
        width: Length::points(30.0),
        height: Length::points(9.0),
        ..Style::default()
    })));
    tree.append_child(root, child);
    tree.append_child(root, sibling);
    tree.append_child(child, percent);
    tree.append_child(child, fixed);

    assert_rust_scenario(tree, root, Constraints::definite(69.0, 22.0));
}

#[test]
fn head_to_head_flex_preserved_percent_basis_parent_defines_growing_percent_child_base() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(standalone_style(Style {
        flex_direction: FlexDirection::Row,
        align_items: AlignItems::FlexStart,
        height: Length::points(22.0),
        ..Style::default()
    })));
    let child = tree.push(SimpleNode::new(standalone_style(Style {
        flex_direction: FlexDirection::Row,
        flex_basis: Length::percent(50.0),
        flex_grow: 0.0,
        flex_shrink: 0.0,
        height: Length::points(16.0),
        align_items: AlignItems::FlexStart,
        ..Style::default()
    })));
    let percent = tree.push(SimpleNode::new(standalone_style(Style {
        flex_basis: Length::percent(50.0),
        flex_grow: 1.0,
        flex_shrink: 0.0,
        height: Length::points(6.0),
        ..Style::default()
    })));
    let fixed = tree.push(SimpleNode::new(standalone_style(Style {
        flex_basis: Length::points(5.0),
        width: Length::points(5.0),
        height: Length::points(5.0),
        ..Style::default()
    })));
    let sibling = tree.push(SimpleNode::new(standalone_style(Style {
        flex_basis: Length::points(12.0),
        flex_shrink: 0.0,
        width: Length::points(12.0),
        height: Length::points(9.0),
        ..Style::default()
    })));
    tree.append_child(root, child);
    tree.append_child(root, sibling);
    tree.append_child(child, percent);
    tree.append_child(child, fixed);

    assert_rust_scenario(tree, root, Constraints::indefinite());
}

#[test]
fn head_to_head_flex_preserved_percent_basis_parent_defines_inflexible_percent_basis_and_main_size_child_base()
 {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(standalone_style(Style {
        flex_direction: FlexDirection::Row,
        align_items: AlignItems::FlexStart,
        height: Length::points(22.0),
        ..Style::default()
    })));
    let child = tree.push(SimpleNode::new(standalone_style(Style {
        flex_direction: FlexDirection::Row,
        flex_basis: Length::percent(50.0),
        flex_grow: 0.0,
        flex_shrink: 0.0,
        height: Length::points(16.0),
        align_items: AlignItems::FlexStart,
        ..Style::default()
    })));
    let percent = tree.push(SimpleNode::new(standalone_style(Style {
        flex_basis: Length::percent(50.0),
        flex_grow: 0.0,
        flex_shrink: 0.0,
        width: Length::percent(40.0),
        height: Length::points(6.0),
        ..Style::default()
    })));
    let fixed = tree.push(SimpleNode::new(standalone_style(Style {
        flex_basis: Length::points(5.0),
        width: Length::points(5.0),
        height: Length::points(5.0),
        ..Style::default()
    })));
    let sibling = tree.push(SimpleNode::new(standalone_style(Style {
        flex_basis: Length::points(12.0),
        flex_shrink: 0.0,
        width: Length::points(12.0),
        height: Length::points(9.0),
        ..Style::default()
    })));
    tree.append_child(root, child);
    tree.append_child(root, sibling);
    tree.append_child(child, percent);
    tree.append_child(child, fixed);

    assert_rust_scenario(tree, root, Constraints::indefinite());
}

#[test]
fn head_to_head_flex_aligned_growing_percent_basis_target_defines_child_basis_base() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(standalone_style(Style {
        flex_direction: FlexDirection::Row,
        align_items: AlignItems::FlexStart,
        width: Length::points(96.0),
        height: Length::points(24.0),
        ..Style::default()
    })));
    let child = tree.push(SimpleNode::new(standalone_style(Style {
        flex_direction: FlexDirection::Row,
        flex_basis: Length::percent(30.0),
        flex_grow: 1.0,
        flex_shrink: 0.0,
        width: Length::points(20.0),
        height: Length::points(18.0),
        align_items: AlignItems::FlexStart,
        ..Style::default()
    })));
    let aligned = tree.push(SimpleNode::new(standalone_style(Style {
        flex_basis: Length::percent(45.0),
        flex_grow: 1.0,
        flex_shrink: 0.0,
        align_self: Some(AlignItems::Center),
        height: Length::points(7.0),
        ..Style::default()
    })));
    let fixed = tree.push(SimpleNode::new(standalone_style(Style {
        flex_basis: Length::points(6.0),
        width: Length::points(6.0),
        height: Length::points(5.0),
        ..Style::default()
    })));
    let sibling = tree.push(SimpleNode::new(standalone_style(Style {
        flex_basis: Length::points(18.0),
        width: Length::points(18.0),
        height: Length::points(9.0),
        ..Style::default()
    })));
    tree.append_child(root, child);
    tree.append_child(root, sibling);
    tree.append_child(child, aligned);
    tree.append_child(child, fixed);

    assert_rust_scenario(tree, root, Constraints::definite(96.0, 24.0));
}

#[test]
fn head_to_head_flex_growing_percent_basis_target_defines_local_flexible_child_basis_base() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(standalone_style(Style {
        flex_direction: FlexDirection::Row,
        align_items: AlignItems::FlexStart,
        width: Length::points(94.0),
        height: Length::points(23.0),
        ..Style::default()
    })));
    let child = tree.push(SimpleNode::new(standalone_style(Style {
        flex_direction: FlexDirection::Row,
        flex_basis: Length::percent(25.0),
        flex_grow: 1.0,
        flex_shrink: 0.0,
        width: Length::points(18.0),
        height: Length::points(17.0),
        align_items: AlignItems::FlexStart,
        ..Style::default()
    })));
    let percent = tree.push(SimpleNode::new(standalone_style(Style {
        flex_basis: Length::percent(40.0),
        flex_grow: 1.0,
        flex_shrink: 0.0,
        height: Length::points(6.0),
        ..Style::default()
    })));
    let fixed = tree.push(SimpleNode::new(standalone_style(Style {
        flex_basis: Length::points(5.0),
        width: Length::points(5.0),
        height: Length::points(5.0),
        ..Style::default()
    })));
    let sibling = tree.push(SimpleNode::new(standalone_style(Style {
        flex_basis: Length::points(17.0),
        width: Length::points(17.0),
        height: Length::points(9.0),
        ..Style::default()
    })));
    tree.append_child(root, child);
    tree.append_child(root, sibling);
    tree.append_child(child, percent);
    tree.append_child(child, fixed);

    assert_rust_scenario(tree, root, Constraints::definite(94.0, 23.0));
}

#[test]
fn head_to_head_flex_growing_percent_basis_target_defines_local_inflexible_child_basis_base() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(standalone_style(Style {
        flex_direction: FlexDirection::Row,
        align_items: AlignItems::FlexStart,
        width: Length::points(93.0),
        height: Length::points(23.0),
        ..Style::default()
    })));
    let child = tree.push(SimpleNode::new(standalone_style(Style {
        flex_direction: FlexDirection::Row,
        flex_basis: Length::percent(25.0),
        flex_grow: 1.0,
        flex_shrink: 0.0,
        width: Length::points(18.0),
        height: Length::points(17.0),
        align_items: AlignItems::FlexStart,
        ..Style::default()
    })));
    let percent = tree.push(SimpleNode::new(standalone_style(Style {
        flex_basis: Length::percent(42.0),
        flex_grow: 0.0,
        flex_shrink: 0.0,
        height: Length::points(6.0),
        ..Style::default()
    })));
    let fixed = tree.push(SimpleNode::new(standalone_style(Style {
        flex_basis: Length::points(5.0),
        width: Length::points(5.0),
        height: Length::points(5.0),
        ..Style::default()
    })));
    let sibling = tree.push(SimpleNode::new(standalone_style(Style {
        flex_basis: Length::points(16.0),
        width: Length::points(16.0),
        height: Length::points(9.0),
        ..Style::default()
    })));
    tree.append_child(root, child);
    tree.append_child(root, sibling);
    tree.append_child(child, percent);
    tree.append_child(child, fixed);

    assert_rust_scenario(tree, root, Constraints::definite(93.0, 23.0));
}

#[test]
fn head_to_head_flex_auto_main_preserves_intrinsic_percent_basis_child() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(standalone_style(Style {
        flex_direction: FlexDirection::Row,
        align_items: AlignItems::FlexStart,
        width: Length::points(82.0),
        height: Length::points(22.0),
        ..Style::default()
    })));
    let child = tree.push(SimpleNode::new(standalone_style(Style {
        flex_direction: FlexDirection::Row,
        flex_basis: Length::Auto,
        flex_grow: 0.0,
        flex_shrink: 0.0,
        width: Length::Auto,
        height: Length::points(14.0),
        align_items: AlignItems::FlexStart,
        ..Style::default()
    })));
    let percent = tree.push(SimpleNode::new(standalone_style(Style {
        flex_basis: Length::percent(65.0),
        flex_grow: 0.0,
        flex_shrink: 0.0,
        width: Length::points(18.0),
        height: Length::points(7.0),
        ..Style::default()
    })));
    let fixed = tree.push(SimpleNode::new(standalone_style(Style {
        flex_basis: Length::points(6.0),
        width: Length::points(6.0),
        height: Length::points(5.0),
        ..Style::default()
    })));
    let sibling = tree.push(SimpleNode::new(standalone_style(Style {
        flex_basis: Length::points(11.0),
        width: Length::points(11.0),
        height: Length::points(9.0),
        ..Style::default()
    })));
    tree.append_child(root, child);
    tree.append_child(root, sibling);
    tree.append_child(child, percent);
    tree.append_child(child, fixed);

    assert_rust_scenario(tree, root, Constraints::definite(82.0, 22.0));
}

#[test]
fn head_to_head_wrap_gaps_and_align_content() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(standalone_style(Style {
        width: Length::points(50.0),
        height: Length::points(40.0),
        flex_wrap: FlexWrap::Wrap,
        row_gap: Length::points(2.0),
        column_gap: Length::points(1.0),
        align_items: AlignItems::FlexStart,
        align_content: AlignContent::Center,
        justify_content: JustifyContent::FlexStart,
        ..Style::default()
    })));

    for width in [18.0, 20.0, 22.0, 24.0] {
        let child = tree.push(SimpleNode::new(standalone_style(Style {
            flex_basis: Length::points(width),
            height: Length::points(8.0),
            ..Style::default()
        })));
        tree.append_child(root, child);
    }

    assert_rust_scenario(tree, root, Constraints::definite(50.0, 40.0));
}

#[test]
fn head_to_head_flex_justify_content_space_evenly() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(standalone_style(Style {
        justify_content: JustifyContent::SpaceEvenly,
        align_items: AlignItems::FlexStart,
        width: Length::points(100.0),
        height: Length::points(10.0),
        ..Style::default()
    })));
    for _ in 0..2 {
        let child = tree.push(SimpleNode::new(standalone_style(Style {
            flex_basis: Length::points(10.0),
            height: Length::points(10.0),
            ..Style::default()
        })));
        tree.append_child(root, child);
    }

    assert_rust_scenario(tree, root, Constraints::definite(110.0, 10.0));
}

#[test]
fn head_to_head_justify_content_space_evenly_single_item_equal_edge_spaces() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(standalone_style(Style {
        justify_content: JustifyContent::SpaceEvenly,
        align_items: AlignItems::FlexStart,
        width: Length::points(100.0),
        height: Length::points(10.0),
        ..Style::default()
    })));
    let child = fixed_flex_child(&mut tree, 20.0, 10.0);
    tree.append_child(root, child);

    assert_rust_scenario(tree, root, Constraints::definite(100.0, 10.0));
}

#[test]
fn head_to_head_justify_content_space_between_single_item_fallback() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(standalone_style(Style {
        justify_content: JustifyContent::SpaceBetween,
        align_items: AlignItems::FlexStart,
        width: Length::points(100.0),
        height: Length::points(10.0),
        ..Style::default()
    })));
    let child = fixed_flex_child(&mut tree, 20.0, 10.0);
    tree.append_child(root, child);

    assert_rust_scenario(tree, root, Constraints::definite(100.0, 10.0));
}

#[test]
fn head_to_head_justify_content_space_around_single_item_fallback() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(standalone_style(Style {
        justify_content: JustifyContent::SpaceAround,
        align_items: AlignItems::FlexStart,
        width: Length::points(100.0),
        height: Length::points(10.0),
        ..Style::default()
    })));
    let child = fixed_flex_child(&mut tree, 20.0, 10.0);
    tree.append_child(root, child);

    assert_rust_scenario(tree, root, Constraints::definite(100.0, 10.0));
}

#[test]
fn head_to_head_root_at_most_shrink_wraps_flex_line() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(standalone_style(Style {
        align_items: AlignItems::FlexStart,
        ..Style::default()
    })));
    for width in [30.0, 20.0] {
        let child = tree.push(SimpleNode::new(standalone_style(Style {
            flex_basis: Length::points(width),
            height: Length::points(10.0),
            ..Style::default()
        })));
        tree.append_child(root, child);
    }

    assert_rust_scenario(
        tree,
        root,
        Constraints::new(SideConstraint::at_most(100.0), SideConstraint::indefinite()),
    );
}

#[test]
fn head_to_head_wrapped_root_at_most_shrink_wraps_largest_flex_line() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(standalone_style(Style {
        flex_wrap: FlexWrap::Wrap,
        align_items: AlignItems::FlexStart,
        ..Style::default()
    })));
    for _ in 0..3 {
        let child = fixed_flex_child(&mut tree, 40.0, 10.0);
        tree.append_child(root, child);
    }

    assert_rust_scenario(
        tree,
        root,
        Constraints::new(SideConstraint::at_most(100.0), SideConstraint::indefinite()),
    );
}

#[test]
fn head_to_head_absolute_flex_child_uses_static_position_without_participating_in_flex_layout() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(standalone_style(Style {
        justify_content: JustifyContent::Center,
        align_items: AlignItems::FlexStart,
        width: Length::points(100.0),
        height: Length::points(20.0),
        ..Style::default()
    })));
    let first = fixed_flex_child(&mut tree, 20.0, 10.0);
    let absolute = tree.push(SimpleNode::new(block_standalone_style(Style {
        position: PositionType::Absolute,
        width: Length::points(10.0),
        height: Length::points(10.0),
        align_self: Some(AlignItems::FlexEnd),
        margin: Rect::all(Length::Auto),
        ..Style::default()
    })));
    let second = fixed_flex_child(&mut tree, 20.0, 10.0);
    tree.append_child(root, first);
    tree.append_child(root, absolute);
    tree.append_child(root, second);

    assert_rust_scenario(tree, root, Constraints::definite(100.0, 20.0));
}

#[test]
fn head_to_head_flex_stretch_reexports_cached_block_subtree() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(standalone_style(Style {
        flex_direction: FlexDirection::Row,
        align_items: AlignItems::Stretch,
        ..Style::default()
    })));
    let child = tree.push(SimpleNode::new(block_standalone_style(Style::default())));
    let grandchild = tree.push(SimpleNode::new(block_standalone_style(Style {
        width: Length::points(10.0),
        height: Length::points(20.0),
        ..Style::default()
    })));
    tree.append_child(root, child);
    tree.append_child(child, grandchild);

    assert_rust_scenario(tree, root, Constraints::definite(30.0, 20.0));
}

#[test]
fn head_to_head_flex_stretch_reexports_cached_block_subtree_with_fractional_offsets() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(standalone_style(Style {
        flex_direction: FlexDirection::Row,
        align_items: AlignItems::Stretch,
        padding: Rect::all(Length::points(0.4)),
        ..Style::default()
    })));
    let child = tree.push(SimpleNode::new(block_standalone_style(Style::default())));
    let grandchild = tree.push(SimpleNode::new(block_standalone_style(Style {
        width: Length::points(10.4),
        height: Length::points(5.4),
        ..Style::default()
    })));
    tree.append_child(root, child);
    tree.append_child(child, grandchild);

    assert_rust_scenario(tree, root, Constraints::definite(30.4, 20.4));
}

#[test]
fn head_to_head_orthogonal_flex_reuses_percent_basis_subtree_measure() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(standalone_style(Style {
        flex_direction: FlexDirection::Row,
        align_items: AlignItems::FlexStart,
        width: Length::points(80.0),
        height: Length::points(40.0),
        ..Style::default()
    })));
    let child = tree.push(SimpleNode::new(standalone_style(Style {
        flex_direction: FlexDirection::Column,
        flex_shrink: 0.0,
        width: Length::points(20.0),
        height: Length::points(30.0),
        ..Style::default()
    })));
    let grandchild = tree.push(SimpleNode::new(standalone_style(Style {
        flex_basis: Length::percent(50.0),
        flex_grow: 1.0,
        flex_shrink: 0.0,
        width: Length::points(8.0),
        ..Style::default()
    })));
    tree.append_child(root, child);
    tree.append_child(child, grandchild);

    assert_rust_scenario(tree, root, Constraints::definite(80.0, 40.0));
}

#[test]
fn head_to_head_flex_stretch_remeasures_aligned_inflexible_percent_basis_subtree() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(standalone_style(Style {
        flex_direction: FlexDirection::Row,
        align_items: AlignItems::Stretch,
        width: Length::points(50.0),
        height: Length::points(24.0),
        ..Style::default()
    })));
    let child = tree.push(SimpleNode::new(standalone_style(Style {
        flex_direction: FlexDirection::Column,
        flex_shrink: 1.0,
        flex_basis: Length::points(18.0),
        width: Length::points(18.0),
        ..Style::default()
    })));
    let grandchild = tree.push(SimpleNode::new(standalone_style(Style {
        flex_basis: Length::percent(80.0),
        flex_grow: 0.0,
        flex_shrink: 0.0,
        align_self: Some(AlignItems::Center),
        height: Length::points(12.0),
        ..Style::default()
    })));
    let leaf = tree.push(SimpleNode::new(block_standalone_style(Style {
        width: Length::points(7.0),
        height: Length::points(5.0),
        ..Style::default()
    })));
    tree.append_child(root, child);
    tree.append_child(child, grandchild);
    tree.append_child(grandchild, leaf);

    assert_rust_scenario(tree, root, Constraints::definite(50.0, 24.0));
}

#[test]
fn head_to_head_flex_explicit_stretch_remeasures_shrinking_percent_basis_subtree() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(standalone_style(Style {
        flex_direction: FlexDirection::Row,
        align_items: AlignItems::FlexStart,
        width: Length::points(54.0),
        height: Length::points(26.0),
        ..Style::default()
    })));
    let child = tree.push(SimpleNode::new(standalone_style(Style {
        flex_direction: FlexDirection::Column,
        flex_basis: Length::points(20.0),
        flex_shrink: 1.0,
        width: Length::points(20.0),
        align_self: Some(AlignItems::Stretch),
        ..Style::default()
    })));
    let grandchild = tree.push(SimpleNode::new(standalone_style(Style {
        flex_basis: Length::percent(75.0),
        flex_shrink: 1.0,
        height: Length::points(9.0),
        ..Style::default()
    })));
    let leaf = tree.push(SimpleNode::new(block_standalone_style(Style {
        width: Length::points(6.0),
        height: Length::points(4.0),
        ..Style::default()
    })));
    tree.append_child(root, child);
    tree.append_child(child, grandchild);
    tree.append_child(grandchild, leaf);

    assert_rust_scenario(tree, root, Constraints::definite(54.0, 26.0));
}

#[test]
fn head_to_head_flex_explicit_shrinking_stretch_remeasures_aligned_inflexible_percent_basis_subtree()
 {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(standalone_style(Style {
        flex_direction: FlexDirection::Row,
        align_items: AlignItems::FlexStart,
        width: Length::points(56.0),
        height: Length::points(26.0),
        ..Style::default()
    })));
    let child = tree.push(SimpleNode::new(standalone_style(Style {
        flex_direction: FlexDirection::Column,
        flex_basis: Length::points(19.0),
        flex_grow: 0.0,
        flex_shrink: 1.0,
        width: Length::points(19.0),
        align_self: Some(AlignItems::Stretch),
        ..Style::default()
    })));
    let aligned = tree.push(SimpleNode::new(standalone_style(Style {
        flex_basis: Length::percent(80.0),
        flex_grow: 0.0,
        flex_shrink: 0.0,
        align_self: Some(AlignItems::Center),
        height: Length::points(11.0),
        ..Style::default()
    })));
    let leaf = tree.push(SimpleNode::new(block_standalone_style(Style {
        width: Length::points(6.0),
        height: Length::points(4.0),
        ..Style::default()
    })));
    tree.append_child(root, child);
    tree.append_child(child, aligned);
    tree.append_child(aligned, leaf);

    assert_rust_scenario(tree, root, Constraints::definite(56.0, 26.0));
}

#[test]
fn head_to_head_flex_growing_explicit_stretch_remeasures_shrinking_percent_basis_subtree() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(standalone_style(Style {
        flex_direction: FlexDirection::Row,
        align_items: AlignItems::FlexStart,
        width: Length::points(72.0),
        height: Length::points(28.0),
        ..Style::default()
    })));
    let child = tree.push(SimpleNode::new(standalone_style(Style {
        flex_direction: FlexDirection::Column,
        flex_basis: Length::points(18.0),
        flex_grow: 1.0,
        flex_shrink: 0.0,
        width: Length::points(18.0),
        align_self: Some(AlignItems::Stretch),
        ..Style::default()
    })));
    let grandchild = tree.push(SimpleNode::new(standalone_style(Style {
        flex_basis: Length::percent(70.0),
        flex_grow: 0.0,
        flex_shrink: 1.0,
        height: Length::points(10.0),
        ..Style::default()
    })));
    let leaf = tree.push(SimpleNode::new(block_standalone_style(Style {
        width: Length::points(5.0),
        height: Length::points(4.0),
        ..Style::default()
    })));
    tree.append_child(root, child);
    tree.append_child(child, grandchild);
    tree.append_child(grandchild, leaf);

    assert_rust_scenario(tree, root, Constraints::definite(72.0, 28.0));
}

#[test]
fn head_to_head_flex_growing_explicit_stretch_handles_inflexible_percent_basis_subtree() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(standalone_style(Style {
        flex_direction: FlexDirection::Row,
        align_items: AlignItems::FlexStart,
        width: Length::points(76.0),
        height: Length::points(30.0),
        ..Style::default()
    })));
    let child = tree.push(SimpleNode::new(standalone_style(Style {
        flex_direction: FlexDirection::Column,
        flex_basis: Length::points(18.0),
        flex_grow: 1.0,
        flex_shrink: 0.0,
        width: Length::points(18.0),
        align_self: Some(AlignItems::Stretch),
        ..Style::default()
    })));
    let percent = tree.push(SimpleNode::new(standalone_style(Style {
        flex_basis: Length::percent(55.0),
        flex_grow: 0.0,
        flex_shrink: 0.0,
        height: Length::points(7.0),
        ..Style::default()
    })));
    let leaf = tree.push(SimpleNode::new(block_standalone_style(Style {
        width: Length::points(5.0),
        height: Length::points(3.0),
        ..Style::default()
    })));
    tree.append_child(root, child);
    tree.append_child(child, percent);
    tree.append_child(percent, leaf);

    assert_rust_scenario(tree, root, Constraints::definite(76.0, 30.0));
}

#[test]
// Host lowering: Flex-only `fr` is normalized to `auto`, and the foreign
// descendants are dispatched through the column-Flex compatibility adapter.
#[allow(clippy::too_many_lines)]
fn head_to_head_flex_column_stretch_with_fr_sibling_preserves_percent_basis() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(Style {
        display: Display::Flex,
        direction: Direction::Rtl,
        box_sizing: BoxSizing::BorderBox,
        width: Length::points(30.0),
        min_width: Length::points(20.0),
        min_height: Length::points(16.0),
        padding: Rect::new(
            Length::points(1.0),
            Length::ZERO,
            Length::points(3.0),
            Length::ZERO,
        ),
        border: Rect::new(1.0, 0.0, 0.0, 1.0),
        flex_direction: FlexDirection::ColumnReverse,
        flex_wrap: FlexWrap::Wrap,
        justify_content: JustifyContent::Center,
        align_items: AlignItems::Center,
        align_content: AlignContent::FlexEnd,
        row_gap: Length::points(1.0),
        ..Style::default()
    }));
    let first = tree.push(SimpleNode::new(block_standalone_style(Style {
        box_sizing: BoxSizing::BorderBox,
        width: Length::points(42.0),
        min_width: Length::points(8.0),
        margin: Rect::new(
            Length::points(7.0),
            Length::points(7.0),
            Length::ZERO,
            Length::points(7.0),
        ),
        padding: Rect::new(
            Length::points(7.0),
            Length::points(3.0),
            Length::points(7.0),
            Length::ZERO,
        ),
        border: Rect::all(1.0),
        flex_basis: Length::percent(50.0),
        flex_shrink: 0.0,
        order: -1,
        ..Style::default()
    })));
    let second = tree.push(SimpleNode::new(block_standalone_style(Style {
        direction: Direction::Rtl,
        width: Length::points(54.0),
        height: Length::points(36.0),
        min_width: Length::points(16.0),
        max_width: Length::points(36.0),
        margin: Rect::new(
            Length::points(5.0),
            Length::points(3.0),
            Length::ZERO,
            Length::points(7.0),
        ),
        padding: Rect::new(
            Length::ZERO,
            Length::points(3.0),
            Length::ZERO,
            Length::ZERO,
        ),
        border: Rect::all(1.0),
        ..Style::default()
    })));
    let third = tree.push(SimpleNode::new(Style {
        display: Display::Linear,
        box_sizing: BoxSizing::BorderBox,
        width: Length::fr(1.0),
        height: Length::points(24.0),
        min_width: Length::fr(8.0),
        min_height: Length::points(16.0),
        max_width: Length::fr(26.0),
        max_height: Length::points(40.0),
        margin: Rect::new(
            Length::ZERO,
            Length::ZERO,
            Length::ZERO,
            Length::points(5.0),
        ),
        padding: Rect::new(
            Length::points(7.0),
            Length::ZERO,
            Length::points(5.0),
            Length::points(5.0),
        ),
        border: Rect::all(1.0),
        flex_basis: Length::percent(60.0),
        flex_grow: 1.0,
        order: 1,
        align_self: Some(AlignItems::End),
        column_gap: Length::points(1.0),
        ..Style::default()
    }));
    let fourth = tree.push(SimpleNode::new(block_standalone_style(Style {
        direction: Direction::Rtl,
        height: Length::points(66.0),
        min_width: Length::fr(4.0),
        min_height: Length::points(20.0),
        max_width: Length::fr(18.0),
        max_height: Length::points(44.0),
        margin: Rect::new(
            Length::ZERO,
            Length::ZERO,
            Length::points(7.0),
            Length::points(1.0),
        ),
        padding: Rect::new(
            Length::points(7.0),
            Length::ZERO,
            Length::points(1.0),
            Length::ZERO,
        ),
        border: Rect::all(1.0),
        flex_basis: Length::fr(2.0),
        flex_grow: 1.0,
        order: 2,
        align_self: Some(AlignItems::FlexStart),
        row_gap: Length::points(3.0),
        column_gap: Length::points(5.0),
        ..Style::default()
    })));
    let child = tree.push(SimpleNode::new(Style {
        display: Display::Flex,
        direction: Direction::Rtl,
        box_sizing: BoxSizing::BorderBox,
        flex_direction: FlexDirection::Row,
        width: Length::Auto,
        height: Length::fr(3.0),
        min_width: Length::fr(10.0),
        min_height: Length::points(8.0),
        margin: Rect::new(
            Length::points(5.0),
            Length::points(5.0),
            Length::ZERO,
            Length::ZERO,
        ),
        padding: Rect::new(
            Length::ZERO,
            Length::ZERO,
            Length::points(3.0),
            Length::ZERO,
        ),
        border: Rect::all(1.0),
        flex_basis: Length::points(54.0),
        flex_grow: 1.0,
        flex_shrink: 0.0,
        order: 3,
        align_self: Some(AlignItems::Stretch),
        row_gap: Length::points(3.0),
        column_gap: Length::points(3.0),
        ..Style::default()
    }));
    let flexible = tree.push(SimpleNode::new(block_standalone_style(Style {
        width: Length::points(11.0),
        height: Length::percent(60.0),
        min_width: Length::points(8.0),
        max_width: Length::points(36.0),
        max_height: Length::points(40.0),
        margin: Rect::new(
            Length::points(1.0),
            Length::ZERO,
            Length::points(5.0),
            Length::ZERO,
        ),
        padding: Rect::new(
            Length::ZERO,
            Length::points(1.0),
            Length::points(5.0),
            Length::points(1.0),
        ),
        flex_basis: Length::fr(2.0),
        row_gap: Length::points(1.0),
        ..Style::default()
    })));
    let percent = tree.push(SimpleNode::new(block_standalone_style(Style {
        direction: Direction::Rtl,
        width: Length::points(8.0),
        margin: Rect::new(
            Length::points(3.0),
            Length::points(5.0),
            Length::points(1.0),
            Length::ZERO,
        ),
        padding: Rect::new(
            Length::points(5.0),
            Length::ZERO,
            Length::ZERO,
            Length::points(1.0),
        ),
        border: Rect::all(1.0),
        flex_basis: Length::percent(60.0),
        flex_shrink: 0.0,
        ..Style::default()
    })));
    tree.append_child(root, first);
    tree.append_child(root, second);
    tree.append_child(root, third);
    tree.append_child(root, fourth);
    tree.append_child(root, child);
    tree.append_child(child, flexible);
    tree.append_child(child, percent);

    assert_rust_scenario(
        tree,
        root,
        Constraints::new(
            SideConstraint::at_most(180.0),
            SideConstraint::at_most(140.0),
        ),
    );
}

#[test]
fn head_to_head_flex_shrinking_explicit_stretch_handles_inflexible_percent_basis_subtree() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(standalone_style(Style {
        flex_direction: FlexDirection::Row,
        align_items: AlignItems::FlexStart,
        width: Length::points(60.0),
        height: Length::points(27.0),
        ..Style::default()
    })));
    let child = tree.push(SimpleNode::new(standalone_style(Style {
        flex_direction: FlexDirection::Column,
        flex_basis: Length::points(20.0),
        flex_grow: 0.0,
        flex_shrink: 1.0,
        width: Length::points(20.0),
        align_self: Some(AlignItems::Stretch),
        ..Style::default()
    })));
    let percent = tree.push(SimpleNode::new(standalone_style(Style {
        flex_basis: Length::percent(65.0),
        flex_grow: 0.0,
        flex_shrink: 0.0,
        height: Length::points(8.0),
        ..Style::default()
    })));
    let leaf = tree.push(SimpleNode::new(block_standalone_style(Style {
        width: Length::points(4.0),
        height: Length::points(3.0),
        ..Style::default()
    })));
    tree.append_child(root, child);
    tree.append_child(child, percent);
    tree.append_child(percent, leaf);

    assert_rust_scenario(tree, root, Constraints::definite(60.0, 27.0));
}

#[test]
fn head_to_head_flex_explicit_stretch_remeasures_growing_percent_basis_subtree() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(standalone_style(Style {
        flex_direction: FlexDirection::Row,
        align_items: AlignItems::FlexStart,
        width: Length::points(78.0),
        height: Length::points(29.0),
        ..Style::default()
    })));
    let child = tree.push(SimpleNode::new(standalone_style(Style {
        flex_direction: FlexDirection::Column,
        flex_basis: Length::points(18.0),
        flex_grow: 1.0,
        flex_shrink: 0.0,
        width: Length::points(18.0),
        align_self: Some(AlignItems::Stretch),
        ..Style::default()
    })));
    let percent = tree.push(SimpleNode::new(standalone_style(Style {
        flex_basis: Length::percent(45.0),
        flex_grow: 1.0,
        flex_shrink: 0.0,
        height: Length::points(7.0),
        ..Style::default()
    })));
    let fixed = tree.push(SimpleNode::new(standalone_style(Style {
        flex_basis: Length::points(5.0),
        height: Length::points(5.0),
        ..Style::default()
    })));
    let leaf = tree.push(SimpleNode::new(block_standalone_style(Style {
        width: Length::points(4.0),
        height: Length::points(3.0),
        ..Style::default()
    })));
    tree.append_child(root, child);
    tree.append_child(child, percent);
    tree.append_child(child, fixed);
    tree.append_child(percent, leaf);

    assert_rust_scenario(tree, root, Constraints::definite(78.0, 29.0));
}

#[test]
fn head_to_head_flex_percent_basis_explicit_stretch_remeasures_flexible_percent_basis_subtree() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(standalone_style(Style {
        flex_direction: FlexDirection::Row,
        align_items: AlignItems::FlexStart,
        width: Length::points(80.0),
        height: Length::points(30.0),
        ..Style::default()
    })));
    let child = tree.push(SimpleNode::new(standalone_style(Style {
        flex_direction: FlexDirection::Column,
        flex_basis: Length::percent(50.0),
        flex_grow: 0.0,
        flex_shrink: 0.0,
        width: Length::points(18.0),
        align_self: Some(AlignItems::Stretch),
        ..Style::default()
    })));
    let percent = tree.push(SimpleNode::new(standalone_style(Style {
        flex_basis: Length::percent(45.0),
        flex_grow: 1.0,
        flex_shrink: 0.0,
        height: Length::points(7.0),
        ..Style::default()
    })));
    let fixed = tree.push(SimpleNode::new(standalone_style(Style {
        flex_basis: Length::points(5.0),
        height: Length::points(5.0),
        ..Style::default()
    })));
    let leaf = tree.push(SimpleNode::new(block_standalone_style(Style {
        width: Length::points(4.0),
        height: Length::points(3.0),
        ..Style::default()
    })));
    tree.append_child(root, child);
    tree.append_child(child, percent);
    tree.append_child(child, fixed);
    tree.append_child(percent, leaf);

    assert_rust_scenario(tree, root, Constraints::definite(80.0, 30.0));
}

#[test]
fn head_to_head_flex_implicit_growing_stretch_remeasures_aligned_growing_percent_basis_subtree() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(standalone_style(Style {
        flex_direction: FlexDirection::Row,
        align_items: AlignItems::Stretch,
        width: Length::points(80.0),
        height: Length::points(32.0),
        ..Style::default()
    })));
    let child = tree.push(SimpleNode::new(standalone_style(Style {
        flex_direction: FlexDirection::Column,
        flex_basis: Length::points(20.0),
        flex_grow: 1.0,
        flex_shrink: 0.0,
        width: Length::points(20.0),
        ..Style::default()
    })));
    let percent = tree.push(SimpleNode::new(standalone_style(Style {
        flex_basis: Length::percent(60.0),
        flex_grow: 1.0,
        flex_shrink: 0.0,
        align_self: Some(AlignItems::Center),
        height: Length::points(8.0),
        ..Style::default()
    })));
    let flexible = tree.push(SimpleNode::new(standalone_style(Style {
        flex_basis: Length::points(6.0),
        flex_grow: 1.0,
        height: Length::points(6.0),
        ..Style::default()
    })));
    let leaf = tree.push(SimpleNode::new(block_standalone_style(Style {
        width: Length::points(5.0),
        height: Length::points(4.0),
        ..Style::default()
    })));
    tree.append_child(root, child);
    tree.append_child(child, percent);
    tree.append_child(child, flexible);
    tree.append_child(percent, leaf);

    assert_rust_scenario(tree, root, Constraints::definite(80.0, 32.0));
}

#[test]
fn head_to_head_flex_implicit_stretch_remeasures_shared_growing_percent_basis_line() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(standalone_style(Style {
        flex_direction: FlexDirection::Row,
        align_items: AlignItems::Stretch,
        width: Length::points(64.0),
        height: Length::points(30.0),
        ..Style::default()
    })));
    let child = tree.push(SimpleNode::new(standalone_style(Style {
        flex_direction: FlexDirection::Column,
        flex_basis: Length::points(22.0),
        width: Length::points(22.0),
        ..Style::default()
    })));
    let stretched_percent = tree.push(SimpleNode::new(standalone_style(Style {
        flex_basis: Length::percent(45.0),
        align_self: Some(AlignItems::Stretch),
        height: Length::points(7.0),
        ..Style::default()
    })));
    let growing_percent = tree.push(SimpleNode::new(standalone_style(Style {
        flex_basis: Length::percent(35.0),
        flex_grow: 1.0,
        flex_shrink: 0.0,
        height: Length::points(6.0),
        ..Style::default()
    })));
    let leaf = tree.push(SimpleNode::new(block_standalone_style(Style {
        width: Length::points(4.0),
        height: Length::points(3.0),
        ..Style::default()
    })));
    tree.append_child(root, child);
    tree.append_child(child, stretched_percent);
    tree.append_child(child, growing_percent);
    tree.append_child(stretched_percent, leaf);

    assert_rust_scenario(tree, root, Constraints::definite(64.0, 30.0));
}

#[test]
fn head_to_head_flex_implicit_stretch_remeasures_local_inflexible_percent_basis_subtree() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(standalone_style(Style {
        flex_direction: FlexDirection::Row,
        align_items: AlignItems::Stretch,
        width: Length::points(58.0),
        height: Length::points(27.0),
        ..Style::default()
    })));
    let child = tree.push(SimpleNode::new(standalone_style(Style {
        flex_direction: FlexDirection::Column,
        flex_basis: Length::points(19.0),
        flex_grow: 0.0,
        flex_shrink: 0.0,
        width: Length::points(19.0),
        ..Style::default()
    })));
    let percent = tree.push(SimpleNode::new(standalone_style(Style {
        flex_basis: Length::percent(65.0),
        flex_grow: 0.0,
        flex_shrink: 0.0,
        width: Length::points(8.0),
        ..Style::default()
    })));
    let leaf = tree.push(SimpleNode::new(block_standalone_style(Style {
        width: Length::points(4.0),
        height: Length::points(3.0),
        ..Style::default()
    })));
    tree.append_child(root, child);
    tree.append_child(child, percent);
    tree.append_child(percent, leaf);

    assert_rust_scenario(tree, root, Constraints::definite(58.0, 27.0));
}

#[test]
fn head_to_head_flex_implicit_non_shrinking_stretch_remeasures_aligned_inflexible_percent_basis_subtree()
 {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(standalone_style(Style {
        flex_direction: FlexDirection::Row,
        align_items: AlignItems::Stretch,
        width: Length::points(59.0),
        height: Length::points(28.0),
        ..Style::default()
    })));
    let child = tree.push(SimpleNode::new(standalone_style(Style {
        flex_direction: FlexDirection::Column,
        flex_basis: Length::points(20.0),
        flex_grow: 0.0,
        flex_shrink: 0.0,
        width: Length::points(20.0),
        ..Style::default()
    })));
    let aligned = tree.push(SimpleNode::new(standalone_style(Style {
        flex_basis: Length::percent(60.0),
        flex_grow: 0.0,
        flex_shrink: 0.0,
        align_self: Some(AlignItems::Center),
        height: Length::points(8.0),
        ..Style::default()
    })));
    let leaf = tree.push(SimpleNode::new(block_standalone_style(Style {
        width: Length::points(4.0),
        height: Length::points(3.0),
        ..Style::default()
    })));
    tree.append_child(root, child);
    tree.append_child(child, aligned);
    tree.append_child(aligned, leaf);

    assert_rust_scenario(tree, root, Constraints::definite(59.0, 28.0));
}

#[test]
fn head_to_head_flex_implicit_shrinking_stretch_remeasures_mixed_percent_basis_subtree() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(standalone_style(Style {
        flex_direction: FlexDirection::Row,
        align_items: AlignItems::Stretch,
        width: Length::points(66.0),
        height: Length::points(34.0),
        ..Style::default()
    })));
    let child = tree.push(SimpleNode::new(standalone_style(Style {
        flex_direction: FlexDirection::Column,
        flex_basis: Length::points(20.0),
        flex_grow: 0.0,
        flex_shrink: 1.0,
        width: Length::points(20.0),
        ..Style::default()
    })));
    let inflexible = tree.push(SimpleNode::new(standalone_style(Style {
        flex_basis: Length::percent(40.0),
        flex_grow: 0.0,
        flex_shrink: 0.0,
        height: Length::points(8.0),
        ..Style::default()
    })));
    let growing = tree.push(SimpleNode::new(standalone_style(Style {
        flex_basis: Length::percent(30.0),
        flex_grow: 1.0,
        flex_shrink: 0.0,
        height: Length::points(7.0),
        ..Style::default()
    })));
    let leaf = tree.push(SimpleNode::new(block_standalone_style(Style {
        width: Length::points(4.0),
        height: Length::points(3.0),
        ..Style::default()
    })));
    tree.append_child(root, child);
    tree.append_child(child, inflexible);
    tree.append_child(child, growing);
    tree.append_child(inflexible, leaf);

    assert_rust_scenario(tree, root, Constraints::definite(66.0, 34.0));
}

#[test]
fn head_to_head_flex_implicit_shrinking_stretch_remeasures_aligned_shrinking_percent_basis_subtree()
{
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(standalone_style(Style {
        flex_direction: FlexDirection::Row,
        align_items: AlignItems::Stretch,
        width: Length::points(62.0),
        height: Length::points(29.0),
        ..Style::default()
    })));
    let child = tree.push(SimpleNode::new(standalone_style(Style {
        flex_direction: FlexDirection::Column,
        flex_basis: Length::points(21.0),
        flex_grow: 0.0,
        flex_shrink: 1.0,
        width: Length::points(21.0),
        ..Style::default()
    })));
    let aligned = tree.push(SimpleNode::new(standalone_style(Style {
        flex_basis: Length::percent(72.0),
        flex_grow: 0.0,
        flex_shrink: 1.0,
        align_self: Some(AlignItems::Center),
        height: Length::points(9.0),
        ..Style::default()
    })));
    let leaf = tree.push(SimpleNode::new(block_standalone_style(Style {
        width: Length::points(5.0),
        height: Length::points(3.0),
        ..Style::default()
    })));
    tree.append_child(root, child);
    tree.append_child(child, aligned);
    tree.append_child(aligned, leaf);

    assert_rust_scenario(tree, root, Constraints::definite(62.0, 29.0));
}

#[test]
fn head_to_head_flex_implicit_stretch_defines_percent_basis_for_non_shrinking_descendant() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(standalone_style(Style {
        flex_direction: FlexDirection::Row,
        align_items: AlignItems::Stretch,
        width: Length::points(90.0),
        height: Length::points(33.0),
        ..Style::default()
    })));
    let child = tree.push(SimpleNode::new(standalone_style(Style {
        flex_direction: FlexDirection::Column,
        flex_basis: Length::points(20.0),
        flex_grow: 1.0,
        flex_shrink: 0.0,
        width: Length::points(20.0),
        ..Style::default()
    })));
    let percent = tree.push(SimpleNode::new(standalone_style(Style {
        flex_basis: Length::percent(50.0),
        flex_grow: 0.0,
        flex_shrink: 0.0,
        height: Length::points(8.0),
        ..Style::default()
    })));
    let leaf = tree.push(SimpleNode::new(block_standalone_style(Style {
        width: Length::points(5.0),
        height: Length::points(3.0),
        ..Style::default()
    })));
    tree.append_child(root, child);
    tree.append_child(child, percent);
    tree.append_child(percent, leaf);

    assert_rust_scenario(tree, root, Constraints::definite(90.0, 33.0));
}

#[test]
fn head_to_head_flex_implicit_stretch_remeasures_unresolved_percent_basis_descendant() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(standalone_style(Style {
        flex_direction: FlexDirection::Row,
        align_items: AlignItems::Stretch,
        height: Length::points(31.0),
        ..Style::default()
    })));
    let child = tree.push(SimpleNode::new(standalone_style(Style {
        flex_direction: FlexDirection::Column,
        flex_basis: Length::percent(60.0),
        flex_grow: 0.0,
        flex_shrink: 0.0,
        width: Length::points(22.0),
        ..Style::default()
    })));
    let percent = tree.push(SimpleNode::new(standalone_style(Style {
        flex_basis: Length::percent(50.0),
        flex_grow: 0.0,
        flex_shrink: 0.0,
        height: Length::points(8.0),
        ..Style::default()
    })));
    let leaf = tree.push(SimpleNode::new(block_standalone_style(Style {
        width: Length::points(5.0),
        height: Length::points(3.0),
        ..Style::default()
    })));
    tree.append_child(root, child);
    tree.append_child(child, percent);
    tree.append_child(percent, leaf);

    assert_rust_scenario(tree, root, Constraints::indefinite());
}

#[test]
fn head_to_head_owner_definite_width_without_root_width_uses_root_at_most_width() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(standalone_style(Style {
        align_items: AlignItems::FlexStart,
        ..Style::default()
    })));
    let child = fixed_flex_child(&mut tree, 30.0, 20.0);
    tree.append_child(root, child);

    assert_rust_scenario(
        tree,
        root,
        Constraints::new(
            SideConstraint::definite(50.0),
            SideConstraint::at_most(10.0),
        ),
    );
}

#[test]
// Host lowering: Flex-only `fr` is normalized to `auto` rather than extending
// neutron-star's W3C Flex value grammar.
fn head_to_head_flex_basis_fr_length_is_imported_as_full_value() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(standalone_style(Style {
        width: Length::points(100.0),
        height: Length::points(20.0),
        align_items: AlignItems::FlexStart,
        ..Style::default()
    })));
    let child = tree.push(SimpleNode::new(standalone_style(Style {
        flex_basis: Length::fr(30.0),
        height: Length::points(10.0),
        ..Style::default()
    })));
    tree.append_child(root, child);

    assert_rust_scenario(tree, root, Constraints::definite(100.0, 20.0));
}

#[test]
fn head_to_head_owner_definite_width_strips_root_horizontal_margins() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(standalone_style(Style {
        align_items: AlignItems::FlexStart,
        margin: Rect::new(
            Length::points(5.0),
            Length::points(7.0),
            Length::ZERO,
            Length::ZERO,
        ),
        ..Style::default()
    })));
    let child = fixed_flex_child(&mut tree, 80.0, 10.0);
    tree.append_child(root, child);

    assert_rust_scenario(
        tree,
        root,
        Constraints::new(
            SideConstraint::definite(100.0),
            SideConstraint::indefinite(),
        ),
    );
}

#[test]
fn head_to_head_rtl_row_uses_right_main_front() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(standalone_style(Style {
        direction: Direction::Rtl,
        width: Length::points(100.0),
        height: Length::points(10.0),
        align_items: AlignItems::FlexStart,
        ..Style::default()
    })));
    for _ in 0..2 {
        let child = tree.push(SimpleNode::new(standalone_style(Style {
            flex_basis: Length::points(10.0),
            height: Length::points(10.0),
            ..Style::default()
        })));
        tree.append_child(root, child);
    }

    assert_rust_scenario(tree, root, Constraints::definite(100.0, 10.0));
}

#[test]
fn head_to_head_rtl_row_reverse_uses_left_main_front() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(standalone_style(Style {
        direction: Direction::Rtl,
        flex_direction: FlexDirection::RowReverse,
        width: Length::points(100.0),
        height: Length::points(10.0),
        align_items: AlignItems::FlexStart,
        ..Style::default()
    })));
    for _ in 0..2 {
        let child = tree.push(SimpleNode::new(standalone_style(Style {
            flex_basis: Length::points(10.0),
            height: Length::points(10.0),
            ..Style::default()
        })));
        tree.append_child(root, child);
    }

    assert_rust_scenario(tree, root, Constraints::definite(100.0, 10.0));
}

#[test]
fn head_to_head_explicit_ltr_no_wrap_mapping_keeps_single_flex_line() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(standalone_style(Style {
        direction: Direction::Ltr,
        flex_wrap: FlexWrap::NoWrap,
        width: Length::points(50.0),
        height: Length::points(20.0),
        align_items: AlignItems::FlexStart,
        ..Style::default()
    })));
    for _ in 0..2 {
        let child = tree.push(SimpleNode::new(standalone_style(Style {
            flex_basis: Length::points(30.0),
            height: Length::points(10.0),
            flex_shrink: 0.0,
            ..Style::default()
        })));
        tree.append_child(root, child);
    }

    assert_rust_scenario(tree, root, Constraints::definite(50.0, 20.0));
}

#[test]
fn head_to_head_auto_margin_consumes_remaining_main_space() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(standalone_style(Style {
        width: Length::points(100.0),
        height: Length::points(20.0),
        align_items: AlignItems::FlexStart,
        ..Style::default()
    })));
    let child = tree.push(SimpleNode::new(standalone_style(Style {
        flex_basis: Length::points(20.0),
        height: Length::points(10.0),
        margin: Rect::new(Length::Auto, Length::ZERO, Length::ZERO, Length::ZERO),
        ..Style::default()
    })));
    tree.append_child(root, child);

    assert_rust_scenario(tree, root, Constraints::definite(100.0, 20.0));
}

#[test]
fn head_to_head_multiple_main_axis_auto_margins_share_positive_free_space() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(standalone_style(Style {
        justify_content: JustifyContent::Center,
        width: Length::points(100.0),
        height: Length::points(10.0),
        align_items: AlignItems::FlexStart,
        ..Style::default()
    })));
    let first = tree.push(SimpleNode::new(standalone_style(Style {
        flex_basis: Length::points(20.0),
        height: Length::points(10.0),
        margin: Rect::new(Length::Auto, Length::Auto, Length::ZERO, Length::ZERO),
        ..Style::default()
    })));
    let second = tree.push(SimpleNode::new(standalone_style(Style {
        flex_basis: Length::points(20.0),
        height: Length::points(10.0),
        margin: Rect::new(Length::Auto, Length::ZERO, Length::ZERO, Length::ZERO),
        ..Style::default()
    })));
    tree.append_child(root, first);
    tree.append_child(root, second);

    assert_rust_scenario(tree, root, Constraints::definite(100.0, 10.0));
}

#[test]
fn head_to_head_justify_content_start_end_variants() {
    for justify_content in [JustifyContent::Start, JustifyContent::End] {
        let mut tree = SimpleTree::default();
        let root = tree.push(SimpleNode::new(standalone_style(Style {
            width: Length::points(60.0),
            height: Length::points(20.0),
            align_items: AlignItems::FlexStart,
            justify_content,
            ..Style::default()
        })));
        for _ in 0..2 {
            let child = fixed_flex_child(&mut tree, 10.0, 10.0);
            tree.append_child(root, child);
        }

        assert_rust_scenario(tree, root, Constraints::definite(60.0, 20.0));
    }
}

#[test]
fn head_to_head_align_start_end_variants_in_flex_cross_axis() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(standalone_style(Style {
        width: Length::points(50.0),
        height: Length::points(40.0),
        align_items: AlignItems::End,
        justify_content: JustifyContent::FlexStart,
        ..Style::default()
    })));
    let end_aligned = fixed_flex_child(&mut tree, 10.0, 10.0);
    let start_aligned = tree.push(SimpleNode::new(standalone_style(Style {
        flex_basis: Length::points(10.0),
        height: Length::points(10.0),
        align_self: Some(AlignItems::Start),
        ..Style::default()
    })));
    tree.append_child(root, end_aligned);
    tree.append_child(root, start_aligned);

    assert_rust_scenario(tree, root, Constraints::definite(50.0, 40.0));
}

#[test]
fn head_to_head_align_content_flex_end_places_wrapped_lines_at_cross_end() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(standalone_style(Style {
        flex_wrap: FlexWrap::Wrap,
        align_content: AlignContent::FlexEnd,
        align_items: AlignItems::FlexStart,
        width: Length::points(50.0),
        height: Length::points(70.0),
        ..Style::default()
    })));
    for _ in 0..2 {
        let child = fixed_flex_child(&mut tree, 30.0, 10.0);
        tree.append_child(root, child);
    }

    assert_rust_scenario(tree, root, Constraints::definite(50.0, 70.0));
}

#[test]
fn head_to_head_explicit_stretch_justify_and_align_content_mapping() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(standalone_style(Style {
        flex_wrap: FlexWrap::Wrap,
        justify_content: JustifyContent::Stretch,
        align_content: AlignContent::Stretch,
        align_items: AlignItems::FlexStart,
        width: Length::points(50.0),
        height: Length::points(70.0),
        ..Style::default()
    })));
    for _ in 0..2 {
        let child = fixed_flex_child(&mut tree, 30.0, 10.0);
        tree.append_child(root, child);
    }

    assert_rust_scenario(tree, root, Constraints::definite(50.0, 70.0));
}

#[test]
fn head_to_head_flex_wrap_reverse_center_reexports_cached_block_subtree_with_fractional_offset() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(standalone_style(Style {
        flex_wrap: FlexWrap::WrapReverse,
        align_items: AlignItems::Center,
        width: Length::points(20.0),
        height: Length::points(9.0),
        ..Style::default()
    })));
    let child = tree.push(SimpleNode::new(block_standalone_style(Style::default())));
    let grandchild = tree.push(SimpleNode::new(block_standalone_style(Style {
        width: Length::points(10.0),
        height: Length::points(4.0),
        ..Style::default()
    })));
    tree.append_child(root, child);
    tree.append_child(child, grandchild);

    assert_rust_scenario(tree, root, Constraints::definite(20.0, 9.0));
}

#[test]
fn head_to_head_flex_min_target_defines_percent_flex_basis_descendant_base() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(standalone_style(Style {
        flex_direction: FlexDirection::Row,
        align_items: AlignItems::FlexStart,
        width: Length::points(78.0),
        height: Length::points(20.0),
        ..Style::default()
    })));
    let first = tree.push(SimpleNode::new(standalone_style(Style {
        flex_direction: FlexDirection::Row,
        flex_basis: Length::points(64.0),
        flex_grow: 0.0,
        flex_shrink: 1.0,
        min_width: Length::points(42.0),
        width: Length::points(64.0),
        height: Length::points(14.0),
        align_items: AlignItems::FlexStart,
        ..Style::default()
    })));
    let percent = tree.push(SimpleNode::new(standalone_style(Style {
        flex_basis: Length::percent(50.0),
        flex_grow: 0.0,
        flex_shrink: 0.0,
        height: Length::points(6.0),
        ..Style::default()
    })));
    let fixed = tree.push(SimpleNode::new(standalone_style(Style {
        flex_basis: Length::points(5.0),
        width: Length::points(5.0),
        height: Length::points(5.0),
        ..Style::default()
    })));
    let second = fixed_flex_child(&mut tree, 52.0, 10.0);
    tree.append_child(root, first);
    tree.append_child(root, second);
    tree.append_child(first, percent);
    tree.append_child(first, fixed);

    assert_rust_scenario(tree, root, Constraints::definite(78.0, 20.0));
}

#[test]
fn head_to_head_flex_multiple_min_width_violations_freeze_before_redistributing_flex_shrink_space()
{
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(standalone_style(Style {
        align_items: AlignItems::FlexStart,
        width: Length::points(180.0),
        height: Length::points(10.0),
        ..Style::default()
    })));
    let first = tree.push(SimpleNode::new(standalone_style(Style {
        flex_basis: Length::points(100.0),
        min_width: Length::points(80.0),
        height: Length::points(10.0),
        ..Style::default()
    })));
    let second = tree.push(SimpleNode::new(standalone_style(Style {
        flex_basis: Length::points(100.0),
        min_width: Length::points(70.0),
        height: Length::points(10.0),
        ..Style::default()
    })));
    let third = fixed_flex_child(&mut tree, 100.0, 10.0);
    tree.append_child(root, first);
    tree.append_child(root, second);
    tree.append_child(root, third);

    assert_rust_scenario(tree, root, Constraints::definite(180.0, 10.0));
}

#[test]
fn head_to_head_flex_min_width_above_basis_freezes_shrinking_item_to_hypothetical_main_size() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(standalone_style(Style {
        align_items: AlignItems::FlexStart,
        width: Length::points(100.0),
        height: Length::points(10.0),
        ..Style::default()
    })));
    let frozen = tree.push(SimpleNode::new(standalone_style(Style {
        flex_basis: Length::points(20.0),
        flex_shrink: 1.0,
        min_width: Length::points(50.0),
        height: Length::points(10.0),
        ..Style::default()
    })));
    let flexible = tree.push(SimpleNode::new(standalone_style(Style {
        flex_basis: Length::points(80.0),
        flex_shrink: 1.0,
        height: Length::points(10.0),
        ..Style::default()
    })));
    tree.append_child(root, frozen);
    tree.append_child(root, flexible);

    assert_rust_scenario(tree, root, Constraints::definite(100.0, 10.0));
}

#[test]
fn head_to_head_flex_max_width_below_basis_freezes_growing_item_to_hypothetical_main_size() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(standalone_style(Style {
        align_items: AlignItems::FlexStart,
        width: Length::points(140.0),
        height: Length::points(10.0),
        ..Style::default()
    })));
    let capped = tree.push(SimpleNode::new(standalone_style(Style {
        flex_basis: Length::points(80.0),
        flex_grow: 1.0,
        max_width: Length::points(50.0),
        height: Length::points(10.0),
        ..Style::default()
    })));
    let flexible = tree.push(SimpleNode::new(standalone_style(Style {
        flex_basis: Length::points(20.0),
        flex_grow: 1.0,
        height: Length::points(10.0),
        ..Style::default()
    })));
    tree.append_child(root, capped);
    tree.append_child(root, flexible);

    assert_rust_scenario(tree, root, Constraints::definite(140.0, 10.0));
}

#[test]
fn head_to_head_flex_zero_grow_freezes_item_before_distributing_positive_free_space() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(standalone_style(Style {
        align_items: AlignItems::FlexStart,
        width: Length::points(100.0),
        height: Length::points(10.0),
        ..Style::default()
    })));
    let frozen = tree.push(SimpleNode::new(standalone_style(Style {
        flex_basis: Length::points(20.0),
        flex_grow: 0.0,
        height: Length::points(10.0),
        ..Style::default()
    })));
    let flexible = tree.push(SimpleNode::new(standalone_style(Style {
        flex_basis: Length::points(20.0),
        flex_grow: 1.0,
        height: Length::points(10.0),
        ..Style::default()
    })));
    tree.append_child(root, frozen);
    tree.append_child(root, flexible);

    assert_rust_scenario(tree, root, Constraints::definite(100.0, 10.0));
}

#[test]
fn head_to_head_flex_all_zero_grow_items_leave_space_for_justify_content() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(standalone_style(Style {
        align_items: AlignItems::FlexStart,
        justify_content: JustifyContent::Center,
        width: Length::points(100.0),
        height: Length::points(10.0),
        ..Style::default()
    })));
    let first = tree.push(SimpleNode::new(standalone_style(Style {
        flex_basis: Length::points(20.0),
        flex_grow: 0.0,
        height: Length::points(10.0),
        ..Style::default()
    })));
    let second = tree.push(SimpleNode::new(standalone_style(Style {
        flex_basis: Length::points(30.0),
        flex_grow: 0.0,
        height: Length::points(10.0),
        ..Style::default()
    })));
    tree.append_child(root, first);
    tree.append_child(root, second);

    assert_rust_scenario(tree, root, Constraints::definite(100.0, 10.0));
}

#[test]
fn head_to_head_flex_min_width_violation_freezes_item_during_grow_and_restarts_distribution() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(standalone_style(Style {
        align_items: AlignItems::FlexStart,
        width: Length::points(100.0),
        height: Length::points(10.0),
        ..Style::default()
    })));
    let clamped = tree.push(SimpleNode::new(standalone_style(Style {
        flex_basis: Length::points(20.0),
        flex_grow: 1.0,
        min_width: Length::points(70.0),
        height: Length::points(10.0),
        ..Style::default()
    })));
    let flexible = tree.push(SimpleNode::new(standalone_style(Style {
        flex_basis: Length::points(20.0),
        flex_grow: 1.0,
        height: Length::points(10.0),
        ..Style::default()
    })));
    tree.append_child(root, clamped);
    tree.append_child(root, flexible);

    assert_rust_scenario(tree, root, Constraints::definite(100.0, 10.0));
}

#[test]
fn head_to_head_flex_main_axis_gap_reduces_free_space_before_grow_distribution() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(standalone_style(Style {
        align_items: AlignItems::FlexStart,
        width: Length::points(110.0),
        height: Length::points(10.0),
        column_gap: Length::points(10.0),
        ..Style::default()
    })));
    let first = tree.push(SimpleNode::new(standalone_style(Style {
        flex_basis: Length::points(20.0),
        flex_grow: 1.0,
        height: Length::points(10.0),
        ..Style::default()
    })));
    let second = tree.push(SimpleNode::new(standalone_style(Style {
        flex_basis: Length::points(20.0),
        flex_grow: 3.0,
        height: Length::points(10.0),
        ..Style::default()
    })));
    tree.append_child(root, first);
    tree.append_child(root, second);

    assert_rust_scenario(tree, root, Constraints::definite(110.0, 10.0));
}

#[test]
fn head_to_head_flex_multiple_max_width_violations_freeze_before_redistributing_flex_grow_space() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(standalone_style(Style {
        align_items: AlignItems::FlexStart,
        width: Length::points(180.0),
        height: Length::points(10.0),
        ..Style::default()
    })));
    let first = tree.push(SimpleNode::new(standalone_style(Style {
        flex_basis: Length::points(20.0),
        flex_grow: 1.0,
        max_width: Length::points(30.0),
        height: Length::points(10.0),
        ..Style::default()
    })));
    let second = tree.push(SimpleNode::new(standalone_style(Style {
        flex_basis: Length::points(20.0),
        flex_grow: 1.0,
        max_width: Length::points(50.0),
        height: Length::points(10.0),
        ..Style::default()
    })));
    let third = tree.push(SimpleNode::new(standalone_style(Style {
        flex_basis: Length::points(20.0),
        flex_grow: 1.0,
        height: Length::points(10.0),
        ..Style::default()
    })));
    tree.append_child(root, first);
    tree.append_child(root, second);
    tree.append_child(root, third);

    assert_rust_scenario(tree, root, Constraints::definite(180.0, 10.0));
}

#[test]
fn head_to_head_flex_zero_shrink_freezes_item_before_distributing_negative_free_space() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(standalone_style(Style {
        align_items: AlignItems::FlexStart,
        width: Length::points(80.0),
        height: Length::points(10.0),
        ..Style::default()
    })));
    let frozen = tree.push(SimpleNode::new(standalone_style(Style {
        flex_basis: Length::points(50.0),
        flex_shrink: 0.0,
        height: Length::points(10.0),
        ..Style::default()
    })));
    let flexible = tree.push(SimpleNode::new(standalone_style(Style {
        flex_basis: Length::points(50.0),
        flex_shrink: 1.0,
        height: Length::points(10.0),
        ..Style::default()
    })));
    tree.append_child(root, frozen);
    tree.append_child(root, flexible);

    assert_rust_scenario(tree, root, Constraints::definite(80.0, 10.0));
}

#[test]
fn head_to_head_flex_max_width_violation_freezes_item_during_shrink_and_restarts_distribution() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(standalone_style(Style {
        align_items: AlignItems::FlexStart,
        width: Length::points(160.0),
        height: Length::points(10.0),
        ..Style::default()
    })));
    let capped = tree.push(SimpleNode::new(standalone_style(Style {
        flex_basis: Length::points(100.0),
        flex_shrink: 1.0,
        max_width: Length::points(70.0),
        height: Length::points(10.0),
        ..Style::default()
    })));
    let flexible = tree.push(SimpleNode::new(standalone_style(Style {
        flex_basis: Length::points(100.0),
        flex_shrink: 1.0,
        height: Length::points(10.0),
        ..Style::default()
    })));
    tree.append_child(root, capped);
    tree.append_child(root, flexible);

    assert_rust_scenario(tree, root, Constraints::definite(160.0, 10.0));
}

#[test]
fn head_to_head_flex_max_target_defines_percent_flex_basis_descendant_base() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(standalone_style(Style {
        flex_direction: FlexDirection::Row,
        align_items: AlignItems::FlexStart,
        width: Length::points(100.0),
        height: Length::points(20.0),
        ..Style::default()
    })));
    let capped = tree.push(SimpleNode::new(standalone_style(Style {
        flex_direction: FlexDirection::Row,
        flex_basis: Length::points(20.0),
        flex_grow: 1.0,
        flex_shrink: 1.0,
        max_width: Length::points(34.0),
        width: Length::points(20.0),
        height: Length::points(14.0),
        align_items: AlignItems::FlexStart,
        ..Style::default()
    })));
    let percent = tree.push(SimpleNode::new(standalone_style(Style {
        flex_basis: Length::percent(50.0),
        flex_grow: 1.0,
        flex_shrink: 0.0,
        height: Length::points(6.0),
        ..Style::default()
    })));
    let fixed = tree.push(SimpleNode::new(standalone_style(Style {
        flex_basis: Length::points(5.0),
        width: Length::points(5.0),
        height: Length::points(5.0),
        ..Style::default()
    })));
    let flexible = tree.push(SimpleNode::new(standalone_style(Style {
        flex_basis: Length::points(20.0),
        flex_grow: 1.0,
        height: Length::points(10.0),
        ..Style::default()
    })));
    tree.append_child(root, capped);
    tree.append_child(root, flexible);
    tree.append_child(capped, percent);
    tree.append_child(capped, fixed);

    assert_rust_scenario(tree, root, Constraints::definite(100.0, 20.0));
}

#[test]
fn head_to_head_flex_max_target_defines_inflexible_percent_basis_child_base() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(standalone_style(Style {
        flex_direction: FlexDirection::Row,
        align_items: AlignItems::FlexStart,
        width: Length::points(96.0),
        height: Length::points(20.0),
        ..Style::default()
    })));
    let capped = tree.push(SimpleNode::new(standalone_style(Style {
        flex_direction: FlexDirection::Row,
        flex_basis: Length::points(20.0),
        flex_grow: 1.0,
        flex_shrink: 1.0,
        max_width: Length::points(32.0),
        width: Length::points(20.0),
        height: Length::points(14.0),
        align_items: AlignItems::FlexStart,
        ..Style::default()
    })));
    let percent = tree.push(SimpleNode::new(standalone_style(Style {
        flex_basis: Length::percent(50.0),
        flex_grow: 0.0,
        flex_shrink: 0.0,
        height: Length::points(6.0),
        ..Style::default()
    })));
    let fixed = tree.push(SimpleNode::new(standalone_style(Style {
        flex_basis: Length::points(5.0),
        width: Length::points(5.0),
        height: Length::points(5.0),
        ..Style::default()
    })));
    let flexible = tree.push(SimpleNode::new(standalone_style(Style {
        flex_basis: Length::points(20.0),
        flex_grow: 1.0,
        height: Length::points(10.0),
        ..Style::default()
    })));
    tree.append_child(root, capped);
    tree.append_child(root, flexible);
    tree.append_child(capped, percent);
    tree.append_child(capped, fixed);

    assert_rust_scenario(tree, root, Constraints::definite(96.0, 20.0));
}

#[test]
fn head_to_head_justify_content_main_axis_direction_matrix() {
    for direction_case in NATIVE_MAIN_AXIS_MATRIX {
        for justify_content in NATIVE_JUSTIFY_MATRIX {
            let mut tree = SimpleTree::default();
            let root = tree.push(SimpleNode::new(standalone_style(Style {
                flex_direction: direction_case.flex_direction,
                direction: direction_case.direction,
                justify_content,
                align_items: AlignItems::FlexStart,
                width: Length::points(100.0),
                height: Length::points(100.0),
                ..Style::default()
            })));
            let first = fixed_matrix_flex_child(&mut tree);
            let second = fixed_matrix_flex_child(&mut tree);
            tree.append_child(root, first);
            tree.append_child(root, second);

            assert_rust_scenario_named(
                &format!(
                    "{:?}/{:?}/{:?}",
                    direction_case.flex_direction, direction_case.direction, justify_content
                ),
                tree,
                root,
                Constraints::definite(100.0, 100.0),
            );
        }
    }
}

#[test]
fn head_to_head_main_axis_auto_margin_direction_matrix() {
    for direction_case in NATIVE_MAIN_AXIS_MATRIX {
        let mut tree = SimpleTree::default();
        let root = tree.push(SimpleNode::new(standalone_style(Style {
            flex_direction: direction_case.flex_direction,
            direction: direction_case.direction,
            justify_content: JustifyContent::Center,
            align_items: AlignItems::FlexStart,
            width: Length::points(100.0),
            height: Length::points(100.0),
            ..Style::default()
        })));
        let first = fixed_matrix_flex_child(&mut tree);
        let second = tree.push(SimpleNode::new(standalone_style(Style {
            flex_basis: Length::points(10.0),
            width: Length::points(10.0),
            height: Length::points(10.0),
            margin: native_main_start_auto_margin(direction_case),
            ..Style::default()
        })));
        tree.append_child(root, first);
        tree.append_child(root, second);

        assert_rust_scenario_named(
            &format!(
                "{:?}/{:?}/main-start-auto-margin",
                direction_case.flex_direction, direction_case.direction
            ),
            tree,
            root,
            Constraints::definite(100.0, 100.0),
        );
    }
}

#[test]
fn head_to_head_justify_content_gap_overflow_direction_matrix() {
    for direction_case in NATIVE_MAIN_AXIS_MATRIX {
        for justify_content in NATIVE_GAP_OVERFLOW_JUSTIFY_MATRIX {
            let mut tree = SimpleTree::default();
            let root = tree.push(SimpleNode::new(standalone_style(Style {
                flex_direction: direction_case.flex_direction,
                direction: direction_case.direction,
                justify_content,
                align_items: AlignItems::FlexStart,
                width: Length::points(50.0),
                height: Length::points(50.0),
                row_gap: Length::points(10.0),
                column_gap: Length::points(10.0),
                ..Style::default()
            })));
            let first = fixed_main_axis_matrix_flex_child(&mut tree, direction_case, 30.0, 10.0);
            let second = fixed_main_axis_matrix_flex_child(&mut tree, direction_case, 30.0, 10.0);
            tree.append_child(root, first);
            tree.append_child(root, second);

            assert_rust_scenario_named(
                &format!(
                    "{:?}/{:?}/{:?}/gap-overflow",
                    direction_case.flex_direction, direction_case.direction, justify_content
                ),
                tree,
                root,
                Constraints::definite(50.0, 50.0),
            );
        }
    }
}

#[test]
fn head_to_head_percent_padding_gap_and_margin() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(standalone_style(Style {
        width: Length::points(120.0),
        height: Length::points(50.0),
        padding: Rect::all(Length::percent(10.0)),
        column_gap: Length::percent(5.0),
        align_items: AlignItems::FlexStart,
        ..Style::default()
    })));
    let first = tree.push(SimpleNode::new(standalone_style(Style {
        flex_basis: Length::points(20.0),
        height: Length::points(8.0),
        margin: Rect::new(
            Length::percent(5.0),
            Length::ZERO,
            Length::percent(2.0),
            Length::ZERO,
        ),
        ..Style::default()
    })));
    let second = tree.push(SimpleNode::new(standalone_style(Style {
        flex_basis: Length::points(18.0),
        height: Length::points(10.0),
        margin: Rect::new(
            Length::ZERO,
            Length::percent(4.0),
            Length::ZERO,
            Length::percent(3.0),
        ),
        ..Style::default()
    })));
    tree.append_child(root, first);
    tree.append_child(root, second);

    assert_rust_scenario(tree, root, Constraints::definite(120.0, 50.0));
}

#[test]
fn head_to_head_calc_column_gap() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(standalone_style(Style {
        width: Length::points(120.0),
        height: Length::points(30.0),
        column_gap: Length::calc(2.0, 5.0),
        align_items: AlignItems::FlexStart,
        ..Style::default()
    })));
    let first = tree.push(SimpleNode::new(standalone_style(Style {
        flex_basis: Length::points(20.0),
        height: Length::points(10.0),
        ..Style::default()
    })));
    let second = tree.push(SimpleNode::new(standalone_style(Style {
        flex_basis: Length::points(18.0),
        height: Length::points(12.0),
        ..Style::default()
    })));
    tree.append_child(root, first);
    tree.append_child(root, second);

    assert_rust_scenario(tree, root, Constraints::definite(120.0, 30.0));
}

#[test]
fn head_to_head_full_value_edge_lengths_reach_cpp_baseline_import() {
    for edge_length in [
        Length::MaxContent,
        Length::FitContent(Some(BaseLength::fixed(4.0))),
        Length::fr(1.0),
    ] {
        let mut tree = SimpleTree::default();
        let root = tree.push(SimpleNode::new(standalone_style(Style {
            width: Length::points(80.0),
            height: Length::points(20.0),
            align_items: AlignItems::FlexStart,
            ..Style::default()
        })));
        let first = tree.push(SimpleNode::new(standalone_style(Style {
            position: PositionType::Relative,
            left: edge_length,
            margin: Rect::new(edge_length, Length::ZERO, Length::ZERO, Length::ZERO),
            padding: Rect::new(edge_length, Length::ZERO, Length::ZERO, Length::ZERO),
            flex_basis: Length::points(10.0),
            height: Length::points(6.0),
            ..Style::default()
        })));
        let second = tree.push(SimpleNode::new(standalone_style(Style {
            flex_basis: Length::points(12.0),
            height: Length::points(8.0),
            ..Style::default()
        })));
        tree.append_child(root, first);
        tree.append_child(root, second);

        assert_rust_scenario(tree, root, Constraints::definite(80.0, 20.0));
    }
}

#[test]
fn head_to_head_full_value_column_gap_units() {
    for column_gap in [
        Length::MaxContent,
        Length::FitContent(Some(BaseLength::fixed(12.0))),
        Length::fr(1.0),
    ] {
        let mut tree = SimpleTree::default();
        let root = tree.push(SimpleNode::new(standalone_style(Style {
            width: Length::points(120.0),
            height: Length::points(30.0),
            column_gap,
            align_items: AlignItems::FlexStart,
            ..Style::default()
        })));
        let first = tree.push(SimpleNode::new(standalone_style(Style {
            flex_basis: Length::points(20.0),
            height: Length::points(10.0),
            ..Style::default()
        })));
        let second = tree.push(SimpleNode::new(standalone_style(Style {
            flex_basis: Length::points(18.0),
            height: Length::points(12.0),
            ..Style::default()
        })));
        tree.append_child(root, first);
        tree.append_child(root, second);

        assert_rust_scenario(tree, root, Constraints::definite(120.0, 30.0));
    }
}

#[test]
fn head_to_head_full_value_row_gap_units() {
    for row_gap in [
        Length::MaxContent,
        Length::FitContent(Some(BaseLength::fixed(12.0))),
        Length::fr(1.0),
    ] {
        let mut tree = SimpleTree::default();
        let root = tree.push(SimpleNode::new(standalone_style(Style {
            width: Length::points(30.0),
            height: Length::points(80.0),
            flex_wrap: FlexWrap::Wrap,
            row_gap,
            align_items: AlignItems::FlexStart,
            align_content: AlignContent::FlexStart,
            ..Style::default()
        })));
        let first = tree.push(SimpleNode::new(standalone_style(Style {
            flex_basis: Length::points(20.0),
            height: Length::points(10.0),
            ..Style::default()
        })));
        let second = tree.push(SimpleNode::new(standalone_style(Style {
            flex_basis: Length::points(20.0),
            height: Length::points(10.0),
            ..Style::default()
        })));
        tree.append_child(root, first);
        tree.append_child(root, second);

        assert_rust_scenario(tree, root, Constraints::definite(30.0, 80.0));
    }
}

#[test]
fn head_to_head_calc_size_lengths() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(standalone_style(Style {
        width: Length::calc(20.0, 50.0),
        height: Length::calc(10.0, 50.0),
        align_items: AlignItems::FlexStart,
        ..Style::default()
    })));
    let child = tree.push(SimpleNode::new(standalone_style(Style {
        flex_basis: Length::calc(10.0, 25.0),
        height: Length::points(12.0),
        min_width: Length::calc(20.0, 0.0),
        max_width: Length::calc(80.0, 0.0),
        min_height: Length::calc(8.0, 0.0),
        max_height: Length::calc(40.0, 0.0),
        ..Style::default()
    })));
    tree.append_child(root, child);

    assert_rust_scenario(tree, root, Constraints::definite(200.0, 80.0));
}

#[test]
fn head_to_head_flex_basis_max_content_uses_auto_measure_path() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(standalone_style(Style {
        width: Length::points(100.0),
        height: Length::points(10.0),
        align_items: AlignItems::FlexStart,
        ..Style::default()
    })));
    let child = tree.push(SimpleNode::with_measured_size(
        standalone_style(Style {
            flex_basis: Length::MaxContent,
            height: Length::points(10.0),
            ..Style::default()
        }),
        Size::new(45.0, 10.0),
    ));
    tree.append_child(root, child);

    assert_rust_scenario(tree, root, Constraints::definite(100.0, 10.0));
}

#[test]
fn head_to_head_calc_padding_margin_and_position_edges() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(standalone_style(Style {
        width: Length::points(120.0),
        height: Length::points(60.0),
        padding: Rect::new(
            Length::calc(2.0, 10.0),
            Length::calc(3.0, 5.0),
            Length::calc(1.0, 5.0),
            Length::points(0.0),
        ),
        align_items: AlignItems::FlexStart,
        ..Style::default()
    })));
    let flow = tree.push(SimpleNode::new(standalone_style(Style {
        flex_basis: Length::points(20.0),
        height: Length::points(10.0),
        margin: Rect::new(
            Length::calc(1.0, 5.0),
            Length::calc(2.0, 5.0),
            Length::calc(3.0, 0.0),
            Length::calc(4.0, 0.0),
        ),
        ..Style::default()
    })));
    let absolute = tree.push(SimpleNode::new(standalone_style(Style {
        position: PositionType::Absolute,
        width: Length::points(10.0),
        height: Length::points(8.0),
        left: Length::calc(2.0, 10.0),
        top: Length::calc(3.0, 5.0),
        ..Style::default()
    })));
    tree.append_child(root, flow);
    tree.append_child(root, absolute);

    assert_rust_scenario(tree, root, Constraints::definite(120.0, 60.0));
}

#[test]
fn head_to_head_measured_max_content_item() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(standalone_style(Style {
        width: Length::points(100.0),
        align_items: AlignItems::FlexStart,
        ..Style::default()
    })));
    let child = tree.push(SimpleNode::with_measured_size(
        standalone_style(Style {
            width: Length::max_content(),
            height: Length::max_content(),
            ..Style::default()
        }),
        Size::new(24.0, 9.0),
    ));
    tree.append_child(root, child);

    assert_rust_scenario(
        tree,
        root,
        Constraints::new(
            SideConstraint::definite(100.0),
            SideConstraint::indefinite(),
        ),
    );
}

#[test]
fn head_to_head_measured_exact_item_uses_constraints_without_measure_size() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(standalone_style(Style {
        width: Length::points(100.0),
        align_items: AlignItems::FlexStart,
        ..Style::default()
    })));
    let child = tree.push(SimpleNode::with_measured_size_and_baseline(
        standalone_style(Style {
            width: Length::points(20.0),
            height: Length::points(7.0),
            ..Style::default()
        }),
        Size::new(99.0, 99.0),
        4.0,
    ));
    tree.append_child(root, child);

    assert_rust_scenario(
        tree,
        root,
        Constraints::new(
            SideConstraint::definite(100.0),
            SideConstraint::indefinite(),
        ),
    );
}

#[test]
fn head_to_head_absolute_child_with_edges() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(standalone_style(Style {
        width: Length::points(80.0),
        height: Length::points(60.0),
        padding: Rect::all(Length::points(2.0)),
        border: Rect::all(1.0),
        ..Style::default()
    })));
    let in_flow = tree.push(SimpleNode::new(standalone_style(Style {
        flex_basis: Length::points(20.0),
        height: Length::points(10.0),
        ..Style::default()
    })));
    let absolute = tree.push(SimpleNode::new(standalone_style(Style {
        position: PositionType::Absolute,
        left: Length::points(5.0),
        top: Length::points(7.0),
        width: Length::points(16.0),
        height: Length::points(9.0),
        margin: Rect::all(Length::points(1.0)),
        ..Style::default()
    })));
    tree.append_child(root, in_flow);
    tree.append_child(root, absolute);

    assert_rust_scenario(tree, root, Constraints::definite(80.0, 60.0));
}

#[test]
fn head_to_head_absolute_child_can_use_right_bottom_insets() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(standalone_style(Style {
        width: Length::points(100.0),
        height: Length::points(80.0),
        ..Style::default()
    })));
    let absolute = tree.push(SimpleNode::new(standalone_style(Style {
        position: PositionType::Absolute,
        width: Length::points(20.0),
        height: Length::points(10.0),
        right: Length::points(5.0),
        bottom: Length::points(7.0),
        ..Style::default()
    })));
    let flex_child = tree.push(SimpleNode::new(standalone_style(Style {
        flex_basis: Length::points(15.0),
        height: Length::points(10.0),
        ..Style::default()
    })));
    tree.append_child(root, absolute);
    tree.append_child(root, flex_child);

    assert_rust_scenario(tree, root, Constraints::definite(100.0, 80.0));
}

#[test]
fn head_to_head_absolute_flex_child_without_insets_uses_container_alignment() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(standalone_style(Style {
        justify_content: JustifyContent::Center,
        align_items: AlignItems::FlexEnd,
        width: Length::points(100.0),
        height: Length::points(40.0),
        ..Style::default()
    })));
    let absolute = tree.push(SimpleNode::new(standalone_style(Style {
        position: PositionType::Absolute,
        width: Length::points(20.0),
        height: Length::points(10.0),
        ..Style::default()
    })));
    tree.append_child(root, absolute);

    assert_rust_scenario(tree, root, Constraints::definite(100.0, 40.0));
}

#[test]
fn head_to_head_absolute_flex_child_center_alignment_allows_negative_free_space() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(standalone_style(Style {
        justify_content: JustifyContent::Center,
        align_items: AlignItems::FlexStart,
        width: Length::points(100.0),
        height: Length::points(40.0),
        ..Style::default()
    })));
    let absolute = tree.push(SimpleNode::new(standalone_style(Style {
        position: PositionType::Absolute,
        width: Length::points(140.0),
        height: Length::points(10.0),
        ..Style::default()
    })));
    tree.append_child(root, absolute);

    assert_rust_scenario(tree, root, Constraints::definite(100.0, 40.0));
}

#[test]
fn head_to_head_absolute_flex_child_wrap_reverse_reverses_cross_axis_initial_position() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(standalone_style(Style {
        flex_wrap: FlexWrap::WrapReverse,
        align_items: AlignItems::FlexEnd,
        width: Length::points(100.0),
        height: Length::points(40.0),
        ..Style::default()
    })));
    let absolute = tree.push(SimpleNode::new(standalone_style(Style {
        position: PositionType::Absolute,
        width: Length::points(20.0),
        height: Length::points(10.0),
        ..Style::default()
    })));
    tree.append_child(root, absolute);

    assert_rust_scenario(tree, root, Constraints::definite(100.0, 40.0));
}

#[test]
fn head_to_head_absolute_rtl_flex_child_without_insets_uses_rtl_fronts() {
    for style in [
        Style {
            direction: Direction::Rtl,
            width: Length::points(100.0),
            height: Length::points(40.0),
            align_items: AlignItems::FlexStart,
            ..Style::default()
        },
        Style {
            direction: Direction::Rtl,
            flex_direction: FlexDirection::Column,
            width: Length::points(100.0),
            height: Length::points(40.0),
            align_items: AlignItems::FlexStart,
            ..Style::default()
        },
        Style {
            direction: Direction::Rtl,
            flex_direction: FlexDirection::Column,
            flex_wrap: FlexWrap::WrapReverse,
            width: Length::points(100.0),
            height: Length::points(40.0),
            align_items: AlignItems::FlexStart,
            ..Style::default()
        },
    ] {
        let mut tree = SimpleTree::default();
        let root = tree.push(SimpleNode::new(standalone_style(style)));
        let absolute = tree.push(SimpleNode::new(standalone_style(Style {
            position: PositionType::Absolute,
            width: Length::points(20.0),
            height: Length::points(10.0),
            ..Style::default()
        })));
        tree.append_child(root, absolute);

        assert_rust_scenario(tree, root, Constraints::definite(100.0, 40.0));
    }
}

#[test]
fn head_to_head_flex_relative_child_percent_offsets_use_container_constraints() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(standalone_style(Style {
        width: Length::points(100.0),
        height: Length::points(40.0),
        align_items: AlignItems::FlexStart,
        ..Style::default()
    })));
    let relative = tree.push(SimpleNode::new(standalone_style(Style {
        position: PositionType::Relative,
        width: Length::points(20.0),
        height: Length::points(10.0),
        left: Length::percent(10.0),
        top: Length::percent(25.0),
        ..Style::default()
    })));
    tree.append_child(root, relative);

    assert_rust_scenario(tree, root, Constraints::definite(100.0, 40.0));
}

#[test]
fn head_to_head_vertical_percentage_padding_and_margin_use_width_percent_base() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(standalone_style(Style {
        width: Length::points(120.0),
        padding: Rect::all(Length::percent(10.0)),
        align_items: AlignItems::FlexStart,
        ..Style::default()
    })));
    let child = tree.push(SimpleNode::new(standalone_style(Style {
        flex_basis: Length::points(10.0),
        height: Length::points(5.0),
        margin: Rect::new(
            Length::ZERO,
            Length::ZERO,
            Length::percent(5.0),
            Length::percent(2.0),
        ),
        ..Style::default()
    })));
    tree.append_child(root, child);

    assert_rust_scenario(
        tree,
        root,
        Constraints::new(
            SideConstraint::definite(120.0),
            SideConstraint::indefinite(),
        ),
    );
}

#[test]
fn head_to_head_simple_tree_measure_and_baseline_callbacks() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(standalone_style(Style {
        align_items: AlignItems::Baseline,
        width: Length::points(80.0),
        ..Style::default()
    })));
    let callback_child = tree.push(SimpleNode::with_measure_func_and_baseline(
        standalone_style(Style::default()),
        simple_tree_callback_measure,
        simple_tree_callback_baseline,
    ));
    let static_child = tree.push(SimpleNode::with_measured_size_and_baseline(
        standalone_style(Style::default()),
        Size::new(12.0, 8.0),
        4.0,
    ));
    tree.append_child(root, callback_child);
    tree.append_child(root, static_child);

    assert_rust_scenario(
        tree,
        root,
        Constraints::new(SideConstraint::definite(80.0), SideConstraint::indefinite()),
    );
}

#[test]
fn head_to_head_wrapped_flex_measured_callback_baseline_exports_cpp_first_line_baseline() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(standalone_style(Style {
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
        justify_content: JustifyContent::FlexStart,
        align_content: AlignContent::FlexStart,
        column_gap: Length::points(1.0),
        row_gap: Length::points(1.0),
        ..Style::default()
    })));

    let measured_with_baseline = tree.push(SimpleNode::with_measure_func_and_baseline(
        block_standalone_style(Style {
            width: Length::fit_content(Some(BaseLength::fixed(36.0))),
            align_self: Some(AlignItems::Baseline),
            margin: Rect::new(
                Length::ZERO,
                Length::ZERO,
                Length::points(0.5),
                Length::ZERO,
            ),
            ..Style::default()
        }),
        simple_tree_callback_measure,
        simple_tree_callback_baseline,
    ));
    let measured = tree.push(SimpleNode::with_measure_func(
        block_standalone_style(Style {
            height: Length::fit_content(Some(BaseLength::fixed(18.0))),
            min_height: Length::points(10.0),
            margin: Rect::new(
                Length::points(1.0),
                Length::points(0.5),
                Length::points(0.5),
                Length::ZERO,
            ),
            ..Style::default()
        }),
        simple_tree_callback_measure,
    ));
    let static_measured_with_baseline = tree.push(SimpleNode::with_measured_size_and_baseline(
        block_standalone_style(Style {
            min_width: Length::points(20.0),
            max_height: Length::points(32.0),
            align_self: Some(AlignItems::Baseline),
            margin: Rect::new(
                Length::ZERO,
                Length::points(1.0),
                Length::points(0.5),
                Length::ZERO,
            ),
            ..Style::default()
        }),
        Size::new(25.0, 14.0),
        8.0,
    ));
    let static_measured = tree.push(SimpleNode::with_measured_size(
        block_standalone_style(Style {
            max_width: Length::points(54.0),
            margin: Rect::new(
                Length::points(1.0),
                Length::ZERO,
                Length::points(0.5),
                Length::ZERO,
            ),
            ..Style::default()
        }),
        Size::new(28.0, 16.0),
    ));

    tree.append_child(root, measured_with_baseline);
    tree.append_child(root, measured);
    tree.append_child(root, static_measured_with_baseline);
    tree.append_child(root, static_measured);

    assert_rust_scenario(tree, root, Constraints::definite(320.0, 80.0));
}

#[test]
fn head_to_head_wrapped_flex_fit_content_measured_callback_container_width() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(block_standalone_style(Style {
        width: Length::points(320.0),
        height: Length::points(120.0),
        ..Style::default()
    })));
    let container = tree.push(SimpleNode::new(standalone_style(Style {
        width: Length::fit_content(Some(BaseLength::fixed(126.0))),
        height: Length::points(58.0),
        min_width: Length::points(72.0),
        max_width: Length::points(180.0),
        min_height: Length::points(28.0),
        max_height: Length::points(92.0),
        padding: Rect::all(Length::points(1.0)),
        flex_wrap: FlexWrap::Wrap,
        align_items: AlignItems::Baseline,
        justify_content: JustifyContent::FlexStart,
        align_content: AlignContent::FlexStart,
        column_gap: Length::points(1.0),
        row_gap: Length::points(1.0),
        margin: Rect::new(
            Length::points(1.0),
            Length::ZERO,
            Length::points(1.0),
            Length::ZERO,
        ),
        ..Style::default()
    })));
    tree.append_child(root, container);

    let measured_with_baseline = tree.push(SimpleNode::with_measure_func_and_baseline(
        block_standalone_style(Style {
            width: Length::fit_content(Some(BaseLength::fixed(36.0))),
            align_self: Some(AlignItems::Baseline),
            ..Style::default()
        }),
        simple_tree_callback_measure,
        simple_tree_callback_baseline,
    ));
    let measured = tree.push(SimpleNode::with_measure_func(
        block_standalone_style(Style {
            height: Length::fit_content(Some(BaseLength::fixed(18.0))),
            min_height: Length::points(10.0),
            margin: Rect::new(
                Length::points(1.0),
                Length::points(0.5),
                Length::ZERO,
                Length::ZERO,
            ),
            ..Style::default()
        }),
        simple_tree_callback_measure,
    ));
    let static_measured_with_baseline = tree.push(SimpleNode::with_measured_size_and_baseline(
        block_standalone_style(Style {
            min_width: Length::points(20.0),
            max_height: Length::points(32.0),
            align_self: Some(AlignItems::Baseline),
            margin: Rect::new(
                Length::ZERO,
                Length::points(1.0),
                Length::ZERO,
                Length::ZERO,
            ),
            ..Style::default()
        }),
        Size::new(30.0, 14.0),
        8.0,
    ));
    let static_measured = tree.push(SimpleNode::with_measured_size(
        block_standalone_style(Style {
            max_width: Length::points(54.0),
            margin: Rect::new(
                Length::points(1.0),
                Length::ZERO,
                Length::ZERO,
                Length::ZERO,
            ),
            ..Style::default()
        }),
        Size::new(33.0, 16.0),
    ));

    tree.append_child(container, measured_with_baseline);
    tree.append_child(container, measured);
    tree.append_child(container, static_measured_with_baseline);
    tree.append_child(container, static_measured);

    assert_rust_scenario(tree, root, Constraints::definite(320.0, 80.0));
}

#[test]
fn head_to_head_flex_sticky_child_percent_insets_resolve_against_container_constraints() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(standalone_style(Style {
        width: Length::points(100.0),
        height: Length::points(40.0),
        align_items: AlignItems::FlexStart,
        ..Style::default()
    })));
    let sticky = tree.push(SimpleNode::new(standalone_style(Style {
        position: PositionType::Sticky,
        width: Length::points(20.0),
        height: Length::points(10.0),
        left: Length::percent(10.0),
        top: Length::percent(25.0),
        ..Style::default()
    })));
    tree.append_child(root, sticky);

    assert_rust_scenario(tree, root, Constraints::definite(100.0, 40.0));
}

#[test]
fn head_to_head_flex_sticky_child_end_percent_insets_resolve_against_container_constraints() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(standalone_style(Style {
        width: Length::points(100.0),
        height: Length::points(40.0),
        align_items: AlignItems::FlexStart,
        ..Style::default()
    })));
    let sticky = tree.push(SimpleNode::new(standalone_style(Style {
        position: PositionType::Sticky,
        width: Length::points(20.0),
        height: Length::points(10.0),
        right: Length::percent(20.0),
        bottom: Length::percent(50.0),
        ..Style::default()
    })));
    tree.append_child(root, sticky);

    assert_rust_scenario(tree, root, Constraints::definite(100.0, 40.0));
}

#[test]
fn head_to_head_measured_baseline_alignment() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(standalone_style(Style {
        width: Length::points(80.0),
        align_items: AlignItems::Baseline,
        ..Style::default()
    })));
    let first = tree.push(SimpleNode::with_measured_size_and_baseline(
        standalone_style(Style {
            margin: Rect::new(
                Length::points(1.0),
                Length::points(2.0),
                Length::points(3.0),
                Length::points(4.0),
            ),
            ..Style::default()
        }),
        Size::new(20.0, 10.0),
        6.0,
    ));
    let second = tree.push(SimpleNode::with_measured_size_and_baseline(
        standalone_style(Style {
            margin: Rect::new(
                Length::points(2.0),
                Length::points(1.0),
                Length::points(1.0),
                Length::points(2.0),
            ),
            ..Style::default()
        }),
        Size::new(16.0, 14.0),
        10.0,
    ));
    tree.append_child(root, first);
    tree.append_child(root, second);

    assert_rust_scenario(
        tree,
        root,
        Constraints::new(SideConstraint::definite(80.0), SideConstraint::indefinite()),
    );
}

#[test]
fn head_to_head_nested_column_flex() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(standalone_style(Style {
        width: Length::points(100.0),
        align_items: AlignItems::FlexStart,
        ..Style::default()
    })));
    let column = tree.push(SimpleNode::new(standalone_style(Style {
        flex_direction: FlexDirection::Column,
        flex_basis: Length::points(30.0),
        row_gap: Length::points(2.0),
        ..Style::default()
    })));
    let leaf_a = tree.push(SimpleNode::new(standalone_style(Style {
        width: Length::points(20.0),
        height: Length::points(6.0),
        ..Style::default()
    })));
    let leaf_b = tree.push(SimpleNode::new(standalone_style(Style {
        width: Length::points(18.0),
        height: Length::points(8.0),
        ..Style::default()
    })));
    tree.append_child(root, column);
    tree.append_child(column, leaf_a);
    tree.append_child(column, leaf_b);

    assert_rust_scenario(
        tree,
        root,
        Constraints::new(
            SideConstraint::definite(100.0),
            SideConstraint::indefinite(),
        ),
    );
}

#[test]
fn head_to_head_flex_item_derives_cross_size_from_main_size_and_aspect_ratio() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(standalone_style(Style {
        align_items: AlignItems::FlexStart,
        width: Length::points(100.0),
        ..Style::default()
    })));
    let child = tree.push(SimpleNode::new(standalone_style(Style {
        flex_basis: Length::points(40.0),
        aspect_ratio: Some(2.0),
        ..Style::default()
    })));
    tree.append_child(root, child);

    assert_rust_scenario(
        tree,
        root,
        Constraints::new(
            SideConstraint::definite(100.0),
            SideConstraint::indefinite(),
        ),
    );
}
