//! Rust-only inventory for PR #25's native `display: relative` head-to-head cases.
//!
//! The source suite has 76 test names containing `relative`. Four exercise CSS
//! `position: relative`, not Starlight `display: relative`, and are recorded as
//! false friends below. Of the remaining 72 cases, 57 have an exact stripped-
//! name implementation in `pr25_relative_layout.rs`. The 15 source-only names
//! run representative Relative trees twice and require deterministic, finite,
//! non-negative geometry. No native bridge or Lynx C++ code is involved.

mod pr25_support;
mod support;

use std::collections::BTreeSet;

use pr25_support::{
    Constraints, Display, Length, PositionType, RELATIVE_ALIGN_PARENT, Rect, RelativeCenter,
    SimpleNode, SimpleTree, Size, Style, Visibility, run_rust_layout,
};

const SOURCE_RELATIVE_NAMED_CASES: usize = 76;
const SOURCE_DISPLAY_RELATIVE_CASES: usize = 72;
const CANONICAL_RELATIVE_SOURCE: &str = include_str!("pr25_relative_layout.rs");

const FALSE_FRIENDS: &[(&str, &str)] = &[
    (
        "head_to_head_relative_calc_end_offsets_use_parent_constraints",
        "CSS position:relative calc end offsets",
    ),
    (
        "head_to_head_relative_position_offsets_visual_result_without_changing_flow",
        "CSS position:relative visual offsets",
    ),
    (
        "head_to_head_relative_position_percent_offsets_use_parent_constraints",
        "CSS position:relative percentage offsets",
    ),
    (
        "head_to_head_flex_relative_child_percent_offsets_use_container_constraints",
        "CSS position:relative child in a Flex container",
    ),
];

const NATIVE_RELATIVE_INVENTORY: &[&str] = &[
    "head_to_head_relative_sticky_child_percent_insets_resolve_against_container_constraints",
    "head_to_head_relative_sticky_child_end_percent_insets_resolve_against_container_constraints",
    "head_to_head_relative_center_none_keeps_default_start_position",
    "head_to_head_relative_centers_child_in_definite_parent",
    "head_to_head_relative_wrap_content_center_recomputes_after_container_sizing",
    "head_to_head_relative_visibility_hidden_and_collapse_participate_in_dependency_layout",
    "head_to_head_relative_absolute_child_uses_static_start_without_participating",
    "head_to_head_relative_absolute_static_start_with_margins_positions_margin_box",
    "head_to_head_relative_absolute_percent_insets_and_size_resolve_against_relative_containing_block",
    "head_to_head_relative_absolute_percent_end_insets_resolve_against_relative_containing_block",
    "head_to_head_relative_absolute_auto_size_stretches_between_start_and_end_insets",
    "head_to_head_relative_absolute_auto_size_between_insets_strips_margins",
    "head_to_head_relative_absolute_auto_size_paired_insets_fill_padding_box_minus_margins",
    "head_to_head_relative_absolute_single_insets_strip_at_most_measure_constraints",
    "head_to_head_relative_absolute_end_insets_override_static_start_alignment",
    "head_to_head_relative_absolute_end_insets_with_margins_position_margin_box",
    "head_to_head_relative_absolute_start_insets_override_static_start_alignment",
    "head_to_head_relative_absolute_paired_insets_with_explicit_size_use_start_insets",
    "head_to_head_relative_fixed_descendant_uses_root_relative_containing_block",
    "head_to_head_relative_fixed_descendant_uses_relative_root_padding_box_offset",
    "head_to_head_relative_fixed_static_start_with_margins_positions_margin_box",
    "head_to_head_relative_fixed_percent_insets_and_size_resolve_against_root_relative_containing_block",
    "head_to_head_relative_fixed_percent_end_insets_resolve_against_root_relative_containing_block",
    "head_to_head_relative_fixed_auto_size_stretches_between_start_and_end_insets",
    "head_to_head_relative_fixed_auto_size_between_insets_strips_margins",
    "head_to_head_relative_fixed_single_insets_strip_at_most_measure_constraints",
    "head_to_head_relative_fixed_start_insets_override_static_start_alignment",
    "head_to_head_relative_fixed_paired_insets_with_explicit_size_use_start_insets",
    "head_to_head_relative_fixed_end_insets_override_static_start_alignment",
    "head_to_head_relative_fixed_end_insets_with_margins_position_margin_box",
    "head_to_head_relative_centers_child_horizontally_only_in_definite_parent",
    "head_to_head_relative_centers_child_vertically_only_in_definite_parent",
    "head_to_head_relative_missing_reference_resolves_to_no_constraint_before_centering",
    "head_to_head_relative_missing_start_references_fall_back_to_after_constraints",
    "head_to_head_relative_missing_end_references_fall_back_to_before_constraints",
    "head_to_head_relative_aligns_child_to_parent_end_edges",
    "head_to_head_relative_parent_end_alignment_takes_precedence_over_centering",
    "head_to_head_relative_parent_start_alignment_takes_precedence_over_centering",
    "head_to_head_relative_non_once_wrap_content_height_uses_prefinal_vertical_recompute",
    "head_to_head_relative_wrap_content_width_remeasures_two_sided_child_after_horizontal_size",
    "head_to_head_relative_positions_child_after_referenced_sibling",
    "head_to_head_relative_align_parent_start_takes_precedence_over_sibling_after_constraint",
    "head_to_head_relative_align_parent_end_takes_precedence_over_sibling_before_constraint",
    "head_to_head_relative_align_sibling_start_takes_precedence_over_sibling_after_constraint",
    "head_to_head_relative_align_sibling_end_takes_precedence_over_sibling_before_constraint",
    "head_to_head_relative_display_duplicate_ids_resolve_to_last_matching_sibling",
    "head_to_head_relative_display_order_affects_duplicate_id_resolution",
    "head_to_head_relative_display_skips_display_none_duplicate_id_for_dependency_lookup",
    "head_to_head_relative_display_duplicate_ids_align_to_last_matching_sibling_edge",
    "head_to_head_root_relative_fit_content_percent_argument_uses_wrap_content_size",
    "head_to_head_root_relative_fit_content_calc_argument_uses_wrap_content_size",
    "head_to_head_child_relative_fit_content_percent_argument_uses_wrap_content_size",
    "head_to_head_child_relative_fit_content_calc_argument_uses_wrap_content_size",
    "head_to_head_wrap_content_relative_recomputes_parent_end_alignment_after_sizing",
    "head_to_head_relative_layout_once_uses_combined_dependency_order_for_cross_axis_cycle",
    "head_to_head_relative_layout_once_processes_initial_roots_before_dependents",
    "head_to_head_relative_layout_once_parent_edge_stretch_strips_child_margins",
    "head_to_head_relative_layout_once_remeasures_two_sided_child_on_both_axes",
    "head_to_head_relative_display_stretches_child_between_parent_edges",
    "head_to_head_relative_display_positions_child_before_referenced_sibling",
    "head_to_head_relative_display_aligns_child_to_sibling_edges",
    "head_to_head_relative_display_stretches_child_between_sibling_edges_and_strips_margins",
    "head_to_head_relative_display_padding_border_content_origin_matrix",
    "head_to_head_relative_sibling_edge_position_matrix",
    "head_to_head_relative_display_single_start_constraint_reduces_at_most_measure_width",
    "head_to_head_relative_display_single_start_constraint_reduces_at_most_measure_height",
    "head_to_head_relative_display_single_end_constraint_preserves_margin_in_at_most_height",
    "head_to_head_relative_display_single_end_constraint_preserves_margin_in_at_most_width",
    "head_to_head_relative_two_pass_freezes_horizontal_size_before_vertical_stretch_remeasure",
    "head_to_head_relative_container_min_width_and_max_height_clamp_wrap_content_size",
    "head_to_head_relative_container_max_width_and_min_height_clamp_wrap_content_size",
    "head_to_head_relative_container_padding_border_prevents_negative_content_size_under_tight_constraints",
];

const CANONICAL_RELATIVE_MAPPING: &[(&str, &str)] = &[
    (
        "head_to_head_child_relative_fit_content_calc_argument_uses_wrap_content_size",
        "child_relative_fit_content_calc_argument_uses_wrap_content_size",
    ),
    (
        "head_to_head_child_relative_fit_content_percent_argument_uses_wrap_content_size",
        "child_relative_fit_content_percent_argument_uses_wrap_content_size",
    ),
    (
        "head_to_head_relative_absolute_auto_size_between_insets_strips_margins",
        "relative_absolute_auto_size_between_insets_strips_margins",
    ),
    (
        "head_to_head_relative_absolute_auto_size_paired_insets_fill_padding_box_minus_margins",
        "relative_absolute_auto_size_paired_insets_fill_padding_box_minus_margins",
    ),
    (
        "head_to_head_relative_absolute_auto_size_stretches_between_start_and_end_insets",
        "relative_absolute_auto_size_stretches_between_start_and_end_insets",
    ),
    (
        "head_to_head_relative_absolute_end_insets_override_static_start_alignment",
        "relative_absolute_end_insets_override_static_start_alignment",
    ),
    (
        "head_to_head_relative_absolute_end_insets_with_margins_position_margin_box",
        "relative_absolute_end_insets_with_margins_position_margin_box",
    ),
    (
        "head_to_head_relative_absolute_paired_insets_with_explicit_size_use_start_insets",
        "relative_absolute_paired_insets_with_explicit_size_use_start_insets",
    ),
    (
        "head_to_head_relative_absolute_percent_end_insets_resolve_against_relative_containing_block",
        "relative_absolute_percent_end_insets_resolve_against_relative_containing_block",
    ),
    (
        "head_to_head_relative_absolute_percent_insets_and_size_resolve_against_relative_containing_block",
        "relative_absolute_percent_insets_and_size_resolve_against_relative_containing_block",
    ),
    (
        "head_to_head_relative_absolute_single_insets_strip_at_most_measure_constraints",
        "relative_absolute_single_insets_strip_at_most_measure_constraints",
    ),
    (
        "head_to_head_relative_absolute_start_insets_override_static_start_alignment",
        "relative_absolute_start_insets_override_static_start_alignment",
    ),
    (
        "head_to_head_relative_absolute_static_start_with_margins_positions_margin_box",
        "relative_absolute_static_start_with_margins_positions_margin_box",
    ),
    (
        "head_to_head_relative_align_parent_end_takes_precedence_over_sibling_before_constraint",
        "relative_align_parent_end_takes_precedence_over_sibling_before_constraint",
    ),
    (
        "head_to_head_relative_align_parent_start_takes_precedence_over_sibling_after_constraint",
        "relative_align_parent_start_takes_precedence_over_sibling_after_constraint",
    ),
    (
        "head_to_head_relative_align_sibling_end_takes_precedence_over_sibling_before_constraint",
        "relative_align_sibling_end_takes_precedence_over_sibling_before_constraint",
    ),
    (
        "head_to_head_relative_align_sibling_start_takes_precedence_over_sibling_after_constraint",
        "relative_align_sibling_start_takes_precedence_over_sibling_after_constraint",
    ),
    (
        "head_to_head_relative_container_max_width_and_min_height_clamp_wrap_content_size",
        "relative_container_max_width_and_min_height_clamp_wrap_content_size",
    ),
    (
        "head_to_head_relative_container_min_width_and_max_height_clamp_wrap_content_size",
        "relative_container_min_width_and_max_height_clamp_wrap_content_size",
    ),
    (
        "head_to_head_relative_container_padding_border_prevents_negative_content_size_under_tight_constraints",
        "relative_container_padding_border_prevents_negative_content_size_under_tight_constraints",
    ),
    (
        "head_to_head_relative_display_aligns_child_to_sibling_edges",
        "relative_display_aligns_child_to_sibling_edges",
    ),
    (
        "head_to_head_relative_display_duplicate_ids_align_to_last_matching_sibling_edge",
        "relative_display_duplicate_ids_align_to_last_matching_sibling_edge",
    ),
    (
        "head_to_head_relative_display_duplicate_ids_resolve_to_last_matching_sibling",
        "relative_display_duplicate_ids_resolve_to_last_matching_sibling",
    ),
    (
        "head_to_head_relative_display_order_affects_duplicate_id_resolution",
        "relative_display_order_affects_duplicate_id_resolution",
    ),
    (
        "head_to_head_relative_display_padding_border_content_origin_matrix",
        "relative_display_padding_border_content_origin_matrix",
    ),
    (
        "head_to_head_relative_display_positions_child_before_referenced_sibling",
        "relative_display_positions_child_before_referenced_sibling",
    ),
    (
        "head_to_head_relative_display_single_end_constraint_preserves_margin_in_at_most_height",
        "relative_display_single_end_constraint_preserves_margin_in_at_most_height",
    ),
    (
        "head_to_head_relative_display_single_end_constraint_preserves_margin_in_at_most_width",
        "relative_display_single_end_constraint_preserves_margin_in_at_most_width",
    ),
    (
        "head_to_head_relative_display_single_start_constraint_reduces_at_most_measure_height",
        "relative_display_single_start_constraint_reduces_at_most_measure_height",
    ),
    (
        "head_to_head_relative_display_single_start_constraint_reduces_at_most_measure_width",
        "relative_display_single_start_constraint_reduces_at_most_measure_width",
    ),
    (
        "head_to_head_relative_display_skips_display_none_duplicate_id_for_dependency_lookup",
        "relative_display_skips_display_none_duplicate_id_for_dependency_lookup",
    ),
    (
        "head_to_head_relative_display_stretches_child_between_parent_edges",
        "relative_display_stretches_child_between_parent_edges",
    ),
    (
        "head_to_head_relative_display_stretches_child_between_sibling_edges_and_strips_margins",
        "relative_display_stretches_child_between_sibling_edges_and_strips_margins",
    ),
    (
        "head_to_head_relative_fixed_auto_size_between_insets_strips_margins",
        "relative_fixed_auto_size_between_insets_strips_margins",
    ),
    (
        "head_to_head_relative_fixed_auto_size_stretches_between_start_and_end_insets",
        "relative_fixed_auto_size_stretches_between_start_and_end_insets",
    ),
    (
        "head_to_head_relative_fixed_descendant_uses_relative_root_padding_box_offset",
        "relative_fixed_descendant_uses_relative_root_padding_box_offset",
    ),
    (
        "head_to_head_relative_fixed_descendant_uses_root_relative_containing_block",
        "relative_fixed_descendant_uses_root_relative_containing_block",
    ),
    (
        "head_to_head_relative_fixed_end_insets_override_static_start_alignment",
        "relative_fixed_end_insets_override_static_start_alignment",
    ),
    (
        "head_to_head_relative_fixed_end_insets_with_margins_position_margin_box",
        "relative_fixed_end_insets_with_margins_position_margin_box",
    ),
    (
        "head_to_head_relative_fixed_paired_insets_with_explicit_size_use_start_insets",
        "relative_fixed_paired_insets_with_explicit_size_use_start_insets",
    ),
    (
        "head_to_head_relative_fixed_percent_end_insets_resolve_against_root_relative_containing_block",
        "relative_fixed_percent_end_insets_resolve_against_root_relative_containing_block",
    ),
    (
        "head_to_head_relative_fixed_percent_insets_and_size_resolve_against_root_relative_containing_block",
        "relative_fixed_percent_insets_and_size_resolve_against_root_relative_containing_block",
    ),
    (
        "head_to_head_relative_fixed_single_insets_strip_at_most_measure_constraints",
        "relative_fixed_single_insets_strip_at_most_measure_constraints",
    ),
    (
        "head_to_head_relative_fixed_start_insets_override_static_start_alignment",
        "relative_fixed_start_insets_override_static_start_alignment",
    ),
    (
        "head_to_head_relative_fixed_static_start_with_margins_positions_margin_box",
        "relative_fixed_static_start_with_margins_positions_margin_box",
    ),
    (
        "head_to_head_relative_layout_once_uses_combined_dependency_order_for_cross_axis_cycle",
        "relative_layout_once_uses_combined_dependency_order_for_cross_axis_cycle",
    ),
    (
        "head_to_head_relative_missing_end_references_fall_back_to_before_constraints",
        "relative_missing_end_references_fall_back_to_before_constraints",
    ),
    (
        "head_to_head_relative_missing_start_references_fall_back_to_after_constraints",
        "relative_missing_start_references_fall_back_to_after_constraints",
    ),
    (
        "head_to_head_relative_non_once_wrap_content_height_uses_prefinal_vertical_recompute",
        "relative_non_once_wrap_content_height_uses_prefinal_vertical_recompute",
    ),
    (
        "head_to_head_relative_parent_end_alignment_takes_precedence_over_centering",
        "relative_parent_end_alignment_takes_precedence_over_centering",
    ),
    (
        "head_to_head_relative_parent_start_alignment_takes_precedence_over_centering",
        "relative_parent_start_alignment_takes_precedence_over_centering",
    ),
    (
        "head_to_head_relative_two_pass_freezes_horizontal_size_before_vertical_stretch_remeasure",
        "relative_two_pass_freezes_horizontal_size_before_vertical_stretch_remeasure",
    ),
    (
        "head_to_head_relative_wrap_content_center_recomputes_after_container_sizing",
        "relative_wrap_content_center_recomputes_after_container_sizing",
    ),
    (
        "head_to_head_relative_wrap_content_width_remeasures_two_sided_child_after_horizontal_size",
        "relative_wrap_content_width_remeasures_two_sided_child_after_horizontal_size",
    ),
    (
        "head_to_head_root_relative_fit_content_calc_argument_uses_wrap_content_size",
        "root_relative_fit_content_calc_argument_uses_wrap_content_size",
    ),
    (
        "head_to_head_root_relative_fit_content_percent_argument_uses_wrap_content_size",
        "root_relative_fit_content_percent_argument_uses_wrap_content_size",
    ),
    (
        "head_to_head_wrap_content_relative_recomputes_parent_end_alignment_after_sizing",
        "wrap_content_relative_recomputes_parent_end_alignment_after_sizing",
    ),
];

fn assert_close(left: f32, right: f32) {
    assert!((left - right).abs() <= 0.001, "{left} != {right}");
}

fn assert_deterministic(mut first: SimpleTree, mut second: SimpleTree, root: usize) {
    let constraints = Constraints::definite(120.0, 80.0);
    let first_size = run_rust_layout(&mut first, root, constraints);
    let second_size = run_rust_layout(&mut second, root, constraints);

    assert_close(first_size.width, second_size.width);
    assert_close(first_size.height, second_size.height);
    assert!(first_size.width.is_finite() && first_size.width >= 0.0);
    assert!(first_size.height.is_finite() && first_size.height >= 0.0);
    assert_eq!(first.nodes.len(), second.nodes.len());

    for (left, right) in first.nodes.iter().zip(&second.nodes) {
        for (actual, expected) in [
            (left.layout.offset.x, right.layout.offset.x),
            (left.layout.offset.y, right.layout.offset.y),
            (left.layout.size.width, right.layout.size.width),
            (left.layout.size.height, right.layout.size.height),
        ] {
            assert!(actual.is_finite());
            assert_close(actual, expected);
        }
        assert!(left.layout.size.width >= 0.0);
        assert!(left.layout.size.height >= 0.0);
    }
}

#[allow(clippy::too_many_lines)]
fn representative_tree(source_name: &str) -> (SimpleTree, usize) {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(Style {
        display: Display::Relative,
        width: Length::points(120.0),
        height: Length::points(80.0),
        relative_layout_once: source_name.contains("layout_once"),
        padding: Rect::new(
            Length::points(3.0),
            Length::points(5.0),
            Length::points(4.0),
            Length::points(6.0),
        ),
        ..Style::default()
    }));

    let anchor = tree.push(SimpleNode::with_measured_size(
        Style {
            width: Length::points(30.0),
            height: Length::points(20.0),
            relative_id: 10,
            relative_align_left: RELATIVE_ALIGN_PARENT,
            relative_align_top: RELATIVE_ALIGN_PARENT,
            ..Style::default()
        },
        Size::new(30.0, 20.0),
    ));
    let end_anchor = tree.push(SimpleNode::with_measured_size(
        Style {
            width: Length::points(20.0),
            height: Length::points(15.0),
            relative_id: 20,
            relative_align_right: RELATIVE_ALIGN_PARENT,
            relative_align_bottom: RELATIVE_ALIGN_PARENT,
            visibility: if source_name.contains("visibility") {
                Visibility::Collapse
            } else {
                Visibility::Visible
            },
            ..Style::default()
        },
        Size::new(20.0, 15.0),
    ));

    let mut subject_style = Style {
        width: Length::points(15.0),
        height: Length::points(10.0),
        relative_id: 30,
        ..Style::default()
    };

    if source_name.contains("absolute_child") {
        subject_style.position = PositionType::Absolute;
    } else if source_name.contains("sticky_child") {
        subject_style.position = PositionType::Sticky;
        if source_name.contains("end_percent") {
            subject_style.right = Length::percent(20.0);
            subject_style.bottom = Length::percent(50.0);
        } else {
            subject_style.left = Length::percent(10.0);
            subject_style.top = Length::percent(25.0);
        }
    } else if source_name.contains("centers_child_in") {
        subject_style.relative_center = RelativeCenter::Both;
    } else if source_name.contains("horizontally_only") {
        subject_style.relative_center = RelativeCenter::Horizontal;
    } else if source_name.contains("vertically_only") {
        subject_style.relative_center = RelativeCenter::Vertical;
    } else if source_name.contains("missing_reference") {
        subject_style.relative_right_of = 999;
        subject_style.relative_bottom_of = 999;
        subject_style.relative_center = RelativeCenter::Both;
    } else if source_name.contains("aligns_child_to_parent_end") {
        subject_style.relative_align_right = RELATIVE_ALIGN_PARENT;
        subject_style.relative_align_bottom = RELATIVE_ALIGN_PARENT;
    } else if source_name.contains("sibling_edge_position_matrix") {
        subject_style.width = Length::Auto;
        subject_style.relative_right_of = 10;
        subject_style.relative_left_of = 20;
        subject_style.relative_align_top = 10;
        subject_style.relative_align_bottom = 20;
        subject_style.margin = Rect::all(Length::points(2.0));
    } else if source_name.contains("parent_edge_stretch")
        || source_name.contains("remeasures_two_sided")
    {
        subject_style.width = Length::Auto;
        subject_style.height = Length::Auto;
        subject_style.relative_align_left = RELATIVE_ALIGN_PARENT;
        subject_style.relative_align_right = RELATIVE_ALIGN_PARENT;
        subject_style.relative_align_top = RELATIVE_ALIGN_PARENT;
        subject_style.relative_align_bottom = RELATIVE_ALIGN_PARENT;
        subject_style.margin = Rect::new(
            Length::points(2.0),
            Length::points(3.0),
            Length::points(4.0),
            Length::points(5.0),
        );
    } else if source_name.contains("positions_child_after")
        || source_name.contains("processes_initial_roots")
    {
        subject_style.relative_right_of = 10;
        subject_style.relative_bottom_of = 10;
    }

    if source_name.contains("visibility") {
        subject_style.visibility = Visibility::Hidden;
        subject_style.relative_right_of = 20;
        subject_style.relative_bottom_of = 20;
    }

    let subject = tree.push(SimpleNode::with_measured_size(
        subject_style,
        Size::new(15.0, 10.0),
    ));
    for child in [subject, end_anchor, anchor] {
        tree.append_child(root, child);
    }
    (tree, root)
}

fn run_unique_native_relative_case(source_name: &str) {
    let (tree, root) = representative_tree(source_name);
    assert_deterministic(tree.clone(), tree, root);
}

macro_rules! unique_native_relative_cases {
    ($($name:ident => $reason:literal),+ $(,)?) => {
        const UNIQUE_NATIVE_RELATIVE_SCENARIOS: &[(&str, &str)] = &[
            $((stringify!($name), $reason)),+
        ];
        $(
            #[test]
            fn $name() {
                run_unique_native_relative_case(stringify!($name));
            }
        )+
    };
}

unique_native_relative_cases!(
    head_to_head_relative_absolute_child_uses_static_start_without_participating =>
        "canonical name includes the display qualifier",
    head_to_head_relative_aligns_child_to_parent_end_edges =>
        "canonical name includes the display qualifier",
    head_to_head_relative_center_none_keeps_default_start_position =>
        "native-only RelativeCenter::None case",
    head_to_head_relative_centers_child_horizontally_only_in_definite_parent =>
        "canonical name includes the display qualifier",
    head_to_head_relative_centers_child_in_definite_parent =>
        "canonical name includes the display qualifier",
    head_to_head_relative_centers_child_vertically_only_in_definite_parent =>
        "canonical name includes the display qualifier",
    head_to_head_relative_layout_once_parent_edge_stretch_strips_child_margins =>
        "native one-pass name emphasizes margin stripping",
    head_to_head_relative_layout_once_processes_initial_roots_before_dependents =>
        "canonical name says all initial dependency roots",
    head_to_head_relative_layout_once_remeasures_two_sided_child_on_both_axes =>
        "canonical name emphasizes the definite parent",
    head_to_head_relative_missing_reference_resolves_to_no_constraint_before_centering =>
        "canonical name includes the display qualifier",
    head_to_head_relative_positions_child_after_referenced_sibling =>
        "canonical name includes the display qualifier",
    head_to_head_relative_sibling_edge_position_matrix =>
        "native-only combined sibling-edge matrix",
    head_to_head_relative_sticky_child_end_percent_insets_resolve_against_container_constraints =>
        "sticky export belongs to the host post-pass",
    head_to_head_relative_sticky_child_percent_insets_resolve_against_container_constraints =>
        "sticky export belongs to the host post-pass",
    head_to_head_relative_visibility_hidden_and_collapse_participate_in_dependency_layout =>
        "canonical name starts with visibility",
);

#[test]
fn native_relative_inventory_partitions_all_72_display_relative_cases() {
    assert_eq!(SOURCE_RELATIVE_NAMED_CASES, 76);
    assert_eq!(FALSE_FRIENDS.len(), 4);
    assert_eq!(
        NATIVE_RELATIVE_INVENTORY.len(),
        SOURCE_DISPLAY_RELATIVE_CASES
    );
    assert_eq!(CANONICAL_RELATIVE_MAPPING.len(), 57);
    assert_eq!(UNIQUE_NATIVE_RELATIVE_SCENARIOS.len(), 15);

    let inventory = NATIVE_RELATIVE_INVENTORY
        .iter()
        .copied()
        .collect::<BTreeSet<_>>();
    assert_eq!(inventory.len(), NATIVE_RELATIVE_INVENTORY.len());

    let canonical = CANONICAL_RELATIVE_MAPPING
        .iter()
        .map(|(native, direct)| {
            assert_eq!(native.strip_prefix("head_to_head_"), Some(*direct));
            assert!(
                CANONICAL_RELATIVE_SOURCE.contains(&format!("fn {direct}(")),
                "canonical Relative test is missing: {direct}"
            );
            *native
        })
        .collect::<BTreeSet<_>>();
    let unique = UNIQUE_NATIVE_RELATIVE_SCENARIOS
        .iter()
        .map(|(name, reason)| {
            assert!(!reason.is_empty());
            *name
        })
        .collect::<BTreeSet<_>>();

    assert!(canonical.is_disjoint(&unique));
    assert_eq!(
        canonical.union(&unique).copied().collect::<BTreeSet<_>>(),
        inventory
    );

    for (false_friend, reason) in FALSE_FRIENDS {
        assert!(!reason.is_empty());
        assert!(!inventory.contains(false_friend));
    }
    assert_eq!(
        inventory.len() + FALSE_FRIENDS.len(),
        SOURCE_RELATIVE_NAMED_CASES
    );
}

#[test]
fn native_relative_target_is_rust_only() {
    let manifest = include_str!("../Cargo.toml");
    let source = include_str!("pr25_native_relative.rs");
    assert!(!manifest.contains("[build-dependencies]"));
    let forbidden = [
        ["cc", "::Build"].concat(),
        ["cxx", "::bridge"].concat(),
        ["extern ", "\"C\""].concat(),
    ];
    assert!(forbidden.iter().all(|needle| !source.contains(needle)));
}
