//! Public-boundary translations of the Flex-focused internal engine and
//! solver tests from PupilTong/lynx#25.

mod pr25_support;
mod support;

use pr25_support::*;

const SOLVER_MAPPINGS: [(&str, &str); 24] = [
    (
        "used_flex_factor_is_grow_only_when_hypothetical_sum_is_less_than_container",
        "flex_factor_selection_uses_hypothetical_sizes_not_flex_base_sum",
    ),
    (
        "unfrozen_items_start_from_flex_base_size_before_distribution",
        "flex_grow_distributes_free_space_proportionally",
    ),
    (
        "distributes_positive_free_space_by_flex_grow",
        "flex_grow_distributes_free_space_proportionally",
    ),
    (
        "initial_free_space_uses_frozen_targets_unfrozen_bases_outer_sizes_and_gap",
        "initial_free_space_uses_frozen_targets_outer_margins_and_gap",
    ),
    (
        "grow_factor_sum_below_one_leaves_part_of_positive_free_space",
        "flex_grow_sum_below_one_leaves_remaining_space_for_justify_content",
    ),
    (
        "shrink_factor_sum_below_one_leaves_part_of_negative_free_space",
        "flex_shrink_sum_below_one_leaves_negative_space_for_justify_content",
    ),
    (
        "distributes_negative_free_space_by_scaled_flex_shrink_factor",
        "flex_shrink_distribution_is_scaled_by_flex_base_size",
    ),
    (
        "freezes_min_violations_and_recomputes_remaining_shrink_space",
        "multiple_min_width_violations_freeze_before_redistributing_flex_shrink_space",
    ),
    (
        "freezes_max_violations_and_recomputes_remaining_grow_space",
        "multiple_max_width_violations_freeze_before_redistributing_flex_grow_space",
    ),
    (
        "clamps_negative_inner_main_sizes_to_zero_before_freezing_min_violations",
        "flex_shrink_negative_inner_size_is_floored_after_outer_margins",
    ),
    (
        "freezes_inflexible_items_to_hypothetical_main_size",
        "zero_flex_grow_freezes_item_before_distributing_positive_free_space",
    ),
    (
        "shrink_mode_freezes_items_with_base_smaller_than_hypothetical_size",
        "min_width_above_flex_basis_freezes_shrinking_item_to_hypothetical_main_size",
    ),
    (
        "flex_percent_base_tracks_definite_and_suppressed_sources",
        "definite_flex_basis_post_flexing_main_size_defines_descendant_percent_flex_basis_base",
    ),
    (
        "flex_justify_interval_covers_distribution_and_overflow_fallbacks",
        "justify_content_negative_free_space_direction_matrix_uses_w3c_fallbacks",
    ),
    (
        "flex_align_content_covers_negative_and_distribution_fallbacks",
        "align_content_space_evenly_uses_negative_space_when_lines_overflow",
    ),
    (
        "flex_cross_axis_auto_margins_resolve_w3c_positive_and_overflow_cases",
        "overflowing_cross_axis_auto_margins_place_overflow_at_cross_end",
    ),
    (
        "flex_basis_and_default_constraints_cover_non_measured_paths",
        "flex_line_length_definite_flex_basis_overrides_main_size_property",
    ),
    (
        "flex_cache_reuse_helpers_cover_mode_matrix_and_guard_paths",
        "stretched_flex_item_relayouts_percent_height_child_with_definite_cross_size",
    ),
    (
        "flex_percent_base_and_min_clamp_helpers_cover_w3c_definiteness_branches",
        "automatic_minimum_uses_aspect_ratio_transferred_size",
    ),
    (
        "flex_line_collection_and_collapsed_resolution_cover_empty_and_collapsed_paths",
        "flex_wrap_collects_zero_sized_item_after_exact_fit_on_same_line",
    ),
    (
        "flex_baseline_helpers_cover_empty_line_and_no_row_cases",
        "flex_column_container_baseline_uses_first_item_baseline_after_main_axis_alignment",
    ),
    (
        "flex_used_margin_writes_main_and_cross_axis_auto_margins",
        "cross_axis_auto_margin_direction_and_wrap_reverse_matrix_resolves_margins",
    ),
    (
        "flex_align_cross_offset_accounts_for_alignment_margin_and_reverse_cross_axis",
        "align_items_cross_axis_direction_and_wrap_reverse_matrix_places_items",
    ),
    (
        "flex_out_of_flow_alignment_maps_static_position_to_flex_axes",
        "absolute_rtl_flex_child_without_insets_uses_physical_fronts",
    ),
];

fn assert_close(actual: f32, expected: f32) {
    assert!(
        (actual - expected).abs() <= 0.01,
        "expected {expected}, got {actual}"
    );
}

fn fixed_leaf(tree: &mut SimpleTree, width: f32, height: f32) -> usize {
    tree.push(SimpleNode::new(Style {
        width: Length::points(width),
        height: Length::points(height),
        ..Style::default()
    }))
}

#[test]
#[allow(clippy::too_many_lines)] // Keep the nine source-engine cases visibly in one matrix.
fn engine_flex_dispatch_matrix_covers_nine_source_invariants() {
    let mut covered = 0usize;

    // Flex percent propagation: the flexed main size becomes the descendant's percentage base.
    {
        let mut tree = SimpleTree::default();
        let root = tree.push(SimpleNode::new(Style {
            display: Display::Flex,
            width: Length::points(100.0),
            height: Length::points(20.0),
            align_items: AlignItems::FlexStart,
            ..Style::default()
        }));
        let item = tree.push(SimpleNode::new(Style {
            display: Display::Flex,
            flex_basis: Length::points(40.0),
            flex_grow: 1.0,
            height: Length::points(20.0),
            ..Style::default()
        }));
        let descendant = tree.push(SimpleNode::new(Style {
            width: Length::percent(50.0),
            height: Length::points(5.0),
            ..Style::default()
        }));
        tree.append_child(root, item);
        tree.append_child(item, descendant);
        run_rust_layout(&mut tree, root, Constraints::definite(100.0, 20.0));
        assert_close(tree.nodes[descendant].layout.size.width, 50.0);
        covered += 1;
    }

    // Grow distribution.
    {
        let mut tree = SimpleTree::default();
        let root = tree.push(SimpleNode::new(Style {
            display: Display::Flex,
            width: Length::points(120.0),
            height: Length::points(20.0),
            ..Style::default()
        }));
        let first = tree.push(SimpleNode::new(Style {
            flex_basis: Length::points(20.0),
            flex_grow: 1.0,
            ..Style::default()
        }));
        let second = tree.push(SimpleNode::new(Style {
            flex_basis: Length::points(20.0),
            flex_grow: 3.0,
            ..Style::default()
        }));
        tree.append_child(root, first);
        tree.append_child(root, second);
        run_rust_layout(&mut tree, root, Constraints::definite(120.0, 20.0));
        assert_close(tree.nodes[first].layout.size.width, 40.0);
        assert_close(tree.nodes[second].layout.size.width, 80.0);
        covered += 1;
    }

    // Host measurement wins for a semantic leaf even when its authored style will later be read
    // as a flex item by its parent.
    {
        let mut tree = SimpleTree::default();
        let root = tree.push(SimpleNode::new(Style {
            display: Display::Flex,
            width: Length::points(50.0),
            align_items: AlignItems::FlexStart,
            ..Style::default()
        }));
        let child = tree.push(SimpleNode::with_measured_size_and_baseline(
            Style::default(),
            Size::new(12.0, 8.0),
            5.0,
        ));
        tree.append_child(root, child);
        run_rust_layout(
            &mut tree,
            root,
            Constraints::new(SideConstraint::definite(50.0), SideConstraint::indefinite()),
        );
        assert_eq!(tree.nodes[child].layout.size, Size::new(12.0, 8.0));
        assert_eq!(tree.nodes[child].layout.baseline, Some(5.0));
        covered += 1;
    }

    // display:none cleanup at the Flex dispatch boundary.
    {
        let mut tree = SimpleTree::default();
        let root = tree.push(SimpleNode::new(Style {
            display: Display::Flex,
            width: Length::points(20.0),
            height: Length::points(10.0),
            ..Style::default()
        }));
        let hidden = tree.push(SimpleNode::new(Style {
            display: Display::None,
            width: Length::points(99.0),
            height: Length::points(99.0),
            ..Style::default()
        }));
        tree.append_child(root, hidden);
        run_rust_layout(&mut tree, root, Constraints::definite(20.0, 10.0));
        assert_eq!(tree.nodes[hidden].layout.size, Size::ZERO);
        covered += 1;
    }

    // Stretch relayout exports the nested subtree's final geometry.
    {
        let mut tree = SimpleTree::default();
        let root = tree.push(SimpleNode::new(Style {
            display: Display::Flex,
            width: Length::points(30.0),
            height: Length::points(20.0),
            ..Style::default()
        }));
        let child = tree.push(SimpleNode::new(Style {
            display: Display::Flex,
            ..Style::default()
        }));
        let grandchild = fixed_leaf(&mut tree, 10.0, 20.0);
        tree.append_child(root, child);
        tree.append_child(child, grandchild);
        run_rust_layout(&mut tree, root, Constraints::definite(30.0, 20.0));
        assert_eq!(tree.nodes[child].layout.size, Size::new(10.0, 20.0));
        assert_eq!(tree.nodes[grandchild].layout.size, Size::new(10.0, 20.0));
        covered += 1;
    }

    // justify-content:center.
    {
        let mut tree = SimpleTree::default();
        let root = tree.push(SimpleNode::new(Style {
            display: Display::Flex,
            width: Length::points(100.0),
            justify_content: JustifyContent::Center,
            ..Style::default()
        }));
        let first = fixed_leaf(&mut tree, 20.0, 10.0);
        let second = fixed_leaf(&mut tree, 20.0, 10.0);
        tree.append_child(root, first);
        tree.append_child(root, second);
        run_rust_layout(
            &mut tree,
            root,
            Constraints::new(
                SideConstraint::definite(100.0),
                SideConstraint::indefinite(),
            ),
        );
        assert_close(tree.nodes[first].layout.offset.x, 30.0);
        assert_close(tree.nodes[second].layout.offset.x, 50.0);
        covered += 1;
    }

    // `fr` outside Grid is not CSS Flex syntax. The compatibility host lowers this source-only
    // raw-value case to auto, preserving neutron-star's standards-oriented protocol boundary.
    {
        let mut tree = SimpleTree::default();
        let root = tree.push(SimpleNode::new(Style {
            display: Display::Flex,
            width: Length::points(100.0),
            height: Length::points(20.0),
            align_items: AlignItems::FlexStart,
            ..Style::default()
        }));
        let child = tree.push(SimpleNode::new(Style {
            width: Length::points(12.0),
            height: Length::points(10.0),
            flex_basis: Length::Fr(30.0),
            ..Style::default()
        }));
        tree.append_child(root, child);
        run_rust_layout(&mut tree, root, Constraints::definite(100.0, 20.0));
        assert_close(tree.nodes[child].layout.size.width, 12.0);
        covered += 1;
    }

    // Canonical CSS column-gap and row-gap paths replace Starlight's raw-value gap tests.
    {
        let mut tree = SimpleTree::default();
        let root = tree.push(SimpleNode::new(Style {
            display: Display::Flex,
            width: Length::points(120.0),
            height: Length::points(30.0),
            column_gap: Length::points(12.0),
            align_items: AlignItems::FlexStart,
            ..Style::default()
        }));
        let first = fixed_leaf(&mut tree, 20.0, 10.0);
        let second = fixed_leaf(&mut tree, 18.0, 12.0);
        tree.append_child(root, first);
        tree.append_child(root, second);
        run_rust_layout(&mut tree, root, Constraints::definite(120.0, 30.0));
        assert_close(tree.nodes[second].layout.offset.x, 32.0);
        covered += 1;
    }
    {
        let mut tree = SimpleTree::default();
        let root = tree.push(SimpleNode::new(Style {
            display: Display::Flex,
            width: Length::points(30.0),
            height: Length::points(80.0),
            flex_wrap: FlexWrap::Wrap,
            row_gap: Length::points(12.0),
            align_items: AlignItems::FlexStart,
            align_content: AlignContent::FlexStart,
            ..Style::default()
        }));
        let first = fixed_leaf(&mut tree, 20.0, 10.0);
        let second = fixed_leaf(&mut tree, 20.0, 10.0);
        tree.append_child(root, first);
        tree.append_child(root, second);
        run_rust_layout(&mut tree, root, Constraints::definite(30.0, 80.0));
        assert_close(tree.nodes[second].layout.offset.y, 22.0);
        covered += 1;
    }

    assert_eq!(covered, 9);
}

#[test]
fn flex_solver_inventory_maps_all_24_non_linear_source_tests() {
    let baseline = include_str!("flexbox.rs");
    let canonical = include_str!("pr25_flex_layout.rs");
    let additional = include_str!("pr25_flex_additional.rs");

    assert_eq!(SOLVER_MAPPINGS.len(), 24);
    for (source, target) in SOLVER_MAPPINGS {
        let needle = format!("fn {target}(");
        assert!(
            baseline.contains(&needle)
                || canonical.contains(&needle)
                || additional.contains(&needle),
            "solver source test {source} must map to existing canonical target {target}"
        );
    }
}
