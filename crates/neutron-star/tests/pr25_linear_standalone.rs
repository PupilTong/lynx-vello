//! Rust-only migration of every PR #25 standalone `display: linear` head-to-head fixture.
//!
//! The original test target compared these Rust builders with a Lynx C++
//! baseline. Per the migration scope, this target preserves all 458 Rust test
//! functions, all matrix loops and aggregate rows (543 layout executions), and their source-shaped
//! tree construction, but deliberately omits C++/FFI code.

#![allow(clippy::too_many_lines, clippy::similar_names)]

mod pr25_linear_standalone_support;
mod pr25_support;
mod support;

use std::collections::BTreeSet;

use pr25_linear_standalone_support::*;

const SOURCE_TESTS: &str = include_str!("pr25_linear_standalone_inventory.txt");

fn source_execution_count(name: &str) -> usize {
    match name {
        "standalone_owned_tree_matches_cpp_for_absolute_linear_cross_axis_fallback_order"
        | "standalone_owned_tree_matches_cpp_for_absolute_vertical_linear_main_axis_static_position" => {
            4
        }
        "standalone_owned_tree_matches_cpp_for_linear_absolute_horizontal_physical_cross_static_positions_with_margins"
        | "standalone_owned_tree_matches_cpp_for_linear_absolute_vertical_reverse_static_positions_with_margins"
        | "standalone_owned_tree_matches_cpp_for_linear_fixed_horizontal_physical_cross_static_positions_with_margins"
        | "standalone_owned_tree_matches_cpp_for_linear_space_between_multi_item"
        | "standalone_owned_tree_matches_cpp_for_linear_justify_content_center_and_flex_end"
        | "standalone_owned_tree_matches_cpp_for_absolute_linear_initial_alignment"
        | "standalone_owned_tree_matches_cpp_for_linear_empty_container_baseline_matrix" => 2,
        "standalone_owned_tree_matches_cpp_for_linear_container_clamp_matrix"
        | "standalone_owned_tree_matches_cpp_for_linear_unallocated_weight_space"
        | "standalone_owned_tree_matches_cpp_for_linear_justify_content_start_fallbacks" => 3,
        "standalone_owned_tree_matches_cpp_for_linear_weight_min_max_freeze_distribution"
        | "standalone_owned_tree_matches_cpp_for_out_of_flow_intrinsic_sizing" => 8,
        "standalone_owned_tree_matches_cpp_for_linear_gravity_mapping" => 14,
        "standalone_owned_tree_matches_cpp_for_linear_layout_gravity_mapping" => 31,
        "standalone_owned_tree_matches_cpp_for_linear_cross_gravity_mapping" => 10,
        _ => 1,
    }
}

#[test]
fn standalone_linear_source_inventory_is_exact_and_executes_all_rows() {
    let names = SOURCE_TESTS
        .lines()
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>();
    assert_eq!(names.len(), 458);
    assert_eq!(names.iter().copied().collect::<BTreeSet<_>>().len(), 458);
    assert_eq!(
        names
            .iter()
            .map(|name| source_execution_count(name))
            .sum::<usize>(),
        543
    );

    let migrated = include_str!("pr25_linear_standalone.rs");
    let rust_runner = ["run_standalone_", "rust("].concat();
    assert_eq!(migrated.matches(&rust_runner).count(), 469);
    for name in names {
        assert!(
            migrated.contains(&format!("fn {name}")),
            "missing source-shaped standalone Linear test {name}"
        );
    }
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_horizontal_physical_pixel_rounding_with_fractional_edges()
 {
    let half_pixel = StandaloneConfig::with_physical_pixels_per_layout_unit(2.0);
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node_with_config(half_pixel);
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root orientation");
    tree.set_width(root, Length::points(121.25))
        .expect("set root width");
    tree.set_height(root, Length::points(58.75))
        .expect("set root height");
    tree.set_padding(root, StandaloneEdge::Left, Length::points(2.25))
        .expect("set root left padding");
    tree.set_padding(root, StandaloneEdge::Right, Length::points(3.25))
        .expect("set root right padding");
    tree.set_padding(root, StandaloneEdge::Top, Length::points(1.25))
        .expect("set root top padding");
    tree.set_padding(root, StandaloneEdge::Bottom, Length::points(2.75))
        .expect("set root bottom padding");
    tree.set_border(root, StandaloneEdge::Left, 0.6)
        .expect("set root left border");
    tree.set_border(root, StandaloneEdge::Right, 1.4)
        .expect("set root right border");
    tree.set_border(root, StandaloneEdge::Top, 0.4)
        .expect("set root top border");
    tree.set_border(root, StandaloneEdge::Bottom, 1.6)
        .expect("set root bottom border");
    tree.set_gap(root, StandaloneGap::Column, Length::points(1.25))
        .expect("set root column gap");

    let measured = tree.create_default_node_with_config(half_pixel);
    tree.set_display(measured, Display::Block)
        .expect("set measured display");
    tree.set_box_sizing(measured, BoxSizing::ContentBox)
        .expect("set measured box sizing");
    tree.set_measured_size(measured, Some(Size::new(19.25, 10.75)))
        .expect("set measured size");
    tree.set_margin(measured, StandaloneEdge::Left, Length::points(0.75))
        .expect("set measured left margin");
    tree.set_margin(measured, StandaloneEdge::Right, Length::points(1.25))
        .expect("set measured right margin");
    tree.set_margin(measured, StandaloneEdge::Top, Length::points(1.75))
        .expect("set measured top margin");

    let sized = tree.create_default_node_with_config(half_pixel);
    tree.set_display(sized, Display::Block)
        .expect("set sized display");
    tree.set_box_sizing(sized, BoxSizing::ContentBox)
        .expect("set sized box sizing");
    tree.set_width(sized, Length::points(24.25))
        .expect("set sized width");
    tree.set_height(sized, Length::points(14.75))
        .expect("set sized height");
    tree.set_padding(sized, StandaloneEdge::All, Length::points(1.25))
        .expect("set sized padding");
    tree.set_border(sized, StandaloneEdge::All, 0.6)
        .expect("set sized border");
    tree.set_margin(sized, StandaloneEdge::Left, Length::points(1.75))
        .expect("set sized left margin");
    tree.set_margin(sized, StandaloneEdge::Bottom, Length::points(2.25))
        .expect("set sized bottom margin");

    tree.append_child(root, measured)
        .expect("append measured child");
    tree.append_child(root, sized).expect("append sized child");

    run_standalone_rust(tree, root, Constraints::definite(130.0, 66.0))
        .expect("linear horizontal physical-pixel rounding with fractional edges parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_vertical_reverse_physical_pixel_rounding_with_ordered_margins()
 {
    let half_pixel = StandaloneConfig::with_physical_pixels_per_layout_unit(2.0);
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node_with_config(half_pixel);
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::VerticalReverse)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::Center)
        .expect("set root gravity");
    tree.set_width(root, Length::points(76.5))
        .expect("set root width");
    tree.set_height(root, Length::points(138.25))
        .expect("set root height");
    tree.set_padding(root, StandaloneEdge::Horizontal, Length::points(2.25))
        .expect("set root horizontal padding");
    tree.set_padding(root, StandaloneEdge::Vertical, Length::points(1.75))
        .expect("set root vertical padding");
    tree.set_border(root, StandaloneEdge::All, 0.75)
        .expect("set root border");

    let first = tree.create_default_node_with_config(half_pixel);
    tree.set_display(first, Display::Block)
        .expect("set first display");
    tree.set_box_sizing(first, BoxSizing::ContentBox)
        .expect("set first box sizing");
    tree.set_measured_size(first, Some(Size::new(22.25, 17.75)))
        .expect("set first measured size");
    tree.set_margin(first, StandaloneEdge::Top, Length::points(1.25))
        .expect("set first top margin");
    tree.set_margin(first, StandaloneEdge::Bottom, Length::points(2.75))
        .expect("set first bottom margin");
    tree.set_order(first, 2).expect("set first order");

    let second = tree.create_default_node_with_config(half_pixel);
    tree.set_display(second, Display::Block)
        .expect("set second display");
    tree.set_box_sizing(second, BoxSizing::ContentBox)
        .expect("set second box sizing");
    tree.set_width(second, Length::points(28.25))
        .expect("set second width");
    tree.set_height(second, Length::points(19.25))
        .expect("set second height");
    tree.set_margin(second, StandaloneEdge::Top, Length::points(0.75))
        .expect("set second top margin");
    tree.set_margin(second, StandaloneEdge::Bottom, Length::points(1.75))
        .expect("set second bottom margin");
    tree.set_order(second, -1).expect("set second order");

    tree.append_child(root, first).expect("append first child");
    tree.append_child(root, second)
        .expect("append second child");

    run_standalone_rust(tree, root, Constraints::definite(82.0, 144.0))
        .expect("linear vertical-reverse physical-pixel rounding with ordered margins parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_relative_position_physical_pixel_rounding() {
    let half_pixel = StandaloneConfig::with_physical_pixels_per_layout_unit(2.0);
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node_with_config(half_pixel);
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root orientation");
    tree.set_width(root, Length::points(142.25))
        .expect("set root width");
    tree.set_height(root, Length::points(74.75))
        .expect("set root height");
    tree.set_padding(root, StandaloneEdge::All, Length::points(2.25))
        .expect("set root padding");
    tree.set_border(root, StandaloneEdge::All, 0.75)
        .expect("set root border");

    let relative = tree.create_default_node_with_config(half_pixel);
    tree.set_display(relative, Display::Block)
        .expect("set relative display");
    tree.set_box_sizing(relative, BoxSizing::ContentBox)
        .expect("set relative box sizing");
    tree.set_position_type(relative, PositionType::Relative)
        .expect("set relative position type");
    tree.set_width(relative, Length::points(31.25))
        .expect("set relative width");
    tree.set_height(relative, Length::points(18.75))
        .expect("set relative height");
    tree.set_position(relative, StandaloneEdge::Left, Length::points(3.25))
        .expect("set relative left");
    tree.set_position(relative, StandaloneEdge::Right, Length::points(6.75))
        .expect("set relative right");
    tree.set_position(relative, StandaloneEdge::Top, Length::points(2.75))
        .expect("set relative top");
    tree.set_position(relative, StandaloneEdge::Bottom, Length::points(5.25))
        .expect("set relative bottom");
    tree.set_margin(relative, StandaloneEdge::Left, Length::points(1.25))
        .expect("set relative left margin");
    tree.set_margin(relative, StandaloneEdge::Right, Length::points(1.75))
        .expect("set relative right margin");

    let follower = tree.create_default_node_with_config(half_pixel);
    tree.set_display(follower, Display::Block)
        .expect("set follower display");
    tree.set_box_sizing(follower, BoxSizing::ContentBox)
        .expect("set follower box sizing");
    tree.set_width(follower, Length::points(24.75))
        .expect("set follower width");
    tree.set_height(follower, Length::points(16.25))
        .expect("set follower height");

    tree.append_child(root, relative)
        .expect("append relative child");
    tree.append_child(root, follower).expect("append follower");

    run_standalone_rust(tree, root, Constraints::definite(150.0, 82.0))
        .expect("linear relative-position physical-pixel rounding parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_absolute_physical_pixel_rounding_with_fractional_insets()
 {
    let half_pixel = StandaloneConfig::with_physical_pixels_per_layout_unit(2.0);
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node_with_config(half_pixel);
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root orientation");
    tree.set_width(root, Length::points(154.25))
        .expect("set root width");
    tree.set_height(root, Length::points(88.75))
        .expect("set root height");
    tree.set_padding(root, StandaloneEdge::Left, Length::points(2.25))
        .expect("set root left padding");
    tree.set_padding(root, StandaloneEdge::Top, Length::points(1.75))
        .expect("set root top padding");
    tree.set_border(root, StandaloneEdge::All, 0.75)
        .expect("set root border");

    let flow = tree.create_default_node_with_config(half_pixel);
    tree.set_display(flow, Display::Block)
        .expect("set flow display");
    tree.set_box_sizing(flow, BoxSizing::ContentBox)
        .expect("set flow box sizing");
    tree.set_width(flow, Length::points(20.25))
        .expect("set flow width");
    tree.set_height(flow, Length::points(12.75))
        .expect("set flow height");

    let absolute = tree.create_default_node_with_config(half_pixel);
    tree.set_display(absolute, Display::Block)
        .expect("set absolute display");
    tree.set_box_sizing(absolute, BoxSizing::ContentBox)
        .expect("set absolute box sizing");
    tree.set_position_type(absolute, PositionType::Absolute)
        .expect("set absolute position type");
    tree.set_width(absolute, Length::points(30.25))
        .expect("set absolute width");
    tree.set_height(absolute, Length::points(16.75))
        .expect("set absolute height");
    tree.set_position(absolute, StandaloneEdge::Left, Length::percent(9.5))
        .expect("set absolute left");
    tree.set_position(absolute, StandaloneEdge::Top, Length::calc(1.25, 12.5))
        .expect("set absolute top");
    tree.set_margin(absolute, StandaloneEdge::Left, Length::points(1.25))
        .expect("set absolute left margin");
    tree.set_margin(absolute, StandaloneEdge::Top, Length::points(0.75))
        .expect("set absolute top margin");
    tree.set_padding(absolute, StandaloneEdge::All, Length::points(0.75))
        .expect("set absolute padding");
    tree.set_border(absolute, StandaloneEdge::All, 0.5)
        .expect("set absolute border");

    tree.append_child(root, flow).expect("append flow child");
    tree.append_child(root, absolute)
        .expect("append absolute child");

    run_standalone_rust(tree, root, Constraints::definite(162.0, 96.0))
        .expect("linear absolute physical-pixel rounding with fractional insets parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_absolute_auto_size_physical_pixel_rounding_between_insets()
 {
    let half_pixel = StandaloneConfig::with_physical_pixels_per_layout_unit(2.0);
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node_with_config(half_pixel);
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Vertical)
        .expect("set root orientation");
    tree.set_width(root, Length::points(132.25))
        .expect("set root width");
    tree.set_height(root, Length::points(118.75))
        .expect("set root height");
    tree.set_padding(root, StandaloneEdge::Horizontal, Length::points(2.25))
        .expect("set root horizontal padding");
    tree.set_padding(root, StandaloneEdge::Vertical, Length::points(1.25))
        .expect("set root vertical padding");
    tree.set_border(root, StandaloneEdge::All, 0.75)
        .expect("set root border");

    let absolute = tree.create_default_node_with_config(half_pixel);
    tree.set_display(absolute, Display::Block)
        .expect("set absolute display");
    tree.set_box_sizing(absolute, BoxSizing::ContentBox)
        .expect("set absolute box sizing");
    tree.set_position_type(absolute, PositionType::Absolute)
        .expect("set absolute position type");
    tree.set_position(absolute, StandaloneEdge::Left, Length::calc(2.25, 7.5))
        .expect("set absolute left");
    tree.set_position(absolute, StandaloneEdge::Right, Length::percent(8.5))
        .expect("set absolute right");
    tree.set_position(absolute, StandaloneEdge::Top, Length::points(4.25))
        .expect("set absolute top");
    tree.set_position(absolute, StandaloneEdge::Bottom, Length::calc(3.25, 5.5))
        .expect("set absolute bottom");
    tree.set_margin(absolute, StandaloneEdge::Left, Length::points(1.25))
        .expect("set absolute left margin");
    tree.set_margin(absolute, StandaloneEdge::Right, Length::points(1.75))
        .expect("set absolute right margin");
    tree.set_margin(absolute, StandaloneEdge::Top, Length::points(0.75))
        .expect("set absolute top margin");
    tree.set_margin(absolute, StandaloneEdge::Bottom, Length::points(1.25))
        .expect("set absolute bottom margin");
    tree.set_padding(absolute, StandaloneEdge::All, Length::points(1.25))
        .expect("set absolute padding");
    tree.set_border(absolute, StandaloneEdge::All, 0.5)
        .expect("set absolute border");

    tree.append_child(root, absolute)
        .expect("append absolute child");

    run_standalone_rust(tree, root, Constraints::definite(140.0, 126.0))
        .expect("linear absolute auto-size physical-pixel rounding between insets parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_nested_fixed_physical_pixel_rounding_against_root()
{
    let half_pixel = StandaloneConfig::with_physical_pixels_per_layout_unit(2.0);
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node_with_config(half_pixel);
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root orientation");
    tree.set_width(root, Length::points(166.25))
        .expect("set root width");
    tree.set_height(root, Length::points(92.75))
        .expect("set root height");
    tree.set_padding(root, StandaloneEdge::All, Length::points(2.25))
        .expect("set root padding");
    tree.set_border(root, StandaloneEdge::All, 0.75)
        .expect("set root border");

    let nested = tree.create_default_node_with_config(half_pixel);
    tree.set_display(nested, Display::Block)
        .expect("set nested display");
    tree.set_box_sizing(nested, BoxSizing::ContentBox)
        .expect("set nested box sizing");
    tree.set_width(nested, Length::points(70.25))
        .expect("set nested width");
    tree.set_height(nested, Length::points(40.75))
        .expect("set nested height");
    tree.set_margin(nested, StandaloneEdge::Left, Length::points(1.25))
        .expect("set nested left margin");
    tree.set_margin(nested, StandaloneEdge::Top, Length::points(0.75))
        .expect("set nested top margin");

    let fixed = tree.create_default_node_with_config(half_pixel);
    tree.set_display(fixed, Display::Block)
        .expect("set fixed display");
    tree.set_box_sizing(fixed, BoxSizing::ContentBox)
        .expect("set fixed box sizing");
    tree.set_position_type(fixed, PositionType::Fixed)
        .expect("set fixed position type");
    tree.set_width(fixed, Length::points(26.25))
        .expect("set fixed width");
    tree.set_height(fixed, Length::points(14.75))
        .expect("set fixed height");
    tree.set_position(fixed, StandaloneEdge::Right, Length::calc(2.25, 7.5))
        .expect("set fixed right");
    tree.set_position(fixed, StandaloneEdge::Bottom, Length::percent(9.5))
        .expect("set fixed bottom");
    tree.set_margin(fixed, StandaloneEdge::Right, Length::points(1.25))
        .expect("set fixed right margin");
    tree.set_margin(fixed, StandaloneEdge::Bottom, Length::points(0.75))
        .expect("set fixed bottom margin");
    tree.set_padding(fixed, StandaloneEdge::All, Length::points(0.75))
        .expect("set fixed padding");
    tree.set_border(fixed, StandaloneEdge::All, 0.5)
        .expect("set fixed border");

    tree.append_child(root, nested).expect("append nested");
    tree.append_child(nested, fixed).expect("append fixed");

    run_standalone_rust(tree, root, Constraints::definite(174.0, 100.0))
        .expect("linear nested fixed physical-pixel rounding against root parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_vertical_reverse_fixed_static_physical_pixel_rounding()
 {
    let half_pixel = StandaloneConfig::with_physical_pixels_per_layout_unit(2.0);
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node_with_config(half_pixel);
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::VerticalReverse)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::Center)
        .expect("set root gravity");
    tree.set_width(root, Length::points(96.25))
        .expect("set root width");
    tree.set_height(root, Length::points(150.75))
        .expect("set root height");
    tree.set_padding(root, StandaloneEdge::All, Length::points(2.25))
        .expect("set root padding");
    tree.set_border(root, StandaloneEdge::All, 0.75)
        .expect("set root border");

    let flow = tree.create_default_node_with_config(half_pixel);
    tree.set_display(flow, Display::Block)
        .expect("set flow display");
    tree.set_box_sizing(flow, BoxSizing::ContentBox)
        .expect("set flow box sizing");
    tree.set_width(flow, Length::points(36.25))
        .expect("set flow width");
    tree.set_height(flow, Length::points(18.75))
        .expect("set flow height");

    let fixed = tree.create_default_node_with_config(half_pixel);
    tree.set_display(fixed, Display::Block)
        .expect("set fixed display");
    tree.set_box_sizing(fixed, BoxSizing::ContentBox)
        .expect("set fixed box sizing");
    tree.set_position_type(fixed, PositionType::Fixed)
        .expect("set fixed position type");
    tree.set_width(fixed, Length::points(28.25))
        .expect("set fixed width");
    tree.set_height(fixed, Length::points(16.75))
        .expect("set fixed height");
    tree.set_linear_layout_gravity(fixed, LinearLayoutGravity::Bottom)
        .expect("set fixed layout gravity");
    tree.set_margin(fixed, StandaloneEdge::Top, Length::points(1.25))
        .expect("set fixed top margin");
    tree.set_margin(fixed, StandaloneEdge::Bottom, Length::points(1.75))
        .expect("set fixed bottom margin");
    tree.set_margin(fixed, StandaloneEdge::Left, Length::points(0.75))
        .expect("set fixed left margin");

    tree.append_child(root, flow).expect("append flow");
    tree.append_child(root, fixed).expect("append fixed");

    run_standalone_rust(tree, root, Constraints::definite(104.0, 158.0))
        .expect("linear vertical-reverse fixed static physical-pixel rounding parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_horizontal_linear_baseline_physical_pixel_rounding() {
    let half_pixel = StandaloneConfig::with_physical_pixels_per_layout_unit(2.0);
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node_with_config(half_pixel);
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root orientation");
    tree.set_width(root, Length::points(122.25))
        .expect("set root width");
    tree.set_height(root, Length::points(58.75))
        .expect("set root height");
    tree.set_padding(root, StandaloneEdge::All, Length::points(1.25))
        .expect("set root padding");
    tree.set_border(root, StandaloneEdge::All, 0.75)
        .expect("set root border");

    let first = tree.create_default_node_with_config(half_pixel);
    tree.set_display(first, Display::Block)
        .expect("set first display");
    tree.set_box_sizing(first, BoxSizing::ContentBox)
        .expect("set first box sizing");
    tree.set_measured_size(first, Some(Size::new(22.25, 14.75)))
        .expect("set first measured size");
    tree.set_baseline(first, Some(7.25))
        .expect("set first baseline");
    tree.set_margin(first, StandaloneEdge::Top, Length::points(1.25))
        .expect("set first top margin");
    tree.set_margin(first, StandaloneEdge::Bottom, Length::points(0.75))
        .expect("set first bottom margin");

    let second = tree.create_default_node_with_config(half_pixel);
    tree.set_display(second, Display::Block)
        .expect("set second display");
    tree.set_box_sizing(second, BoxSizing::ContentBox)
        .expect("set second box sizing");
    tree.set_measured_size(second, Some(Size::new(24.75, 18.25)))
        .expect("set second measured size");
    tree.set_baseline(second, Some(11.75))
        .expect("set second baseline");
    tree.set_margin(second, StandaloneEdge::Top, Length::points(0.75))
        .expect("set second top margin");
    tree.set_margin(second, StandaloneEdge::Bottom, Length::points(1.25))
        .expect("set second bottom margin");

    tree.append_child(root, first).expect("append first");
    tree.append_child(root, second).expect("append second");

    run_standalone_rust(tree, root, Constraints::definite(130.0, 66.0))
        .expect("horizontal linear baseline physical-pixel rounding parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_cross_auto_margin_physical_pixel_rounding() {
    let half_pixel = StandaloneConfig::with_physical_pixels_per_layout_unit(2.0);
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node_with_config(half_pixel);
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root orientation");
    tree.set_width(root, Length::points(132.25))
        .expect("set root width");
    tree.set_height(root, Length::points(72.75))
        .expect("set root height");
    tree.set_padding(root, StandaloneEdge::All, Length::points(2.25))
        .expect("set root padding");
    tree.set_border(root, StandaloneEdge::All, 0.75)
        .expect("set root border");

    let auto_margin = tree.create_default_node_with_config(half_pixel);
    tree.set_display(auto_margin, Display::Block)
        .expect("set auto-margin display");
    tree.set_box_sizing(auto_margin, BoxSizing::ContentBox)
        .expect("set auto-margin box sizing");
    tree.set_width(auto_margin, Length::points(28.25))
        .expect("set auto-margin width");
    tree.set_height(auto_margin, Length::points(16.75))
        .expect("set auto-margin height");
    tree.set_margin(auto_margin, StandaloneEdge::Top, Length::Auto)
        .expect("set auto-margin top");
    tree.set_margin(auto_margin, StandaloneEdge::Bottom, Length::points(1.25))
        .expect("set auto-margin bottom");

    let follower = tree.create_default_node_with_config(half_pixel);
    tree.set_display(follower, Display::Block)
        .expect("set follower display");
    tree.set_box_sizing(follower, BoxSizing::ContentBox)
        .expect("set follower box sizing");
    tree.set_width(follower, Length::points(20.75))
        .expect("set follower width");
    tree.set_height(follower, Length::points(12.25))
        .expect("set follower height");
    tree.set_margin(follower, StandaloneEdge::Top, Length::points(1.75))
        .expect("set follower top margin");

    tree.append_child(root, auto_margin)
        .expect("append auto-margin child");
    tree.append_child(root, follower).expect("append follower");

    run_standalone_rust(tree, root, Constraints::definite(140.0, 80.0))
        .expect("linear cross auto-margin physical-pixel rounding parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_weighted_child_physical_pixel_rounding() {
    let half_pixel = StandaloneConfig::with_physical_pixels_per_layout_unit(2.0);
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node_with_config(half_pixel);
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root orientation");
    tree.set_width(root, Length::points(150.25))
        .expect("set root width");
    tree.set_height(root, Length::points(62.75))
        .expect("set root height");
    tree.set_padding(root, StandaloneEdge::Horizontal, Length::points(2.25))
        .expect("set root horizontal padding");
    tree.set_padding(root, StandaloneEdge::Vertical, Length::points(1.75))
        .expect("set root vertical padding");
    tree.set_border(root, StandaloneEdge::All, 0.75)
        .expect("set root border");
    tree.set_linear_weight_sum(root, 3.25)
        .expect("set root weight sum");

    let fixed = tree.create_default_node_with_config(half_pixel);
    tree.set_display(fixed, Display::Block)
        .expect("set fixed display");
    tree.set_box_sizing(fixed, BoxSizing::ContentBox)
        .expect("set fixed box sizing");
    tree.set_width(fixed, Length::points(22.25))
        .expect("set fixed width");
    tree.set_height(fixed, Length::points(14.75))
        .expect("set fixed height");
    tree.set_margin(fixed, StandaloneEdge::Right, Length::points(1.25))
        .expect("set fixed right margin");

    let weighted = tree.create_default_node_with_config(half_pixel);
    tree.set_display(weighted, Display::Block)
        .expect("set weighted display");
    tree.set_box_sizing(weighted, BoxSizing::ContentBox)
        .expect("set weighted box sizing");
    tree.set_linear_weight(weighted, 1.75)
        .expect("set weighted weight");
    tree.set_height(weighted, Length::points(18.25))
        .expect("set weighted height");
    tree.set_min_width(weighted, Length::points(24.25))
        .expect("set weighted min width");
    tree.set_max_width(weighted, Length::points(98.75))
        .expect("set weighted max width");
    tree.set_margin(weighted, StandaloneEdge::Left, Length::points(1.75))
        .expect("set weighted left margin");
    tree.set_margin(weighted, StandaloneEdge::Right, Length::points(2.25))
        .expect("set weighted right margin");

    tree.append_child(root, fixed).expect("append fixed child");
    tree.append_child(root, weighted)
        .expect("append weighted child");

    run_standalone_rust(tree, root, Constraints::definite(158.0, 70.0))
        .expect("linear weighted child physical-pixel rounding parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_horizontal_sticky_percent_insets_keep_flow_position()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root orientation");
    tree.set_width(root, Length::points(160.0))
        .expect("set root width");
    tree.set_height(root, Length::points(72.0))
        .expect("set root height");
    tree.set_padding(root, StandaloneEdge::All, Length::points(5.0))
        .expect("set root padding");
    tree.set_border(root, StandaloneEdge::All, 1.0)
        .expect("set root border");

    let sticky = tree.create_default_node();
    tree.set_display(sticky, Display::Block)
        .expect("set sticky display");
    tree.set_box_sizing(sticky, BoxSizing::ContentBox)
        .expect("set sticky box sizing");
    tree.set_position_type(sticky, PositionType::Sticky)
        .expect("set sticky position type");
    tree.set_width(sticky, Length::points(38.0))
        .expect("set sticky width");
    tree.set_height(sticky, Length::points(20.0))
        .expect("set sticky height");
    tree.set_position(sticky, StandaloneEdge::Left, Length::percent(10.0))
        .expect("set sticky left");
    tree.set_position(sticky, StandaloneEdge::Top, Length::percent(25.0))
        .expect("set sticky top");

    let follower = tree.create_default_node();
    tree.set_display(follower, Display::Block)
        .expect("set follower display");
    tree.set_box_sizing(follower, BoxSizing::ContentBox)
        .expect("set follower box sizing");
    tree.set_width(follower, Length::points(26.0))
        .expect("set follower width");
    tree.set_height(follower, Length::points(18.0))
        .expect("set follower height");

    tree.append_child(root, sticky).expect("append sticky");
    tree.append_child(root, follower).expect("append follower");

    run_standalone_rust(tree, root, Constraints::definite(172.0, 84.0))
        .expect("linear horizontal sticky percent insets keep flow position parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_vertical_sticky_calc_end_insets_keep_flow_position()
{
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Vertical)
        .expect("set root orientation");
    tree.set_width(root, Length::points(132.0))
        .expect("set root width");
    tree.set_height(root, Length::points(118.0))
        .expect("set root height");
    tree.set_padding(root, StandaloneEdge::Horizontal, Length::points(6.0))
        .expect("set root horizontal padding");
    tree.set_padding(root, StandaloneEdge::Vertical, Length::points(4.0))
        .expect("set root vertical padding");
    tree.set_border(root, StandaloneEdge::All, 1.0)
        .expect("set root border");

    let before = tree.create_default_node();
    tree.set_display(before, Display::Block)
        .expect("set before display");
    tree.set_box_sizing(before, BoxSizing::ContentBox)
        .expect("set before box sizing");
    tree.set_width(before, Length::points(44.0))
        .expect("set before width");
    tree.set_height(before, Length::points(16.0))
        .expect("set before height");

    let sticky = tree.create_default_node();
    tree.set_display(sticky, Display::Block)
        .expect("set sticky display");
    tree.set_box_sizing(sticky, BoxSizing::ContentBox)
        .expect("set sticky box sizing");
    tree.set_position_type(sticky, PositionType::Sticky)
        .expect("set sticky position type");
    tree.set_width(sticky, Length::points(50.0))
        .expect("set sticky width");
    tree.set_height(sticky, Length::points(24.0))
        .expect("set sticky height");
    tree.set_position(sticky, StandaloneEdge::Right, Length::calc(4.0, 10.0))
        .expect("set sticky right");
    tree.set_position(sticky, StandaloneEdge::Bottom, Length::calc(3.0, 20.0))
        .expect("set sticky bottom");

    tree.append_child(root, before).expect("append before");
    tree.append_child(root, sticky).expect("append sticky");

    run_standalone_rust(tree, root, Constraints::definite(146.0, 128.0))
        .expect("linear vertical sticky calc end insets keep flow position parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_rtl_linear_sticky_paired_insets_export_without_direction_flip()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_direction(root, Direction::Rtl)
        .expect("set root direction");
    tree.set_linear_orientation(root, LinearOrientation::HorizontalReverse)
        .expect("set root orientation");
    tree.set_width(root, Length::points(150.0))
        .expect("set root width");
    tree.set_height(root, Length::points(70.0))
        .expect("set root height");
    tree.set_padding(root, StandaloneEdge::All, Length::points(5.0))
        .expect("set root padding");
    tree.set_border(root, StandaloneEdge::All, 1.0)
        .expect("set root border");

    let sticky = tree.create_default_node();
    tree.set_display(sticky, Display::Block)
        .expect("set sticky display");
    tree.set_box_sizing(sticky, BoxSizing::ContentBox)
        .expect("set sticky box sizing");
    tree.set_position_type(sticky, PositionType::Sticky)
        .expect("set sticky position type");
    tree.set_width(sticky, Length::points(34.0))
        .expect("set sticky width");
    tree.set_height(sticky, Length::points(22.0))
        .expect("set sticky height");
    tree.set_position(sticky, StandaloneEdge::Left, Length::calc(3.0, 10.0))
        .expect("set sticky left");
    tree.set_position(sticky, StandaloneEdge::Right, Length::calc(4.0, 8.0))
        .expect("set sticky right");
    tree.set_position(sticky, StandaloneEdge::Top, Length::percent(20.0))
        .expect("set sticky top");
    tree.set_position(sticky, StandaloneEdge::Bottom, Length::percent(15.0))
        .expect("set sticky bottom");

    let follower = tree.create_default_node();
    tree.set_display(follower, Display::Block)
        .expect("set follower display");
    tree.set_box_sizing(follower, BoxSizing::ContentBox)
        .expect("set follower box sizing");
    tree.set_width(follower, Length::points(20.0))
        .expect("set follower width");
    tree.set_height(follower, Length::points(18.0))
        .expect("set follower height");

    tree.append_child(root, sticky).expect("append sticky");
    tree.append_child(root, follower).expect("append follower");

    run_standalone_rust(tree, root, Constraints::definite(162.0, 82.0))
        .expect("RTL linear sticky paired insets export without direction flip parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_relative_position_left_top_preserves_horizontal_flow()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root orientation");
    tree.set_width(root, Length::points(120.0))
        .expect("set root width");
    tree.set_height(root, Length::points(64.0))
        .expect("set root height");
    tree.set_padding(root, StandaloneEdge::All, Length::points(4.0))
        .expect("set root padding");
    tree.set_border(root, StandaloneEdge::All, 1.0)
        .expect("set root border");

    let relative = tree.create_default_node();
    tree.set_display(relative, Display::Block)
        .expect("set relative display");
    tree.set_box_sizing(relative, BoxSizing::ContentBox)
        .expect("set relative box sizing");
    tree.set_position_type(relative, PositionType::Relative)
        .expect("set relative position type");
    tree.set_width(relative, Length::points(32.0))
        .expect("set relative width");
    tree.set_height(relative, Length::points(18.0))
        .expect("set relative height");
    tree.set_position(relative, StandaloneEdge::Left, Length::points(7.0))
        .expect("set relative left");
    tree.set_position(relative, StandaloneEdge::Top, Length::points(3.0))
        .expect("set relative top");

    let follower = tree.create_default_node();
    tree.set_display(follower, Display::Block)
        .expect("set follower display");
    tree.set_box_sizing(follower, BoxSizing::ContentBox)
        .expect("set follower box sizing");
    tree.set_width(follower, Length::points(24.0))
        .expect("set follower width");
    tree.set_height(follower, Length::points(20.0))
        .expect("set follower height");

    tree.append_child(root, relative)
        .expect("append relative child");
    tree.append_child(root, follower).expect("append follower");

    run_standalone_rust(tree, root, Constraints::definite(132.0, 76.0))
        .expect("linear relative-position left/top preserves horizontal normal flow parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_relative_position_right_bottom_calc_preserves_vertical_flow()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Vertical)
        .expect("set root orientation");
    tree.set_width(root, Length::points(126.0))
        .expect("set root width");
    tree.set_height(root, Length::points(120.0))
        .expect("set root height");
    tree.set_padding(root, StandaloneEdge::Horizontal, Length::points(6.0))
        .expect("set root horizontal padding");
    tree.set_padding(root, StandaloneEdge::Vertical, Length::points(5.0))
        .expect("set root vertical padding");
    tree.set_border(root, StandaloneEdge::All, 1.0)
        .expect("set root border");

    let before = tree.create_default_node();
    tree.set_display(before, Display::Block)
        .expect("set before display");
    tree.set_box_sizing(before, BoxSizing::ContentBox)
        .expect("set before box sizing");
    tree.set_width(before, Length::points(40.0))
        .expect("set before width");
    tree.set_height(before, Length::points(16.0))
        .expect("set before height");

    let relative = tree.create_default_node();
    tree.set_display(relative, Display::Block)
        .expect("set relative display");
    tree.set_box_sizing(relative, BoxSizing::ContentBox)
        .expect("set relative box sizing");
    tree.set_position_type(relative, PositionType::Relative)
        .expect("set relative position type");
    tree.set_width(relative, Length::points(50.0))
        .expect("set relative width");
    tree.set_height(relative, Length::points(22.0))
        .expect("set relative height");
    tree.set_position(relative, StandaloneEdge::Right, Length::calc(4.0, 10.0))
        .expect("set relative right");
    tree.set_position(relative, StandaloneEdge::Bottom, Length::calc(3.0, 20.0))
        .expect("set relative bottom");

    tree.append_child(root, before)
        .expect("append before child");
    tree.append_child(root, relative)
        .expect("append relative child");

    run_standalone_rust(tree, root, Constraints::definite(140.0, 132.0))
        .expect("linear relative-position right/bottom calc preserves vertical normal flow parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_relative_position_start_insets_win_over_end_insets()
{
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root orientation");
    tree.set_width(root, Length::points(160.0))
        .expect("set root width");
    tree.set_height(root, Length::points(90.0))
        .expect("set root height");
    tree.set_padding(root, StandaloneEdge::All, Length::points(5.0))
        .expect("set root padding");
    tree.set_border(root, StandaloneEdge::All, 1.0)
        .expect("set root border");

    let relative = tree.create_default_node();
    tree.set_display(relative, Display::Block)
        .expect("set relative display");
    tree.set_box_sizing(relative, BoxSizing::ContentBox)
        .expect("set relative box sizing");
    tree.set_position_type(relative, PositionType::Relative)
        .expect("set relative position type");
    tree.set_width(relative, Length::points(44.0))
        .expect("set relative width");
    tree.set_height(relative, Length::points(24.0))
        .expect("set relative height");
    tree.set_position(relative, StandaloneEdge::Left, Length::percent(10.0))
        .expect("set relative left");
    tree.set_position(relative, StandaloneEdge::Right, Length::points(21.0))
        .expect("set relative right");
    tree.set_position(relative, StandaloneEdge::Top, Length::calc(2.0, 25.0))
        .expect("set relative top");
    tree.set_position(relative, StandaloneEdge::Bottom, Length::points(17.0))
        .expect("set relative bottom");

    let follower = tree.create_default_node();
    tree.set_display(follower, Display::Block)
        .expect("set follower display");
    tree.set_box_sizing(follower, BoxSizing::ContentBox)
        .expect("set follower box sizing");
    tree.set_width(follower, Length::points(28.0))
        .expect("set follower width");
    tree.set_height(follower, Length::points(18.0))
        .expect("set follower height");

    tree.append_child(root, relative)
        .expect("append relative child");
    tree.append_child(root, follower).expect("append follower");

    run_standalone_rust(tree, root, Constraints::definite(172.0, 102.0))
        .expect("linear relative-position start insets win over end insets parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_absolute_linear_initial_alignment() {
    for (case_name, tree, root, constraints) in [
        absolute_linear_gravity_alignment_tree(),
        absolute_rtl_horizontal_linear_front_alignment_tree(),
    ] {
        run_standalone_rust(tree, root, constraints)
            .unwrap_or_else(|error| panic!("{case_name} parity failed: {error}"));
    }
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_absolute_static_position_with_margins() {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::Center)
        .expect("set root gravity");
    tree.set_width(root, Length::points(100.0))
        .expect("set root width");
    tree.set_height(root, Length::points(50.0))
        .expect("set root height");

    let absolute = tree.create_default_node();
    tree.set_display(absolute, Display::Block)
        .expect("set absolute display");
    tree.set_box_sizing(absolute, BoxSizing::ContentBox)
        .expect("set absolute box sizing");
    tree.set_position_type(absolute, PositionType::Absolute)
        .expect("set absolute position");
    tree.set_linear_layout_gravity(absolute, LinearLayoutGravity::End)
        .expect("set absolute layout gravity");
    tree.set_width(absolute, Length::points(10.0))
        .expect("set absolute width");
    tree.set_height(absolute, Length::points(8.0))
        .expect("set absolute height");
    tree.set_margin(absolute, StandaloneEdge::Left, Length::points(3.0))
        .expect("set absolute left margin");
    tree.set_margin(absolute, StandaloneEdge::Right, Length::points(7.0))
        .expect("set absolute right margin");
    tree.set_margin(absolute, StandaloneEdge::Top, Length::points(4.0))
        .expect("set absolute top margin");
    tree.set_margin(absolute, StandaloneEdge::Bottom, Length::points(6.0))
        .expect("set absolute bottom margin");
    tree.append_child(root, absolute)
        .expect("append absolute child");

    run_standalone_rust(tree, root, Constraints::definite(100.0, 50.0))
        .expect("linear absolute static position with margins parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_absolute_rtl_static_position_with_margins() {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_direction(root, Direction::Rtl)
        .expect("set root direction");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::None)
        .expect("set root gravity");
    tree.set_width(root, Length::points(100.0))
        .expect("set root width");
    tree.set_height(root, Length::points(50.0))
        .expect("set root height");

    let absolute = tree.create_default_node();
    tree.set_display(absolute, Display::Block)
        .expect("set absolute display");
    tree.set_box_sizing(absolute, BoxSizing::ContentBox)
        .expect("set absolute box sizing");
    tree.set_position_type(absolute, PositionType::Absolute)
        .expect("set absolute position");
    tree.set_linear_layout_gravity(absolute, LinearLayoutGravity::Start)
        .expect("set absolute layout gravity");
    tree.set_width(absolute, Length::points(10.0))
        .expect("set absolute width");
    tree.set_height(absolute, Length::points(8.0))
        .expect("set absolute height");
    tree.set_margin(absolute, StandaloneEdge::Left, Length::points(3.0))
        .expect("set absolute left margin");
    tree.set_margin(absolute, StandaloneEdge::Right, Length::points(7.0))
        .expect("set absolute right margin");
    tree.set_margin(absolute, StandaloneEdge::Top, Length::points(4.0))
        .expect("set absolute top margin");
    tree.set_margin(absolute, StandaloneEdge::Bottom, Length::points(6.0))
        .expect("set absolute bottom margin");
    tree.append_child(root, absolute)
        .expect("append absolute child");

    run_standalone_rust(tree, root, Constraints::definite(100.0, 50.0))
        .expect("linear absolute RTL static position with margins parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_absolute_linear_cross_axis_fallback_order() {
    {
        let mut tree = StandaloneTree::new();
        let root = tree.create_default_node();
        tree.set_display(root, Display::Linear)
            .expect("set root display");
        tree.set_box_sizing(root, BoxSizing::ContentBox)
            .expect("set root box sizing");
        tree.set_linear_orientation(root, LinearOrientation::Horizontal)
            .expect("set root orientation");
        tree.set_align_items(root, AlignItems::FlexStart)
            .expect("set root align items");
        tree.set_linear_cross_gravity(root, LinearCrossGravity::End)
            .expect("set root cross gravity");
        tree.set_width(root, Length::points(100.0))
            .expect("set root width");
        tree.set_height(root, Length::points(50.0))
            .expect("set root height");

        let absolute = tree.create_default_node();
        tree.set_display(absolute, Display::Block)
            .expect("set absolute display");
        tree.set_box_sizing(absolute, BoxSizing::ContentBox)
            .expect("set absolute box sizing");
        tree.set_position_type(absolute, PositionType::Absolute)
            .expect("set absolute position");
        tree.set_align_self(absolute, Some(AlignItems::Center))
            .expect("set absolute align self");
        tree.set_width(absolute, Length::points(20.0))
            .expect("set absolute width");
        tree.set_height(absolute, Length::points(10.0))
            .expect("set absolute height");
        tree.append_child(root, absolute)
            .expect("append absolute child");

        run_standalone_rust(tree, root, Constraints::definite(100.0, 50.0))
            .expect("absolute linear align-self static cross-position parity");
    }

    {
        let mut tree = StandaloneTree::new();
        let root = tree.create_default_node();
        tree.set_display(root, Display::Linear)
            .expect("set root display");
        tree.set_box_sizing(root, BoxSizing::ContentBox)
            .expect("set root box sizing");
        tree.set_linear_orientation(root, LinearOrientation::Horizontal)
            .expect("set root orientation");
        tree.set_align_items(root, AlignItems::FlexStart)
            .expect("set root align items");
        tree.set_linear_cross_gravity(root, LinearCrossGravity::End)
            .expect("set root cross gravity");
        tree.set_width(root, Length::points(100.0))
            .expect("set root width");
        tree.set_height(root, Length::points(50.0))
            .expect("set root height");

        let absolute = tree.create_default_node();
        tree.set_display(absolute, Display::Block)
            .expect("set absolute display");
        tree.set_box_sizing(absolute, BoxSizing::ContentBox)
            .expect("set absolute box sizing");
        tree.set_position_type(absolute, PositionType::Absolute)
            .expect("set absolute position");
        tree.set_width(absolute, Length::points(20.0))
            .expect("set absolute width");
        tree.set_height(absolute, Length::points(10.0))
            .expect("set absolute height");
        tree.append_child(root, absolute)
            .expect("append absolute child");

        run_standalone_rust(tree, root, Constraints::definite(100.0, 50.0))
            .expect("absolute linear cross-gravity static cross-position parity");
    }

    {
        let mut tree = StandaloneTree::new();
        let root = tree.create_default_node();
        tree.set_display(root, Display::Linear)
            .expect("set root display");
        tree.set_box_sizing(root, BoxSizing::ContentBox)
            .expect("set root box sizing");
        tree.set_linear_orientation(root, LinearOrientation::Horizontal)
            .expect("set root orientation");
        tree.set_align_items(root, AlignItems::FlexEnd)
            .expect("set root align items");
        tree.set_linear_cross_gravity(root, LinearCrossGravity::None)
            .expect("set root cross gravity");
        tree.set_width(root, Length::points(100.0))
            .expect("set root width");
        tree.set_height(root, Length::points(50.0))
            .expect("set root height");

        let absolute = tree.create_default_node();
        tree.set_display(absolute, Display::Block)
            .expect("set absolute display");
        tree.set_box_sizing(absolute, BoxSizing::ContentBox)
            .expect("set absolute box sizing");
        tree.set_position_type(absolute, PositionType::Absolute)
            .expect("set absolute position");
        tree.set_width(absolute, Length::points(20.0))
            .expect("set absolute width");
        tree.set_height(absolute, Length::points(10.0))
            .expect("set absolute height");
        tree.append_child(root, absolute)
            .expect("append absolute child");

        run_standalone_rust(tree, root, Constraints::definite(100.0, 50.0))
            .expect("absolute linear align-items static cross-position parity");
    }

    {
        let mut tree = StandaloneTree::new();
        let root = tree.create_default_node();
        tree.set_display(root, Display::Linear)
            .expect("set root display");
        tree.set_box_sizing(root, BoxSizing::ContentBox)
            .expect("set root box sizing");
        tree.set_linear_orientation(root, LinearOrientation::Horizontal)
            .expect("set root orientation");
        tree.set_align_items(root, AlignItems::Stretch)
            .expect("set root align items");
        tree.set_linear_cross_gravity(root, LinearCrossGravity::None)
            .expect("set root cross gravity");
        tree.set_width(root, Length::points(100.0))
            .expect("set root width");
        tree.set_height(root, Length::points(50.0))
            .expect("set root height");

        let absolute = tree.create_default_node();
        tree.set_display(absolute, Display::Block)
            .expect("set absolute display");
        tree.set_box_sizing(absolute, BoxSizing::ContentBox)
            .expect("set absolute box sizing");
        tree.set_position_type(absolute, PositionType::Absolute)
            .expect("set absolute position");
        tree.set_width(absolute, Length::points(20.0))
            .expect("set absolute width");
        tree.set_height(absolute, Length::points(10.0))
            .expect("set absolute height");
        tree.append_child(root, absolute)
            .expect("append absolute child");

        run_standalone_rust(tree, root, Constraints::definite(100.0, 50.0))
            .expect("absolute linear stretch align-items non-fallback parity");
    }
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_absolute_align_self_center_static_position_over_cross_gravity_end()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root orientation");
    tree.set_align_items(root, AlignItems::FlexStart)
        .expect("set root align items");
    tree.set_linear_cross_gravity(root, LinearCrossGravity::End)
        .expect("set root cross gravity");
    tree.set_width(root, Length::points(140.0))
        .expect("set root width");
    tree.set_height(root, Length::points(84.0))
        .expect("set root height");

    let absolute = tree.create_default_node();
    tree.set_display(absolute, Display::Block)
        .expect("set absolute display");
    tree.set_box_sizing(absolute, BoxSizing::ContentBox)
        .expect("set absolute box sizing");
    tree.set_position_type(absolute, PositionType::Absolute)
        .expect("set absolute position");
    tree.set_align_self(absolute, Some(AlignItems::Center))
        .expect("set absolute align self");
    tree.set_width(absolute, Length::points(28.0))
        .expect("set absolute width");
    tree.set_height(absolute, Length::points(16.0))
        .expect("set absolute height");
    tree.set_margin(absolute, StandaloneEdge::Top, Length::calc(3.0, 8.0))
        .expect("set absolute top margin");
    tree.set_margin(absolute, StandaloneEdge::Bottom, Length::calc(5.0, 4.0))
        .expect("set absolute bottom margin");
    tree.append_child(root, absolute)
        .expect("append absolute child");

    run_standalone_rust(tree, root, Constraints::definite(140.0, 84.0))
        .expect("linear absolute align-self center static position over cross-gravity parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_absolute_layout_gravity_top_static_position_over_align_self_end()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root orientation");
    tree.set_align_items(root, AlignItems::FlexStart)
        .expect("set root align items");
    tree.set_linear_cross_gravity(root, LinearCrossGravity::Center)
        .expect("set root cross gravity");
    tree.set_width(root, Length::points(144.0))
        .expect("set root width");
    tree.set_height(root, Length::points(88.0))
        .expect("set root height");

    let absolute = tree.create_default_node();
    tree.set_display(absolute, Display::Block)
        .expect("set absolute display");
    tree.set_box_sizing(absolute, BoxSizing::ContentBox)
        .expect("set absolute box sizing");
    tree.set_position_type(absolute, PositionType::Absolute)
        .expect("set absolute position");
    tree.set_linear_layout_gravity(absolute, LinearLayoutGravity::Top)
        .expect("set absolute layout gravity");
    tree.set_align_self(absolute, Some(AlignItems::FlexEnd))
        .expect("set absolute align self");
    tree.set_width(absolute, Length::points(24.0))
        .expect("set absolute width");
    tree.set_height(absolute, Length::points(14.0))
        .expect("set absolute height");
    tree.set_margin(absolute, StandaloneEdge::Top, Length::percent(6.0))
        .expect("set absolute top margin");
    tree.set_margin(absolute, StandaloneEdge::Bottom, Length::percent(4.0))
        .expect("set absolute bottom margin");
    tree.append_child(root, absolute)
        .expect("append absolute child");

    run_standalone_rust(tree, root, Constraints::definite(144.0, 88.0))
        .expect("linear absolute layout-gravity top static position over align-self parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_fixed_align_self_center_static_position_over_cross_gravity_end()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root orientation");
    tree.set_align_items(root, AlignItems::FlexStart)
        .expect("set root align items");
    tree.set_linear_cross_gravity(root, LinearCrossGravity::End)
        .expect("set root cross gravity");
    tree.set_width(root, Length::points(160.0))
        .expect("set root width");
    tree.set_height(root, Length::points(90.0))
        .expect("set root height");

    let nested = tree.create_default_node();
    tree.set_display(nested, Display::Block)
        .expect("set nested display");
    tree.set_box_sizing(nested, BoxSizing::ContentBox)
        .expect("set nested box sizing");
    tree.set_width(nested, Length::points(36.0))
        .expect("set nested width");
    tree.set_height(nested, Length::points(24.0))
        .expect("set nested height");

    let fixed = tree.create_default_node();
    tree.set_display(fixed, Display::Block)
        .expect("set fixed display");
    tree.set_box_sizing(fixed, BoxSizing::ContentBox)
        .expect("set fixed box sizing");
    tree.set_position_type(fixed, PositionType::Fixed)
        .expect("set fixed position");
    tree.set_align_self(fixed, Some(AlignItems::Center))
        .expect("set fixed align self");
    tree.set_width(fixed, Length::points(30.0))
        .expect("set fixed width");
    tree.set_height(fixed, Length::points(18.0))
        .expect("set fixed height");
    tree.set_margin(fixed, StandaloneEdge::Top, Length::calc(4.0, 7.0))
        .expect("set fixed top margin");
    tree.set_margin(fixed, StandaloneEdge::Bottom, Length::calc(2.0, 6.0))
        .expect("set fixed bottom margin");

    tree.append_child(root, nested).expect("append nested");
    tree.append_child(nested, fixed).expect("append fixed");

    run_standalone_rust(tree, root, Constraints::definite(160.0, 90.0))
        .expect("linear fixed align-self center static position over cross-gravity parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_fixed_layout_gravity_bottom_static_position_over_align_self_center()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root orientation");
    tree.set_align_items(root, AlignItems::FlexStart)
        .expect("set root align items");
    tree.set_linear_cross_gravity(root, LinearCrossGravity::None)
        .expect("set root cross gravity");
    tree.set_width(root, Length::points(168.0))
        .expect("set root width");
    tree.set_height(root, Length::points(92.0))
        .expect("set root height");

    let nested = tree.create_default_node();
    tree.set_display(nested, Display::Block)
        .expect("set nested display");
    tree.set_box_sizing(nested, BoxSizing::ContentBox)
        .expect("set nested box sizing");
    tree.set_width(nested, Length::points(40.0))
        .expect("set nested width");
    tree.set_height(nested, Length::points(26.0))
        .expect("set nested height");

    let fixed = tree.create_default_node();
    tree.set_display(fixed, Display::Block)
        .expect("set fixed display");
    tree.set_box_sizing(fixed, BoxSizing::ContentBox)
        .expect("set fixed box sizing");
    tree.set_position_type(fixed, PositionType::Fixed)
        .expect("set fixed position");
    tree.set_linear_layout_gravity(fixed, LinearLayoutGravity::Bottom)
        .expect("set fixed layout gravity");
    tree.set_align_self(fixed, Some(AlignItems::Center))
        .expect("set fixed align self");
    tree.set_width(fixed, Length::points(26.0))
        .expect("set fixed width");
    tree.set_height(fixed, Length::points(12.0))
        .expect("set fixed height");
    tree.set_margin(fixed, StandaloneEdge::Top, Length::percent(5.0))
        .expect("set fixed top margin");
    tree.set_margin(fixed, StandaloneEdge::Bottom, Length::percent(3.0))
        .expect("set fixed bottom margin");

    tree.append_child(root, nested).expect("append nested");
    tree.append_child(nested, fixed).expect("append fixed");

    run_standalone_rust(tree, root, Constraints::definite(168.0, 92.0))
        .expect("linear fixed layout-gravity bottom static position over align-self parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_vertical_linear_absolute_gravity_end_static_position_over_justify_center()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Vertical)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::End)
        .expect("set root gravity");
    tree.set_justify_content(root, JustifyContent::Center)
        .expect("set root justify content");
    tree.set_width(root, Length::points(76.0))
        .expect("set root width");
    tree.set_height(root, Length::points(156.0))
        .expect("set root height");

    let absolute = tree.create_default_node();
    tree.set_display(absolute, Display::Block)
        .expect("set absolute display");
    tree.set_box_sizing(absolute, BoxSizing::ContentBox)
        .expect("set absolute box sizing");
    tree.set_position_type(absolute, PositionType::Absolute)
        .expect("set absolute position");
    tree.set_width(absolute, Length::points(28.0))
        .expect("set absolute width");
    tree.set_height(absolute, Length::points(18.0))
        .expect("set absolute height");
    tree.set_margin(absolute, StandaloneEdge::Top, Length::calc(3.0, 8.0))
        .expect("set absolute top margin");
    tree.set_margin(absolute, StandaloneEdge::Bottom, Length::calc(5.0, 6.0))
        .expect("set absolute bottom margin");
    tree.append_child(root, absolute)
        .expect("append absolute child");

    run_standalone_rust(tree, root, Constraints::definite(76.0, 156.0))
        .expect("vertical linear absolute gravity end static position over justify parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_vertical_linear_fixed_justify_flex_end_static_position_without_linear_gravity()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Vertical)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::None)
        .expect("set root gravity");
    tree.set_justify_content(root, JustifyContent::FlexEnd)
        .expect("set root justify content");
    tree.set_width(root, Length::points(82.0))
        .expect("set root width");
    tree.set_height(root, Length::points(164.0))
        .expect("set root height");

    let nested = tree.create_default_node();
    tree.set_display(nested, Display::Block)
        .expect("set nested display");
    tree.set_box_sizing(nested, BoxSizing::ContentBox)
        .expect("set nested box sizing");
    tree.set_width(nested, Length::points(36.0))
        .expect("set nested width");
    tree.set_height(nested, Length::points(24.0))
        .expect("set nested height");

    let fixed = tree.create_default_node();
    tree.set_display(fixed, Display::Block)
        .expect("set fixed display");
    tree.set_box_sizing(fixed, BoxSizing::ContentBox)
        .expect("set fixed box sizing");
    tree.set_position_type(fixed, PositionType::Fixed)
        .expect("set fixed position");
    tree.set_width(fixed, Length::points(30.0))
        .expect("set fixed width");
    tree.set_height(fixed, Length::points(16.0))
        .expect("set fixed height");
    tree.set_margin(fixed, StandaloneEdge::Top, Length::percent(5.0))
        .expect("set fixed top margin");
    tree.set_margin(fixed, StandaloneEdge::Bottom, Length::percent(4.0))
        .expect("set fixed bottom margin");

    tree.append_child(root, nested).expect("append nested");
    tree.append_child(nested, fixed).expect("append fixed");

    run_standalone_rust(tree, root, Constraints::definite(82.0, 164.0))
        .expect("vertical linear fixed justify flex-end static position without gravity parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_vertical_linear_absolute_justify_center_static_position_without_linear_gravity()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Vertical)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::None)
        .expect("set root gravity");
    tree.set_justify_content(root, JustifyContent::Center)
        .expect("set root justify content");
    tree.set_width(root, Length::points(84.0))
        .expect("set root width");
    tree.set_height(root, Length::points(166.0))
        .expect("set root height");

    let absolute = tree.create_default_node();
    tree.set_display(absolute, Display::Block)
        .expect("set absolute display");
    tree.set_box_sizing(absolute, BoxSizing::ContentBox)
        .expect("set absolute box sizing");
    tree.set_position_type(absolute, PositionType::Absolute)
        .expect("set absolute position");
    tree.set_width(absolute, Length::points(26.0))
        .expect("set absolute width");
    tree.set_height(absolute, Length::points(14.0))
        .expect("set absolute height");
    tree.set_margin(absolute, StandaloneEdge::Top, Length::calc(4.0, 7.0))
        .expect("set absolute top margin");
    tree.set_margin(absolute, StandaloneEdge::Bottom, Length::calc(2.0, 5.0))
        .expect("set absolute bottom margin");
    tree.append_child(root, absolute)
        .expect("append absolute child");

    run_standalone_rust(tree, root, Constraints::definite(84.0, 166.0))
        .expect("vertical linear absolute justify center static position without gravity parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_vertical_linear_absolute_justify_flex_end_static_position_without_linear_gravity()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Vertical)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::None)
        .expect("set root gravity");
    tree.set_justify_content(root, JustifyContent::FlexEnd)
        .expect("set root justify content");
    tree.set_width(root, Length::points(86.0))
        .expect("set root width");
    tree.set_height(root, Length::points(168.0))
        .expect("set root height");

    let absolute = tree.create_default_node();
    tree.set_display(absolute, Display::Block)
        .expect("set absolute display");
    tree.set_box_sizing(absolute, BoxSizing::ContentBox)
        .expect("set absolute box sizing");
    tree.set_position_type(absolute, PositionType::Absolute)
        .expect("set absolute position");
    tree.set_width(absolute, Length::points(24.0))
        .expect("set absolute width");
    tree.set_height(absolute, Length::points(16.0))
        .expect("set absolute height");
    tree.set_margin(absolute, StandaloneEdge::Top, Length::percent(4.0))
        .expect("set absolute top margin");
    tree.set_margin(absolute, StandaloneEdge::Bottom, Length::percent(6.0))
        .expect("set absolute bottom margin");
    tree.append_child(root, absolute)
        .expect("append absolute child");

    run_standalone_rust(tree, root, Constraints::definite(86.0, 168.0))
        .expect("vertical linear absolute justify flex-end static position without gravity parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_vertical_linear_absolute_justify_space_between_static_start_without_linear_gravity()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Vertical)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::None)
        .expect("set root gravity");
    tree.set_justify_content(root, JustifyContent::SpaceBetween)
        .expect("set root justify content");
    tree.set_width(root, Length::points(88.0))
        .expect("set root width");
    tree.set_height(root, Length::points(170.0))
        .expect("set root height");

    let absolute = tree.create_default_node();
    tree.set_display(absolute, Display::Block)
        .expect("set absolute display");
    tree.set_box_sizing(absolute, BoxSizing::ContentBox)
        .expect("set absolute box sizing");
    tree.set_position_type(absolute, PositionType::Absolute)
        .expect("set absolute position");
    tree.set_width(absolute, Length::points(22.0))
        .expect("set absolute width");
    tree.set_height(absolute, Length::points(15.0))
        .expect("set absolute height");
    tree.set_margin(absolute, StandaloneEdge::Top, Length::calc(3.0, 8.0))
        .expect("set absolute top margin");
    tree.set_margin(absolute, StandaloneEdge::Bottom, Length::calc(5.0, 3.0))
        .expect("set absolute bottom margin");
    tree.append_child(root, absolute)
        .expect("append absolute child");

    run_standalone_rust(tree, root, Constraints::definite(88.0, 170.0))
        .expect("vertical linear absolute justify space-between static start parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_vertical_linear_absolute_justify_space_around_static_start_without_linear_gravity()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Vertical)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::None)
        .expect("set root gravity");
    tree.set_justify_content(root, JustifyContent::SpaceAround)
        .expect("set root justify content");
    tree.set_width(root, Length::points(90.0))
        .expect("set root width");
    tree.set_height(root, Length::points(172.0))
        .expect("set root height");

    let absolute = tree.create_default_node();
    tree.set_display(absolute, Display::Block)
        .expect("set absolute display");
    tree.set_box_sizing(absolute, BoxSizing::ContentBox)
        .expect("set absolute box sizing");
    tree.set_position_type(absolute, PositionType::Absolute)
        .expect("set absolute position");
    tree.set_width(absolute, Length::points(20.0))
        .expect("set absolute width");
    tree.set_height(absolute, Length::points(13.0))
        .expect("set absolute height");
    tree.set_margin(absolute, StandaloneEdge::Top, Length::percent(5.0))
        .expect("set absolute top margin");
    tree.set_margin(absolute, StandaloneEdge::Bottom, Length::percent(2.0))
        .expect("set absolute bottom margin");
    tree.append_child(root, absolute)
        .expect("append absolute child");

    run_standalone_rust(tree, root, Constraints::definite(90.0, 172.0))
        .expect("vertical linear absolute justify space-around static start parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_vertical_linear_absolute_justify_flex_start_static_position_without_linear_gravity()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Vertical)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::None)
        .expect("set root gravity");
    tree.set_justify_content(root, JustifyContent::FlexStart)
        .expect("set root justify content");
    tree.set_width(root, Length::points(91.0))
        .expect("set root width");
    tree.set_height(root, Length::points(173.0))
        .expect("set root height");

    let absolute = tree.create_default_node();
    tree.set_display(absolute, Display::Block)
        .expect("set absolute display");
    tree.set_box_sizing(absolute, BoxSizing::ContentBox)
        .expect("set absolute box sizing");
    tree.set_position_type(absolute, PositionType::Absolute)
        .expect("set absolute position");
    tree.set_width(absolute, Length::points(31.0))
        .expect("set absolute width");
    tree.set_height(absolute, Length::points(14.0))
        .expect("set absolute height");
    tree.set_margin(absolute, StandaloneEdge::Top, Length::percent(4.0))
        .expect("set absolute top margin");
    tree.set_margin(absolute, StandaloneEdge::Bottom, Length::percent(6.0))
        .expect("set absolute bottom margin");
    tree.append_child(root, absolute)
        .expect("append absolute child");

    run_standalone_rust(tree, root, Constraints::definite(91.0, 173.0))
        .expect("vertical linear absolute justify flex-start static position parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_vertical_linear_absolute_justify_start_static_position_without_linear_gravity()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Vertical)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::None)
        .expect("set root gravity");
    tree.set_justify_content(root, JustifyContent::Start)
        .expect("set root justify content");
    tree.set_width(root, Length::points(93.0))
        .expect("set root width");
    tree.set_height(root, Length::points(175.0))
        .expect("set root height");

    let absolute = tree.create_default_node();
    tree.set_display(absolute, Display::Block)
        .expect("set absolute display");
    tree.set_box_sizing(absolute, BoxSizing::ContentBox)
        .expect("set absolute box sizing");
    tree.set_position_type(absolute, PositionType::Absolute)
        .expect("set absolute position");
    tree.set_width(absolute, Length::points(33.0))
        .expect("set absolute width");
    tree.set_height(absolute, Length::points(15.0))
        .expect("set absolute height");
    tree.set_margin(absolute, StandaloneEdge::Top, Length::calc(3.0, 5.0))
        .expect("set absolute top margin");
    tree.set_margin(absolute, StandaloneEdge::Bottom, Length::calc(5.0, 3.0))
        .expect("set absolute bottom margin");
    tree.append_child(root, absolute)
        .expect("append absolute child");

    run_standalone_rust(tree, root, Constraints::definite(93.0, 175.0))
        .expect("vertical linear absolute justify start static position parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_vertical_linear_absolute_justify_end_static_position_without_linear_gravity()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Vertical)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::None)
        .expect("set root gravity");
    tree.set_justify_content(root, JustifyContent::End)
        .expect("set root justify content");
    tree.set_width(root, Length::points(95.0))
        .expect("set root width");
    tree.set_height(root, Length::points(177.0))
        .expect("set root height");

    let absolute = tree.create_default_node();
    tree.set_display(absolute, Display::Block)
        .expect("set absolute display");
    tree.set_box_sizing(absolute, BoxSizing::ContentBox)
        .expect("set absolute box sizing");
    tree.set_position_type(absolute, PositionType::Absolute)
        .expect("set absolute position");
    tree.set_width(absolute, Length::points(35.0))
        .expect("set absolute width");
    tree.set_height(absolute, Length::points(16.0))
        .expect("set absolute height");
    tree.set_margin(absolute, StandaloneEdge::Top, Length::percent(5.0))
        .expect("set absolute top margin");
    tree.set_margin(absolute, StandaloneEdge::Bottom, Length::percent(3.0))
        .expect("set absolute bottom margin");
    tree.append_child(root, absolute)
        .expect("append absolute child");

    run_standalone_rust(tree, root, Constraints::definite(95.0, 177.0))
        .expect("vertical linear absolute justify end static position parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_vertical_linear_absolute_justify_space_evenly_static_start_without_linear_gravity()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Vertical)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::None)
        .expect("set root gravity");
    tree.set_justify_content(root, JustifyContent::SpaceEvenly)
        .expect("set root justify content");
    tree.set_width(root, Length::points(97.0))
        .expect("set root width");
    tree.set_height(root, Length::points(179.0))
        .expect("set root height");

    let absolute = tree.create_default_node();
    tree.set_display(absolute, Display::Block)
        .expect("set absolute display");
    tree.set_box_sizing(absolute, BoxSizing::ContentBox)
        .expect("set absolute box sizing");
    tree.set_position_type(absolute, PositionType::Absolute)
        .expect("set absolute position");
    tree.set_width(absolute, Length::points(37.0))
        .expect("set absolute width");
    tree.set_height(absolute, Length::points(17.0))
        .expect("set absolute height");
    tree.set_margin(absolute, StandaloneEdge::Top, Length::calc(4.0, 4.0))
        .expect("set absolute top margin");
    tree.set_margin(absolute, StandaloneEdge::Bottom, Length::calc(6.0, 2.0))
        .expect("set absolute bottom margin");
    tree.append_child(root, absolute)
        .expect("append absolute child");

    run_standalone_rust(tree, root, Constraints::definite(97.0, 179.0))
        .expect("vertical linear absolute justify space-evenly static start parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_vertical_linear_absolute_justify_stretch_static_start_without_linear_gravity()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Vertical)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::None)
        .expect("set root gravity");
    tree.set_justify_content(root, JustifyContent::Stretch)
        .expect("set root justify content");
    tree.set_width(root, Length::points(99.0))
        .expect("set root width");
    tree.set_height(root, Length::points(181.0))
        .expect("set root height");

    let absolute = tree.create_default_node();
    tree.set_display(absolute, Display::Block)
        .expect("set absolute display");
    tree.set_box_sizing(absolute, BoxSizing::ContentBox)
        .expect("set absolute box sizing");
    tree.set_position_type(absolute, PositionType::Absolute)
        .expect("set absolute position");
    tree.set_width(absolute, Length::points(39.0))
        .expect("set absolute width");
    tree.set_height(absolute, Length::points(18.0))
        .expect("set absolute height");
    tree.set_margin(absolute, StandaloneEdge::Top, Length::percent(6.0))
        .expect("set absolute top margin");
    tree.set_margin(absolute, StandaloneEdge::Bottom, Length::percent(4.0))
        .expect("set absolute bottom margin");
    tree.append_child(root, absolute)
        .expect("append absolute child");

    run_standalone_rust(tree, root, Constraints::definite(99.0, 181.0))
        .expect("vertical linear absolute justify stretch static start parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_vertical_linear_fixed_justify_center_static_position_without_linear_gravity()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Vertical)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::None)
        .expect("set root gravity");
    tree.set_justify_content(root, JustifyContent::Center)
        .expect("set root justify content");
    tree.set_width(root, Length::points(92.0))
        .expect("set root width");
    tree.set_height(root, Length::points(174.0))
        .expect("set root height");

    let nested = tree.create_default_node();
    tree.set_display(nested, Display::Block)
        .expect("set nested display");
    tree.set_box_sizing(nested, BoxSizing::ContentBox)
        .expect("set nested box sizing");
    tree.set_width(nested, Length::points(34.0))
        .expect("set nested width");
    tree.set_height(nested, Length::points(22.0))
        .expect("set nested height");

    let fixed = tree.create_default_node();
    tree.set_display(fixed, Display::Block)
        .expect("set fixed display");
    tree.set_box_sizing(fixed, BoxSizing::ContentBox)
        .expect("set fixed box sizing");
    tree.set_position_type(fixed, PositionType::Fixed)
        .expect("set fixed position");
    tree.set_width(fixed, Length::points(28.0))
        .expect("set fixed width");
    tree.set_height(fixed, Length::points(14.0))
        .expect("set fixed height");
    tree.set_margin(fixed, StandaloneEdge::Top, Length::calc(4.0, 6.0))
        .expect("set fixed top margin");
    tree.set_margin(fixed, StandaloneEdge::Bottom, Length::calc(3.0, 5.0))
        .expect("set fixed bottom margin");

    tree.append_child(root, nested).expect("append nested");
    tree.append_child(nested, fixed).expect("append fixed");

    run_standalone_rust(tree, root, Constraints::definite(92.0, 174.0))
        .expect("vertical linear fixed justify center static position without gravity parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_vertical_linear_fixed_justify_space_evenly_static_start_without_linear_gravity()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Vertical)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::None)
        .expect("set root gravity");
    tree.set_justify_content(root, JustifyContent::SpaceEvenly)
        .expect("set root justify content");
    tree.set_width(root, Length::points(94.0))
        .expect("set root width");
    tree.set_height(root, Length::points(176.0))
        .expect("set root height");

    let nested = tree.create_default_node();
    tree.set_display(nested, Display::Block)
        .expect("set nested display");
    tree.set_box_sizing(nested, BoxSizing::ContentBox)
        .expect("set nested box sizing");
    tree.set_width(nested, Length::points(32.0))
        .expect("set nested width");
    tree.set_height(nested, Length::points(24.0))
        .expect("set nested height");

    let fixed = tree.create_default_node();
    tree.set_display(fixed, Display::Block)
        .expect("set fixed display");
    tree.set_box_sizing(fixed, BoxSizing::ContentBox)
        .expect("set fixed box sizing");
    tree.set_position_type(fixed, PositionType::Fixed)
        .expect("set fixed position");
    tree.set_width(fixed, Length::points(26.0))
        .expect("set fixed width");
    tree.set_height(fixed, Length::points(16.0))
        .expect("set fixed height");
    tree.set_margin(fixed, StandaloneEdge::Top, Length::percent(6.0))
        .expect("set fixed top margin");
    tree.set_margin(fixed, StandaloneEdge::Bottom, Length::percent(4.0))
        .expect("set fixed bottom margin");

    tree.append_child(root, nested).expect("append nested");
    tree.append_child(nested, fixed).expect("append fixed");

    run_standalone_rust(tree, root, Constraints::definite(94.0, 176.0))
        .expect("vertical linear fixed justify space-evenly static start parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_vertical_linear_fixed_justify_stretch_static_start_without_linear_gravity()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Vertical)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::None)
        .expect("set root gravity");
    tree.set_justify_content(root, JustifyContent::Stretch)
        .expect("set root justify content");
    tree.set_width(root, Length::points(96.0))
        .expect("set root width");
    tree.set_height(root, Length::points(178.0))
        .expect("set root height");

    let nested = tree.create_default_node();
    tree.set_display(nested, Display::Block)
        .expect("set nested display");
    tree.set_box_sizing(nested, BoxSizing::ContentBox)
        .expect("set nested box sizing");
    tree.set_width(nested, Length::points(30.0))
        .expect("set nested width");
    tree.set_height(nested, Length::points(20.0))
        .expect("set nested height");

    let fixed = tree.create_default_node();
    tree.set_display(fixed, Display::Block)
        .expect("set fixed display");
    tree.set_box_sizing(fixed, BoxSizing::ContentBox)
        .expect("set fixed box sizing");
    tree.set_position_type(fixed, PositionType::Fixed)
        .expect("set fixed position");
    tree.set_width(fixed, Length::points(24.0))
        .expect("set fixed width");
    tree.set_height(fixed, Length::points(15.0))
        .expect("set fixed height");
    tree.set_margin(fixed, StandaloneEdge::Top, Length::calc(5.0, 4.0))
        .expect("set fixed top margin");
    tree.set_margin(fixed, StandaloneEdge::Bottom, Length::calc(2.0, 7.0))
        .expect("set fixed bottom margin");

    tree.append_child(root, nested).expect("append nested");
    tree.append_child(nested, fixed).expect("append fixed");

    run_standalone_rust(tree, root, Constraints::definite(96.0, 178.0))
        .expect("vertical linear fixed justify stretch static start parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_vertical_linear_fixed_justify_flex_start_static_position_without_linear_gravity()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Vertical)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::None)
        .expect("set root gravity");
    tree.set_justify_content(root, JustifyContent::FlexStart)
        .expect("set root justify content");
    tree.set_width(root, Length::points(98.0))
        .expect("set root width");
    tree.set_height(root, Length::points(180.0))
        .expect("set root height");

    let nested = tree.create_default_node();
    tree.set_display(nested, Display::Block)
        .expect("set nested display");
    tree.set_box_sizing(nested, BoxSizing::ContentBox)
        .expect("set nested box sizing");
    tree.set_width(nested, Length::points(35.0))
        .expect("set nested width");
    tree.set_height(nested, Length::points(21.0))
        .expect("set nested height");

    let fixed = tree.create_default_node();
    tree.set_display(fixed, Display::Block)
        .expect("set fixed display");
    tree.set_box_sizing(fixed, BoxSizing::ContentBox)
        .expect("set fixed box sizing");
    tree.set_position_type(fixed, PositionType::Fixed)
        .expect("set fixed position");
    tree.set_width(fixed, Length::points(25.0))
        .expect("set fixed width");
    tree.set_height(fixed, Length::points(14.0))
        .expect("set fixed height");
    tree.set_margin(fixed, StandaloneEdge::Top, Length::percent(4.0))
        .expect("set fixed top margin");
    tree.set_margin(fixed, StandaloneEdge::Bottom, Length::percent(6.0))
        .expect("set fixed bottom margin");

    tree.append_child(root, nested).expect("append nested");
    tree.append_child(nested, fixed).expect("append fixed");

    run_standalone_rust(tree, root, Constraints::definite(98.0, 180.0))
        .expect("vertical linear fixed justify flex-start static position parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_vertical_linear_fixed_justify_start_static_position_without_linear_gravity()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Vertical)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::None)
        .expect("set root gravity");
    tree.set_justify_content(root, JustifyContent::Start)
        .expect("set root justify content");
    tree.set_width(root, Length::points(100.0))
        .expect("set root width");
    tree.set_height(root, Length::points(182.0))
        .expect("set root height");

    let nested = tree.create_default_node();
    tree.set_display(nested, Display::Block)
        .expect("set nested display");
    tree.set_box_sizing(nested, BoxSizing::ContentBox)
        .expect("set nested box sizing");
    tree.set_width(nested, Length::points(37.0))
        .expect("set nested width");
    tree.set_height(nested, Length::points(22.0))
        .expect("set nested height");

    let fixed = tree.create_default_node();
    tree.set_display(fixed, Display::Block)
        .expect("set fixed display");
    tree.set_box_sizing(fixed, BoxSizing::ContentBox)
        .expect("set fixed box sizing");
    tree.set_position_type(fixed, PositionType::Fixed)
        .expect("set fixed position");
    tree.set_width(fixed, Length::points(27.0))
        .expect("set fixed width");
    tree.set_height(fixed, Length::points(15.0))
        .expect("set fixed height");
    tree.set_margin(fixed, StandaloneEdge::Top, Length::calc(2.0, 5.0))
        .expect("set fixed top margin");
    tree.set_margin(fixed, StandaloneEdge::Bottom, Length::calc(4.0, 3.0))
        .expect("set fixed bottom margin");

    tree.append_child(root, nested).expect("append nested");
    tree.append_child(nested, fixed).expect("append fixed");

    run_standalone_rust(tree, root, Constraints::definite(100.0, 182.0))
        .expect("vertical linear fixed justify start static position parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_vertical_linear_fixed_justify_end_static_position_without_linear_gravity()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Vertical)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::None)
        .expect("set root gravity");
    tree.set_justify_content(root, JustifyContent::End)
        .expect("set root justify content");
    tree.set_width(root, Length::points(102.0))
        .expect("set root width");
    tree.set_height(root, Length::points(184.0))
        .expect("set root height");

    let nested = tree.create_default_node();
    tree.set_display(nested, Display::Block)
        .expect("set nested display");
    tree.set_box_sizing(nested, BoxSizing::ContentBox)
        .expect("set nested box sizing");
    tree.set_width(nested, Length::points(41.0))
        .expect("set nested width");
    tree.set_height(nested, Length::points(24.0))
        .expect("set nested height");

    let fixed = tree.create_default_node();
    tree.set_display(fixed, Display::Block)
        .expect("set fixed display");
    tree.set_box_sizing(fixed, BoxSizing::ContentBox)
        .expect("set fixed box sizing");
    tree.set_position_type(fixed, PositionType::Fixed)
        .expect("set fixed position");
    tree.set_width(fixed, Length::points(31.0))
        .expect("set fixed width");
    tree.set_height(fixed, Length::points(17.0))
        .expect("set fixed height");
    tree.set_margin(fixed, StandaloneEdge::Top, Length::percent(5.0))
        .expect("set fixed top margin");
    tree.set_margin(fixed, StandaloneEdge::Bottom, Length::percent(3.0))
        .expect("set fixed bottom margin");

    tree.append_child(root, nested).expect("append nested");
    tree.append_child(nested, fixed).expect("append fixed");

    run_standalone_rust(tree, root, Constraints::definite(102.0, 184.0))
        .expect("vertical linear fixed justify end static position parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_vertical_linear_fixed_justify_space_between_static_start_without_linear_gravity()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Vertical)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::None)
        .expect("set root gravity");
    tree.set_justify_content(root, JustifyContent::SpaceBetween)
        .expect("set root justify content");
    tree.set_width(root, Length::points(104.0))
        .expect("set root width");
    tree.set_height(root, Length::points(186.0))
        .expect("set root height");

    let nested = tree.create_default_node();
    tree.set_display(nested, Display::Block)
        .expect("set nested display");
    tree.set_box_sizing(nested, BoxSizing::ContentBox)
        .expect("set nested box sizing");
    tree.set_width(nested, Length::points(43.0))
        .expect("set nested width");
    tree.set_height(nested, Length::points(25.0))
        .expect("set nested height");

    let fixed = tree.create_default_node();
    tree.set_display(fixed, Display::Block)
        .expect("set fixed display");
    tree.set_box_sizing(fixed, BoxSizing::ContentBox)
        .expect("set fixed box sizing");
    tree.set_position_type(fixed, PositionType::Fixed)
        .expect("set fixed position");
    tree.set_width(fixed, Length::points(33.0))
        .expect("set fixed width");
    tree.set_height(fixed, Length::points(18.0))
        .expect("set fixed height");
    tree.set_margin(fixed, StandaloneEdge::Top, Length::calc(3.0, 4.0))
        .expect("set fixed top margin");
    tree.set_margin(fixed, StandaloneEdge::Bottom, Length::calc(5.0, 2.0))
        .expect("set fixed bottom margin");

    tree.append_child(root, nested).expect("append nested");
    tree.append_child(nested, fixed).expect("append fixed");

    run_standalone_rust(tree, root, Constraints::definite(104.0, 186.0))
        .expect("vertical linear fixed justify space-between static start parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_vertical_linear_fixed_justify_space_around_static_start_without_linear_gravity()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Vertical)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::None)
        .expect("set root gravity");
    tree.set_justify_content(root, JustifyContent::SpaceAround)
        .expect("set root justify content");
    tree.set_width(root, Length::points(106.0))
        .expect("set root width");
    tree.set_height(root, Length::points(188.0))
        .expect("set root height");

    let nested = tree.create_default_node();
    tree.set_display(nested, Display::Block)
        .expect("set nested display");
    tree.set_box_sizing(nested, BoxSizing::ContentBox)
        .expect("set nested box sizing");
    tree.set_width(nested, Length::points(45.0))
        .expect("set nested width");
    tree.set_height(nested, Length::points(26.0))
        .expect("set nested height");

    let fixed = tree.create_default_node();
    tree.set_display(fixed, Display::Block)
        .expect("set fixed display");
    tree.set_box_sizing(fixed, BoxSizing::ContentBox)
        .expect("set fixed box sizing");
    tree.set_position_type(fixed, PositionType::Fixed)
        .expect("set fixed position");
    tree.set_width(fixed, Length::points(35.0))
        .expect("set fixed width");
    tree.set_height(fixed, Length::points(19.0))
        .expect("set fixed height");
    tree.set_margin(fixed, StandaloneEdge::Top, Length::percent(6.0))
        .expect("set fixed top margin");
    tree.set_margin(fixed, StandaloneEdge::Bottom, Length::percent(4.0))
        .expect("set fixed bottom margin");

    tree.append_child(root, nested).expect("append nested");
    tree.append_child(nested, fixed).expect("append fixed");

    run_standalone_rust(tree, root, Constraints::definite(106.0, 188.0))
        .expect("vertical linear fixed justify space-around static start parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_horizontal_linear_absolute_justify_center_static_position_without_linear_gravity()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::None)
        .expect("set root gravity");
    tree.set_justify_content(root, JustifyContent::Center)
        .expect("set root justify content");
    tree.set_width(root, Length::points(178.0))
        .expect("set root width");
    tree.set_height(root, Length::points(96.0))
        .expect("set root height");

    let absolute = tree.create_default_node();
    tree.set_display(absolute, Display::Block)
        .expect("set absolute display");
    tree.set_box_sizing(absolute, BoxSizing::ContentBox)
        .expect("set absolute box sizing");
    tree.set_position_type(absolute, PositionType::Absolute)
        .expect("set absolute position");
    tree.set_width(absolute, Length::points(24.0))
        .expect("set absolute width");
    tree.set_height(absolute, Length::points(15.0))
        .expect("set absolute height");
    tree.set_margin(absolute, StandaloneEdge::Left, Length::calc(5.0, 4.0))
        .expect("set absolute left margin");
    tree.set_margin(absolute, StandaloneEdge::Right, Length::calc(2.0, 7.0))
        .expect("set absolute right margin");
    tree.append_child(root, absolute)
        .expect("append absolute child");

    run_standalone_rust(tree, root, Constraints::definite(178.0, 96.0))
        .expect("horizontal linear absolute justify center static position without gravity parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_horizontal_linear_absolute_justify_flex_end_static_position_without_linear_gravity()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::None)
        .expect("set root gravity");
    tree.set_justify_content(root, JustifyContent::FlexEnd)
        .expect("set root justify content");
    tree.set_width(root, Length::points(180.0))
        .expect("set root width");
    tree.set_height(root, Length::points(98.0))
        .expect("set root height");

    let absolute = tree.create_default_node();
    tree.set_display(absolute, Display::Block)
        .expect("set absolute display");
    tree.set_box_sizing(absolute, BoxSizing::ContentBox)
        .expect("set absolute box sizing");
    tree.set_position_type(absolute, PositionType::Absolute)
        .expect("set absolute position");
    tree.set_width(absolute, Length::points(26.0))
        .expect("set absolute width");
    tree.set_height(absolute, Length::points(14.0))
        .expect("set absolute height");
    tree.set_margin(absolute, StandaloneEdge::Left, Length::percent(6.0))
        .expect("set absolute left margin");
    tree.set_margin(absolute, StandaloneEdge::Right, Length::percent(4.0))
        .expect("set absolute right margin");
    tree.append_child(root, absolute)
        .expect("append absolute child");

    run_standalone_rust(tree, root, Constraints::definite(180.0, 98.0)).expect(
        "horizontal linear absolute justify flex-end static position without gravity parity",
    );
}

#[test]
fn standalone_owned_tree_matches_cpp_for_horizontal_linear_absolute_justify_space_between_static_start_without_linear_gravity()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::None)
        .expect("set root gravity");
    tree.set_justify_content(root, JustifyContent::SpaceBetween)
        .expect("set root justify content");
    tree.set_width(root, Length::points(182.0))
        .expect("set root width");
    tree.set_height(root, Length::points(100.0))
        .expect("set root height");

    let absolute = tree.create_default_node();
    tree.set_display(absolute, Display::Block)
        .expect("set absolute display");
    tree.set_box_sizing(absolute, BoxSizing::ContentBox)
        .expect("set absolute box sizing");
    tree.set_position_type(absolute, PositionType::Absolute)
        .expect("set absolute position");
    tree.set_width(absolute, Length::points(28.0))
        .expect("set absolute width");
    tree.set_height(absolute, Length::points(16.0))
        .expect("set absolute height");
    tree.set_margin(absolute, StandaloneEdge::Left, Length::calc(4.0, 6.0))
        .expect("set absolute left margin");
    tree.set_margin(absolute, StandaloneEdge::Right, Length::calc(3.0, 5.0))
        .expect("set absolute right margin");
    tree.append_child(root, absolute)
        .expect("append absolute child");

    run_standalone_rust(tree, root, Constraints::definite(182.0, 100.0))
        .expect("horizontal linear absolute justify space-between static start parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_horizontal_linear_absolute_justify_space_around_static_start_without_linear_gravity()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::None)
        .expect("set root gravity");
    tree.set_justify_content(root, JustifyContent::SpaceAround)
        .expect("set root justify content");
    tree.set_width(root, Length::points(184.0))
        .expect("set root width");
    tree.set_height(root, Length::points(102.0))
        .expect("set root height");

    let absolute = tree.create_default_node();
    tree.set_display(absolute, Display::Block)
        .expect("set absolute display");
    tree.set_box_sizing(absolute, BoxSizing::ContentBox)
        .expect("set absolute box sizing");
    tree.set_position_type(absolute, PositionType::Absolute)
        .expect("set absolute position");
    tree.set_width(absolute, Length::points(30.0))
        .expect("set absolute width");
    tree.set_height(absolute, Length::points(13.0))
        .expect("set absolute height");
    tree.set_margin(absolute, StandaloneEdge::Left, Length::percent(3.0))
        .expect("set absolute left margin");
    tree.set_margin(absolute, StandaloneEdge::Right, Length::percent(7.0))
        .expect("set absolute right margin");
    tree.append_child(root, absolute)
        .expect("append absolute child");

    run_standalone_rust(tree, root, Constraints::definite(184.0, 102.0))
        .expect("horizontal linear absolute justify space-around static start parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_horizontal_linear_absolute_justify_flex_start_static_position_without_linear_gravity()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::None)
        .expect("set root gravity");
    tree.set_justify_content(root, JustifyContent::FlexStart)
        .expect("set root justify content");
    tree.set_width(root, Length::points(185.0))
        .expect("set root width");
    tree.set_height(root, Length::points(103.0))
        .expect("set root height");

    let absolute = tree.create_default_node();
    tree.set_display(absolute, Display::Block)
        .expect("set absolute display");
    tree.set_box_sizing(absolute, BoxSizing::ContentBox)
        .expect("set absolute box sizing");
    tree.set_position_type(absolute, PositionType::Absolute)
        .expect("set absolute position");
    tree.set_width(absolute, Length::points(31.0))
        .expect("set absolute width");
    tree.set_height(absolute, Length::points(14.0))
        .expect("set absolute height");
    tree.set_margin(absolute, StandaloneEdge::Left, Length::percent(4.0))
        .expect("set absolute left margin");
    tree.set_margin(absolute, StandaloneEdge::Right, Length::percent(6.0))
        .expect("set absolute right margin");
    tree.append_child(root, absolute)
        .expect("append absolute child");

    run_standalone_rust(tree, root, Constraints::definite(185.0, 103.0))
        .expect("horizontal linear absolute justify flex-start static position parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_horizontal_linear_absolute_justify_start_static_position_without_linear_gravity()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::None)
        .expect("set root gravity");
    tree.set_justify_content(root, JustifyContent::Start)
        .expect("set root justify content");
    tree.set_width(root, Length::points(187.0))
        .expect("set root width");
    tree.set_height(root, Length::points(105.0))
        .expect("set root height");

    let absolute = tree.create_default_node();
    tree.set_display(absolute, Display::Block)
        .expect("set absolute display");
    tree.set_box_sizing(absolute, BoxSizing::ContentBox)
        .expect("set absolute box sizing");
    tree.set_position_type(absolute, PositionType::Absolute)
        .expect("set absolute position");
    tree.set_width(absolute, Length::points(33.0))
        .expect("set absolute width");
    tree.set_height(absolute, Length::points(15.0))
        .expect("set absolute height");
    tree.set_margin(absolute, StandaloneEdge::Left, Length::calc(3.0, 5.0))
        .expect("set absolute left margin");
    tree.set_margin(absolute, StandaloneEdge::Right, Length::calc(5.0, 3.0))
        .expect("set absolute right margin");
    tree.append_child(root, absolute)
        .expect("append absolute child");

    run_standalone_rust(tree, root, Constraints::definite(187.0, 105.0))
        .expect("horizontal linear absolute justify start static position parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_horizontal_linear_absolute_justify_end_static_position_without_linear_gravity()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::None)
        .expect("set root gravity");
    tree.set_justify_content(root, JustifyContent::End)
        .expect("set root justify content");
    tree.set_width(root, Length::points(189.0))
        .expect("set root width");
    tree.set_height(root, Length::points(107.0))
        .expect("set root height");

    let absolute = tree.create_default_node();
    tree.set_display(absolute, Display::Block)
        .expect("set absolute display");
    tree.set_box_sizing(absolute, BoxSizing::ContentBox)
        .expect("set absolute box sizing");
    tree.set_position_type(absolute, PositionType::Absolute)
        .expect("set absolute position");
    tree.set_width(absolute, Length::points(35.0))
        .expect("set absolute width");
    tree.set_height(absolute, Length::points(16.0))
        .expect("set absolute height");
    tree.set_margin(absolute, StandaloneEdge::Left, Length::percent(5.0))
        .expect("set absolute left margin");
    tree.set_margin(absolute, StandaloneEdge::Right, Length::percent(3.0))
        .expect("set absolute right margin");
    tree.append_child(root, absolute)
        .expect("append absolute child");

    run_standalone_rust(tree, root, Constraints::definite(189.0, 107.0))
        .expect("horizontal linear absolute justify end static position parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_horizontal_linear_absolute_justify_space_evenly_static_start_without_linear_gravity()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::None)
        .expect("set root gravity");
    tree.set_justify_content(root, JustifyContent::SpaceEvenly)
        .expect("set root justify content");
    tree.set_width(root, Length::points(191.0))
        .expect("set root width");
    tree.set_height(root, Length::points(109.0))
        .expect("set root height");

    let absolute = tree.create_default_node();
    tree.set_display(absolute, Display::Block)
        .expect("set absolute display");
    tree.set_box_sizing(absolute, BoxSizing::ContentBox)
        .expect("set absolute box sizing");
    tree.set_position_type(absolute, PositionType::Absolute)
        .expect("set absolute position");
    tree.set_width(absolute, Length::points(37.0))
        .expect("set absolute width");
    tree.set_height(absolute, Length::points(17.0))
        .expect("set absolute height");
    tree.set_margin(absolute, StandaloneEdge::Left, Length::calc(4.0, 4.0))
        .expect("set absolute left margin");
    tree.set_margin(absolute, StandaloneEdge::Right, Length::calc(6.0, 2.0))
        .expect("set absolute right margin");
    tree.append_child(root, absolute)
        .expect("append absolute child");

    run_standalone_rust(tree, root, Constraints::definite(191.0, 109.0))
        .expect("horizontal linear absolute justify space-evenly static start parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_horizontal_linear_absolute_justify_stretch_static_start_without_linear_gravity()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::None)
        .expect("set root gravity");
    tree.set_justify_content(root, JustifyContent::Stretch)
        .expect("set root justify content");
    tree.set_width(root, Length::points(193.0))
        .expect("set root width");
    tree.set_height(root, Length::points(111.0))
        .expect("set root height");

    let absolute = tree.create_default_node();
    tree.set_display(absolute, Display::Block)
        .expect("set absolute display");
    tree.set_box_sizing(absolute, BoxSizing::ContentBox)
        .expect("set absolute box sizing");
    tree.set_position_type(absolute, PositionType::Absolute)
        .expect("set absolute position");
    tree.set_width(absolute, Length::points(39.0))
        .expect("set absolute width");
    tree.set_height(absolute, Length::points(18.0))
        .expect("set absolute height");
    tree.set_margin(absolute, StandaloneEdge::Left, Length::percent(6.0))
        .expect("set absolute left margin");
    tree.set_margin(absolute, StandaloneEdge::Right, Length::percent(4.0))
        .expect("set absolute right margin");
    tree.append_child(root, absolute)
        .expect("append absolute child");

    run_standalone_rust(tree, root, Constraints::definite(193.0, 111.0))
        .expect("horizontal linear absolute justify stretch static start parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_horizontal_linear_fixed_justify_center_static_position_without_linear_gravity()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::None)
        .expect("set root gravity");
    tree.set_justify_content(root, JustifyContent::Center)
        .expect("set root justify content");
    tree.set_width(root, Length::points(186.0))
        .expect("set root width");
    tree.set_height(root, Length::points(104.0))
        .expect("set root height");

    let nested = tree.create_default_node();
    tree.set_display(nested, Display::Block)
        .expect("set nested display");
    tree.set_box_sizing(nested, BoxSizing::ContentBox)
        .expect("set nested box sizing");
    tree.set_width(nested, Length::points(34.0))
        .expect("set nested width");
    tree.set_height(nested, Length::points(22.0))
        .expect("set nested height");

    let fixed = tree.create_default_node();
    tree.set_display(fixed, Display::Block)
        .expect("set fixed display");
    tree.set_box_sizing(fixed, BoxSizing::ContentBox)
        .expect("set fixed box sizing");
    tree.set_position_type(fixed, PositionType::Fixed)
        .expect("set fixed position");
    tree.set_width(fixed, Length::points(28.0))
        .expect("set fixed width");
    tree.set_height(fixed, Length::points(14.0))
        .expect("set fixed height");
    tree.set_margin(fixed, StandaloneEdge::Left, Length::calc(5.0, 3.0))
        .expect("set fixed left margin");
    tree.set_margin(fixed, StandaloneEdge::Right, Length::calc(2.0, 8.0))
        .expect("set fixed right margin");

    tree.append_child(root, nested).expect("append nested");
    tree.append_child(nested, fixed).expect("append fixed");

    run_standalone_rust(tree, root, Constraints::definite(186.0, 104.0))
        .expect("horizontal linear fixed justify center static position without gravity parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_horizontal_linear_fixed_justify_space_evenly_static_start_without_linear_gravity()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::None)
        .expect("set root gravity");
    tree.set_justify_content(root, JustifyContent::SpaceEvenly)
        .expect("set root justify content");
    tree.set_width(root, Length::points(188.0))
        .expect("set root width");
    tree.set_height(root, Length::points(106.0))
        .expect("set root height");

    let nested = tree.create_default_node();
    tree.set_display(nested, Display::Block)
        .expect("set nested display");
    tree.set_box_sizing(nested, BoxSizing::ContentBox)
        .expect("set nested box sizing");
    tree.set_width(nested, Length::points(32.0))
        .expect("set nested width");
    tree.set_height(nested, Length::points(20.0))
        .expect("set nested height");

    let fixed = tree.create_default_node();
    tree.set_display(fixed, Display::Block)
        .expect("set fixed display");
    tree.set_box_sizing(fixed, BoxSizing::ContentBox)
        .expect("set fixed box sizing");
    tree.set_position_type(fixed, PositionType::Fixed)
        .expect("set fixed position");
    tree.set_width(fixed, Length::points(26.0))
        .expect("set fixed width");
    tree.set_height(fixed, Length::points(16.0))
        .expect("set fixed height");
    tree.set_margin(fixed, StandaloneEdge::Left, Length::percent(5.0))
        .expect("set fixed left margin");
    tree.set_margin(fixed, StandaloneEdge::Right, Length::percent(4.0))
        .expect("set fixed right margin");

    tree.append_child(root, nested).expect("append nested");
    tree.append_child(nested, fixed).expect("append fixed");

    run_standalone_rust(tree, root, Constraints::definite(188.0, 106.0))
        .expect("horizontal linear fixed justify space-evenly static start parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_horizontal_linear_fixed_justify_stretch_static_start_without_linear_gravity()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::None)
        .expect("set root gravity");
    tree.set_justify_content(root, JustifyContent::Stretch)
        .expect("set root justify content");
    tree.set_width(root, Length::points(190.0))
        .expect("set root width");
    tree.set_height(root, Length::points(108.0))
        .expect("set root height");

    let nested = tree.create_default_node();
    tree.set_display(nested, Display::Block)
        .expect("set nested display");
    tree.set_box_sizing(nested, BoxSizing::ContentBox)
        .expect("set nested box sizing");
    tree.set_width(nested, Length::points(30.0))
        .expect("set nested width");
    tree.set_height(nested, Length::points(24.0))
        .expect("set nested height");

    let fixed = tree.create_default_node();
    tree.set_display(fixed, Display::Block)
        .expect("set fixed display");
    tree.set_box_sizing(fixed, BoxSizing::ContentBox)
        .expect("set fixed box sizing");
    tree.set_position_type(fixed, PositionType::Fixed)
        .expect("set fixed position");
    tree.set_width(fixed, Length::points(24.0))
        .expect("set fixed width");
    tree.set_height(fixed, Length::points(15.0))
        .expect("set fixed height");
    tree.set_margin(fixed, StandaloneEdge::Left, Length::calc(4.0, 5.0))
        .expect("set fixed left margin");
    tree.set_margin(fixed, StandaloneEdge::Right, Length::calc(7.0, 2.0))
        .expect("set fixed right margin");

    tree.append_child(root, nested).expect("append nested");
    tree.append_child(nested, fixed).expect("append fixed");

    run_standalone_rust(tree, root, Constraints::definite(190.0, 108.0))
        .expect("horizontal linear fixed justify stretch static start parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_horizontal_linear_fixed_justify_flex_start_static_position_without_linear_gravity()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::None)
        .expect("set root gravity");
    tree.set_justify_content(root, JustifyContent::FlexStart)
        .expect("set root justify content");
    tree.set_width(root, Length::points(192.0))
        .expect("set root width");
    tree.set_height(root, Length::points(110.0))
        .expect("set root height");

    let nested = tree.create_default_node();
    tree.set_display(nested, Display::Block)
        .expect("set nested display");
    tree.set_box_sizing(nested, BoxSizing::ContentBox)
        .expect("set nested box sizing");
    tree.set_width(nested, Length::points(35.0))
        .expect("set nested width");
    tree.set_height(nested, Length::points(21.0))
        .expect("set nested height");

    let fixed = tree.create_default_node();
    tree.set_display(fixed, Display::Block)
        .expect("set fixed display");
    tree.set_box_sizing(fixed, BoxSizing::ContentBox)
        .expect("set fixed box sizing");
    tree.set_position_type(fixed, PositionType::Fixed)
        .expect("set fixed position");
    tree.set_width(fixed, Length::points(25.0))
        .expect("set fixed width");
    tree.set_height(fixed, Length::points(14.0))
        .expect("set fixed height");
    tree.set_margin(fixed, StandaloneEdge::Left, Length::percent(4.0))
        .expect("set fixed left margin");
    tree.set_margin(fixed, StandaloneEdge::Right, Length::percent(6.0))
        .expect("set fixed right margin");

    tree.append_child(root, nested).expect("append nested");
    tree.append_child(nested, fixed).expect("append fixed");

    run_standalone_rust(tree, root, Constraints::definite(192.0, 110.0))
        .expect("horizontal linear fixed justify flex-start static position parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_horizontal_linear_fixed_justify_start_static_position_without_linear_gravity()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::None)
        .expect("set root gravity");
    tree.set_justify_content(root, JustifyContent::Start)
        .expect("set root justify content");
    tree.set_width(root, Length::points(194.0))
        .expect("set root width");
    tree.set_height(root, Length::points(112.0))
        .expect("set root height");

    let nested = tree.create_default_node();
    tree.set_display(nested, Display::Block)
        .expect("set nested display");
    tree.set_box_sizing(nested, BoxSizing::ContentBox)
        .expect("set nested box sizing");
    tree.set_width(nested, Length::points(37.0))
        .expect("set nested width");
    tree.set_height(nested, Length::points(22.0))
        .expect("set nested height");

    let fixed = tree.create_default_node();
    tree.set_display(fixed, Display::Block)
        .expect("set fixed display");
    tree.set_box_sizing(fixed, BoxSizing::ContentBox)
        .expect("set fixed box sizing");
    tree.set_position_type(fixed, PositionType::Fixed)
        .expect("set fixed position");
    tree.set_width(fixed, Length::points(27.0))
        .expect("set fixed width");
    tree.set_height(fixed, Length::points(15.0))
        .expect("set fixed height");
    tree.set_margin(fixed, StandaloneEdge::Left, Length::calc(2.0, 5.0))
        .expect("set fixed left margin");
    tree.set_margin(fixed, StandaloneEdge::Right, Length::calc(4.0, 3.0))
        .expect("set fixed right margin");

    tree.append_child(root, nested).expect("append nested");
    tree.append_child(nested, fixed).expect("append fixed");

    run_standalone_rust(tree, root, Constraints::definite(194.0, 112.0))
        .expect("horizontal linear fixed justify start static position parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_horizontal_linear_fixed_justify_flex_end_static_position_without_linear_gravity()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::None)
        .expect("set root gravity");
    tree.set_justify_content(root, JustifyContent::FlexEnd)
        .expect("set root justify content");
    tree.set_width(root, Length::points(196.0))
        .expect("set root width");
    tree.set_height(root, Length::points(114.0))
        .expect("set root height");

    let nested = tree.create_default_node();
    tree.set_display(nested, Display::Block)
        .expect("set nested display");
    tree.set_box_sizing(nested, BoxSizing::ContentBox)
        .expect("set nested box sizing");
    tree.set_width(nested, Length::points(39.0))
        .expect("set nested width");
    tree.set_height(nested, Length::points(23.0))
        .expect("set nested height");

    let fixed = tree.create_default_node();
    tree.set_display(fixed, Display::Block)
        .expect("set fixed display");
    tree.set_box_sizing(fixed, BoxSizing::ContentBox)
        .expect("set fixed box sizing");
    tree.set_position_type(fixed, PositionType::Fixed)
        .expect("set fixed position");
    tree.set_width(fixed, Length::points(29.0))
        .expect("set fixed width");
    tree.set_height(fixed, Length::points(16.0))
        .expect("set fixed height");
    tree.set_margin(fixed, StandaloneEdge::Left, Length::percent(6.0))
        .expect("set fixed left margin");
    tree.set_margin(fixed, StandaloneEdge::Right, Length::percent(4.0))
        .expect("set fixed right margin");

    tree.append_child(root, nested).expect("append nested");
    tree.append_child(nested, fixed).expect("append fixed");

    run_standalone_rust(tree, root, Constraints::definite(196.0, 114.0))
        .expect("horizontal linear fixed justify flex-end static position parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_horizontal_linear_fixed_justify_end_static_position_without_linear_gravity()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::None)
        .expect("set root gravity");
    tree.set_justify_content(root, JustifyContent::End)
        .expect("set root justify content");
    tree.set_width(root, Length::points(198.0))
        .expect("set root width");
    tree.set_height(root, Length::points(116.0))
        .expect("set root height");

    let nested = tree.create_default_node();
    tree.set_display(nested, Display::Block)
        .expect("set nested display");
    tree.set_box_sizing(nested, BoxSizing::ContentBox)
        .expect("set nested box sizing");
    tree.set_width(nested, Length::points(41.0))
        .expect("set nested width");
    tree.set_height(nested, Length::points(24.0))
        .expect("set nested height");

    let fixed = tree.create_default_node();
    tree.set_display(fixed, Display::Block)
        .expect("set fixed display");
    tree.set_box_sizing(fixed, BoxSizing::ContentBox)
        .expect("set fixed box sizing");
    tree.set_position_type(fixed, PositionType::Fixed)
        .expect("set fixed position");
    tree.set_width(fixed, Length::points(31.0))
        .expect("set fixed width");
    tree.set_height(fixed, Length::points(17.0))
        .expect("set fixed height");
    tree.set_margin(fixed, StandaloneEdge::Left, Length::calc(3.0, 4.0))
        .expect("set fixed left margin");
    tree.set_margin(fixed, StandaloneEdge::Right, Length::calc(5.0, 2.0))
        .expect("set fixed right margin");

    tree.append_child(root, nested).expect("append nested");
    tree.append_child(nested, fixed).expect("append fixed");

    run_standalone_rust(tree, root, Constraints::definite(198.0, 116.0))
        .expect("horizontal linear fixed justify end static position parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_horizontal_linear_fixed_justify_space_between_static_start_without_linear_gravity()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::None)
        .expect("set root gravity");
    tree.set_justify_content(root, JustifyContent::SpaceBetween)
        .expect("set root justify content");
    tree.set_width(root, Length::points(200.0))
        .expect("set root width");
    tree.set_height(root, Length::points(118.0))
        .expect("set root height");

    let nested = tree.create_default_node();
    tree.set_display(nested, Display::Block)
        .expect("set nested display");
    tree.set_box_sizing(nested, BoxSizing::ContentBox)
        .expect("set nested box sizing");
    tree.set_width(nested, Length::points(43.0))
        .expect("set nested width");
    tree.set_height(nested, Length::points(25.0))
        .expect("set nested height");

    let fixed = tree.create_default_node();
    tree.set_display(fixed, Display::Block)
        .expect("set fixed display");
    tree.set_box_sizing(fixed, BoxSizing::ContentBox)
        .expect("set fixed box sizing");
    tree.set_position_type(fixed, PositionType::Fixed)
        .expect("set fixed position");
    tree.set_width(fixed, Length::points(33.0))
        .expect("set fixed width");
    tree.set_height(fixed, Length::points(18.0))
        .expect("set fixed height");
    tree.set_margin(fixed, StandaloneEdge::Left, Length::percent(5.0))
        .expect("set fixed left margin");
    tree.set_margin(fixed, StandaloneEdge::Right, Length::percent(3.0))
        .expect("set fixed right margin");

    tree.append_child(root, nested).expect("append nested");
    tree.append_child(nested, fixed).expect("append fixed");

    run_standalone_rust(tree, root, Constraints::definite(200.0, 118.0))
        .expect("horizontal linear fixed justify space-between static start parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_horizontal_linear_fixed_justify_space_around_static_start_without_linear_gravity()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::None)
        .expect("set root gravity");
    tree.set_justify_content(root, JustifyContent::SpaceAround)
        .expect("set root justify content");
    tree.set_width(root, Length::points(202.0))
        .expect("set root width");
    tree.set_height(root, Length::points(120.0))
        .expect("set root height");

    let nested = tree.create_default_node();
    tree.set_display(nested, Display::Block)
        .expect("set nested display");
    tree.set_box_sizing(nested, BoxSizing::ContentBox)
        .expect("set nested box sizing");
    tree.set_width(nested, Length::points(45.0))
        .expect("set nested width");
    tree.set_height(nested, Length::points(26.0))
        .expect("set nested height");

    let fixed = tree.create_default_node();
    tree.set_display(fixed, Display::Block)
        .expect("set fixed display");
    tree.set_box_sizing(fixed, BoxSizing::ContentBox)
        .expect("set fixed box sizing");
    tree.set_position_type(fixed, PositionType::Fixed)
        .expect("set fixed position");
    tree.set_width(fixed, Length::points(35.0))
        .expect("set fixed width");
    tree.set_height(fixed, Length::points(19.0))
        .expect("set fixed height");
    tree.set_margin(fixed, StandaloneEdge::Left, Length::calc(4.0, 3.0))
        .expect("set fixed left margin");
    tree.set_margin(fixed, StandaloneEdge::Right, Length::calc(2.0, 6.0))
        .expect("set fixed right margin");

    tree.append_child(root, nested).expect("append nested");
    tree.append_child(nested, fixed).expect("append fixed");

    run_standalone_rust(tree, root, Constraints::definite(202.0, 120.0))
        .expect("horizontal linear fixed justify space-around static start parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_rtl_horizontal_linear_absolute_justify_center_static_position_without_linear_gravity()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_direction(root, Direction::Rtl)
        .expect("set root direction");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::None)
        .expect("set root gravity");
    tree.set_justify_content(root, JustifyContent::Center)
        .expect("set root justify content");
    tree.set_width(root, Length::points(192.0))
        .expect("set root width");
    tree.set_height(root, Length::points(110.0))
        .expect("set root height");

    let absolute = tree.create_default_node();
    tree.set_display(absolute, Display::Block)
        .expect("set absolute display");
    tree.set_box_sizing(absolute, BoxSizing::ContentBox)
        .expect("set absolute box sizing");
    tree.set_position_type(absolute, PositionType::Absolute)
        .expect("set absolute position");
    tree.set_width(absolute, Length::points(24.0))
        .expect("set absolute width");
    tree.set_height(absolute, Length::points(15.0))
        .expect("set absolute height");
    tree.set_margin(absolute, StandaloneEdge::Left, Length::calc(5.0, 4.0))
        .expect("set absolute left margin");
    tree.set_margin(absolute, StandaloneEdge::Right, Length::calc(2.0, 7.0))
        .expect("set absolute right margin");
    tree.append_child(root, absolute)
        .expect("append absolute child");

    run_standalone_rust(tree, root, Constraints::definite(192.0, 110.0))
        .expect("RTL horizontal linear absolute justify center static position parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_rtl_horizontal_linear_absolute_justify_flex_end_static_position_without_linear_gravity()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_direction(root, Direction::Rtl)
        .expect("set root direction");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::None)
        .expect("set root gravity");
    tree.set_justify_content(root, JustifyContent::FlexEnd)
        .expect("set root justify content");
    tree.set_width(root, Length::points(194.0))
        .expect("set root width");
    tree.set_height(root, Length::points(112.0))
        .expect("set root height");

    let absolute = tree.create_default_node();
    tree.set_display(absolute, Display::Block)
        .expect("set absolute display");
    tree.set_box_sizing(absolute, BoxSizing::ContentBox)
        .expect("set absolute box sizing");
    tree.set_position_type(absolute, PositionType::Absolute)
        .expect("set absolute position");
    tree.set_width(absolute, Length::points(26.0))
        .expect("set absolute width");
    tree.set_height(absolute, Length::points(14.0))
        .expect("set absolute height");
    tree.set_margin(absolute, StandaloneEdge::Left, Length::percent(6.0))
        .expect("set absolute left margin");
    tree.set_margin(absolute, StandaloneEdge::Right, Length::percent(4.0))
        .expect("set absolute right margin");
    tree.append_child(root, absolute)
        .expect("append absolute child");

    run_standalone_rust(tree, root, Constraints::definite(194.0, 112.0))
        .expect("RTL horizontal linear absolute justify flex-end static position parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_rtl_horizontal_linear_absolute_justify_space_between_static_start_without_linear_gravity()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_direction(root, Direction::Rtl)
        .expect("set root direction");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::None)
        .expect("set root gravity");
    tree.set_justify_content(root, JustifyContent::SpaceBetween)
        .expect("set root justify content");
    tree.set_width(root, Length::points(196.0))
        .expect("set root width");
    tree.set_height(root, Length::points(114.0))
        .expect("set root height");

    let absolute = tree.create_default_node();
    tree.set_display(absolute, Display::Block)
        .expect("set absolute display");
    tree.set_box_sizing(absolute, BoxSizing::ContentBox)
        .expect("set absolute box sizing");
    tree.set_position_type(absolute, PositionType::Absolute)
        .expect("set absolute position");
    tree.set_width(absolute, Length::points(28.0))
        .expect("set absolute width");
    tree.set_height(absolute, Length::points(16.0))
        .expect("set absolute height");
    tree.set_margin(absolute, StandaloneEdge::Left, Length::calc(4.0, 6.0))
        .expect("set absolute left margin");
    tree.set_margin(absolute, StandaloneEdge::Right, Length::calc(3.0, 5.0))
        .expect("set absolute right margin");
    tree.append_child(root, absolute)
        .expect("append absolute child");

    run_standalone_rust(tree, root, Constraints::definite(196.0, 114.0))
        .expect("RTL horizontal linear absolute justify space-between static start parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_rtl_horizontal_linear_absolute_justify_space_around_static_start_without_linear_gravity()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_direction(root, Direction::Rtl)
        .expect("set root direction");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::None)
        .expect("set root gravity");
    tree.set_justify_content(root, JustifyContent::SpaceAround)
        .expect("set root justify content");
    tree.set_width(root, Length::points(198.0))
        .expect("set root width");
    tree.set_height(root, Length::points(116.0))
        .expect("set root height");

    let absolute = tree.create_default_node();
    tree.set_display(absolute, Display::Block)
        .expect("set absolute display");
    tree.set_box_sizing(absolute, BoxSizing::ContentBox)
        .expect("set absolute box sizing");
    tree.set_position_type(absolute, PositionType::Absolute)
        .expect("set absolute position");
    tree.set_width(absolute, Length::points(30.0))
        .expect("set absolute width");
    tree.set_height(absolute, Length::points(13.0))
        .expect("set absolute height");
    tree.set_margin(absolute, StandaloneEdge::Left, Length::percent(3.0))
        .expect("set absolute left margin");
    tree.set_margin(absolute, StandaloneEdge::Right, Length::percent(7.0))
        .expect("set absolute right margin");
    tree.append_child(root, absolute)
        .expect("append absolute child");

    run_standalone_rust(tree, root, Constraints::definite(198.0, 116.0))
        .expect("RTL horizontal linear absolute justify space-around static start parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_rtl_horizontal_linear_absolute_justify_flex_start_static_position_without_linear_gravity()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_direction(root, Direction::Rtl)
        .expect("set root direction");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::None)
        .expect("set root gravity");
    tree.set_justify_content(root, JustifyContent::FlexStart)
        .expect("set root justify content");
    tree.set_width(root, Length::points(199.0))
        .expect("set root width");
    tree.set_height(root, Length::points(117.0))
        .expect("set root height");

    let absolute = tree.create_default_node();
    tree.set_display(absolute, Display::Block)
        .expect("set absolute display");
    tree.set_box_sizing(absolute, BoxSizing::ContentBox)
        .expect("set absolute box sizing");
    tree.set_position_type(absolute, PositionType::Absolute)
        .expect("set absolute position");
    tree.set_width(absolute, Length::points(31.0))
        .expect("set absolute width");
    tree.set_height(absolute, Length::points(14.0))
        .expect("set absolute height");
    tree.set_margin(absolute, StandaloneEdge::Left, Length::percent(4.0))
        .expect("set absolute left margin");
    tree.set_margin(absolute, StandaloneEdge::Right, Length::percent(6.0))
        .expect("set absolute right margin");
    tree.append_child(root, absolute)
        .expect("append absolute child");

    run_standalone_rust(tree, root, Constraints::definite(199.0, 117.0))
        .expect("RTL horizontal linear absolute justify flex-start static position parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_rtl_horizontal_linear_absolute_justify_start_static_position_without_linear_gravity()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_direction(root, Direction::Rtl)
        .expect("set root direction");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::None)
        .expect("set root gravity");
    tree.set_justify_content(root, JustifyContent::Start)
        .expect("set root justify content");
    tree.set_width(root, Length::points(201.0))
        .expect("set root width");
    tree.set_height(root, Length::points(119.0))
        .expect("set root height");

    let absolute = tree.create_default_node();
    tree.set_display(absolute, Display::Block)
        .expect("set absolute display");
    tree.set_box_sizing(absolute, BoxSizing::ContentBox)
        .expect("set absolute box sizing");
    tree.set_position_type(absolute, PositionType::Absolute)
        .expect("set absolute position");
    tree.set_width(absolute, Length::points(33.0))
        .expect("set absolute width");
    tree.set_height(absolute, Length::points(15.0))
        .expect("set absolute height");
    tree.set_margin(absolute, StandaloneEdge::Left, Length::calc(3.0, 5.0))
        .expect("set absolute left margin");
    tree.set_margin(absolute, StandaloneEdge::Right, Length::calc(5.0, 3.0))
        .expect("set absolute right margin");
    tree.append_child(root, absolute)
        .expect("append absolute child");

    run_standalone_rust(tree, root, Constraints::definite(201.0, 119.0))
        .expect("RTL horizontal linear absolute justify start static position parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_rtl_horizontal_linear_absolute_justify_end_static_position_without_linear_gravity()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_direction(root, Direction::Rtl)
        .expect("set root direction");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::None)
        .expect("set root gravity");
    tree.set_justify_content(root, JustifyContent::End)
        .expect("set root justify content");
    tree.set_width(root, Length::points(203.0))
        .expect("set root width");
    tree.set_height(root, Length::points(121.0))
        .expect("set root height");

    let absolute = tree.create_default_node();
    tree.set_display(absolute, Display::Block)
        .expect("set absolute display");
    tree.set_box_sizing(absolute, BoxSizing::ContentBox)
        .expect("set absolute box sizing");
    tree.set_position_type(absolute, PositionType::Absolute)
        .expect("set absolute position");
    tree.set_width(absolute, Length::points(35.0))
        .expect("set absolute width");
    tree.set_height(absolute, Length::points(16.0))
        .expect("set absolute height");
    tree.set_margin(absolute, StandaloneEdge::Left, Length::percent(5.0))
        .expect("set absolute left margin");
    tree.set_margin(absolute, StandaloneEdge::Right, Length::percent(3.0))
        .expect("set absolute right margin");
    tree.append_child(root, absolute)
        .expect("append absolute child");

    run_standalone_rust(tree, root, Constraints::definite(203.0, 121.0))
        .expect("RTL horizontal linear absolute justify end static position parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_rtl_horizontal_linear_absolute_justify_space_evenly_static_start_without_linear_gravity()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_direction(root, Direction::Rtl)
        .expect("set root direction");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::None)
        .expect("set root gravity");
    tree.set_justify_content(root, JustifyContent::SpaceEvenly)
        .expect("set root justify content");
    tree.set_width(root, Length::points(205.0))
        .expect("set root width");
    tree.set_height(root, Length::points(123.0))
        .expect("set root height");

    let absolute = tree.create_default_node();
    tree.set_display(absolute, Display::Block)
        .expect("set absolute display");
    tree.set_box_sizing(absolute, BoxSizing::ContentBox)
        .expect("set absolute box sizing");
    tree.set_position_type(absolute, PositionType::Absolute)
        .expect("set absolute position");
    tree.set_width(absolute, Length::points(37.0))
        .expect("set absolute width");
    tree.set_height(absolute, Length::points(17.0))
        .expect("set absolute height");
    tree.set_margin(absolute, StandaloneEdge::Left, Length::calc(4.0, 4.0))
        .expect("set absolute left margin");
    tree.set_margin(absolute, StandaloneEdge::Right, Length::calc(6.0, 2.0))
        .expect("set absolute right margin");
    tree.append_child(root, absolute)
        .expect("append absolute child");

    run_standalone_rust(tree, root, Constraints::definite(205.0, 123.0))
        .expect("RTL horizontal linear absolute justify space-evenly static start parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_rtl_horizontal_linear_absolute_justify_stretch_static_start_without_linear_gravity()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_direction(root, Direction::Rtl)
        .expect("set root direction");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::None)
        .expect("set root gravity");
    tree.set_justify_content(root, JustifyContent::Stretch)
        .expect("set root justify content");
    tree.set_width(root, Length::points(207.0))
        .expect("set root width");
    tree.set_height(root, Length::points(125.0))
        .expect("set root height");

    let absolute = tree.create_default_node();
    tree.set_display(absolute, Display::Block)
        .expect("set absolute display");
    tree.set_box_sizing(absolute, BoxSizing::ContentBox)
        .expect("set absolute box sizing");
    tree.set_position_type(absolute, PositionType::Absolute)
        .expect("set absolute position");
    tree.set_width(absolute, Length::points(39.0))
        .expect("set absolute width");
    tree.set_height(absolute, Length::points(18.0))
        .expect("set absolute height");
    tree.set_margin(absolute, StandaloneEdge::Left, Length::percent(6.0))
        .expect("set absolute left margin");
    tree.set_margin(absolute, StandaloneEdge::Right, Length::percent(4.0))
        .expect("set absolute right margin");
    tree.append_child(root, absolute)
        .expect("append absolute child");

    run_standalone_rust(tree, root, Constraints::definite(207.0, 125.0))
        .expect("RTL horizontal linear absolute justify stretch static start parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_rtl_horizontal_linear_fixed_justify_center_static_position_without_linear_gravity()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_direction(root, Direction::Rtl)
        .expect("set root direction");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::None)
        .expect("set root gravity");
    tree.set_justify_content(root, JustifyContent::Center)
        .expect("set root justify content");
    tree.set_width(root, Length::points(200.0))
        .expect("set root width");
    tree.set_height(root, Length::points(118.0))
        .expect("set root height");

    let nested = tree.create_default_node();
    tree.set_display(nested, Display::Block)
        .expect("set nested display");
    tree.set_box_sizing(nested, BoxSizing::ContentBox)
        .expect("set nested box sizing");
    tree.set_width(nested, Length::points(34.0))
        .expect("set nested width");
    tree.set_height(nested, Length::points(22.0))
        .expect("set nested height");

    let fixed = tree.create_default_node();
    tree.set_display(fixed, Display::Block)
        .expect("set fixed display");
    tree.set_box_sizing(fixed, BoxSizing::ContentBox)
        .expect("set fixed box sizing");
    tree.set_position_type(fixed, PositionType::Fixed)
        .expect("set fixed position");
    tree.set_width(fixed, Length::points(28.0))
        .expect("set fixed width");
    tree.set_height(fixed, Length::points(14.0))
        .expect("set fixed height");
    tree.set_margin(fixed, StandaloneEdge::Left, Length::calc(5.0, 3.0))
        .expect("set fixed left margin");
    tree.set_margin(fixed, StandaloneEdge::Right, Length::calc(2.0, 8.0))
        .expect("set fixed right margin");

    tree.append_child(root, nested).expect("append nested");
    tree.append_child(nested, fixed).expect("append fixed");

    run_standalone_rust(tree, root, Constraints::definite(200.0, 118.0))
        .expect("RTL horizontal linear fixed justify center static position parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_rtl_horizontal_linear_fixed_justify_space_evenly_static_start_without_linear_gravity()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_direction(root, Direction::Rtl)
        .expect("set root direction");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::None)
        .expect("set root gravity");
    tree.set_justify_content(root, JustifyContent::SpaceEvenly)
        .expect("set root justify content");
    tree.set_width(root, Length::points(202.0))
        .expect("set root width");
    tree.set_height(root, Length::points(120.0))
        .expect("set root height");

    let nested = tree.create_default_node();
    tree.set_display(nested, Display::Block)
        .expect("set nested display");
    tree.set_box_sizing(nested, BoxSizing::ContentBox)
        .expect("set nested box sizing");
    tree.set_width(nested, Length::points(32.0))
        .expect("set nested width");
    tree.set_height(nested, Length::points(20.0))
        .expect("set nested height");

    let fixed = tree.create_default_node();
    tree.set_display(fixed, Display::Block)
        .expect("set fixed display");
    tree.set_box_sizing(fixed, BoxSizing::ContentBox)
        .expect("set fixed box sizing");
    tree.set_position_type(fixed, PositionType::Fixed)
        .expect("set fixed position");
    tree.set_width(fixed, Length::points(26.0))
        .expect("set fixed width");
    tree.set_height(fixed, Length::points(16.0))
        .expect("set fixed height");
    tree.set_margin(fixed, StandaloneEdge::Left, Length::percent(5.0))
        .expect("set fixed left margin");
    tree.set_margin(fixed, StandaloneEdge::Right, Length::percent(4.0))
        .expect("set fixed right margin");

    tree.append_child(root, nested).expect("append nested");
    tree.append_child(nested, fixed).expect("append fixed");

    run_standalone_rust(tree, root, Constraints::definite(202.0, 120.0))
        .expect("RTL horizontal linear fixed justify space-evenly static start parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_rtl_horizontal_linear_fixed_justify_stretch_static_start_without_linear_gravity()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_direction(root, Direction::Rtl)
        .expect("set root direction");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::None)
        .expect("set root gravity");
    tree.set_justify_content(root, JustifyContent::Stretch)
        .expect("set root justify content");
    tree.set_width(root, Length::points(204.0))
        .expect("set root width");
    tree.set_height(root, Length::points(122.0))
        .expect("set root height");

    let nested = tree.create_default_node();
    tree.set_display(nested, Display::Block)
        .expect("set nested display");
    tree.set_box_sizing(nested, BoxSizing::ContentBox)
        .expect("set nested box sizing");
    tree.set_width(nested, Length::points(30.0))
        .expect("set nested width");
    tree.set_height(nested, Length::points(24.0))
        .expect("set nested height");

    let fixed = tree.create_default_node();
    tree.set_display(fixed, Display::Block)
        .expect("set fixed display");
    tree.set_box_sizing(fixed, BoxSizing::ContentBox)
        .expect("set fixed box sizing");
    tree.set_position_type(fixed, PositionType::Fixed)
        .expect("set fixed position");
    tree.set_width(fixed, Length::points(24.0))
        .expect("set fixed width");
    tree.set_height(fixed, Length::points(15.0))
        .expect("set fixed height");
    tree.set_margin(fixed, StandaloneEdge::Left, Length::calc(4.0, 5.0))
        .expect("set fixed left margin");
    tree.set_margin(fixed, StandaloneEdge::Right, Length::calc(7.0, 2.0))
        .expect("set fixed right margin");

    tree.append_child(root, nested).expect("append nested");
    tree.append_child(nested, fixed).expect("append fixed");

    run_standalone_rust(tree, root, Constraints::definite(204.0, 122.0))
        .expect("RTL horizontal linear fixed justify stretch static start parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_rtl_horizontal_linear_fixed_justify_flex_start_static_position_without_linear_gravity()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_direction(root, Direction::Rtl)
        .expect("set root direction");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::None)
        .expect("set root gravity");
    tree.set_justify_content(root, JustifyContent::FlexStart)
        .expect("set root justify content");
    tree.set_width(root, Length::points(206.0))
        .expect("set root width");
    tree.set_height(root, Length::points(124.0))
        .expect("set root height");

    let nested = tree.create_default_node();
    tree.set_display(nested, Display::Block)
        .expect("set nested display");
    tree.set_box_sizing(nested, BoxSizing::ContentBox)
        .expect("set nested box sizing");
    tree.set_width(nested, Length::points(35.0))
        .expect("set nested width");
    tree.set_height(nested, Length::points(21.0))
        .expect("set nested height");

    let fixed = tree.create_default_node();
    tree.set_display(fixed, Display::Block)
        .expect("set fixed display");
    tree.set_box_sizing(fixed, BoxSizing::ContentBox)
        .expect("set fixed box sizing");
    tree.set_position_type(fixed, PositionType::Fixed)
        .expect("set fixed position");
    tree.set_width(fixed, Length::points(25.0))
        .expect("set fixed width");
    tree.set_height(fixed, Length::points(14.0))
        .expect("set fixed height");
    tree.set_margin(fixed, StandaloneEdge::Left, Length::percent(4.0))
        .expect("set fixed left margin");
    tree.set_margin(fixed, StandaloneEdge::Right, Length::percent(6.0))
        .expect("set fixed right margin");

    tree.append_child(root, nested).expect("append nested");
    tree.append_child(nested, fixed).expect("append fixed");

    run_standalone_rust(tree, root, Constraints::definite(206.0, 124.0))
        .expect("RTL horizontal linear fixed justify flex-start static position parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_rtl_horizontal_linear_fixed_justify_start_static_position_without_linear_gravity()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_direction(root, Direction::Rtl)
        .expect("set root direction");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::None)
        .expect("set root gravity");
    tree.set_justify_content(root, JustifyContent::Start)
        .expect("set root justify content");
    tree.set_width(root, Length::points(208.0))
        .expect("set root width");
    tree.set_height(root, Length::points(126.0))
        .expect("set root height");

    let nested = tree.create_default_node();
    tree.set_display(nested, Display::Block)
        .expect("set nested display");
    tree.set_box_sizing(nested, BoxSizing::ContentBox)
        .expect("set nested box sizing");
    tree.set_width(nested, Length::points(37.0))
        .expect("set nested width");
    tree.set_height(nested, Length::points(22.0))
        .expect("set nested height");

    let fixed = tree.create_default_node();
    tree.set_display(fixed, Display::Block)
        .expect("set fixed display");
    tree.set_box_sizing(fixed, BoxSizing::ContentBox)
        .expect("set fixed box sizing");
    tree.set_position_type(fixed, PositionType::Fixed)
        .expect("set fixed position");
    tree.set_width(fixed, Length::points(27.0))
        .expect("set fixed width");
    tree.set_height(fixed, Length::points(15.0))
        .expect("set fixed height");
    tree.set_margin(fixed, StandaloneEdge::Left, Length::calc(2.0, 5.0))
        .expect("set fixed left margin");
    tree.set_margin(fixed, StandaloneEdge::Right, Length::calc(4.0, 3.0))
        .expect("set fixed right margin");

    tree.append_child(root, nested).expect("append nested");
    tree.append_child(nested, fixed).expect("append fixed");

    run_standalone_rust(tree, root, Constraints::definite(208.0, 126.0))
        .expect("RTL horizontal linear fixed justify start static position parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_rtl_horizontal_linear_fixed_justify_flex_end_static_position_without_linear_gravity()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_direction(root, Direction::Rtl)
        .expect("set root direction");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::None)
        .expect("set root gravity");
    tree.set_justify_content(root, JustifyContent::FlexEnd)
        .expect("set root justify content");
    tree.set_width(root, Length::points(210.0))
        .expect("set root width");
    tree.set_height(root, Length::points(128.0))
        .expect("set root height");

    let nested = tree.create_default_node();
    tree.set_display(nested, Display::Block)
        .expect("set nested display");
    tree.set_box_sizing(nested, BoxSizing::ContentBox)
        .expect("set nested box sizing");
    tree.set_width(nested, Length::points(39.0))
        .expect("set nested width");
    tree.set_height(nested, Length::points(23.0))
        .expect("set nested height");

    let fixed = tree.create_default_node();
    tree.set_display(fixed, Display::Block)
        .expect("set fixed display");
    tree.set_box_sizing(fixed, BoxSizing::ContentBox)
        .expect("set fixed box sizing");
    tree.set_position_type(fixed, PositionType::Fixed)
        .expect("set fixed position");
    tree.set_width(fixed, Length::points(29.0))
        .expect("set fixed width");
    tree.set_height(fixed, Length::points(16.0))
        .expect("set fixed height");
    tree.set_margin(fixed, StandaloneEdge::Left, Length::percent(6.0))
        .expect("set fixed left margin");
    tree.set_margin(fixed, StandaloneEdge::Right, Length::percent(4.0))
        .expect("set fixed right margin");

    tree.append_child(root, nested).expect("append nested");
    tree.append_child(nested, fixed).expect("append fixed");

    run_standalone_rust(tree, root, Constraints::definite(210.0, 128.0))
        .expect("RTL horizontal linear fixed justify flex-end static position parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_rtl_horizontal_linear_fixed_justify_end_static_position_without_linear_gravity()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_direction(root, Direction::Rtl)
        .expect("set root direction");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::None)
        .expect("set root gravity");
    tree.set_justify_content(root, JustifyContent::End)
        .expect("set root justify content");
    tree.set_width(root, Length::points(212.0))
        .expect("set root width");
    tree.set_height(root, Length::points(130.0))
        .expect("set root height");

    let nested = tree.create_default_node();
    tree.set_display(nested, Display::Block)
        .expect("set nested display");
    tree.set_box_sizing(nested, BoxSizing::ContentBox)
        .expect("set nested box sizing");
    tree.set_width(nested, Length::points(41.0))
        .expect("set nested width");
    tree.set_height(nested, Length::points(24.0))
        .expect("set nested height");

    let fixed = tree.create_default_node();
    tree.set_display(fixed, Display::Block)
        .expect("set fixed display");
    tree.set_box_sizing(fixed, BoxSizing::ContentBox)
        .expect("set fixed box sizing");
    tree.set_position_type(fixed, PositionType::Fixed)
        .expect("set fixed position");
    tree.set_width(fixed, Length::points(31.0))
        .expect("set fixed width");
    tree.set_height(fixed, Length::points(17.0))
        .expect("set fixed height");
    tree.set_margin(fixed, StandaloneEdge::Left, Length::calc(3.0, 4.0))
        .expect("set fixed left margin");
    tree.set_margin(fixed, StandaloneEdge::Right, Length::calc(5.0, 2.0))
        .expect("set fixed right margin");

    tree.append_child(root, nested).expect("append nested");
    tree.append_child(nested, fixed).expect("append fixed");

    run_standalone_rust(tree, root, Constraints::definite(212.0, 130.0))
        .expect("RTL horizontal linear fixed justify end static position parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_rtl_horizontal_linear_fixed_justify_space_between_static_start_without_linear_gravity()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_direction(root, Direction::Rtl)
        .expect("set root direction");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::None)
        .expect("set root gravity");
    tree.set_justify_content(root, JustifyContent::SpaceBetween)
        .expect("set root justify content");
    tree.set_width(root, Length::points(214.0))
        .expect("set root width");
    tree.set_height(root, Length::points(132.0))
        .expect("set root height");

    let nested = tree.create_default_node();
    tree.set_display(nested, Display::Block)
        .expect("set nested display");
    tree.set_box_sizing(nested, BoxSizing::ContentBox)
        .expect("set nested box sizing");
    tree.set_width(nested, Length::points(43.0))
        .expect("set nested width");
    tree.set_height(nested, Length::points(25.0))
        .expect("set nested height");

    let fixed = tree.create_default_node();
    tree.set_display(fixed, Display::Block)
        .expect("set fixed display");
    tree.set_box_sizing(fixed, BoxSizing::ContentBox)
        .expect("set fixed box sizing");
    tree.set_position_type(fixed, PositionType::Fixed)
        .expect("set fixed position");
    tree.set_width(fixed, Length::points(33.0))
        .expect("set fixed width");
    tree.set_height(fixed, Length::points(18.0))
        .expect("set fixed height");
    tree.set_margin(fixed, StandaloneEdge::Left, Length::percent(5.0))
        .expect("set fixed left margin");
    tree.set_margin(fixed, StandaloneEdge::Right, Length::percent(3.0))
        .expect("set fixed right margin");

    tree.append_child(root, nested).expect("append nested");
    tree.append_child(nested, fixed).expect("append fixed");

    run_standalone_rust(tree, root, Constraints::definite(214.0, 132.0))
        .expect("RTL horizontal linear fixed justify space-between static start parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_rtl_horizontal_linear_fixed_justify_space_around_static_start_without_linear_gravity()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_direction(root, Direction::Rtl)
        .expect("set root direction");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::None)
        .expect("set root gravity");
    tree.set_justify_content(root, JustifyContent::SpaceAround)
        .expect("set root justify content");
    tree.set_width(root, Length::points(216.0))
        .expect("set root width");
    tree.set_height(root, Length::points(134.0))
        .expect("set root height");

    let nested = tree.create_default_node();
    tree.set_display(nested, Display::Block)
        .expect("set nested display");
    tree.set_box_sizing(nested, BoxSizing::ContentBox)
        .expect("set nested box sizing");
    tree.set_width(nested, Length::points(45.0))
        .expect("set nested width");
    tree.set_height(nested, Length::points(26.0))
        .expect("set nested height");

    let fixed = tree.create_default_node();
    tree.set_display(fixed, Display::Block)
        .expect("set fixed display");
    tree.set_box_sizing(fixed, BoxSizing::ContentBox)
        .expect("set fixed box sizing");
    tree.set_position_type(fixed, PositionType::Fixed)
        .expect("set fixed position");
    tree.set_width(fixed, Length::points(35.0))
        .expect("set fixed width");
    tree.set_height(fixed, Length::points(19.0))
        .expect("set fixed height");
    tree.set_margin(fixed, StandaloneEdge::Left, Length::calc(4.0, 3.0))
        .expect("set fixed left margin");
    tree.set_margin(fixed, StandaloneEdge::Right, Length::calc(2.0, 6.0))
        .expect("set fixed right margin");

    tree.append_child(root, nested).expect("append nested");
    tree.append_child(nested, fixed).expect("append fixed");

    run_standalone_rust(tree, root, Constraints::definite(216.0, 134.0))
        .expect("RTL horizontal linear fixed justify space-around static start parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_horizontal_reverse_linear_absolute_justify_center_static_position_without_linear_gravity()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::HorizontalReverse)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::None)
        .expect("set root gravity");
    tree.set_justify_content(root, JustifyContent::Center)
        .expect("set root justify content");
    tree.set_width(root, Length::points(206.0))
        .expect("set root width");
    tree.set_height(root, Length::points(124.0))
        .expect("set root height");

    let absolute = tree.create_default_node();
    tree.set_display(absolute, Display::Block)
        .expect("set absolute display");
    tree.set_box_sizing(absolute, BoxSizing::ContentBox)
        .expect("set absolute box sizing");
    tree.set_position_type(absolute, PositionType::Absolute)
        .expect("set absolute position");
    tree.set_width(absolute, Length::points(24.0))
        .expect("set absolute width");
    tree.set_height(absolute, Length::points(15.0))
        .expect("set absolute height");
    tree.set_margin(absolute, StandaloneEdge::Left, Length::calc(5.0, 4.0))
        .expect("set absolute left margin");
    tree.set_margin(absolute, StandaloneEdge::Right, Length::calc(2.0, 7.0))
        .expect("set absolute right margin");
    tree.append_child(root, absolute)
        .expect("append absolute child");

    run_standalone_rust(tree, root, Constraints::definite(206.0, 124.0))
        .expect("horizontal-reverse linear absolute justify center static position parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_horizontal_reverse_linear_absolute_justify_flex_end_static_position_without_linear_gravity()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::HorizontalReverse)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::None)
        .expect("set root gravity");
    tree.set_justify_content(root, JustifyContent::FlexEnd)
        .expect("set root justify content");
    tree.set_width(root, Length::points(208.0))
        .expect("set root width");
    tree.set_height(root, Length::points(126.0))
        .expect("set root height");

    let absolute = tree.create_default_node();
    tree.set_display(absolute, Display::Block)
        .expect("set absolute display");
    tree.set_box_sizing(absolute, BoxSizing::ContentBox)
        .expect("set absolute box sizing");
    tree.set_position_type(absolute, PositionType::Absolute)
        .expect("set absolute position");
    tree.set_width(absolute, Length::points(26.0))
        .expect("set absolute width");
    tree.set_height(absolute, Length::points(14.0))
        .expect("set absolute height");
    tree.set_margin(absolute, StandaloneEdge::Left, Length::percent(6.0))
        .expect("set absolute left margin");
    tree.set_margin(absolute, StandaloneEdge::Right, Length::percent(4.0))
        .expect("set absolute right margin");
    tree.append_child(root, absolute)
        .expect("append absolute child");

    run_standalone_rust(tree, root, Constraints::definite(208.0, 126.0))
        .expect("horizontal-reverse linear absolute justify flex-end static position parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_horizontal_reverse_linear_absolute_justify_space_between_static_start_without_linear_gravity()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::HorizontalReverse)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::None)
        .expect("set root gravity");
    tree.set_justify_content(root, JustifyContent::SpaceBetween)
        .expect("set root justify content");
    tree.set_width(root, Length::points(210.0))
        .expect("set root width");
    tree.set_height(root, Length::points(128.0))
        .expect("set root height");

    let absolute = tree.create_default_node();
    tree.set_display(absolute, Display::Block)
        .expect("set absolute display");
    tree.set_box_sizing(absolute, BoxSizing::ContentBox)
        .expect("set absolute box sizing");
    tree.set_position_type(absolute, PositionType::Absolute)
        .expect("set absolute position");
    tree.set_width(absolute, Length::points(28.0))
        .expect("set absolute width");
    tree.set_height(absolute, Length::points(16.0))
        .expect("set absolute height");
    tree.set_margin(absolute, StandaloneEdge::Left, Length::calc(4.0, 6.0))
        .expect("set absolute left margin");
    tree.set_margin(absolute, StandaloneEdge::Right, Length::calc(3.0, 5.0))
        .expect("set absolute right margin");
    tree.append_child(root, absolute)
        .expect("append absolute child");

    run_standalone_rust(tree, root, Constraints::definite(210.0, 128.0))
        .expect("horizontal-reverse linear absolute justify space-between static start parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_horizontal_reverse_linear_absolute_justify_space_around_static_start_without_linear_gravity()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::HorizontalReverse)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::None)
        .expect("set root gravity");
    tree.set_justify_content(root, JustifyContent::SpaceAround)
        .expect("set root justify content");
    tree.set_width(root, Length::points(212.0))
        .expect("set root width");
    tree.set_height(root, Length::points(130.0))
        .expect("set root height");

    let absolute = tree.create_default_node();
    tree.set_display(absolute, Display::Block)
        .expect("set absolute display");
    tree.set_box_sizing(absolute, BoxSizing::ContentBox)
        .expect("set absolute box sizing");
    tree.set_position_type(absolute, PositionType::Absolute)
        .expect("set absolute position");
    tree.set_width(absolute, Length::points(30.0))
        .expect("set absolute width");
    tree.set_height(absolute, Length::points(13.0))
        .expect("set absolute height");
    tree.set_margin(absolute, StandaloneEdge::Left, Length::percent(3.0))
        .expect("set absolute left margin");
    tree.set_margin(absolute, StandaloneEdge::Right, Length::percent(7.0))
        .expect("set absolute right margin");
    tree.append_child(root, absolute)
        .expect("append absolute child");

    run_standalone_rust(tree, root, Constraints::definite(212.0, 130.0))
        .expect("horizontal-reverse linear absolute justify space-around static start parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_horizontal_reverse_linear_absolute_justify_flex_start_static_position_without_linear_gravity()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::HorizontalReverse)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::None)
        .expect("set root gravity");
    tree.set_justify_content(root, JustifyContent::FlexStart)
        .expect("set root justify content");
    tree.set_width(root, Length::points(214.0))
        .expect("set root width");
    tree.set_height(root, Length::points(132.0))
        .expect("set root height");

    let absolute = tree.create_default_node();
    tree.set_display(absolute, Display::Block)
        .expect("set absolute display");
    tree.set_box_sizing(absolute, BoxSizing::ContentBox)
        .expect("set absolute box sizing");
    tree.set_position_type(absolute, PositionType::Absolute)
        .expect("set absolute position");
    tree.set_width(absolute, Length::points(32.0))
        .expect("set absolute width");
    tree.set_height(absolute, Length::points(14.0))
        .expect("set absolute height");
    tree.set_margin(absolute, StandaloneEdge::Left, Length::percent(4.0))
        .expect("set absolute left margin");
    tree.set_margin(absolute, StandaloneEdge::Right, Length::percent(6.0))
        .expect("set absolute right margin");
    tree.append_child(root, absolute)
        .expect("append absolute child");

    run_standalone_rust(tree, root, Constraints::definite(214.0, 132.0))
        .expect("horizontal-reverse linear absolute justify flex-start static position parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_horizontal_reverse_linear_absolute_justify_start_static_position_without_linear_gravity()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::HorizontalReverse)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::None)
        .expect("set root gravity");
    tree.set_justify_content(root, JustifyContent::Start)
        .expect("set root justify content");
    tree.set_width(root, Length::points(216.0))
        .expect("set root width");
    tree.set_height(root, Length::points(134.0))
        .expect("set root height");

    let absolute = tree.create_default_node();
    tree.set_display(absolute, Display::Block)
        .expect("set absolute display");
    tree.set_box_sizing(absolute, BoxSizing::ContentBox)
        .expect("set absolute box sizing");
    tree.set_position_type(absolute, PositionType::Absolute)
        .expect("set absolute position");
    tree.set_width(absolute, Length::points(34.0))
        .expect("set absolute width");
    tree.set_height(absolute, Length::points(15.0))
        .expect("set absolute height");
    tree.set_margin(absolute, StandaloneEdge::Left, Length::calc(2.0, 5.0))
        .expect("set absolute left margin");
    tree.set_margin(absolute, StandaloneEdge::Right, Length::calc(4.0, 3.0))
        .expect("set absolute right margin");
    tree.append_child(root, absolute)
        .expect("append absolute child");

    run_standalone_rust(tree, root, Constraints::definite(216.0, 134.0))
        .expect("horizontal-reverse linear absolute justify start static position parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_horizontal_reverse_linear_absolute_justify_end_static_position_without_linear_gravity()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::HorizontalReverse)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::None)
        .expect("set root gravity");
    tree.set_justify_content(root, JustifyContent::End)
        .expect("set root justify content");
    tree.set_width(root, Length::points(218.0))
        .expect("set root width");
    tree.set_height(root, Length::points(136.0))
        .expect("set root height");

    let absolute = tree.create_default_node();
    tree.set_display(absolute, Display::Block)
        .expect("set absolute display");
    tree.set_box_sizing(absolute, BoxSizing::ContentBox)
        .expect("set absolute box sizing");
    tree.set_position_type(absolute, PositionType::Absolute)
        .expect("set absolute position");
    tree.set_width(absolute, Length::points(36.0))
        .expect("set absolute width");
    tree.set_height(absolute, Length::points(16.0))
        .expect("set absolute height");
    tree.set_margin(absolute, StandaloneEdge::Left, Length::percent(5.0))
        .expect("set absolute left margin");
    tree.set_margin(absolute, StandaloneEdge::Right, Length::percent(3.0))
        .expect("set absolute right margin");
    tree.append_child(root, absolute)
        .expect("append absolute child");

    run_standalone_rust(tree, root, Constraints::definite(218.0, 136.0))
        .expect("horizontal-reverse linear absolute justify end static position parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_horizontal_reverse_linear_absolute_justify_space_evenly_static_start_without_linear_gravity()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::HorizontalReverse)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::None)
        .expect("set root gravity");
    tree.set_justify_content(root, JustifyContent::SpaceEvenly)
        .expect("set root justify content");
    tree.set_width(root, Length::points(220.0))
        .expect("set root width");
    tree.set_height(root, Length::points(138.0))
        .expect("set root height");

    let absolute = tree.create_default_node();
    tree.set_display(absolute, Display::Block)
        .expect("set absolute display");
    tree.set_box_sizing(absolute, BoxSizing::ContentBox)
        .expect("set absolute box sizing");
    tree.set_position_type(absolute, PositionType::Absolute)
        .expect("set absolute position");
    tree.set_width(absolute, Length::points(38.0))
        .expect("set absolute width");
    tree.set_height(absolute, Length::points(17.0))
        .expect("set absolute height");
    tree.set_margin(absolute, StandaloneEdge::Left, Length::calc(5.0, 4.0))
        .expect("set absolute left margin");
    tree.set_margin(absolute, StandaloneEdge::Right, Length::calc(3.0, 6.0))
        .expect("set absolute right margin");
    tree.append_child(root, absolute)
        .expect("append absolute child");

    run_standalone_rust(tree, root, Constraints::definite(220.0, 138.0))
        .expect("horizontal-reverse linear absolute justify space-evenly static start parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_horizontal_reverse_linear_absolute_justify_stretch_static_start_without_linear_gravity()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::HorizontalReverse)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::None)
        .expect("set root gravity");
    tree.set_justify_content(root, JustifyContent::Stretch)
        .expect("set root justify content");
    tree.set_width(root, Length::points(222.0))
        .expect("set root width");
    tree.set_height(root, Length::points(140.0))
        .expect("set root height");

    let absolute = tree.create_default_node();
    tree.set_display(absolute, Display::Block)
        .expect("set absolute display");
    tree.set_box_sizing(absolute, BoxSizing::ContentBox)
        .expect("set absolute box sizing");
    tree.set_position_type(absolute, PositionType::Absolute)
        .expect("set absolute position");
    tree.set_width(absolute, Length::points(40.0))
        .expect("set absolute width");
    tree.set_height(absolute, Length::points(18.0))
        .expect("set absolute height");
    tree.set_margin(absolute, StandaloneEdge::Left, Length::percent(6.0))
        .expect("set absolute left margin");
    tree.set_margin(absolute, StandaloneEdge::Right, Length::percent(4.0))
        .expect("set absolute right margin");
    tree.append_child(root, absolute)
        .expect("append absolute child");

    run_standalone_rust(tree, root, Constraints::definite(222.0, 140.0))
        .expect("horizontal-reverse linear absolute justify stretch static start parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_horizontal_reverse_linear_absolute_justify_flex_start_static_position_with_inflow_sibling()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::HorizontalReverse)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::None)
        .expect("set root gravity");
    tree.set_justify_content(root, JustifyContent::FlexStart)
        .expect("set root justify content");
    tree.set_width(root, Length::points(224.0))
        .expect("set root width");
    tree.set_height(root, Length::points(142.0))
        .expect("set root height");

    let inflow = tree.create_default_node();
    tree.set_display(inflow, Display::Block)
        .expect("set inflow display");
    tree.set_box_sizing(inflow, BoxSizing::ContentBox)
        .expect("set inflow box sizing");
    tree.set_width(inflow, Length::points(28.0))
        .expect("set inflow width");
    tree.set_height(inflow, Length::points(19.0))
        .expect("set inflow height");
    tree.set_margin(inflow, StandaloneEdge::Left, Length::points(7.0))
        .expect("set inflow left margin");
    tree.set_margin(inflow, StandaloneEdge::Right, Length::points(5.0))
        .expect("set inflow right margin");

    let absolute = tree.create_default_node();
    tree.set_display(absolute, Display::Block)
        .expect("set absolute display");
    tree.set_box_sizing(absolute, BoxSizing::ContentBox)
        .expect("set absolute box sizing");
    tree.set_position_type(absolute, PositionType::Absolute)
        .expect("set absolute position");
    tree.set_width(absolute, Length::points(42.0))
        .expect("set absolute width");
    tree.set_height(absolute, Length::points(20.0))
        .expect("set absolute height");
    tree.set_margin(absolute, StandaloneEdge::Left, Length::calc(4.0, 5.0))
        .expect("set absolute left margin");
    tree.set_margin(absolute, StandaloneEdge::Right, Length::calc(6.0, 2.0))
        .expect("set absolute right margin");

    tree.append_child(root, inflow)
        .expect("append inflow child");
    tree.append_child(root, absolute)
        .expect("append absolute child");

    run_standalone_rust(tree, root, Constraints::definite(224.0, 142.0))
        .expect("horizontal-reverse linear absolute justify flex-start with inflow sibling parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_horizontal_reverse_linear_fixed_justify_center_static_position_without_linear_gravity()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::HorizontalReverse)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::None)
        .expect("set root gravity");
    tree.set_justify_content(root, JustifyContent::Center)
        .expect("set root justify content");
    tree.set_width(root, Length::points(214.0))
        .expect("set root width");
    tree.set_height(root, Length::points(132.0))
        .expect("set root height");

    let nested = tree.create_default_node();
    tree.set_display(nested, Display::Block)
        .expect("set nested display");
    tree.set_box_sizing(nested, BoxSizing::ContentBox)
        .expect("set nested box sizing");
    tree.set_width(nested, Length::points(34.0))
        .expect("set nested width");
    tree.set_height(nested, Length::points(22.0))
        .expect("set nested height");

    let fixed = tree.create_default_node();
    tree.set_display(fixed, Display::Block)
        .expect("set fixed display");
    tree.set_box_sizing(fixed, BoxSizing::ContentBox)
        .expect("set fixed box sizing");
    tree.set_position_type(fixed, PositionType::Fixed)
        .expect("set fixed position");
    tree.set_width(fixed, Length::points(28.0))
        .expect("set fixed width");
    tree.set_height(fixed, Length::points(14.0))
        .expect("set fixed height");
    tree.set_margin(fixed, StandaloneEdge::Left, Length::calc(5.0, 3.0))
        .expect("set fixed left margin");
    tree.set_margin(fixed, StandaloneEdge::Right, Length::calc(2.0, 8.0))
        .expect("set fixed right margin");

    tree.append_child(root, nested).expect("append nested");
    tree.append_child(nested, fixed).expect("append fixed");

    run_standalone_rust(tree, root, Constraints::definite(214.0, 132.0))
        .expect("horizontal-reverse linear fixed justify center static position parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_horizontal_reverse_linear_fixed_justify_space_evenly_static_start_without_linear_gravity()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::HorizontalReverse)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::None)
        .expect("set root gravity");
    tree.set_justify_content(root, JustifyContent::SpaceEvenly)
        .expect("set root justify content");
    tree.set_width(root, Length::points(216.0))
        .expect("set root width");
    tree.set_height(root, Length::points(134.0))
        .expect("set root height");

    let nested = tree.create_default_node();
    tree.set_display(nested, Display::Block)
        .expect("set nested display");
    tree.set_box_sizing(nested, BoxSizing::ContentBox)
        .expect("set nested box sizing");
    tree.set_width(nested, Length::points(32.0))
        .expect("set nested width");
    tree.set_height(nested, Length::points(20.0))
        .expect("set nested height");

    let fixed = tree.create_default_node();
    tree.set_display(fixed, Display::Block)
        .expect("set fixed display");
    tree.set_box_sizing(fixed, BoxSizing::ContentBox)
        .expect("set fixed box sizing");
    tree.set_position_type(fixed, PositionType::Fixed)
        .expect("set fixed position");
    tree.set_width(fixed, Length::points(26.0))
        .expect("set fixed width");
    tree.set_height(fixed, Length::points(16.0))
        .expect("set fixed height");
    tree.set_margin(fixed, StandaloneEdge::Left, Length::percent(5.0))
        .expect("set fixed left margin");
    tree.set_margin(fixed, StandaloneEdge::Right, Length::percent(4.0))
        .expect("set fixed right margin");

    tree.append_child(root, nested).expect("append nested");
    tree.append_child(nested, fixed).expect("append fixed");

    run_standalone_rust(tree, root, Constraints::definite(216.0, 134.0))
        .expect("horizontal-reverse linear fixed justify space-evenly static start parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_horizontal_reverse_linear_fixed_justify_stretch_static_start_without_linear_gravity()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::HorizontalReverse)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::None)
        .expect("set root gravity");
    tree.set_justify_content(root, JustifyContent::Stretch)
        .expect("set root justify content");
    tree.set_width(root, Length::points(218.0))
        .expect("set root width");
    tree.set_height(root, Length::points(136.0))
        .expect("set root height");

    let nested = tree.create_default_node();
    tree.set_display(nested, Display::Block)
        .expect("set nested display");
    tree.set_box_sizing(nested, BoxSizing::ContentBox)
        .expect("set nested box sizing");
    tree.set_width(nested, Length::points(30.0))
        .expect("set nested width");
    tree.set_height(nested, Length::points(24.0))
        .expect("set nested height");

    let fixed = tree.create_default_node();
    tree.set_display(fixed, Display::Block)
        .expect("set fixed display");
    tree.set_box_sizing(fixed, BoxSizing::ContentBox)
        .expect("set fixed box sizing");
    tree.set_position_type(fixed, PositionType::Fixed)
        .expect("set fixed position");
    tree.set_width(fixed, Length::points(24.0))
        .expect("set fixed width");
    tree.set_height(fixed, Length::points(15.0))
        .expect("set fixed height");
    tree.set_margin(fixed, StandaloneEdge::Left, Length::calc(4.0, 5.0))
        .expect("set fixed left margin");
    tree.set_margin(fixed, StandaloneEdge::Right, Length::calc(7.0, 2.0))
        .expect("set fixed right margin");

    tree.append_child(root, nested).expect("append nested");
    tree.append_child(nested, fixed).expect("append fixed");

    run_standalone_rust(tree, root, Constraints::definite(218.0, 136.0))
        .expect("horizontal-reverse linear fixed justify stretch static start parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_horizontal_reverse_linear_fixed_justify_flex_start_static_position_without_linear_gravity()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::HorizontalReverse)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::None)
        .expect("set root gravity");
    tree.set_justify_content(root, JustifyContent::FlexStart)
        .expect("set root justify content");
    tree.set_width(root, Length::points(220.0))
        .expect("set root width");
    tree.set_height(root, Length::points(138.0))
        .expect("set root height");

    let nested = tree.create_default_node();
    tree.set_display(nested, Display::Block)
        .expect("set nested display");
    tree.set_box_sizing(nested, BoxSizing::ContentBox)
        .expect("set nested box sizing");
    tree.set_width(nested, Length::points(34.0))
        .expect("set nested width");
    tree.set_height(nested, Length::points(21.0))
        .expect("set nested height");

    let fixed = tree.create_default_node();
    tree.set_display(fixed, Display::Block)
        .expect("set fixed display");
    tree.set_box_sizing(fixed, BoxSizing::ContentBox)
        .expect("set fixed box sizing");
    tree.set_position_type(fixed, PositionType::Fixed)
        .expect("set fixed position");
    tree.set_width(fixed, Length::points(25.0))
        .expect("set fixed width");
    tree.set_height(fixed, Length::points(14.0))
        .expect("set fixed height");
    tree.set_margin(fixed, StandaloneEdge::Left, Length::percent(4.0))
        .expect("set fixed left margin");
    tree.set_margin(fixed, StandaloneEdge::Right, Length::percent(6.0))
        .expect("set fixed right margin");

    tree.append_child(root, nested).expect("append nested");
    tree.append_child(nested, fixed).expect("append fixed");

    run_standalone_rust(tree, root, Constraints::definite(220.0, 138.0))
        .expect("horizontal-reverse linear fixed justify flex-start static position parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_horizontal_reverse_linear_fixed_justify_start_static_position_without_linear_gravity()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::HorizontalReverse)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::None)
        .expect("set root gravity");
    tree.set_justify_content(root, JustifyContent::Start)
        .expect("set root justify content");
    tree.set_width(root, Length::points(222.0))
        .expect("set root width");
    tree.set_height(root, Length::points(140.0))
        .expect("set root height");

    let nested = tree.create_default_node();
    tree.set_display(nested, Display::Block)
        .expect("set nested display");
    tree.set_box_sizing(nested, BoxSizing::ContentBox)
        .expect("set nested box sizing");
    tree.set_width(nested, Length::points(36.0))
        .expect("set nested width");
    tree.set_height(nested, Length::points(20.0))
        .expect("set nested height");

    let fixed = tree.create_default_node();
    tree.set_display(fixed, Display::Block)
        .expect("set fixed display");
    tree.set_box_sizing(fixed, BoxSizing::ContentBox)
        .expect("set fixed box sizing");
    tree.set_position_type(fixed, PositionType::Fixed)
        .expect("set fixed position");
    tree.set_width(fixed, Length::points(27.0))
        .expect("set fixed width");
    tree.set_height(fixed, Length::points(15.0))
        .expect("set fixed height");
    tree.set_margin(fixed, StandaloneEdge::Left, Length::calc(2.0, 5.0))
        .expect("set fixed left margin");
    tree.set_margin(fixed, StandaloneEdge::Right, Length::calc(4.0, 3.0))
        .expect("set fixed right margin");

    tree.append_child(root, nested).expect("append nested");
    tree.append_child(nested, fixed).expect("append fixed");

    run_standalone_rust(tree, root, Constraints::definite(222.0, 140.0))
        .expect("horizontal-reverse linear fixed justify start static position parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_horizontal_reverse_linear_fixed_justify_flex_end_static_position_without_linear_gravity()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::HorizontalReverse)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::None)
        .expect("set root gravity");
    tree.set_justify_content(root, JustifyContent::FlexEnd)
        .expect("set root justify content");
    tree.set_width(root, Length::points(224.0))
        .expect("set root width");
    tree.set_height(root, Length::points(142.0))
        .expect("set root height");

    let nested = tree.create_default_node();
    tree.set_display(nested, Display::Block)
        .expect("set nested display");
    tree.set_box_sizing(nested, BoxSizing::ContentBox)
        .expect("set nested box sizing");
    tree.set_width(nested, Length::points(38.0))
        .expect("set nested width");
    tree.set_height(nested, Length::points(22.0))
        .expect("set nested height");

    let fixed = tree.create_default_node();
    tree.set_display(fixed, Display::Block)
        .expect("set fixed display");
    tree.set_box_sizing(fixed, BoxSizing::ContentBox)
        .expect("set fixed box sizing");
    tree.set_position_type(fixed, PositionType::Fixed)
        .expect("set fixed position");
    tree.set_width(fixed, Length::points(29.0))
        .expect("set fixed width");
    tree.set_height(fixed, Length::points(16.0))
        .expect("set fixed height");
    tree.set_margin(fixed, StandaloneEdge::Left, Length::percent(6.0))
        .expect("set fixed left margin");
    tree.set_margin(fixed, StandaloneEdge::Right, Length::percent(4.0))
        .expect("set fixed right margin");

    tree.append_child(root, nested).expect("append nested");
    tree.append_child(nested, fixed).expect("append fixed");

    run_standalone_rust(tree, root, Constraints::definite(224.0, 142.0))
        .expect("horizontal-reverse linear fixed justify flex-end static position parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_horizontal_reverse_linear_fixed_justify_end_static_position_without_linear_gravity()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::HorizontalReverse)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::None)
        .expect("set root gravity");
    tree.set_justify_content(root, JustifyContent::End)
        .expect("set root justify content");
    tree.set_width(root, Length::points(226.0))
        .expect("set root width");
    tree.set_height(root, Length::points(144.0))
        .expect("set root height");

    let nested = tree.create_default_node();
    tree.set_display(nested, Display::Block)
        .expect("set nested display");
    tree.set_box_sizing(nested, BoxSizing::ContentBox)
        .expect("set nested box sizing");
    tree.set_width(nested, Length::points(40.0))
        .expect("set nested width");
    tree.set_height(nested, Length::points(24.0))
        .expect("set nested height");

    let fixed = tree.create_default_node();
    tree.set_display(fixed, Display::Block)
        .expect("set fixed display");
    tree.set_box_sizing(fixed, BoxSizing::ContentBox)
        .expect("set fixed box sizing");
    tree.set_position_type(fixed, PositionType::Fixed)
        .expect("set fixed position");
    tree.set_width(fixed, Length::points(31.0))
        .expect("set fixed width");
    tree.set_height(fixed, Length::points(17.0))
        .expect("set fixed height");
    tree.set_margin(fixed, StandaloneEdge::Left, Length::calc(3.0, 4.0))
        .expect("set fixed left margin");
    tree.set_margin(fixed, StandaloneEdge::Right, Length::calc(5.0, 2.0))
        .expect("set fixed right margin");

    tree.append_child(root, nested).expect("append nested");
    tree.append_child(nested, fixed).expect("append fixed");

    run_standalone_rust(tree, root, Constraints::definite(226.0, 144.0))
        .expect("horizontal-reverse linear fixed justify end static position parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_horizontal_reverse_linear_fixed_justify_space_between_static_start_without_linear_gravity()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::HorizontalReverse)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::None)
        .expect("set root gravity");
    tree.set_justify_content(root, JustifyContent::SpaceBetween)
        .expect("set root justify content");
    tree.set_width(root, Length::points(228.0))
        .expect("set root width");
    tree.set_height(root, Length::points(146.0))
        .expect("set root height");

    let nested = tree.create_default_node();
    tree.set_display(nested, Display::Block)
        .expect("set nested display");
    tree.set_box_sizing(nested, BoxSizing::ContentBox)
        .expect("set nested box sizing");
    tree.set_width(nested, Length::points(42.0))
        .expect("set nested width");
    tree.set_height(nested, Length::points(23.0))
        .expect("set nested height");

    let fixed = tree.create_default_node();
    tree.set_display(fixed, Display::Block)
        .expect("set fixed display");
    tree.set_box_sizing(fixed, BoxSizing::ContentBox)
        .expect("set fixed box sizing");
    tree.set_position_type(fixed, PositionType::Fixed)
        .expect("set fixed position");
    tree.set_width(fixed, Length::points(33.0))
        .expect("set fixed width");
    tree.set_height(fixed, Length::points(18.0))
        .expect("set fixed height");
    tree.set_margin(fixed, StandaloneEdge::Left, Length::percent(5.0))
        .expect("set fixed left margin");
    tree.set_margin(fixed, StandaloneEdge::Right, Length::percent(3.0))
        .expect("set fixed right margin");

    tree.append_child(root, nested).expect("append nested");
    tree.append_child(nested, fixed).expect("append fixed");

    run_standalone_rust(tree, root, Constraints::definite(228.0, 146.0))
        .expect("horizontal-reverse linear fixed justify space-between static start parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_horizontal_reverse_linear_fixed_justify_space_around_static_start_without_linear_gravity()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::HorizontalReverse)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::None)
        .expect("set root gravity");
    tree.set_justify_content(root, JustifyContent::SpaceAround)
        .expect("set root justify content");
    tree.set_width(root, Length::points(230.0))
        .expect("set root width");
    tree.set_height(root, Length::points(148.0))
        .expect("set root height");

    let nested = tree.create_default_node();
    tree.set_display(nested, Display::Block)
        .expect("set nested display");
    tree.set_box_sizing(nested, BoxSizing::ContentBox)
        .expect("set nested box sizing");
    tree.set_width(nested, Length::points(44.0))
        .expect("set nested width");
    tree.set_height(nested, Length::points(25.0))
        .expect("set nested height");

    let fixed = tree.create_default_node();
    tree.set_display(fixed, Display::Block)
        .expect("set fixed display");
    tree.set_box_sizing(fixed, BoxSizing::ContentBox)
        .expect("set fixed box sizing");
    tree.set_position_type(fixed, PositionType::Fixed)
        .expect("set fixed position");
    tree.set_width(fixed, Length::points(35.0))
        .expect("set fixed width");
    tree.set_height(fixed, Length::points(19.0))
        .expect("set fixed height");
    tree.set_margin(fixed, StandaloneEdge::Left, Length::calc(4.0, 3.0))
        .expect("set fixed left margin");
    tree.set_margin(fixed, StandaloneEdge::Right, Length::calc(2.0, 6.0))
        .expect("set fixed right margin");

    tree.append_child(root, nested).expect("append nested");
    tree.append_child(nested, fixed).expect("append fixed");

    run_standalone_rust(tree, root, Constraints::definite(230.0, 148.0))
        .expect("horizontal-reverse linear fixed justify space-around static start parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_horizontal_reverse_linear_absolute_justify_center_static_position_with_calc_main_margins_and_padding_border()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::HorizontalReverse)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::None)
        .expect("set root gravity");
    tree.set_justify_content(root, JustifyContent::Center)
        .expect("set root justify content");
    tree.set_width(root, Length::points(218.0))
        .expect("set root width");
    tree.set_height(root, Length::points(132.0))
        .expect("set root height");
    tree.set_padding(root, StandaloneEdge::Left, Length::points(7.0))
        .expect("set root left padding");
    tree.set_padding(root, StandaloneEdge::Right, Length::points(5.0))
        .expect("set root right padding");
    tree.set_padding(root, StandaloneEdge::Top, Length::points(4.0))
        .expect("set root top padding");
    tree.set_border(root, StandaloneEdge::Left, 2.0)
        .expect("set root left border");
    tree.set_border(root, StandaloneEdge::Right, 3.0)
        .expect("set root right border");
    tree.set_border(root, StandaloneEdge::Top, 1.0)
        .expect("set root top border");

    let absolute = tree.create_default_node();
    tree.set_display(absolute, Display::Block)
        .expect("set absolute display");
    tree.set_box_sizing(absolute, BoxSizing::ContentBox)
        .expect("set absolute box sizing");
    tree.set_position_type(absolute, PositionType::Absolute)
        .expect("set absolute position");
    tree.set_width(absolute, Length::points(27.0))
        .expect("set absolute width");
    tree.set_height(absolute, Length::points(15.0))
        .expect("set absolute height");
    tree.set_margin(absolute, StandaloneEdge::Left, Length::calc(4.0, 5.0))
        .expect("set absolute left margin");
    tree.set_margin(absolute, StandaloneEdge::Right, Length::calc(2.0, 7.0))
        .expect("set absolute right margin");
    tree.set_margin(absolute, StandaloneEdge::Top, Length::points(3.0))
        .expect("set absolute top margin");
    tree.append_child(root, absolute)
        .expect("append absolute child");

    run_standalone_rust(tree, root, Constraints::definite(218.0, 132.0)).expect(
        "horizontal-reverse linear absolute justify center with calc margins and padding/border parity",
    );
}

#[test]
fn standalone_owned_tree_matches_cpp_for_horizontal_reverse_linear_absolute_justify_flex_end_static_position_with_percent_main_margins_and_padding_border()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::HorizontalReverse)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::None)
        .expect("set root gravity");
    tree.set_justify_content(root, JustifyContent::FlexEnd)
        .expect("set root justify content");
    tree.set_width(root, Length::points(220.0))
        .expect("set root width");
    tree.set_height(root, Length::points(134.0))
        .expect("set root height");
    tree.set_padding(root, StandaloneEdge::Left, Length::points(6.0))
        .expect("set root left padding");
    tree.set_padding(root, StandaloneEdge::Right, Length::points(8.0))
        .expect("set root right padding");
    tree.set_padding(root, StandaloneEdge::Top, Length::points(5.0))
        .expect("set root top padding");
    tree.set_border(root, StandaloneEdge::Left, 3.0)
        .expect("set root left border");
    tree.set_border(root, StandaloneEdge::Right, 2.0)
        .expect("set root right border");
    tree.set_border(root, StandaloneEdge::Top, 2.0)
        .expect("set root top border");

    let absolute = tree.create_default_node();
    tree.set_display(absolute, Display::Block)
        .expect("set absolute display");
    tree.set_box_sizing(absolute, BoxSizing::ContentBox)
        .expect("set absolute box sizing");
    tree.set_position_type(absolute, PositionType::Absolute)
        .expect("set absolute position");
    tree.set_width(absolute, Length::points(29.0))
        .expect("set absolute width");
    tree.set_height(absolute, Length::points(14.0))
        .expect("set absolute height");
    tree.set_margin(absolute, StandaloneEdge::Left, Length::percent(5.0))
        .expect("set absolute left margin");
    tree.set_margin(absolute, StandaloneEdge::Right, Length::percent(7.0))
        .expect("set absolute right margin");
    tree.set_margin(absolute, StandaloneEdge::Top, Length::points(4.0))
        .expect("set absolute top margin");
    tree.append_child(root, absolute)
        .expect("append absolute child");

    run_standalone_rust(tree, root, Constraints::definite(220.0, 134.0)).expect(
        "horizontal-reverse linear absolute justify flex-end with percent margins and padding/border parity",
    );
}

#[test]
fn standalone_owned_tree_matches_cpp_for_horizontal_reverse_linear_absolute_justify_space_between_static_start_with_calc_main_margins_and_padding_border()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::HorizontalReverse)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::None)
        .expect("set root gravity");
    tree.set_justify_content(root, JustifyContent::SpaceBetween)
        .expect("set root justify content");
    tree.set_width(root, Length::points(222.0))
        .expect("set root width");
    tree.set_height(root, Length::points(136.0))
        .expect("set root height");
    tree.set_padding(root, StandaloneEdge::Left, Length::points(5.0))
        .expect("set root left padding");
    tree.set_padding(root, StandaloneEdge::Right, Length::points(9.0))
        .expect("set root right padding");
    tree.set_padding(root, StandaloneEdge::Top, Length::points(6.0))
        .expect("set root top padding");
    tree.set_border(root, StandaloneEdge::Left, 4.0)
        .expect("set root left border");
    tree.set_border(root, StandaloneEdge::Right, 1.0)
        .expect("set root right border");
    tree.set_border(root, StandaloneEdge::Top, 2.0)
        .expect("set root top border");

    let absolute = tree.create_default_node();
    tree.set_display(absolute, Display::Block)
        .expect("set absolute display");
    tree.set_box_sizing(absolute, BoxSizing::ContentBox)
        .expect("set absolute box sizing");
    tree.set_position_type(absolute, PositionType::Absolute)
        .expect("set absolute position");
    tree.set_width(absolute, Length::points(31.0))
        .expect("set absolute width");
    tree.set_height(absolute, Length::points(16.0))
        .expect("set absolute height");
    tree.set_margin(absolute, StandaloneEdge::Left, Length::calc(3.0, 8.0))
        .expect("set absolute left margin");
    tree.set_margin(absolute, StandaloneEdge::Right, Length::calc(6.0, 4.0))
        .expect("set absolute right margin");
    tree.set_margin(absolute, StandaloneEdge::Top, Length::points(5.0))
        .expect("set absolute top margin");
    tree.append_child(root, absolute)
        .expect("append absolute child");

    run_standalone_rust(tree, root, Constraints::definite(222.0, 136.0)).expect(
        "horizontal-reverse linear absolute justify space-between with calc margins and padding/border parity",
    );
}

#[test]
fn standalone_owned_tree_matches_cpp_for_horizontal_reverse_linear_fixed_justify_center_static_position_with_calc_main_margins_and_padding_border()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::HorizontalReverse)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::None)
        .expect("set root gravity");
    tree.set_justify_content(root, JustifyContent::Center)
        .expect("set root justify content");
    tree.set_width(root, Length::points(224.0))
        .expect("set root width");
    tree.set_height(root, Length::points(138.0))
        .expect("set root height");
    tree.set_padding(root, StandaloneEdge::Left, Length::points(7.0))
        .expect("set root left padding");
    tree.set_padding(root, StandaloneEdge::Right, Length::points(5.0))
        .expect("set root right padding");
    tree.set_padding(root, StandaloneEdge::Top, Length::points(4.0))
        .expect("set root top padding");
    tree.set_border(root, StandaloneEdge::Left, 2.0)
        .expect("set root left border");
    tree.set_border(root, StandaloneEdge::Right, 3.0)
        .expect("set root right border");
    tree.set_border(root, StandaloneEdge::Top, 1.0)
        .expect("set root top border");

    let nested = tree.create_default_node();
    tree.set_display(nested, Display::Block)
        .expect("set nested display");
    tree.set_box_sizing(nested, BoxSizing::ContentBox)
        .expect("set nested box sizing");
    tree.set_width(nested, Length::points(36.0))
        .expect("set nested width");
    tree.set_height(nested, Length::points(22.0))
        .expect("set nested height");

    let fixed = tree.create_default_node();
    tree.set_display(fixed, Display::Block)
        .expect("set fixed display");
    tree.set_box_sizing(fixed, BoxSizing::ContentBox)
        .expect("set fixed box sizing");
    tree.set_position_type(fixed, PositionType::Fixed)
        .expect("set fixed position");
    tree.set_width(fixed, Length::points(27.0))
        .expect("set fixed width");
    tree.set_height(fixed, Length::points(15.0))
        .expect("set fixed height");
    tree.set_margin(fixed, StandaloneEdge::Left, Length::calc(4.0, 5.0))
        .expect("set fixed left margin");
    tree.set_margin(fixed, StandaloneEdge::Right, Length::calc(2.0, 7.0))
        .expect("set fixed right margin");
    tree.set_margin(fixed, StandaloneEdge::Top, Length::points(3.0))
        .expect("set fixed top margin");

    tree.append_child(root, nested).expect("append nested");
    tree.append_child(nested, fixed).expect("append fixed");

    run_standalone_rust(tree, root, Constraints::definite(224.0, 138.0)).expect(
        "horizontal-reverse linear fixed justify center with calc margins and padding/border parity",
    );
}

#[test]
fn standalone_owned_tree_matches_cpp_for_horizontal_reverse_linear_fixed_justify_flex_end_static_position_with_percent_main_margins_and_padding_border()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::HorizontalReverse)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::None)
        .expect("set root gravity");
    tree.set_justify_content(root, JustifyContent::FlexEnd)
        .expect("set root justify content");
    tree.set_width(root, Length::points(226.0))
        .expect("set root width");
    tree.set_height(root, Length::points(140.0))
        .expect("set root height");
    tree.set_padding(root, StandaloneEdge::Left, Length::points(6.0))
        .expect("set root left padding");
    tree.set_padding(root, StandaloneEdge::Right, Length::points(8.0))
        .expect("set root right padding");
    tree.set_padding(root, StandaloneEdge::Top, Length::points(5.0))
        .expect("set root top padding");
    tree.set_border(root, StandaloneEdge::Left, 3.0)
        .expect("set root left border");
    tree.set_border(root, StandaloneEdge::Right, 2.0)
        .expect("set root right border");
    tree.set_border(root, StandaloneEdge::Top, 2.0)
        .expect("set root top border");

    let nested = tree.create_default_node();
    tree.set_display(nested, Display::Block)
        .expect("set nested display");
    tree.set_box_sizing(nested, BoxSizing::ContentBox)
        .expect("set nested box sizing");
    tree.set_width(nested, Length::points(38.0))
        .expect("set nested width");
    tree.set_height(nested, Length::points(24.0))
        .expect("set nested height");

    let fixed = tree.create_default_node();
    tree.set_display(fixed, Display::Block)
        .expect("set fixed display");
    tree.set_box_sizing(fixed, BoxSizing::ContentBox)
        .expect("set fixed box sizing");
    tree.set_position_type(fixed, PositionType::Fixed)
        .expect("set fixed position");
    tree.set_width(fixed, Length::points(29.0))
        .expect("set fixed width");
    tree.set_height(fixed, Length::points(14.0))
        .expect("set fixed height");
    tree.set_margin(fixed, StandaloneEdge::Left, Length::percent(5.0))
        .expect("set fixed left margin");
    tree.set_margin(fixed, StandaloneEdge::Right, Length::percent(7.0))
        .expect("set fixed right margin");
    tree.set_margin(fixed, StandaloneEdge::Top, Length::points(4.0))
        .expect("set fixed top margin");

    tree.append_child(root, nested).expect("append nested");
    tree.append_child(nested, fixed).expect("append fixed");

    run_standalone_rust(tree, root, Constraints::definite(226.0, 140.0)).expect(
        "horizontal-reverse linear fixed justify flex-end with percent margins and padding/border parity",
    );
}

#[test]
fn standalone_owned_tree_matches_cpp_for_horizontal_reverse_linear_fixed_justify_space_between_static_start_with_calc_main_margins_and_padding_border()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::HorizontalReverse)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::None)
        .expect("set root gravity");
    tree.set_justify_content(root, JustifyContent::SpaceBetween)
        .expect("set root justify content");
    tree.set_width(root, Length::points(228.0))
        .expect("set root width");
    tree.set_height(root, Length::points(142.0))
        .expect("set root height");
    tree.set_padding(root, StandaloneEdge::Left, Length::points(5.0))
        .expect("set root left padding");
    tree.set_padding(root, StandaloneEdge::Right, Length::points(9.0))
        .expect("set root right padding");
    tree.set_padding(root, StandaloneEdge::Top, Length::points(6.0))
        .expect("set root top padding");
    tree.set_border(root, StandaloneEdge::Left, 4.0)
        .expect("set root left border");
    tree.set_border(root, StandaloneEdge::Right, 1.0)
        .expect("set root right border");
    tree.set_border(root, StandaloneEdge::Top, 2.0)
        .expect("set root top border");

    let nested = tree.create_default_node();
    tree.set_display(nested, Display::Block)
        .expect("set nested display");
    tree.set_box_sizing(nested, BoxSizing::ContentBox)
        .expect("set nested box sizing");
    tree.set_width(nested, Length::points(40.0))
        .expect("set nested width");
    tree.set_height(nested, Length::points(22.0))
        .expect("set nested height");

    let fixed = tree.create_default_node();
    tree.set_display(fixed, Display::Block)
        .expect("set fixed display");
    tree.set_box_sizing(fixed, BoxSizing::ContentBox)
        .expect("set fixed box sizing");
    tree.set_position_type(fixed, PositionType::Fixed)
        .expect("set fixed position");
    tree.set_width(fixed, Length::points(31.0))
        .expect("set fixed width");
    tree.set_height(fixed, Length::points(16.0))
        .expect("set fixed height");
    tree.set_margin(fixed, StandaloneEdge::Left, Length::calc(3.0, 8.0))
        .expect("set fixed left margin");
    tree.set_margin(fixed, StandaloneEdge::Right, Length::calc(6.0, 4.0))
        .expect("set fixed right margin");
    tree.set_margin(fixed, StandaloneEdge::Top, Length::points(5.0))
        .expect("set fixed top margin");

    tree.append_child(root, nested).expect("append nested");
    tree.append_child(nested, fixed).expect("append fixed");

    run_standalone_rust(tree, root, Constraints::definite(228.0, 142.0)).expect(
        "horizontal-reverse linear fixed justify space-between with calc margins and padding/border parity",
    );
}

#[test]
fn standalone_owned_tree_matches_cpp_for_absolute_vertical_linear_main_axis_static_position() {
    {
        let mut tree = StandaloneTree::new();
        let root = tree.create_default_node();
        tree.set_display(root, Display::Linear)
            .expect("set root display");
        tree.set_box_sizing(root, BoxSizing::ContentBox)
            .expect("set root box sizing");
        tree.set_linear_orientation(root, LinearOrientation::Vertical)
            .expect("set root orientation");
        tree.set_linear_gravity(root, LinearGravity::Center)
            .expect("set root gravity");
        tree.set_width(root, Length::points(50.0))
            .expect("set root width");
        tree.set_height(root, Length::points(100.0))
            .expect("set root height");

        let absolute = tree.create_default_node();
        tree.set_display(absolute, Display::Block)
            .expect("set absolute display");
        tree.set_box_sizing(absolute, BoxSizing::ContentBox)
            .expect("set absolute box sizing");
        tree.set_position_type(absolute, PositionType::Absolute)
            .expect("set absolute position");
        tree.set_width(absolute, Length::points(20.0))
            .expect("set absolute width");
        tree.set_height(absolute, Length::points(10.0))
            .expect("set absolute height");
        tree.append_child(root, absolute)
            .expect("append absolute child");

        run_standalone_rust(tree, root, Constraints::definite(50.0, 100.0))
            .expect("absolute vertical linear center static main-position parity");
    }

    {
        let mut tree = StandaloneTree::new();
        let root = tree.create_default_node();
        tree.set_display(root, Display::Linear)
            .expect("set root display");
        tree.set_box_sizing(root, BoxSizing::ContentBox)
            .expect("set root box sizing");
        tree.set_linear_orientation(root, LinearOrientation::Vertical)
            .expect("set root orientation");
        tree.set_linear_gravity(root, LinearGravity::End)
            .expect("set root gravity");
        tree.set_width(root, Length::points(50.0))
            .expect("set root width");
        tree.set_height(root, Length::points(100.0))
            .expect("set root height");

        let absolute = tree.create_default_node();
        tree.set_display(absolute, Display::Block)
            .expect("set absolute display");
        tree.set_box_sizing(absolute, BoxSizing::ContentBox)
            .expect("set absolute box sizing");
        tree.set_position_type(absolute, PositionType::Absolute)
            .expect("set absolute position");
        tree.set_width(absolute, Length::points(20.0))
            .expect("set absolute width");
        tree.set_height(absolute, Length::points(10.0))
            .expect("set absolute height");
        tree.append_child(root, absolute)
            .expect("append absolute child");

        run_standalone_rust(tree, root, Constraints::definite(50.0, 100.0))
            .expect("absolute vertical linear end static main-position parity");
    }

    {
        let mut tree = StandaloneTree::new();
        let root = tree.create_default_node();
        tree.set_display(root, Display::Linear)
            .expect("set root display");
        tree.set_box_sizing(root, BoxSizing::ContentBox)
            .expect("set root box sizing");
        tree.set_linear_orientation(root, LinearOrientation::Vertical)
            .expect("set root orientation");
        tree.set_linear_gravity(root, LinearGravity::Bottom)
            .expect("set root gravity");
        tree.set_width(root, Length::points(50.0))
            .expect("set root width");
        tree.set_height(root, Length::points(100.0))
            .expect("set root height");

        let absolute = tree.create_default_node();
        tree.set_display(absolute, Display::Block)
            .expect("set absolute display");
        tree.set_box_sizing(absolute, BoxSizing::ContentBox)
            .expect("set absolute box sizing");
        tree.set_position_type(absolute, PositionType::Absolute)
            .expect("set absolute position");
        tree.set_width(absolute, Length::points(20.0))
            .expect("set absolute width");
        tree.set_height(absolute, Length::points(10.0))
            .expect("set absolute height");
        tree.append_child(root, absolute)
            .expect("append absolute child");

        run_standalone_rust(tree, root, Constraints::definite(50.0, 100.0))
            .expect("absolute vertical linear physical bottom static main-position parity");
    }

    {
        let mut tree = StandaloneTree::new();
        let root = tree.create_default_node();
        tree.set_display(root, Display::Linear)
            .expect("set root display");
        tree.set_box_sizing(root, BoxSizing::ContentBox)
            .expect("set root box sizing");
        tree.set_linear_orientation(root, LinearOrientation::Vertical)
            .expect("set root orientation");
        tree.set_linear_gravity(root, LinearGravity::Top)
            .expect("set root gravity");
        tree.set_width(root, Length::points(50.0))
            .expect("set root width");
        tree.set_height(root, Length::points(100.0))
            .expect("set root height");

        let absolute = tree.create_default_node();
        tree.set_display(absolute, Display::Block)
            .expect("set absolute display");
        tree.set_box_sizing(absolute, BoxSizing::ContentBox)
            .expect("set absolute box sizing");
        tree.set_position_type(absolute, PositionType::Absolute)
            .expect("set absolute position");
        tree.set_width(absolute, Length::points(20.0))
            .expect("set absolute width");
        tree.set_height(absolute, Length::points(10.0))
            .expect("set absolute height");
        tree.append_child(root, absolute)
            .expect("append absolute child");

        run_standalone_rust(tree, root, Constraints::definite(50.0, 100.0))
            .expect("absolute vertical linear physical top static main-position parity");
    }
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_absolute_horizontal_physical_cross_static_positions_with_margins()
 {
    {
        let mut tree = StandaloneTree::new();
        let root = tree.create_default_node();
        tree.set_display(root, Display::Linear)
            .expect("set root display");
        tree.set_box_sizing(root, BoxSizing::ContentBox)
            .expect("set root box sizing");
        tree.set_linear_orientation(root, LinearOrientation::Horizontal)
            .expect("set root orientation");
        tree.set_linear_gravity(root, LinearGravity::Center)
            .expect("set root gravity");
        tree.set_align_items(root, AlignItems::FlexEnd)
            .expect("set root align items");
        tree.set_width(root, Length::points(150.0))
            .expect("set root width");
        tree.set_height(root, Length::points(90.0))
            .expect("set root height");

        let absolute = tree.create_default_node();
        tree.set_display(absolute, Display::Block)
            .expect("set absolute display");
        tree.set_box_sizing(absolute, BoxSizing::ContentBox)
            .expect("set absolute box sizing");
        tree.set_position_type(absolute, PositionType::Absolute)
            .expect("set absolute position");
        tree.set_linear_layout_gravity(absolute, LinearLayoutGravity::Top)
            .expect("set absolute layout gravity");
        tree.set_width(absolute, Length::points(24.0))
            .expect("set absolute width");
        tree.set_height(absolute, Length::points(12.0))
            .expect("set absolute height");
        tree.set_margin(absolute, StandaloneEdge::Top, Length::calc(3.0, 8.0))
            .expect("set absolute top margin");
        tree.set_margin(absolute, StandaloneEdge::Bottom, Length::calc(5.0, 4.0))
            .expect("set absolute bottom margin");
        tree.append_child(root, absolute)
            .expect("append absolute child");

        run_standalone_rust(tree, root, Constraints::definite(150.0, 90.0))
            .expect("linear absolute horizontal physical top static position parity");
    }

    {
        let mut tree = StandaloneTree::new();
        let root = tree.create_default_node();
        tree.set_display(root, Display::Linear)
            .expect("set root display");
        tree.set_linear_orientation(root, LinearOrientation::Horizontal)
            .expect("set root orientation");
        tree.set_linear_gravity(root, LinearGravity::Center)
            .expect("set root gravity");
        tree.set_align_items(root, AlignItems::FlexStart)
            .expect("set root align items");
        tree.set_width(root, Length::points(150.0))
            .expect("set root width");
        tree.set_height(root, Length::points(90.0))
            .expect("set root height");

        let absolute = tree.create_default_node();
        tree.set_display(absolute, Display::Block)
            .expect("set absolute display");
        tree.set_position_type(absolute, PositionType::Absolute)
            .expect("set absolute position");
        tree.set_linear_layout_gravity(absolute, LinearLayoutGravity::Bottom)
            .expect("set absolute layout gravity");
        tree.set_width(absolute, Length::points(24.0))
            .expect("set absolute width");
        tree.set_height(absolute, Length::points(12.0))
            .expect("set absolute height");
        tree.set_margin(absolute, StandaloneEdge::Top, Length::percent(6.0))
            .expect("set absolute top margin");
        tree.set_margin(absolute, StandaloneEdge::Bottom, Length::percent(4.0))
            .expect("set absolute bottom margin");
        tree.append_child(root, absolute)
            .expect("append absolute child");

        run_standalone_rust(tree, root, Constraints::definite(150.0, 90.0))
            .expect("linear absolute horizontal physical bottom static position parity");
    }
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_absolute_vertical_reverse_static_positions_with_margins()
 {
    {
        let mut tree = StandaloneTree::new();
        let root = tree.create_default_node();
        tree.set_display(root, Display::Linear)
            .expect("set root display");
        tree.set_box_sizing(root, BoxSizing::ContentBox)
            .expect("set root box sizing");
        tree.set_linear_orientation(root, LinearOrientation::VerticalReverse)
            .expect("set root orientation");
        tree.set_linear_gravity(root, LinearGravity::Center)
            .expect("set root gravity");
        tree.set_width(root, Length::points(70.0))
            .expect("set root width");
        tree.set_height(root, Length::points(160.0))
            .expect("set root height");

        let absolute = tree.create_default_node();
        tree.set_display(absolute, Display::Block)
            .expect("set absolute display");
        tree.set_box_sizing(absolute, BoxSizing::ContentBox)
            .expect("set absolute box sizing");
        tree.set_position_type(absolute, PositionType::Absolute)
            .expect("set absolute position");
        tree.set_width(absolute, Length::points(24.0))
            .expect("set absolute width");
        tree.set_height(absolute, Length::points(14.0))
            .expect("set absolute height");
        tree.set_margin(absolute, StandaloneEdge::Top, Length::calc(4.0, 9.0))
            .expect("set absolute top margin");
        tree.set_margin(absolute, StandaloneEdge::Bottom, Length::calc(2.0, 6.0))
            .expect("set absolute bottom margin");
        tree.append_child(root, absolute)
            .expect("append absolute child");

        run_standalone_rust(tree, root, Constraints::definite(70.0, 160.0))
            .expect("linear absolute vertical-reverse center static position parity");
    }

    {
        let mut tree = StandaloneTree::new();
        let root = tree.create_default_node();
        tree.set_display(root, Display::Linear)
            .expect("set root display");
        tree.set_linear_orientation(root, LinearOrientation::VerticalReverse)
            .expect("set root orientation");
        tree.set_linear_gravity(root, LinearGravity::Top)
            .expect("set root gravity");
        tree.set_width(root, Length::points(70.0))
            .expect("set root width");
        tree.set_height(root, Length::points(160.0))
            .expect("set root height");

        let absolute = tree.create_default_node();
        tree.set_display(absolute, Display::Block)
            .expect("set absolute display");
        tree.set_position_type(absolute, PositionType::Absolute)
            .expect("set absolute position");
        tree.set_width(absolute, Length::points(24.0))
            .expect("set absolute width");
        tree.set_height(absolute, Length::points(14.0))
            .expect("set absolute height");
        tree.set_margin(absolute, StandaloneEdge::Top, Length::percent(5.0))
            .expect("set absolute top margin");
        tree.set_margin(absolute, StandaloneEdge::Bottom, Length::percent(3.0))
            .expect("set absolute bottom margin");
        tree.append_child(root, absolute)
            .expect("append absolute child");

        run_standalone_rust(tree, root, Constraints::definite(70.0, 160.0))
            .expect("linear absolute vertical-reverse physical top static position parity");
    }
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_fixed_descendant_static_alignment() {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::Center)
        .expect("set root gravity");
    tree.set_width(root, Length::points(100.0))
        .expect("set root width");
    tree.set_height(root, Length::points(50.0))
        .expect("set root height");

    let nested = tree.create_default_node();
    tree.set_display(nested, Display::Block)
        .expect("set nested display");
    tree.set_box_sizing(nested, BoxSizing::ContentBox)
        .expect("set nested box sizing");
    tree.set_width(nested, Length::points(20.0))
        .expect("set nested width");
    tree.set_height(nested, Length::points(20.0))
        .expect("set nested height");

    let fixed = tree.create_default_node();
    tree.set_display(fixed, Display::Block)
        .expect("set fixed display");
    tree.set_box_sizing(fixed, BoxSizing::ContentBox)
        .expect("set fixed box sizing");
    tree.set_position_type(fixed, PositionType::Fixed)
        .expect("set fixed position");
    tree.set_linear_layout_gravity(fixed, LinearLayoutGravity::End)
        .expect("set fixed layout gravity");
    tree.set_width(fixed, Length::points(10.0))
        .expect("set fixed width");
    tree.set_height(fixed, Length::points(8.0))
        .expect("set fixed height");

    tree.append_child(root, nested).expect("append nested");
    tree.append_child(nested, fixed).expect("append fixed");

    run_standalone_rust(tree, root, Constraints::definite(100.0, 50.0))
        .expect("linear fixed descendant static alignment parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_fixed_static_position_with_margins() {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::Center)
        .expect("set root gravity");
    tree.set_width(root, Length::points(100.0))
        .expect("set root width");
    tree.set_height(root, Length::points(50.0))
        .expect("set root height");

    let nested = tree.create_default_node();
    tree.set_display(nested, Display::Block)
        .expect("set nested display");
    tree.set_box_sizing(nested, BoxSizing::ContentBox)
        .expect("set nested box sizing");
    tree.set_width(nested, Length::points(20.0))
        .expect("set nested width");
    tree.set_height(nested, Length::points(20.0))
        .expect("set nested height");

    let fixed = tree.create_default_node();
    tree.set_display(fixed, Display::Block)
        .expect("set fixed display");
    tree.set_box_sizing(fixed, BoxSizing::ContentBox)
        .expect("set fixed box sizing");
    tree.set_position_type(fixed, PositionType::Fixed)
        .expect("set fixed position");
    tree.set_linear_layout_gravity(fixed, LinearLayoutGravity::End)
        .expect("set fixed layout gravity");
    tree.set_width(fixed, Length::points(10.0))
        .expect("set fixed width");
    tree.set_height(fixed, Length::points(8.0))
        .expect("set fixed height");
    tree.set_margin(fixed, StandaloneEdge::Left, Length::points(3.0))
        .expect("set fixed left margin");
    tree.set_margin(fixed, StandaloneEdge::Right, Length::points(7.0))
        .expect("set fixed right margin");
    tree.set_margin(fixed, StandaloneEdge::Top, Length::points(4.0))
        .expect("set fixed top margin");
    tree.set_margin(fixed, StandaloneEdge::Bottom, Length::points(6.0))
        .expect("set fixed bottom margin");

    tree.append_child(root, nested).expect("append nested");
    tree.append_child(nested, fixed).expect("append fixed");

    run_standalone_rust(tree, root, Constraints::definite(100.0, 50.0))
        .expect("linear fixed static position with margins parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_fixed_rtl_static_position_with_margins() {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_direction(root, Direction::Rtl)
        .expect("set root direction");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::None)
        .expect("set root gravity");
    tree.set_width(root, Length::points(100.0))
        .expect("set root width");
    tree.set_height(root, Length::points(50.0))
        .expect("set root height");

    let nested = tree.create_default_node();
    tree.set_display(nested, Display::Block)
        .expect("set nested display");
    tree.set_box_sizing(nested, BoxSizing::ContentBox)
        .expect("set nested box sizing");
    tree.set_width(nested, Length::points(20.0))
        .expect("set nested width");
    tree.set_height(nested, Length::points(20.0))
        .expect("set nested height");

    let fixed = tree.create_default_node();
    tree.set_display(fixed, Display::Block)
        .expect("set fixed display");
    tree.set_box_sizing(fixed, BoxSizing::ContentBox)
        .expect("set fixed box sizing");
    tree.set_position_type(fixed, PositionType::Fixed)
        .expect("set fixed position");
    tree.set_linear_layout_gravity(fixed, LinearLayoutGravity::Start)
        .expect("set fixed layout gravity");
    tree.set_width(fixed, Length::points(10.0))
        .expect("set fixed width");
    tree.set_height(fixed, Length::points(8.0))
        .expect("set fixed height");
    tree.set_margin(fixed, StandaloneEdge::Left, Length::points(3.0))
        .expect("set fixed left margin");
    tree.set_margin(fixed, StandaloneEdge::Right, Length::points(7.0))
        .expect("set fixed right margin");
    tree.set_margin(fixed, StandaloneEdge::Top, Length::points(4.0))
        .expect("set fixed top margin");
    tree.set_margin(fixed, StandaloneEdge::Bottom, Length::points(6.0))
        .expect("set fixed bottom margin");

    tree.append_child(root, nested).expect("append nested");
    tree.append_child(nested, fixed).expect("append fixed");

    run_standalone_rust(tree, root, Constraints::definite(100.0, 50.0))
        .expect("linear fixed RTL static position with margins parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_fixed_horizontal_physical_cross_static_positions_with_margins()
 {
    {
        let mut tree = StandaloneTree::new();
        let root = tree.create_default_node();
        tree.set_display(root, Display::Linear)
            .expect("set root display");
        tree.set_box_sizing(root, BoxSizing::ContentBox)
            .expect("set root box sizing");
        tree.set_linear_orientation(root, LinearOrientation::Horizontal)
            .expect("set root orientation");
        tree.set_linear_gravity(root, LinearGravity::Center)
            .expect("set root gravity");
        tree.set_align_items(root, AlignItems::FlexEnd)
            .expect("set root align items");
        tree.set_width(root, Length::points(150.0))
            .expect("set root width");
        tree.set_height(root, Length::points(90.0))
            .expect("set root height");

        let nested = tree.create_default_node();
        tree.set_display(nested, Display::Block)
            .expect("set nested display");
        tree.set_width(nested, Length::points(20.0))
            .expect("set nested width");
        tree.set_height(nested, Length::points(20.0))
            .expect("set nested height");

        let fixed = tree.create_default_node();
        tree.set_display(fixed, Display::Block)
            .expect("set fixed display");
        tree.set_box_sizing(fixed, BoxSizing::ContentBox)
            .expect("set fixed box sizing");
        tree.set_position_type(fixed, PositionType::Fixed)
            .expect("set fixed position");
        tree.set_linear_layout_gravity(fixed, LinearLayoutGravity::Top)
            .expect("set fixed layout gravity");
        tree.set_width(fixed, Length::points(24.0))
            .expect("set fixed width");
        tree.set_height(fixed, Length::points(12.0))
            .expect("set fixed height");
        tree.set_margin(fixed, StandaloneEdge::Top, Length::calc(3.0, 8.0))
            .expect("set fixed top margin");
        tree.set_margin(fixed, StandaloneEdge::Bottom, Length::calc(5.0, 4.0))
            .expect("set fixed bottom margin");

        tree.append_child(root, nested).expect("append nested");
        tree.append_child(nested, fixed).expect("append fixed");

        run_standalone_rust(tree, root, Constraints::definite(150.0, 90.0))
            .expect("linear fixed horizontal physical top static position parity");
    }

    {
        let mut tree = StandaloneTree::new();
        let root = tree.create_default_node();
        tree.set_display(root, Display::Linear)
            .expect("set root display");
        tree.set_linear_orientation(root, LinearOrientation::Horizontal)
            .expect("set root orientation");
        tree.set_linear_gravity(root, LinearGravity::Center)
            .expect("set root gravity");
        tree.set_align_items(root, AlignItems::FlexStart)
            .expect("set root align items");
        tree.set_width(root, Length::points(150.0))
            .expect("set root width");
        tree.set_height(root, Length::points(90.0))
            .expect("set root height");

        let nested = tree.create_default_node();
        tree.set_display(nested, Display::Block)
            .expect("set nested display");
        tree.set_width(nested, Length::points(20.0))
            .expect("set nested width");
        tree.set_height(nested, Length::points(20.0))
            .expect("set nested height");

        let fixed = tree.create_default_node();
        tree.set_display(fixed, Display::Block)
            .expect("set fixed display");
        tree.set_position_type(fixed, PositionType::Fixed)
            .expect("set fixed position");
        tree.set_linear_layout_gravity(fixed, LinearLayoutGravity::Bottom)
            .expect("set fixed layout gravity");
        tree.set_width(fixed, Length::points(24.0))
            .expect("set fixed width");
        tree.set_height(fixed, Length::points(12.0))
            .expect("set fixed height");
        tree.set_margin(fixed, StandaloneEdge::Top, Length::percent(6.0))
            .expect("set fixed top margin");
        tree.set_margin(fixed, StandaloneEdge::Bottom, Length::percent(4.0))
            .expect("set fixed bottom margin");

        tree.append_child(root, nested).expect("append nested");
        tree.append_child(nested, fixed).expect("append fixed");

        run_standalone_rust(tree, root, Constraints::definite(150.0, 90.0))
            .expect("linear fixed horizontal physical bottom static position parity");
    }
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_absolute_vertical_reverse_bottom_static_position_with_calc_main_margins()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::VerticalReverse)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::Bottom)
        .expect("set root gravity");
    tree.set_width(root, Length::points(70.0))
        .expect("set root width");
    tree.set_height(root, Length::points(160.0))
        .expect("set root height");

    let absolute = tree.create_default_node();
    tree.set_display(absolute, Display::Block)
        .expect("set absolute display");
    tree.set_box_sizing(absolute, BoxSizing::ContentBox)
        .expect("set absolute box sizing");
    tree.set_position_type(absolute, PositionType::Absolute)
        .expect("set absolute position");
    tree.set_width(absolute, Length::points(24.0))
        .expect("set absolute width");
    tree.set_height(absolute, Length::points(14.0))
        .expect("set absolute height");
    tree.set_margin(absolute, StandaloneEdge::Top, Length::calc(3.0, 8.0))
        .expect("set absolute top margin");
    tree.set_margin(absolute, StandaloneEdge::Bottom, Length::calc(5.0, 4.0))
        .expect("set absolute bottom margin");
    tree.append_child(root, absolute)
        .expect("append absolute child");

    run_standalone_rust(tree, root, Constraints::definite(70.0, 160.0))
        .expect("linear absolute vertical-reverse physical bottom static position parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_absolute_vertical_reverse_end_static_position_with_percent_main_margins()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::VerticalReverse)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::End)
        .expect("set root gravity");
    tree.set_width(root, Length::points(72.0))
        .expect("set root width");
    tree.set_height(root, Length::points(164.0))
        .expect("set root height");

    let absolute = tree.create_default_node();
    tree.set_display(absolute, Display::Block)
        .expect("set absolute display");
    tree.set_box_sizing(absolute, BoxSizing::ContentBox)
        .expect("set absolute box sizing");
    tree.set_position_type(absolute, PositionType::Absolute)
        .expect("set absolute position");
    tree.set_width(absolute, Length::points(22.0))
        .expect("set absolute width");
    tree.set_height(absolute, Length::points(13.0))
        .expect("set absolute height");
    tree.set_margin(absolute, StandaloneEdge::Top, Length::percent(7.0))
        .expect("set absolute top margin");
    tree.set_margin(absolute, StandaloneEdge::Bottom, Length::percent(5.0))
        .expect("set absolute bottom margin");
    tree.append_child(root, absolute)
        .expect("append absolute child");

    run_standalone_rust(tree, root, Constraints::definite(72.0, 164.0))
        .expect("linear absolute vertical-reverse end static position parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_absolute_vertical_reverse_start_static_position_with_calc_main_margins()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::VerticalReverse)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::Start)
        .expect("set root gravity");
    tree.set_width(root, Length::points(74.0))
        .expect("set root width");
    tree.set_height(root, Length::points(168.0))
        .expect("set root height");

    let absolute = tree.create_default_node();
    tree.set_display(absolute, Display::Block)
        .expect("set absolute display");
    tree.set_box_sizing(absolute, BoxSizing::ContentBox)
        .expect("set absolute box sizing");
    tree.set_position_type(absolute, PositionType::Absolute)
        .expect("set absolute position");
    tree.set_width(absolute, Length::points(20.0))
        .expect("set absolute width");
    tree.set_height(absolute, Length::points(16.0))
        .expect("set absolute height");
    tree.set_margin(absolute, StandaloneEdge::Top, Length::calc(2.0, 9.0))
        .expect("set absolute top margin");
    tree.set_margin(absolute, StandaloneEdge::Bottom, Length::calc(6.0, 3.0))
        .expect("set absolute bottom margin");
    tree.append_child(root, absolute)
        .expect("append absolute child");

    run_standalone_rust(tree, root, Constraints::definite(74.0, 168.0))
        .expect("linear absolute vertical-reverse start static position parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_absolute_vertical_reverse_none_static_position_with_percent_main_margins()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::VerticalReverse)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::None)
        .expect("set root gravity");
    tree.set_width(root, Length::points(76.0))
        .expect("set root width");
    tree.set_height(root, Length::points(170.0))
        .expect("set root height");

    let absolute = tree.create_default_node();
    tree.set_display(absolute, Display::Block)
        .expect("set absolute display");
    tree.set_box_sizing(absolute, BoxSizing::ContentBox)
        .expect("set absolute box sizing");
    tree.set_position_type(absolute, PositionType::Absolute)
        .expect("set absolute position");
    tree.set_width(absolute, Length::points(18.0))
        .expect("set absolute width");
    tree.set_height(absolute, Length::points(12.0))
        .expect("set absolute height");
    tree.set_margin(absolute, StandaloneEdge::Top, Length::percent(4.0))
        .expect("set absolute top margin");
    tree.set_margin(absolute, StandaloneEdge::Bottom, Length::percent(8.0))
        .expect("set absolute bottom margin");
    tree.append_child(root, absolute)
        .expect("append absolute child");

    run_standalone_rust(tree, root, Constraints::definite(76.0, 170.0))
        .expect("linear absolute vertical-reverse none static position parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_fixed_vertical_reverse_center_static_position_with_calc_main_margins()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::VerticalReverse)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::Center)
        .expect("set root gravity");
    tree.set_width(root, Length::points(70.0))
        .expect("set root width");
    tree.set_height(root, Length::points(160.0))
        .expect("set root height");

    let nested = tree.create_default_node();
    tree.set_display(nested, Display::Block)
        .expect("set nested display");
    tree.set_width(nested, Length::points(20.0))
        .expect("set nested width");
    tree.set_height(nested, Length::points(20.0))
        .expect("set nested height");

    let fixed = tree.create_default_node();
    tree.set_display(fixed, Display::Block)
        .expect("set fixed display");
    tree.set_box_sizing(fixed, BoxSizing::ContentBox)
        .expect("set fixed box sizing");
    tree.set_position_type(fixed, PositionType::Fixed)
        .expect("set fixed position");
    tree.set_width(fixed, Length::points(24.0))
        .expect("set fixed width");
    tree.set_height(fixed, Length::points(14.0))
        .expect("set fixed height");
    tree.set_margin(fixed, StandaloneEdge::Top, Length::calc(4.0, 9.0))
        .expect("set fixed top margin");
    tree.set_margin(fixed, StandaloneEdge::Bottom, Length::calc(2.0, 6.0))
        .expect("set fixed bottom margin");

    tree.append_child(root, nested).expect("append nested");
    tree.append_child(nested, fixed).expect("append fixed");

    run_standalone_rust(tree, root, Constraints::definite(70.0, 160.0))
        .expect("linear fixed vertical-reverse center static position parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_fixed_vertical_reverse_top_static_position_with_percent_main_margins()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_linear_orientation(root, LinearOrientation::VerticalReverse)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::Top)
        .expect("set root gravity");
    tree.set_width(root, Length::points(70.0))
        .expect("set root width");
    tree.set_height(root, Length::points(160.0))
        .expect("set root height");

    let nested = tree.create_default_node();
    tree.set_display(nested, Display::Block)
        .expect("set nested display");
    tree.set_width(nested, Length::points(20.0))
        .expect("set nested width");
    tree.set_height(nested, Length::points(20.0))
        .expect("set nested height");

    let fixed = tree.create_default_node();
    tree.set_display(fixed, Display::Block)
        .expect("set fixed display");
    tree.set_position_type(fixed, PositionType::Fixed)
        .expect("set fixed position");
    tree.set_width(fixed, Length::points(24.0))
        .expect("set fixed width");
    tree.set_height(fixed, Length::points(14.0))
        .expect("set fixed height");
    tree.set_margin(fixed, StandaloneEdge::Top, Length::percent(5.0))
        .expect("set fixed top margin");
    tree.set_margin(fixed, StandaloneEdge::Bottom, Length::percent(3.0))
        .expect("set fixed bottom margin");

    tree.append_child(root, nested).expect("append nested");
    tree.append_child(nested, fixed).expect("append fixed");

    run_standalone_rust(tree, root, Constraints::definite(70.0, 160.0))
        .expect("linear fixed vertical-reverse physical top static position parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_fixed_vertical_reverse_end_static_position_with_calc_main_margins()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::VerticalReverse)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::End)
        .expect("set root gravity");
    tree.set_width(root, Length::points(78.0))
        .expect("set root width");
    tree.set_height(root, Length::points(172.0))
        .expect("set root height");

    let nested = tree.create_default_node();
    tree.set_display(nested, Display::Block)
        .expect("set nested display");
    tree.set_width(nested, Length::points(18.0))
        .expect("set nested width");
    tree.set_height(nested, Length::points(22.0))
        .expect("set nested height");

    let fixed = tree.create_default_node();
    tree.set_display(fixed, Display::Block)
        .expect("set fixed display");
    tree.set_box_sizing(fixed, BoxSizing::ContentBox)
        .expect("set fixed box sizing");
    tree.set_position_type(fixed, PositionType::Fixed)
        .expect("set fixed position");
    tree.set_width(fixed, Length::points(21.0))
        .expect("set fixed width");
    tree.set_height(fixed, Length::points(15.0))
        .expect("set fixed height");
    tree.set_margin(fixed, StandaloneEdge::Top, Length::calc(5.0, 7.0))
        .expect("set fixed top margin");
    tree.set_margin(fixed, StandaloneEdge::Bottom, Length::calc(3.0, 6.0))
        .expect("set fixed bottom margin");

    tree.append_child(root, nested).expect("append nested");
    tree.append_child(nested, fixed).expect("append fixed");

    run_standalone_rust(tree, root, Constraints::definite(78.0, 172.0))
        .expect("linear fixed vertical-reverse end static position parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_fixed_vertical_reverse_start_static_position_with_percent_main_margins()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::VerticalReverse)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::Start)
        .expect("set root gravity");
    tree.set_width(root, Length::points(80.0))
        .expect("set root width");
    tree.set_height(root, Length::points(176.0))
        .expect("set root height");

    let nested = tree.create_default_node();
    tree.set_display(nested, Display::Block)
        .expect("set nested display");
    tree.set_width(nested, Length::points(18.0))
        .expect("set nested width");
    tree.set_height(nested, Length::points(22.0))
        .expect("set nested height");

    let fixed = tree.create_default_node();
    tree.set_display(fixed, Display::Block)
        .expect("set fixed display");
    tree.set_box_sizing(fixed, BoxSizing::ContentBox)
        .expect("set fixed box sizing");
    tree.set_position_type(fixed, PositionType::Fixed)
        .expect("set fixed position");
    tree.set_width(fixed, Length::points(23.0))
        .expect("set fixed width");
    tree.set_height(fixed, Length::points(13.0))
        .expect("set fixed height");
    tree.set_margin(fixed, StandaloneEdge::Top, Length::percent(6.0))
        .expect("set fixed top margin");
    tree.set_margin(fixed, StandaloneEdge::Bottom, Length::percent(2.0))
        .expect("set fixed bottom margin");

    tree.append_child(root, nested).expect("append nested");
    tree.append_child(nested, fixed).expect("append fixed");

    run_standalone_rust(tree, root, Constraints::definite(80.0, 176.0))
        .expect("linear fixed vertical-reverse start static position parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_fixed_vertical_reverse_bottom_static_position_with_calc_main_margins()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::VerticalReverse)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::Bottom)
        .expect("set root gravity");
    tree.set_width(root, Length::points(82.0))
        .expect("set root width");
    tree.set_height(root, Length::points(180.0))
        .expect("set root height");

    let nested = tree.create_default_node();
    tree.set_display(nested, Display::Block)
        .expect("set nested display");
    tree.set_width(nested, Length::points(20.0))
        .expect("set nested width");
    tree.set_height(nested, Length::points(24.0))
        .expect("set nested height");

    let fixed = tree.create_default_node();
    tree.set_display(fixed, Display::Block)
        .expect("set fixed display");
    tree.set_box_sizing(fixed, BoxSizing::ContentBox)
        .expect("set fixed box sizing");
    tree.set_position_type(fixed, PositionType::Fixed)
        .expect("set fixed position");
    tree.set_width(fixed, Length::points(25.0))
        .expect("set fixed width");
    tree.set_height(fixed, Length::points(14.0))
        .expect("set fixed height");
    tree.set_margin(fixed, StandaloneEdge::Top, Length::calc(7.0, 4.0))
        .expect("set fixed top margin");
    tree.set_margin(fixed, StandaloneEdge::Bottom, Length::calc(2.0, 8.0))
        .expect("set fixed bottom margin");

    tree.append_child(root, nested).expect("append nested");
    tree.append_child(nested, fixed).expect("append fixed");

    run_standalone_rust(tree, root, Constraints::definite(82.0, 180.0))
        .expect("linear fixed vertical-reverse physical bottom static position parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_absolute_vertical_reverse_justify_flex_start_static_position_without_linear_gravity()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::VerticalReverse)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::None)
        .expect("set root gravity");
    tree.set_justify_content(root, JustifyContent::FlexStart)
        .expect("set root justify content");
    tree.set_width(root, Length::points(84.0))
        .expect("set root width");
    tree.set_height(root, Length::points(182.0))
        .expect("set root height");

    let absolute = tree.create_default_node();
    tree.set_display(absolute, Display::Block)
        .expect("set absolute display");
    tree.set_box_sizing(absolute, BoxSizing::ContentBox)
        .expect("set absolute box sizing");
    tree.set_position_type(absolute, PositionType::Absolute)
        .expect("set absolute position");
    tree.set_width(absolute, Length::points(25.0))
        .expect("set absolute width");
    tree.set_height(absolute, Length::points(14.0))
        .expect("set absolute height");
    tree.set_margin(absolute, StandaloneEdge::Top, Length::percent(4.0))
        .expect("set absolute top margin");
    tree.set_margin(absolute, StandaloneEdge::Bottom, Length::percent(6.0))
        .expect("set absolute bottom margin");
    tree.append_child(root, absolute)
        .expect("append absolute child");

    run_standalone_rust(tree, root, Constraints::definite(84.0, 182.0))
        .expect("linear absolute vertical-reverse justify flex-start static position parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_absolute_vertical_reverse_justify_start_static_position_without_linear_gravity()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::VerticalReverse)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::None)
        .expect("set root gravity");
    tree.set_justify_content(root, JustifyContent::Start)
        .expect("set root justify content");
    tree.set_width(root, Length::points(86.0))
        .expect("set root width");
    tree.set_height(root, Length::points(184.0))
        .expect("set root height");

    let absolute = tree.create_default_node();
    tree.set_display(absolute, Display::Block)
        .expect("set absolute display");
    tree.set_box_sizing(absolute, BoxSizing::ContentBox)
        .expect("set absolute box sizing");
    tree.set_position_type(absolute, PositionType::Absolute)
        .expect("set absolute position");
    tree.set_width(absolute, Length::points(27.0))
        .expect("set absolute width");
    tree.set_height(absolute, Length::points(15.0))
        .expect("set absolute height");
    tree.set_margin(absolute, StandaloneEdge::Top, Length::calc(2.0, 5.0))
        .expect("set absolute top margin");
    tree.set_margin(absolute, StandaloneEdge::Bottom, Length::calc(4.0, 3.0))
        .expect("set absolute bottom margin");
    tree.append_child(root, absolute)
        .expect("append absolute child");

    run_standalone_rust(tree, root, Constraints::definite(86.0, 184.0))
        .expect("linear absolute vertical-reverse justify start static position parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_absolute_vertical_reverse_justify_flex_end_static_position_without_linear_gravity()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::VerticalReverse)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::None)
        .expect("set root gravity");
    tree.set_justify_content(root, JustifyContent::FlexEnd)
        .expect("set root justify content");
    tree.set_width(root, Length::points(88.0))
        .expect("set root width");
    tree.set_height(root, Length::points(186.0))
        .expect("set root height");

    let absolute = tree.create_default_node();
    tree.set_display(absolute, Display::Block)
        .expect("set absolute display");
    tree.set_box_sizing(absolute, BoxSizing::ContentBox)
        .expect("set absolute box sizing");
    tree.set_position_type(absolute, PositionType::Absolute)
        .expect("set absolute position");
    tree.set_width(absolute, Length::points(29.0))
        .expect("set absolute width");
    tree.set_height(absolute, Length::points(16.0))
        .expect("set absolute height");
    tree.set_margin(absolute, StandaloneEdge::Top, Length::percent(6.0))
        .expect("set absolute top margin");
    tree.set_margin(absolute, StandaloneEdge::Bottom, Length::percent(4.0))
        .expect("set absolute bottom margin");
    tree.append_child(root, absolute)
        .expect("append absolute child");

    run_standalone_rust(tree, root, Constraints::definite(88.0, 186.0))
        .expect("linear absolute vertical-reverse justify flex-end static position parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_absolute_vertical_reverse_justify_end_static_position_without_linear_gravity()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::VerticalReverse)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::None)
        .expect("set root gravity");
    tree.set_justify_content(root, JustifyContent::End)
        .expect("set root justify content");
    tree.set_width(root, Length::points(90.0))
        .expect("set root width");
    tree.set_height(root, Length::points(188.0))
        .expect("set root height");

    let absolute = tree.create_default_node();
    tree.set_display(absolute, Display::Block)
        .expect("set absolute display");
    tree.set_box_sizing(absolute, BoxSizing::ContentBox)
        .expect("set absolute box sizing");
    tree.set_position_type(absolute, PositionType::Absolute)
        .expect("set absolute position");
    tree.set_width(absolute, Length::points(31.0))
        .expect("set absolute width");
    tree.set_height(absolute, Length::points(17.0))
        .expect("set absolute height");
    tree.set_margin(absolute, StandaloneEdge::Top, Length::calc(3.0, 4.0))
        .expect("set absolute top margin");
    tree.set_margin(absolute, StandaloneEdge::Bottom, Length::calc(5.0, 2.0))
        .expect("set absolute bottom margin");
    tree.append_child(root, absolute)
        .expect("append absolute child");

    run_standalone_rust(tree, root, Constraints::definite(90.0, 188.0))
        .expect("linear absolute vertical-reverse justify end static position parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_absolute_vertical_reverse_justify_center_static_position_without_linear_gravity()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::VerticalReverse)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::None)
        .expect("set root gravity");
    tree.set_justify_content(root, JustifyContent::Center)
        .expect("set root justify content");
    tree.set_width(root, Length::points(92.0))
        .expect("set root width");
    tree.set_height(root, Length::points(190.0))
        .expect("set root height");

    let absolute = tree.create_default_node();
    tree.set_display(absolute, Display::Block)
        .expect("set absolute display");
    tree.set_box_sizing(absolute, BoxSizing::ContentBox)
        .expect("set absolute box sizing");
    tree.set_position_type(absolute, PositionType::Absolute)
        .expect("set absolute position");
    tree.set_width(absolute, Length::points(33.0))
        .expect("set absolute width");
    tree.set_height(absolute, Length::points(18.0))
        .expect("set absolute height");
    tree.set_margin(absolute, StandaloneEdge::Top, Length::percent(5.0))
        .expect("set absolute top margin");
    tree.set_margin(absolute, StandaloneEdge::Bottom, Length::percent(3.0))
        .expect("set absolute bottom margin");
    tree.append_child(root, absolute)
        .expect("append absolute child");

    run_standalone_rust(tree, root, Constraints::definite(92.0, 190.0))
        .expect("linear absolute vertical-reverse justify center static position parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_absolute_vertical_reverse_justify_space_between_static_start_without_linear_gravity()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::VerticalReverse)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::None)
        .expect("set root gravity");
    tree.set_justify_content(root, JustifyContent::SpaceBetween)
        .expect("set root justify content");
    tree.set_width(root, Length::points(94.0))
        .expect("set root width");
    tree.set_height(root, Length::points(192.0))
        .expect("set root height");

    let absolute = tree.create_default_node();
    tree.set_display(absolute, Display::Block)
        .expect("set absolute display");
    tree.set_box_sizing(absolute, BoxSizing::ContentBox)
        .expect("set absolute box sizing");
    tree.set_position_type(absolute, PositionType::Absolute)
        .expect("set absolute position");
    tree.set_width(absolute, Length::points(35.0))
        .expect("set absolute width");
    tree.set_height(absolute, Length::points(19.0))
        .expect("set absolute height");
    tree.set_margin(absolute, StandaloneEdge::Top, Length::calc(4.0, 3.0))
        .expect("set absolute top margin");
    tree.set_margin(absolute, StandaloneEdge::Bottom, Length::calc(2.0, 6.0))
        .expect("set absolute bottom margin");
    tree.append_child(root, absolute)
        .expect("append absolute child");

    run_standalone_rust(tree, root, Constraints::definite(94.0, 192.0))
        .expect("linear absolute vertical-reverse justify space-between static start parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_absolute_vertical_reverse_justify_space_around_static_start_without_linear_gravity()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::VerticalReverse)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::None)
        .expect("set root gravity");
    tree.set_justify_content(root, JustifyContent::SpaceAround)
        .expect("set root justify content");
    tree.set_width(root, Length::points(96.0))
        .expect("set root width");
    tree.set_height(root, Length::points(194.0))
        .expect("set root height");

    let absolute = tree.create_default_node();
    tree.set_display(absolute, Display::Block)
        .expect("set absolute display");
    tree.set_box_sizing(absolute, BoxSizing::ContentBox)
        .expect("set absolute box sizing");
    tree.set_position_type(absolute, PositionType::Absolute)
        .expect("set absolute position");
    tree.set_width(absolute, Length::points(37.0))
        .expect("set absolute width");
    tree.set_height(absolute, Length::points(20.0))
        .expect("set absolute height");
    tree.set_margin(absolute, StandaloneEdge::Top, Length::percent(6.0))
        .expect("set absolute top margin");
    tree.set_margin(absolute, StandaloneEdge::Bottom, Length::percent(4.0))
        .expect("set absolute bottom margin");
    tree.append_child(root, absolute)
        .expect("append absolute child");

    run_standalone_rust(tree, root, Constraints::definite(96.0, 194.0))
        .expect("linear absolute vertical-reverse justify space-around static start parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_absolute_vertical_reverse_justify_space_evenly_static_start_without_linear_gravity()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::VerticalReverse)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::None)
        .expect("set root gravity");
    tree.set_justify_content(root, JustifyContent::SpaceEvenly)
        .expect("set root justify content");
    tree.set_width(root, Length::points(98.0))
        .expect("set root width");
    tree.set_height(root, Length::points(196.0))
        .expect("set root height");

    let absolute = tree.create_default_node();
    tree.set_display(absolute, Display::Block)
        .expect("set absolute display");
    tree.set_box_sizing(absolute, BoxSizing::ContentBox)
        .expect("set absolute box sizing");
    tree.set_position_type(absolute, PositionType::Absolute)
        .expect("set absolute position");
    tree.set_width(absolute, Length::points(39.0))
        .expect("set absolute width");
    tree.set_height(absolute, Length::points(21.0))
        .expect("set absolute height");
    tree.set_margin(absolute, StandaloneEdge::Top, Length::calc(5.0, 2.0))
        .expect("set absolute top margin");
    tree.set_margin(absolute, StandaloneEdge::Bottom, Length::calc(3.0, 4.0))
        .expect("set absolute bottom margin");
    tree.append_child(root, absolute)
        .expect("append absolute child");

    run_standalone_rust(tree, root, Constraints::definite(98.0, 196.0))
        .expect("linear absolute vertical-reverse justify space-evenly static start parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_absolute_vertical_reverse_justify_stretch_static_start_without_linear_gravity()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::VerticalReverse)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::None)
        .expect("set root gravity");
    tree.set_justify_content(root, JustifyContent::Stretch)
        .expect("set root justify content");
    tree.set_width(root, Length::points(100.0))
        .expect("set root width");
    tree.set_height(root, Length::points(198.0))
        .expect("set root height");

    let absolute = tree.create_default_node();
    tree.set_display(absolute, Display::Block)
        .expect("set absolute display");
    tree.set_box_sizing(absolute, BoxSizing::ContentBox)
        .expect("set absolute box sizing");
    tree.set_position_type(absolute, PositionType::Absolute)
        .expect("set absolute position");
    tree.set_width(absolute, Length::points(41.0))
        .expect("set absolute width");
    tree.set_height(absolute, Length::points(22.0))
        .expect("set absolute height");
    tree.set_margin(absolute, StandaloneEdge::Top, Length::percent(7.0))
        .expect("set absolute top margin");
    tree.set_margin(absolute, StandaloneEdge::Bottom, Length::percent(5.0))
        .expect("set absolute bottom margin");
    tree.append_child(root, absolute)
        .expect("append absolute child");

    run_standalone_rust(tree, root, Constraints::definite(100.0, 198.0))
        .expect("linear absolute vertical-reverse justify stretch static start parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_fixed_vertical_reverse_justify_flex_start_static_position_without_linear_gravity()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::VerticalReverse)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::None)
        .expect("set root gravity");
    tree.set_justify_content(root, JustifyContent::FlexStart)
        .expect("set root justify content");
    tree.set_width(root, Length::points(102.0))
        .expect("set root width");
    tree.set_height(root, Length::points(200.0))
        .expect("set root height");

    let nested = tree.create_default_node();
    tree.set_display(nested, Display::Block)
        .expect("set nested display");
    tree.set_width(nested, Length::points(24.0))
        .expect("set nested width");
    tree.set_height(nested, Length::points(20.0))
        .expect("set nested height");

    let fixed = tree.create_default_node();
    tree.set_display(fixed, Display::Block)
        .expect("set fixed display");
    tree.set_box_sizing(fixed, BoxSizing::ContentBox)
        .expect("set fixed box sizing");
    tree.set_position_type(fixed, PositionType::Fixed)
        .expect("set fixed position");
    tree.set_width(fixed, Length::points(26.0))
        .expect("set fixed width");
    tree.set_height(fixed, Length::points(14.0))
        .expect("set fixed height");
    tree.set_margin(fixed, StandaloneEdge::Top, Length::percent(4.0))
        .expect("set fixed top margin");
    tree.set_margin(fixed, StandaloneEdge::Bottom, Length::percent(6.0))
        .expect("set fixed bottom margin");

    tree.append_child(root, nested).expect("append nested");
    tree.append_child(nested, fixed).expect("append fixed");

    run_standalone_rust(tree, root, Constraints::definite(102.0, 200.0))
        .expect("linear fixed vertical-reverse justify flex-start static position parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_fixed_vertical_reverse_justify_start_static_position_without_linear_gravity()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::VerticalReverse)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::None)
        .expect("set root gravity");
    tree.set_justify_content(root, JustifyContent::Start)
        .expect("set root justify content");
    tree.set_width(root, Length::points(104.0))
        .expect("set root width");
    tree.set_height(root, Length::points(202.0))
        .expect("set root height");

    let nested = tree.create_default_node();
    tree.set_display(nested, Display::Block)
        .expect("set nested display");
    tree.set_width(nested, Length::points(26.0))
        .expect("set nested width");
    tree.set_height(nested, Length::points(21.0))
        .expect("set nested height");

    let fixed = tree.create_default_node();
    tree.set_display(fixed, Display::Block)
        .expect("set fixed display");
    tree.set_box_sizing(fixed, BoxSizing::ContentBox)
        .expect("set fixed box sizing");
    tree.set_position_type(fixed, PositionType::Fixed)
        .expect("set fixed position");
    tree.set_width(fixed, Length::points(28.0))
        .expect("set fixed width");
    tree.set_height(fixed, Length::points(15.0))
        .expect("set fixed height");
    tree.set_margin(fixed, StandaloneEdge::Top, Length::calc(2.0, 5.0))
        .expect("set fixed top margin");
    tree.set_margin(fixed, StandaloneEdge::Bottom, Length::calc(4.0, 3.0))
        .expect("set fixed bottom margin");

    tree.append_child(root, nested).expect("append nested");
    tree.append_child(nested, fixed).expect("append fixed");

    run_standalone_rust(tree, root, Constraints::definite(104.0, 202.0))
        .expect("linear fixed vertical-reverse justify start static position parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_fixed_vertical_reverse_justify_flex_end_static_position_without_linear_gravity()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::VerticalReverse)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::None)
        .expect("set root gravity");
    tree.set_justify_content(root, JustifyContent::FlexEnd)
        .expect("set root justify content");
    tree.set_width(root, Length::points(106.0))
        .expect("set root width");
    tree.set_height(root, Length::points(204.0))
        .expect("set root height");

    let nested = tree.create_default_node();
    tree.set_display(nested, Display::Block)
        .expect("set nested display");
    tree.set_width(nested, Length::points(28.0))
        .expect("set nested width");
    tree.set_height(nested, Length::points(22.0))
        .expect("set nested height");

    let fixed = tree.create_default_node();
    tree.set_display(fixed, Display::Block)
        .expect("set fixed display");
    tree.set_box_sizing(fixed, BoxSizing::ContentBox)
        .expect("set fixed box sizing");
    tree.set_position_type(fixed, PositionType::Fixed)
        .expect("set fixed position");
    tree.set_width(fixed, Length::points(30.0))
        .expect("set fixed width");
    tree.set_height(fixed, Length::points(16.0))
        .expect("set fixed height");
    tree.set_margin(fixed, StandaloneEdge::Top, Length::percent(6.0))
        .expect("set fixed top margin");
    tree.set_margin(fixed, StandaloneEdge::Bottom, Length::percent(4.0))
        .expect("set fixed bottom margin");

    tree.append_child(root, nested).expect("append nested");
    tree.append_child(nested, fixed).expect("append fixed");

    run_standalone_rust(tree, root, Constraints::definite(106.0, 204.0))
        .expect("linear fixed vertical-reverse justify flex-end static position parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_fixed_vertical_reverse_justify_end_static_position_without_linear_gravity()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::VerticalReverse)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::None)
        .expect("set root gravity");
    tree.set_justify_content(root, JustifyContent::End)
        .expect("set root justify content");
    tree.set_width(root, Length::points(108.0))
        .expect("set root width");
    tree.set_height(root, Length::points(206.0))
        .expect("set root height");

    let nested = tree.create_default_node();
    tree.set_display(nested, Display::Block)
        .expect("set nested display");
    tree.set_width(nested, Length::points(30.0))
        .expect("set nested width");
    tree.set_height(nested, Length::points(23.0))
        .expect("set nested height");

    let fixed = tree.create_default_node();
    tree.set_display(fixed, Display::Block)
        .expect("set fixed display");
    tree.set_box_sizing(fixed, BoxSizing::ContentBox)
        .expect("set fixed box sizing");
    tree.set_position_type(fixed, PositionType::Fixed)
        .expect("set fixed position");
    tree.set_width(fixed, Length::points(32.0))
        .expect("set fixed width");
    tree.set_height(fixed, Length::points(17.0))
        .expect("set fixed height");
    tree.set_margin(fixed, StandaloneEdge::Top, Length::calc(3.0, 4.0))
        .expect("set fixed top margin");
    tree.set_margin(fixed, StandaloneEdge::Bottom, Length::calc(5.0, 2.0))
        .expect("set fixed bottom margin");

    tree.append_child(root, nested).expect("append nested");
    tree.append_child(nested, fixed).expect("append fixed");

    run_standalone_rust(tree, root, Constraints::definite(108.0, 206.0))
        .expect("linear fixed vertical-reverse justify end static position parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_fixed_vertical_reverse_justify_center_static_position_without_linear_gravity()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::VerticalReverse)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::None)
        .expect("set root gravity");
    tree.set_justify_content(root, JustifyContent::Center)
        .expect("set root justify content");
    tree.set_width(root, Length::points(110.0))
        .expect("set root width");
    tree.set_height(root, Length::points(208.0))
        .expect("set root height");

    let nested = tree.create_default_node();
    tree.set_display(nested, Display::Block)
        .expect("set nested display");
    tree.set_width(nested, Length::points(32.0))
        .expect("set nested width");
    tree.set_height(nested, Length::points(24.0))
        .expect("set nested height");

    let fixed = tree.create_default_node();
    tree.set_display(fixed, Display::Block)
        .expect("set fixed display");
    tree.set_box_sizing(fixed, BoxSizing::ContentBox)
        .expect("set fixed box sizing");
    tree.set_position_type(fixed, PositionType::Fixed)
        .expect("set fixed position");
    tree.set_width(fixed, Length::points(34.0))
        .expect("set fixed width");
    tree.set_height(fixed, Length::points(18.0))
        .expect("set fixed height");
    tree.set_margin(fixed, StandaloneEdge::Top, Length::percent(5.0))
        .expect("set fixed top margin");
    tree.set_margin(fixed, StandaloneEdge::Bottom, Length::percent(3.0))
        .expect("set fixed bottom margin");

    tree.append_child(root, nested).expect("append nested");
    tree.append_child(nested, fixed).expect("append fixed");

    run_standalone_rust(tree, root, Constraints::definite(110.0, 208.0))
        .expect("linear fixed vertical-reverse justify center static position parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_fixed_vertical_reverse_justify_space_between_static_start_without_linear_gravity()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::VerticalReverse)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::None)
        .expect("set root gravity");
    tree.set_justify_content(root, JustifyContent::SpaceBetween)
        .expect("set root justify content");
    tree.set_width(root, Length::points(112.0))
        .expect("set root width");
    tree.set_height(root, Length::points(210.0))
        .expect("set root height");

    let nested = tree.create_default_node();
    tree.set_display(nested, Display::Block)
        .expect("set nested display");
    tree.set_width(nested, Length::points(34.0))
        .expect("set nested width");
    tree.set_height(nested, Length::points(25.0))
        .expect("set nested height");

    let fixed = tree.create_default_node();
    tree.set_display(fixed, Display::Block)
        .expect("set fixed display");
    tree.set_box_sizing(fixed, BoxSizing::ContentBox)
        .expect("set fixed box sizing");
    tree.set_position_type(fixed, PositionType::Fixed)
        .expect("set fixed position");
    tree.set_width(fixed, Length::points(36.0))
        .expect("set fixed width");
    tree.set_height(fixed, Length::points(19.0))
        .expect("set fixed height");
    tree.set_margin(fixed, StandaloneEdge::Top, Length::calc(4.0, 3.0))
        .expect("set fixed top margin");
    tree.set_margin(fixed, StandaloneEdge::Bottom, Length::calc(2.0, 6.0))
        .expect("set fixed bottom margin");

    tree.append_child(root, nested).expect("append nested");
    tree.append_child(nested, fixed).expect("append fixed");

    run_standalone_rust(tree, root, Constraints::definite(112.0, 210.0))
        .expect("linear fixed vertical-reverse justify space-between static start parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_fixed_vertical_reverse_justify_space_around_static_start_without_linear_gravity()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::VerticalReverse)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::None)
        .expect("set root gravity");
    tree.set_justify_content(root, JustifyContent::SpaceAround)
        .expect("set root justify content");
    tree.set_width(root, Length::points(114.0))
        .expect("set root width");
    tree.set_height(root, Length::points(212.0))
        .expect("set root height");

    let nested = tree.create_default_node();
    tree.set_display(nested, Display::Block)
        .expect("set nested display");
    tree.set_width(nested, Length::points(36.0))
        .expect("set nested width");
    tree.set_height(nested, Length::points(26.0))
        .expect("set nested height");

    let fixed = tree.create_default_node();
    tree.set_display(fixed, Display::Block)
        .expect("set fixed display");
    tree.set_box_sizing(fixed, BoxSizing::ContentBox)
        .expect("set fixed box sizing");
    tree.set_position_type(fixed, PositionType::Fixed)
        .expect("set fixed position");
    tree.set_width(fixed, Length::points(38.0))
        .expect("set fixed width");
    tree.set_height(fixed, Length::points(20.0))
        .expect("set fixed height");
    tree.set_margin(fixed, StandaloneEdge::Top, Length::percent(6.0))
        .expect("set fixed top margin");
    tree.set_margin(fixed, StandaloneEdge::Bottom, Length::percent(4.0))
        .expect("set fixed bottom margin");

    tree.append_child(root, nested).expect("append nested");
    tree.append_child(nested, fixed).expect("append fixed");

    run_standalone_rust(tree, root, Constraints::definite(114.0, 212.0))
        .expect("linear fixed vertical-reverse justify space-around static start parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_fixed_vertical_reverse_justify_space_evenly_static_start_without_linear_gravity()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::VerticalReverse)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::None)
        .expect("set root gravity");
    tree.set_justify_content(root, JustifyContent::SpaceEvenly)
        .expect("set root justify content");
    tree.set_width(root, Length::points(116.0))
        .expect("set root width");
    tree.set_height(root, Length::points(214.0))
        .expect("set root height");

    let nested = tree.create_default_node();
    tree.set_display(nested, Display::Block)
        .expect("set nested display");
    tree.set_width(nested, Length::points(38.0))
        .expect("set nested width");
    tree.set_height(nested, Length::points(27.0))
        .expect("set nested height");

    let fixed = tree.create_default_node();
    tree.set_display(fixed, Display::Block)
        .expect("set fixed display");
    tree.set_box_sizing(fixed, BoxSizing::ContentBox)
        .expect("set fixed box sizing");
    tree.set_position_type(fixed, PositionType::Fixed)
        .expect("set fixed position");
    tree.set_width(fixed, Length::points(40.0))
        .expect("set fixed width");
    tree.set_height(fixed, Length::points(21.0))
        .expect("set fixed height");
    tree.set_margin(fixed, StandaloneEdge::Top, Length::calc(5.0, 2.0))
        .expect("set fixed top margin");
    tree.set_margin(fixed, StandaloneEdge::Bottom, Length::calc(3.0, 4.0))
        .expect("set fixed bottom margin");

    tree.append_child(root, nested).expect("append nested");
    tree.append_child(nested, fixed).expect("append fixed");

    run_standalone_rust(tree, root, Constraints::definite(116.0, 214.0))
        .expect("linear fixed vertical-reverse justify space-evenly static start parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_fixed_vertical_reverse_justify_stretch_static_start_without_linear_gravity()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::VerticalReverse)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::None)
        .expect("set root gravity");
    tree.set_justify_content(root, JustifyContent::Stretch)
        .expect("set root justify content");
    tree.set_width(root, Length::points(118.0))
        .expect("set root width");
    tree.set_height(root, Length::points(216.0))
        .expect("set root height");

    let nested = tree.create_default_node();
    tree.set_display(nested, Display::Block)
        .expect("set nested display");
    tree.set_width(nested, Length::points(40.0))
        .expect("set nested width");
    tree.set_height(nested, Length::points(28.0))
        .expect("set nested height");

    let fixed = tree.create_default_node();
    tree.set_display(fixed, Display::Block)
        .expect("set fixed display");
    tree.set_box_sizing(fixed, BoxSizing::ContentBox)
        .expect("set fixed box sizing");
    tree.set_position_type(fixed, PositionType::Fixed)
        .expect("set fixed position");
    tree.set_width(fixed, Length::points(42.0))
        .expect("set fixed width");
    tree.set_height(fixed, Length::points(22.0))
        .expect("set fixed height");
    tree.set_margin(fixed, StandaloneEdge::Top, Length::percent(7.0))
        .expect("set fixed top margin");
    tree.set_margin(fixed, StandaloneEdge::Bottom, Length::percent(5.0))
        .expect("set fixed bottom margin");

    tree.append_child(root, nested).expect("append nested");
    tree.append_child(nested, fixed).expect("append fixed");

    run_standalone_rust(tree, root, Constraints::definite(118.0, 216.0))
        .expect("linear fixed vertical-reverse justify stretch static start parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_fixed_vertical_center_static_position() {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Vertical)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::Center)
        .expect("set root gravity");
    tree.set_width(root, Length::points(50.0))
        .expect("set root width");
    tree.set_height(root, Length::points(100.0))
        .expect("set root height");

    let nested = tree.create_default_node();
    tree.set_display(nested, Display::Block)
        .expect("set nested display");
    tree.set_box_sizing(nested, BoxSizing::ContentBox)
        .expect("set nested box sizing");
    tree.set_width(nested, Length::points(20.0))
        .expect("set nested width");
    tree.set_height(nested, Length::points(20.0))
        .expect("set nested height");

    let fixed = tree.create_default_node();
    tree.set_display(fixed, Display::Block)
        .expect("set fixed display");
    tree.set_box_sizing(fixed, BoxSizing::ContentBox)
        .expect("set fixed box sizing");
    tree.set_position_type(fixed, PositionType::Fixed)
        .expect("set fixed position");
    tree.set_width(fixed, Length::points(20.0))
        .expect("set fixed width");
    tree.set_height(fixed, Length::points(10.0))
        .expect("set fixed height");

    tree.append_child(root, nested).expect("append nested");
    tree.append_child(nested, fixed).expect("append fixed");

    run_standalone_rust(tree, root, Constraints::definite(50.0, 100.0))
        .expect("linear fixed vertical center static-position parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_fixed_vertical_end_static_position() {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Vertical)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::End)
        .expect("set root gravity");
    tree.set_width(root, Length::points(50.0))
        .expect("set root width");
    tree.set_height(root, Length::points(100.0))
        .expect("set root height");

    let nested = tree.create_default_node();
    tree.set_display(nested, Display::Block)
        .expect("set nested display");
    tree.set_box_sizing(nested, BoxSizing::ContentBox)
        .expect("set nested box sizing");
    tree.set_width(nested, Length::points(20.0))
        .expect("set nested width");
    tree.set_height(nested, Length::points(20.0))
        .expect("set nested height");

    let fixed = tree.create_default_node();
    tree.set_display(fixed, Display::Block)
        .expect("set fixed display");
    tree.set_box_sizing(fixed, BoxSizing::ContentBox)
        .expect("set fixed box sizing");
    tree.set_position_type(fixed, PositionType::Fixed)
        .expect("set fixed position");
    tree.set_width(fixed, Length::points(20.0))
        .expect("set fixed width");
    tree.set_height(fixed, Length::points(10.0))
        .expect("set fixed height");

    tree.append_child(root, nested).expect("append nested");
    tree.append_child(nested, fixed).expect("append fixed");

    run_standalone_rust(tree, root, Constraints::definite(50.0, 100.0))
        .expect("linear fixed vertical end static-position parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_fixed_vertical_physical_bottom_static_position() {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Vertical)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::Bottom)
        .expect("set root gravity");
    tree.set_width(root, Length::points(50.0))
        .expect("set root width");
    tree.set_height(root, Length::points(100.0))
        .expect("set root height");

    let nested = tree.create_default_node();
    tree.set_display(nested, Display::Block)
        .expect("set nested display");
    tree.set_box_sizing(nested, BoxSizing::ContentBox)
        .expect("set nested box sizing");
    tree.set_width(nested, Length::points(20.0))
        .expect("set nested width");
    tree.set_height(nested, Length::points(20.0))
        .expect("set nested height");

    let fixed = tree.create_default_node();
    tree.set_display(fixed, Display::Block)
        .expect("set fixed display");
    tree.set_box_sizing(fixed, BoxSizing::ContentBox)
        .expect("set fixed box sizing");
    tree.set_position_type(fixed, PositionType::Fixed)
        .expect("set fixed position");
    tree.set_width(fixed, Length::points(20.0))
        .expect("set fixed width");
    tree.set_height(fixed, Length::points(10.0))
        .expect("set fixed height");

    tree.append_child(root, nested).expect("append nested");
    tree.append_child(nested, fixed).expect("append fixed");

    run_standalone_rust(tree, root, Constraints::definite(50.0, 100.0))
        .expect("linear fixed vertical physical bottom static-position parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_fixed_vertical_physical_top_static_position() {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Vertical)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::Top)
        .expect("set root gravity");
    tree.set_width(root, Length::points(50.0))
        .expect("set root width");
    tree.set_height(root, Length::points(100.0))
        .expect("set root height");

    let nested = tree.create_default_node();
    tree.set_display(nested, Display::Block)
        .expect("set nested display");
    tree.set_box_sizing(nested, BoxSizing::ContentBox)
        .expect("set nested box sizing");
    tree.set_width(nested, Length::points(20.0))
        .expect("set nested width");
    tree.set_height(nested, Length::points(20.0))
        .expect("set nested height");

    let fixed = tree.create_default_node();
    tree.set_display(fixed, Display::Block)
        .expect("set fixed display");
    tree.set_box_sizing(fixed, BoxSizing::ContentBox)
        .expect("set fixed box sizing");
    tree.set_position_type(fixed, PositionType::Fixed)
        .expect("set fixed position");
    tree.set_width(fixed, Length::points(20.0))
        .expect("set fixed width");
    tree.set_height(fixed, Length::points(10.0))
        .expect("set fixed height");

    tree.append_child(root, nested).expect("append nested");
    tree.append_child(nested, fixed).expect("append fixed");

    run_standalone_rust(tree, root, Constraints::definite(50.0, 100.0))
        .expect("linear fixed vertical physical top static-position parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_fixed_start_insets_override_static_alignment() {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::Center)
        .expect("set root gravity");
    tree.set_width(root, Length::points(200.0))
        .expect("set root width");
    tree.set_height(root, Length::points(100.0))
        .expect("set root height");

    let nested = tree.create_default_node();
    tree.set_display(nested, Display::Block)
        .expect("set nested display");
    tree.set_box_sizing(nested, BoxSizing::ContentBox)
        .expect("set nested box sizing");
    tree.set_width(nested, Length::points(40.0))
        .expect("set nested width");
    tree.set_height(nested, Length::points(30.0))
        .expect("set nested height");

    let fixed = tree.create_default_node();
    tree.set_display(fixed, Display::Block)
        .expect("set fixed display");
    tree.set_box_sizing(fixed, BoxSizing::ContentBox)
        .expect("set fixed box sizing");
    tree.set_position_type(fixed, PositionType::Fixed)
        .expect("set fixed position");
    tree.set_linear_layout_gravity(fixed, LinearLayoutGravity::End)
        .expect("set fixed layout gravity");
    tree.set_position(fixed, StandaloneEdge::Left, Length::points(12.0))
        .expect("set fixed left");
    tree.set_position(fixed, StandaloneEdge::Top, Length::points(9.0))
        .expect("set fixed top");
    tree.set_width(fixed, Length::points(20.0))
        .expect("set fixed width");
    tree.set_height(fixed, Length::points(10.0))
        .expect("set fixed height");

    tree.append_child(root, nested).expect("append nested");
    tree.append_child(nested, fixed).expect("append fixed");

    run_standalone_rust(tree, root, Constraints::definite(200.0, 100.0))
        .expect("linear fixed start insets override static alignment parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_fixed_start_insets_with_margins() {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::Center)
        .expect("set root gravity");
    tree.set_width(root, Length::points(200.0))
        .expect("set root width");
    tree.set_height(root, Length::points(100.0))
        .expect("set root height");

    let nested = tree.create_default_node();
    tree.set_display(nested, Display::Block)
        .expect("set nested display");
    tree.set_box_sizing(nested, BoxSizing::ContentBox)
        .expect("set nested box sizing");
    tree.set_width(nested, Length::points(40.0))
        .expect("set nested width");
    tree.set_height(nested, Length::points(30.0))
        .expect("set nested height");

    let fixed = tree.create_default_node();
    tree.set_display(fixed, Display::Block)
        .expect("set fixed display");
    tree.set_box_sizing(fixed, BoxSizing::ContentBox)
        .expect("set fixed box sizing");
    tree.set_position_type(fixed, PositionType::Fixed)
        .expect("set fixed position");
    tree.set_linear_layout_gravity(fixed, LinearLayoutGravity::End)
        .expect("set fixed layout gravity");
    tree.set_position(fixed, StandaloneEdge::Left, Length::points(12.0))
        .expect("set fixed left");
    tree.set_position(fixed, StandaloneEdge::Top, Length::points(9.0))
        .expect("set fixed top");
    tree.set_width(fixed, Length::points(20.0))
        .expect("set fixed width");
    tree.set_height(fixed, Length::points(10.0))
        .expect("set fixed height");
    tree.set_margin(fixed, StandaloneEdge::Left, Length::points(3.0))
        .expect("set fixed left margin");
    tree.set_margin(fixed, StandaloneEdge::Right, Length::points(7.0))
        .expect("set fixed right margin");
    tree.set_margin(fixed, StandaloneEdge::Top, Length::points(4.0))
        .expect("set fixed top margin");
    tree.set_margin(fixed, StandaloneEdge::Bottom, Length::points(6.0))
        .expect("set fixed bottom margin");

    tree.append_child(root, nested).expect("append nested");
    tree.append_child(nested, fixed).expect("append fixed");

    run_standalone_rust(tree, root, Constraints::definite(200.0, 100.0))
        .expect("linear fixed start insets with margins parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_fixed_paired_insets_explicit_size_start_wins() {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::Center)
        .expect("set root gravity");
    tree.set_width(root, Length::points(200.0))
        .expect("set root width");
    tree.set_height(root, Length::points(100.0))
        .expect("set root height");

    let nested = tree.create_default_node();
    tree.set_display(nested, Display::Block)
        .expect("set nested display");
    tree.set_box_sizing(nested, BoxSizing::ContentBox)
        .expect("set nested box sizing");
    tree.set_width(nested, Length::points(40.0))
        .expect("set nested width");
    tree.set_height(nested, Length::points(30.0))
        .expect("set nested height");

    let fixed = tree.create_default_node();
    tree.set_display(fixed, Display::Block)
        .expect("set fixed display");
    tree.set_box_sizing(fixed, BoxSizing::ContentBox)
        .expect("set fixed box sizing");
    tree.set_position_type(fixed, PositionType::Fixed)
        .expect("set fixed position");
    tree.set_linear_layout_gravity(fixed, LinearLayoutGravity::End)
        .expect("set fixed layout gravity");
    tree.set_position(fixed, StandaloneEdge::Left, Length::points(12.0))
        .expect("set fixed left");
    tree.set_position(fixed, StandaloneEdge::Right, Length::points(30.0))
        .expect("set fixed right");
    tree.set_position(fixed, StandaloneEdge::Top, Length::points(9.0))
        .expect("set fixed top");
    tree.set_position(fixed, StandaloneEdge::Bottom, Length::points(25.0))
        .expect("set fixed bottom");
    tree.set_width(fixed, Length::points(20.0))
        .expect("set fixed width");
    tree.set_height(fixed, Length::points(10.0))
        .expect("set fixed height");

    tree.append_child(root, nested).expect("append nested");
    tree.append_child(nested, fixed).expect("append fixed");

    run_standalone_rust(tree, root, Constraints::definite(200.0, 100.0))
        .expect("linear fixed paired insets explicit size start wins parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_fixed_paired_insets_explicit_size_with_margins_start_wins()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::Center)
        .expect("set root gravity");
    tree.set_width(root, Length::points(200.0))
        .expect("set root width");
    tree.set_height(root, Length::points(100.0))
        .expect("set root height");

    let nested = tree.create_default_node();
    tree.set_display(nested, Display::Block)
        .expect("set nested display");
    tree.set_box_sizing(nested, BoxSizing::ContentBox)
        .expect("set nested box sizing");
    tree.set_width(nested, Length::points(40.0))
        .expect("set nested width");
    tree.set_height(nested, Length::points(30.0))
        .expect("set nested height");

    let fixed = tree.create_default_node();
    tree.set_display(fixed, Display::Block)
        .expect("set fixed display");
    tree.set_box_sizing(fixed, BoxSizing::ContentBox)
        .expect("set fixed box sizing");
    tree.set_position_type(fixed, PositionType::Fixed)
        .expect("set fixed position");
    tree.set_linear_layout_gravity(fixed, LinearLayoutGravity::End)
        .expect("set fixed layout gravity");
    tree.set_position(fixed, StandaloneEdge::Left, Length::points(12.0))
        .expect("set fixed left");
    tree.set_position(fixed, StandaloneEdge::Right, Length::points(30.0))
        .expect("set fixed right");
    tree.set_position(fixed, StandaloneEdge::Top, Length::points(9.0))
        .expect("set fixed top");
    tree.set_position(fixed, StandaloneEdge::Bottom, Length::points(25.0))
        .expect("set fixed bottom");
    tree.set_width(fixed, Length::points(20.0))
        .expect("set fixed width");
    tree.set_height(fixed, Length::points(10.0))
        .expect("set fixed height");
    tree.set_margin(fixed, StandaloneEdge::Left, Length::points(3.0))
        .expect("set fixed left margin");
    tree.set_margin(fixed, StandaloneEdge::Right, Length::points(7.0))
        .expect("set fixed right margin");
    tree.set_margin(fixed, StandaloneEdge::Top, Length::points(4.0))
        .expect("set fixed top margin");
    tree.set_margin(fixed, StandaloneEdge::Bottom, Length::points(6.0))
        .expect("set fixed bottom margin");

    tree.append_child(root, nested).expect("append nested");
    tree.append_child(nested, fixed).expect("append fixed");

    run_standalone_rust(tree, root, Constraints::definite(200.0, 100.0))
        .expect("linear fixed paired insets explicit size with margins start wins parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_fixed_percent_paired_insets_explicit_size_start_wins()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::Center)
        .expect("set root gravity");
    tree.set_width(root, Length::points(200.0))
        .expect("set root width");
    tree.set_height(root, Length::points(100.0))
        .expect("set root height");

    let nested = tree.create_default_node();
    tree.set_display(nested, Display::Block)
        .expect("set nested display");
    tree.set_box_sizing(nested, BoxSizing::ContentBox)
        .expect("set nested box sizing");
    tree.set_width(nested, Length::points(40.0))
        .expect("set nested width");
    tree.set_height(nested, Length::points(30.0))
        .expect("set nested height");

    let fixed = tree.create_default_node();
    tree.set_display(fixed, Display::Block)
        .expect("set fixed display");
    tree.set_box_sizing(fixed, BoxSizing::ContentBox)
        .expect("set fixed box sizing");
    tree.set_position_type(fixed, PositionType::Fixed)
        .expect("set fixed position");
    tree.set_linear_layout_gravity(fixed, LinearLayoutGravity::End)
        .expect("set fixed layout gravity");
    tree.set_position(fixed, StandaloneEdge::Left, Length::percent(10.0))
        .expect("set fixed left");
    tree.set_position(fixed, StandaloneEdge::Right, Length::percent(20.0))
        .expect("set fixed right");
    tree.set_position(fixed, StandaloneEdge::Top, Length::percent(15.0))
        .expect("set fixed top");
    tree.set_position(fixed, StandaloneEdge::Bottom, Length::percent(25.0))
        .expect("set fixed bottom");
    tree.set_width(fixed, Length::points(20.0))
        .expect("set fixed width");
    tree.set_height(fixed, Length::points(10.0))
        .expect("set fixed height");

    tree.append_child(root, nested).expect("append nested");
    tree.append_child(nested, fixed).expect("append fixed");

    run_standalone_rust(tree, root, Constraints::definite(200.0, 100.0))
        .expect("linear fixed percent paired insets explicit size start wins parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_fixed_calc_start_insets_with_percent_margins() {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::Center)
        .expect("set root gravity");
    tree.set_width(root, Length::points(210.0))
        .expect("set root width");
    tree.set_height(root, Length::points(120.0))
        .expect("set root height");

    let nested = tree.create_default_node();
    tree.set_display(nested, Display::Block)
        .expect("set nested display");
    tree.set_box_sizing(nested, BoxSizing::ContentBox)
        .expect("set nested box sizing");
    tree.set_width(nested, Length::points(40.0))
        .expect("set nested width");
    tree.set_height(nested, Length::points(30.0))
        .expect("set nested height");

    let fixed = tree.create_default_node();
    tree.set_display(fixed, Display::Block)
        .expect("set fixed display");
    tree.set_box_sizing(fixed, BoxSizing::ContentBox)
        .expect("set fixed box sizing");
    tree.set_position_type(fixed, PositionType::Fixed)
        .expect("set fixed position");
    tree.set_linear_layout_gravity(fixed, LinearLayoutGravity::End)
        .expect("set fixed layout gravity");
    tree.set_position(fixed, StandaloneEdge::Left, Length::calc(6.0, 10.0))
        .expect("set fixed left");
    tree.set_position(fixed, StandaloneEdge::Top, Length::calc(4.0, 20.0))
        .expect("set fixed top");
    tree.set_width(fixed, Length::points(28.0))
        .expect("set fixed width");
    tree.set_height(fixed, Length::points(18.0))
        .expect("set fixed height");
    tree.set_margin(fixed, StandaloneEdge::Left, Length::percent(4.0))
        .expect("set fixed left margin");
    tree.set_margin(fixed, StandaloneEdge::Right, Length::percent(6.0))
        .expect("set fixed right margin");
    tree.set_margin(fixed, StandaloneEdge::Top, Length::percent(5.0))
        .expect("set fixed top margin");
    tree.set_margin(fixed, StandaloneEdge::Bottom, Length::percent(7.0))
        .expect("set fixed bottom margin");

    tree.append_child(root, nested).expect("append nested");
    tree.append_child(nested, fixed).expect("append fixed");

    run_standalone_rust(tree, root, Constraints::definite(210.0, 120.0))
        .expect("linear fixed calc start insets with percent margins parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_fixed_calc_paired_insets_explicit_size_start_wins()
{
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::Center)
        .expect("set root gravity");
    tree.set_width(root, Length::points(210.0))
        .expect("set root width");
    tree.set_height(root, Length::points(120.0))
        .expect("set root height");

    let nested = tree.create_default_node();
    tree.set_display(nested, Display::Block)
        .expect("set nested display");
    tree.set_box_sizing(nested, BoxSizing::ContentBox)
        .expect("set nested box sizing");
    tree.set_width(nested, Length::points(40.0))
        .expect("set nested width");
    tree.set_height(nested, Length::points(30.0))
        .expect("set nested height");

    let fixed = tree.create_default_node();
    tree.set_display(fixed, Display::Block)
        .expect("set fixed display");
    tree.set_box_sizing(fixed, BoxSizing::ContentBox)
        .expect("set fixed box sizing");
    tree.set_position_type(fixed, PositionType::Fixed)
        .expect("set fixed position");
    tree.set_linear_layout_gravity(fixed, LinearLayoutGravity::End)
        .expect("set fixed layout gravity");
    tree.set_position(fixed, StandaloneEdge::Left, Length::calc(6.0, 10.0))
        .expect("set fixed left");
    tree.set_position(fixed, StandaloneEdge::Right, Length::calc(8.0, 12.0))
        .expect("set fixed right");
    tree.set_position(fixed, StandaloneEdge::Top, Length::calc(4.0, 20.0))
        .expect("set fixed top");
    tree.set_position(fixed, StandaloneEdge::Bottom, Length::calc(3.0, 15.0))
        .expect("set fixed bottom");
    tree.set_width(fixed, Length::points(28.0))
        .expect("set fixed width");
    tree.set_height(fixed, Length::points(18.0))
        .expect("set fixed height");
    tree.set_margin(fixed, StandaloneEdge::Left, Length::points(2.0))
        .expect("set fixed left margin");
    tree.set_margin(fixed, StandaloneEdge::Right, Length::points(5.0))
        .expect("set fixed right margin");
    tree.set_margin(fixed, StandaloneEdge::Top, Length::points(3.0))
        .expect("set fixed top margin");
    tree.set_margin(fixed, StandaloneEdge::Bottom, Length::points(4.0))
        .expect("set fixed bottom margin");

    tree.append_child(root, nested).expect("append nested");
    tree.append_child(nested, fixed).expect("append fixed");

    run_standalone_rust(tree, root, Constraints::definite(210.0, 120.0))
        .expect("linear fixed calc paired insets explicit size start wins parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_fixed_auto_size_between_calc_insets_with_margins() {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::Center)
        .expect("set root gravity");
    tree.set_width(root, Length::points(220.0))
        .expect("set root width");
    tree.set_height(root, Length::points(130.0))
        .expect("set root height");

    let nested = tree.create_default_node();
    tree.set_display(nested, Display::Block)
        .expect("set nested display");
    tree.set_box_sizing(nested, BoxSizing::ContentBox)
        .expect("set nested box sizing");
    tree.set_width(nested, Length::points(40.0))
        .expect("set nested width");
    tree.set_height(nested, Length::points(30.0))
        .expect("set nested height");

    let fixed = tree.create_default_node();
    tree.set_display(fixed, Display::Block)
        .expect("set fixed display");
    tree.set_box_sizing(fixed, BoxSizing::ContentBox)
        .expect("set fixed box sizing");
    tree.set_position_type(fixed, PositionType::Fixed)
        .expect("set fixed position");
    tree.set_position(fixed, StandaloneEdge::Left, Length::calc(7.0, 5.0))
        .expect("set fixed left");
    tree.set_position(fixed, StandaloneEdge::Right, Length::calc(9.0, 10.0))
        .expect("set fixed right");
    tree.set_position(fixed, StandaloneEdge::Top, Length::calc(6.0, 8.0))
        .expect("set fixed top");
    tree.set_position(fixed, StandaloneEdge::Bottom, Length::calc(5.0, 12.0))
        .expect("set fixed bottom");
    tree.set_margin(fixed, StandaloneEdge::Left, Length::points(4.0))
        .expect("set fixed left margin");
    tree.set_margin(fixed, StandaloneEdge::Right, Length::points(6.0))
        .expect("set fixed right margin");
    tree.set_margin(fixed, StandaloneEdge::Top, Length::points(3.0))
        .expect("set fixed top margin");
    tree.set_margin(fixed, StandaloneEdge::Bottom, Length::points(7.0))
        .expect("set fixed bottom margin");

    tree.append_child(root, nested).expect("append nested");
    tree.append_child(nested, fixed).expect("append fixed");

    run_standalone_rust(tree, root, Constraints::definite(220.0, 130.0))
        .expect("linear fixed auto-size between calc insets with margins parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_fixed_calc_end_insets_with_percent_margins() {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::Center)
        .expect("set root gravity");
    tree.set_width(root, Length::points(210.0))
        .expect("set root width");
    tree.set_height(root, Length::points(120.0))
        .expect("set root height");

    let nested = tree.create_default_node();
    tree.set_display(nested, Display::Block)
        .expect("set nested display");
    tree.set_box_sizing(nested, BoxSizing::ContentBox)
        .expect("set nested box sizing");
    tree.set_width(nested, Length::points(40.0))
        .expect("set nested width");
    tree.set_height(nested, Length::points(30.0))
        .expect("set nested height");

    let fixed = tree.create_default_node();
    tree.set_display(fixed, Display::Block)
        .expect("set fixed display");
    tree.set_box_sizing(fixed, BoxSizing::ContentBox)
        .expect("set fixed box sizing");
    tree.set_position_type(fixed, PositionType::Fixed)
        .expect("set fixed position");
    tree.set_position(fixed, StandaloneEdge::Right, Length::calc(8.0, 12.0))
        .expect("set fixed right");
    tree.set_position(fixed, StandaloneEdge::Bottom, Length::calc(3.0, 15.0))
        .expect("set fixed bottom");
    tree.set_width(fixed, Length::points(28.0))
        .expect("set fixed width");
    tree.set_height(fixed, Length::points(18.0))
        .expect("set fixed height");
    tree.set_margin(fixed, StandaloneEdge::Left, Length::percent(3.0))
        .expect("set fixed left margin");
    tree.set_margin(fixed, StandaloneEdge::Right, Length::percent(7.0))
        .expect("set fixed right margin");
    tree.set_margin(fixed, StandaloneEdge::Top, Length::percent(4.0))
        .expect("set fixed top margin");
    tree.set_margin(fixed, StandaloneEdge::Bottom, Length::percent(6.0))
        .expect("set fixed bottom margin");

    tree.append_child(root, nested).expect("append nested");
    tree.append_child(nested, fixed).expect("append fixed");

    run_standalone_rust(tree, root, Constraints::definite(210.0, 120.0))
        .expect("linear fixed calc end insets with percent margins parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_fixed_auto_size_between_calc_insets_with_percent_margins()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::Center)
        .expect("set root gravity");
    tree.set_width(root, Length::points(220.0))
        .expect("set root width");
    tree.set_height(root, Length::points(130.0))
        .expect("set root height");

    let nested = tree.create_default_node();
    tree.set_display(nested, Display::Block)
        .expect("set nested display");
    tree.set_box_sizing(nested, BoxSizing::ContentBox)
        .expect("set nested box sizing");
    tree.set_width(nested, Length::points(40.0))
        .expect("set nested width");
    tree.set_height(nested, Length::points(30.0))
        .expect("set nested height");

    let fixed = tree.create_default_measured_node(Size::new(300.0, 160.0));
    tree.set_display(fixed, Display::Block)
        .expect("set fixed display");
    tree.set_box_sizing(fixed, BoxSizing::ContentBox)
        .expect("set fixed box sizing");
    tree.set_position_type(fixed, PositionType::Fixed)
        .expect("set fixed position");
    tree.set_position(fixed, StandaloneEdge::Left, Length::calc(7.0, 5.0))
        .expect("set fixed left");
    tree.set_position(fixed, StandaloneEdge::Right, Length::calc(9.0, 10.0))
        .expect("set fixed right");
    tree.set_position(fixed, StandaloneEdge::Top, Length::calc(6.0, 8.0))
        .expect("set fixed top");
    tree.set_position(fixed, StandaloneEdge::Bottom, Length::calc(5.0, 12.0))
        .expect("set fixed bottom");
    tree.set_margin(fixed, StandaloneEdge::Left, Length::percent(4.0))
        .expect("set fixed left margin");
    tree.set_margin(fixed, StandaloneEdge::Right, Length::percent(6.0))
        .expect("set fixed right margin");
    tree.set_margin(fixed, StandaloneEdge::Top, Length::percent(5.0))
        .expect("set fixed top margin");
    tree.set_margin(fixed, StandaloneEdge::Bottom, Length::percent(7.0))
        .expect("set fixed bottom margin");

    tree.append_child(root, nested).expect("append nested");
    tree.append_child(nested, fixed).expect("append fixed");

    run_standalone_rust(tree, root, Constraints::definite(220.0, 130.0))
        .expect("linear fixed auto-size between calc insets with percent margins parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_fixed_calc_end_insets_with_percent_margins_and_root_padding_border_origin()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::Center)
        .expect("set root gravity");
    tree.set_width(root, Length::points(214.0))
        .expect("set root width");
    tree.set_height(root, Length::points(126.0))
        .expect("set root height");
    tree.set_padding(root, StandaloneEdge::Left, Length::points(6.0))
        .expect("set root left padding");
    tree.set_padding(root, StandaloneEdge::Right, Length::points(8.0))
        .expect("set root right padding");
    tree.set_padding(root, StandaloneEdge::Top, Length::points(4.0))
        .expect("set root top padding");
    tree.set_padding(root, StandaloneEdge::Bottom, Length::points(5.0))
        .expect("set root bottom padding");
    tree.set_border(root, StandaloneEdge::Left, 2.0)
        .expect("set root left border");
    tree.set_border(root, StandaloneEdge::Right, 3.0)
        .expect("set root right border");
    tree.set_border(root, StandaloneEdge::Top, 1.0)
        .expect("set root top border");
    tree.set_border(root, StandaloneEdge::Bottom, 2.0)
        .expect("set root bottom border");

    let nested = tree.create_default_node();
    tree.set_display(nested, Display::Block)
        .expect("set nested display");
    tree.set_box_sizing(nested, BoxSizing::ContentBox)
        .expect("set nested box sizing");
    tree.set_width(nested, Length::points(46.0))
        .expect("set nested width");
    tree.set_height(nested, Length::points(32.0))
        .expect("set nested height");

    let fixed = tree.create_default_node();
    tree.set_display(fixed, Display::Block)
        .expect("set fixed display");
    tree.set_box_sizing(fixed, BoxSizing::ContentBox)
        .expect("set fixed box sizing");
    tree.set_position_type(fixed, PositionType::Fixed)
        .expect("set fixed position");
    tree.set_position(fixed, StandaloneEdge::Right, Length::calc(8.0, 12.0))
        .expect("set fixed right");
    tree.set_position(fixed, StandaloneEdge::Bottom, Length::calc(3.0, 15.0))
        .expect("set fixed bottom");
    tree.set_width(fixed, Length::points(28.0))
        .expect("set fixed width");
    tree.set_height(fixed, Length::points(18.0))
        .expect("set fixed height");
    tree.set_margin(fixed, StandaloneEdge::Left, Length::percent(3.0))
        .expect("set fixed left margin");
    tree.set_margin(fixed, StandaloneEdge::Right, Length::percent(7.0))
        .expect("set fixed right margin");
    tree.set_margin(fixed, StandaloneEdge::Top, Length::percent(4.0))
        .expect("set fixed top margin");
    tree.set_margin(fixed, StandaloneEdge::Bottom, Length::percent(6.0))
        .expect("set fixed bottom margin");

    tree.append_child(root, nested).expect("append nested");
    tree.append_child(nested, fixed).expect("append fixed");

    run_standalone_rust(tree, root, Constraints::definite(214.0, 126.0)).expect(
        "linear fixed calc end insets with percent margins and root padding/border origin parity",
    );
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_fixed_end_insets_override_static_alignment() {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::Center)
        .expect("set root gravity");
    tree.set_width(root, Length::points(200.0))
        .expect("set root width");
    tree.set_height(root, Length::points(100.0))
        .expect("set root height");

    let nested = tree.create_default_node();
    tree.set_display(nested, Display::Block)
        .expect("set nested display");
    tree.set_box_sizing(nested, BoxSizing::ContentBox)
        .expect("set nested box sizing");
    tree.set_width(nested, Length::points(40.0))
        .expect("set nested width");
    tree.set_height(nested, Length::points(30.0))
        .expect("set nested height");

    let fixed = tree.create_default_node();
    tree.set_display(fixed, Display::Block)
        .expect("set fixed display");
    tree.set_box_sizing(fixed, BoxSizing::ContentBox)
        .expect("set fixed box sizing");
    tree.set_position_type(fixed, PositionType::Fixed)
        .expect("set fixed position");
    tree.set_linear_layout_gravity(fixed, LinearLayoutGravity::End)
        .expect("set fixed layout gravity");
    tree.set_position(fixed, StandaloneEdge::Right, Length::points(30.0))
        .expect("set fixed right");
    tree.set_position(fixed, StandaloneEdge::Bottom, Length::points(25.0))
        .expect("set fixed bottom");
    tree.set_width(fixed, Length::points(20.0))
        .expect("set fixed width");
    tree.set_height(fixed, Length::points(10.0))
        .expect("set fixed height");

    tree.append_child(root, nested).expect("append nested");
    tree.append_child(nested, fixed).expect("append fixed");

    run_standalone_rust(tree, root, Constraints::definite(200.0, 100.0))
        .expect("linear fixed end insets override static alignment parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_fixed_end_insets_with_margins() {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::Center)
        .expect("set root gravity");
    tree.set_width(root, Length::points(200.0))
        .expect("set root width");
    tree.set_height(root, Length::points(100.0))
        .expect("set root height");

    let nested = tree.create_default_node();
    tree.set_display(nested, Display::Block)
        .expect("set nested display");
    tree.set_box_sizing(nested, BoxSizing::ContentBox)
        .expect("set nested box sizing");
    tree.set_width(nested, Length::points(40.0))
        .expect("set nested width");
    tree.set_height(nested, Length::points(30.0))
        .expect("set nested height");

    let fixed = tree.create_default_node();
    tree.set_display(fixed, Display::Block)
        .expect("set fixed display");
    tree.set_box_sizing(fixed, BoxSizing::ContentBox)
        .expect("set fixed box sizing");
    tree.set_position_type(fixed, PositionType::Fixed)
        .expect("set fixed position");
    tree.set_linear_layout_gravity(fixed, LinearLayoutGravity::End)
        .expect("set fixed layout gravity");
    tree.set_position(fixed, StandaloneEdge::Right, Length::points(30.0))
        .expect("set fixed right");
    tree.set_position(fixed, StandaloneEdge::Bottom, Length::points(25.0))
        .expect("set fixed bottom");
    tree.set_width(fixed, Length::points(20.0))
        .expect("set fixed width");
    tree.set_height(fixed, Length::points(10.0))
        .expect("set fixed height");
    tree.set_margin(fixed, StandaloneEdge::Left, Length::points(3.0))
        .expect("set fixed left margin");
    tree.set_margin(fixed, StandaloneEdge::Right, Length::points(7.0))
        .expect("set fixed right margin");
    tree.set_margin(fixed, StandaloneEdge::Top, Length::points(4.0))
        .expect("set fixed top margin");
    tree.set_margin(fixed, StandaloneEdge::Bottom, Length::points(6.0))
        .expect("set fixed bottom margin");

    tree.append_child(root, nested).expect("append nested");
    tree.append_child(nested, fixed).expect("append fixed");

    run_standalone_rust(tree, root, Constraints::definite(200.0, 100.0))
        .expect("linear fixed end insets with margins parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_fixed_start_insets_with_percent_margins() {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_width(root, Length::points(200.0))
        .expect("set root width");
    tree.set_height(root, Length::points(100.0))
        .expect("set root height");

    let nested = tree.create_default_node();
    tree.set_display(nested, Display::Block)
        .expect("set nested display");
    tree.set_width(nested, Length::points(40.0))
        .expect("set nested width");
    tree.set_height(nested, Length::points(30.0))
        .expect("set nested height");

    let fixed = fixed_standalone_block(&mut tree, 20.0, 10.0);
    tree.set_position_type(fixed, PositionType::Fixed)
        .expect("set fixed position");
    tree.set_position(fixed, StandaloneEdge::Left, Length::points(12.0))
        .expect("set fixed left");
    tree.set_position(fixed, StandaloneEdge::Top, Length::points(9.0))
        .expect("set fixed top");
    tree.set_margin(fixed, StandaloneEdge::Left, Length::percent(10.0))
        .expect("set fixed left margin");
    tree.set_margin(fixed, StandaloneEdge::Right, Length::percent(5.0))
        .expect("set fixed right margin");
    tree.set_margin(fixed, StandaloneEdge::Top, Length::percent(8.0))
        .expect("set fixed top margin");
    tree.set_margin(fixed, StandaloneEdge::Bottom, Length::percent(4.0))
        .expect("set fixed bottom margin");

    tree.append_child(root, nested).expect("append nested");
    tree.append_child(nested, fixed)
        .expect("append fixed child");

    run_standalone_rust(tree, root, Constraints::definite(200.0, 100.0))
        .expect("linear fixed start insets with percent margins parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_fixed_end_insets_with_percent_margins() {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_width(root, Length::points(200.0))
        .expect("set root width");
    tree.set_height(root, Length::points(100.0))
        .expect("set root height");

    let nested = tree.create_default_node();
    tree.set_display(nested, Display::Block)
        .expect("set nested display");
    tree.set_width(nested, Length::points(40.0))
        .expect("set nested width");
    tree.set_height(nested, Length::points(30.0))
        .expect("set nested height");

    let fixed = fixed_standalone_block(&mut tree, 20.0, 10.0);
    tree.set_position_type(fixed, PositionType::Fixed)
        .expect("set fixed position");
    tree.set_position(fixed, StandaloneEdge::Right, Length::points(30.0))
        .expect("set fixed right");
    tree.set_position(fixed, StandaloneEdge::Bottom, Length::points(25.0))
        .expect("set fixed bottom");
    tree.set_margin(fixed, StandaloneEdge::Left, Length::percent(3.0))
        .expect("set fixed left margin");
    tree.set_margin(fixed, StandaloneEdge::Right, Length::percent(7.0))
        .expect("set fixed right margin");
    tree.set_margin(fixed, StandaloneEdge::Top, Length::percent(4.0))
        .expect("set fixed top margin");
    tree.set_margin(fixed, StandaloneEdge::Bottom, Length::percent(6.0))
        .expect("set fixed bottom margin");

    tree.append_child(root, nested).expect("append nested");
    tree.append_child(nested, fixed)
        .expect("append fixed child");

    run_standalone_rust(tree, root, Constraints::definite(200.0, 100.0))
        .expect("linear fixed end insets with percent margins parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_fixed_auto_size_between_insets_with_percent_margins()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_width(root, Length::points(200.0))
        .expect("set root width");
    tree.set_height(root, Length::points(100.0))
        .expect("set root height");

    let nested = tree.create_default_node();
    tree.set_display(nested, Display::Block)
        .expect("set nested display");
    tree.set_width(nested, Length::points(40.0))
        .expect("set nested width");
    tree.set_height(nested, Length::points(30.0))
        .expect("set nested height");

    let fixed = tree.create_default_node();
    tree.set_display(fixed, Display::Block)
        .expect("set fixed display");
    tree.set_box_sizing(fixed, BoxSizing::ContentBox)
        .expect("set fixed box sizing");
    tree.set_position_type(fixed, PositionType::Fixed)
        .expect("set fixed position");
    tree.set_position(fixed, StandaloneEdge::Left, Length::points(10.0))
        .expect("set fixed left");
    tree.set_position(fixed, StandaloneEdge::Right, Length::points(30.0))
        .expect("set fixed right");
    tree.set_position(fixed, StandaloneEdge::Top, Length::points(20.0))
        .expect("set fixed top");
    tree.set_position(fixed, StandaloneEdge::Bottom, Length::points(25.0))
        .expect("set fixed bottom");
    tree.set_margin(fixed, StandaloneEdge::Left, Length::percent(4.0))
        .expect("set fixed left margin");
    tree.set_margin(fixed, StandaloneEdge::Right, Length::percent(6.0))
        .expect("set fixed right margin");
    tree.set_margin(fixed, StandaloneEdge::Top, Length::percent(5.0))
        .expect("set fixed top margin");
    tree.set_margin(fixed, StandaloneEdge::Bottom, Length::percent(7.0))
        .expect("set fixed bottom margin");

    tree.append_child(root, nested).expect("append nested");
    tree.append_child(nested, fixed)
        .expect("append fixed child");

    run_standalone_rust(tree, root, Constraints::definite(200.0, 100.0))
        .expect("linear fixed auto-size between insets with percent margins parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_fixed_percent_insets_and_size() {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_width(root, Length::points(200.0))
        .expect("set root width");
    tree.set_height(root, Length::points(100.0))
        .expect("set root height");

    let nested = tree.create_default_node();
    tree.set_display(nested, Display::Block)
        .expect("set nested display");
    tree.set_box_sizing(nested, BoxSizing::ContentBox)
        .expect("set nested box sizing");
    tree.set_width(nested, Length::points(40.0))
        .expect("set nested width");
    tree.set_height(nested, Length::points(30.0))
        .expect("set nested height");

    let fixed = tree.create_default_node();
    tree.set_display(fixed, Display::Block)
        .expect("set fixed display");
    tree.set_box_sizing(fixed, BoxSizing::ContentBox)
        .expect("set fixed box sizing");
    tree.set_position_type(fixed, PositionType::Fixed)
        .expect("set fixed position");
    tree.set_position(fixed, StandaloneEdge::Left, Length::percent(10.0))
        .expect("set fixed left");
    tree.set_position(fixed, StandaloneEdge::Top, Length::percent(25.0))
        .expect("set fixed top");
    tree.set_width(fixed, Length::percent(50.0))
        .expect("set fixed width");
    tree.set_height(fixed, Length::percent(20.0))
        .expect("set fixed height");

    tree.append_child(root, nested).expect("append nested");
    tree.append_child(nested, fixed).expect("append fixed");

    run_standalone_rust(tree, root, Constraints::definite(200.0, 100.0))
        .expect("linear fixed percent inset and size parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_fixed_percent_end_insets() {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_width(root, Length::points(200.0))
        .expect("set root width");
    tree.set_height(root, Length::points(100.0))
        .expect("set root height");

    let nested = tree.create_default_node();
    tree.set_display(nested, Display::Block)
        .expect("set nested display");
    tree.set_box_sizing(nested, BoxSizing::ContentBox)
        .expect("set nested box sizing");
    tree.set_width(nested, Length::points(40.0))
        .expect("set nested width");
    tree.set_height(nested, Length::points(30.0))
        .expect("set nested height");

    let fixed = tree.create_default_node();
    tree.set_display(fixed, Display::Block)
        .expect("set fixed display");
    tree.set_box_sizing(fixed, BoxSizing::ContentBox)
        .expect("set fixed box sizing");
    tree.set_position_type(fixed, PositionType::Fixed)
        .expect("set fixed position");
    tree.set_position(fixed, StandaloneEdge::Right, Length::percent(10.0))
        .expect("set fixed right");
    tree.set_position(fixed, StandaloneEdge::Bottom, Length::percent(25.0))
        .expect("set fixed bottom");
    tree.set_width(fixed, Length::percent(50.0))
        .expect("set fixed width");
    tree.set_height(fixed, Length::percent(20.0))
        .expect("set fixed height");

    tree.append_child(root, nested).expect("append nested");
    tree.append_child(nested, fixed).expect("append fixed");

    run_standalone_rust(tree, root, Constraints::definite(200.0, 100.0))
        .expect("linear fixed percent end inset parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_fixed_auto_size_between_insets_with_margins() {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_width(root, Length::points(200.0))
        .expect("set root width");
    tree.set_height(root, Length::points(100.0))
        .expect("set root height");

    let nested = tree.create_default_node();
    tree.set_display(nested, Display::Block)
        .expect("set nested display");
    tree.set_box_sizing(nested, BoxSizing::ContentBox)
        .expect("set nested box sizing");
    tree.set_width(nested, Length::points(40.0))
        .expect("set nested width");
    tree.set_height(nested, Length::points(30.0))
        .expect("set nested height");

    let fixed = tree.create_default_node();
    tree.set_display(fixed, Display::Block)
        .expect("set fixed display");
    tree.set_box_sizing(fixed, BoxSizing::ContentBox)
        .expect("set fixed box sizing");
    tree.set_position_type(fixed, PositionType::Fixed)
        .expect("set fixed position");
    tree.set_position(fixed, StandaloneEdge::Left, Length::points(10.0))
        .expect("set fixed left");
    tree.set_position(fixed, StandaloneEdge::Right, Length::points(30.0))
        .expect("set fixed right");
    tree.set_position(fixed, StandaloneEdge::Top, Length::points(20.0))
        .expect("set fixed top");
    tree.set_position(fixed, StandaloneEdge::Bottom, Length::points(25.0))
        .expect("set fixed bottom");
    tree.set_margin(fixed, StandaloneEdge::Left, Length::points(3.0))
        .expect("set fixed left margin");
    tree.set_margin(fixed, StandaloneEdge::Right, Length::points(7.0))
        .expect("set fixed right margin");
    tree.set_margin(fixed, StandaloneEdge::Top, Length::points(4.0))
        .expect("set fixed top margin");
    tree.set_margin(fixed, StandaloneEdge::Bottom, Length::points(6.0))
        .expect("set fixed bottom margin");

    tree.append_child(root, nested).expect("append nested");
    tree.append_child(nested, fixed).expect("append fixed");

    run_standalone_rust(tree, root, Constraints::definite(200.0, 100.0))
        .expect("linear fixed auto size between insets with margins parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_fixed_single_insets_strip_at_most() {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_width(root, Length::points(100.0))
        .expect("set root width");
    tree.set_height(root, Length::points(50.0))
        .expect("set root height");

    let nested = tree.create_default_node();
    tree.set_display(nested, Display::Block)
        .expect("set nested display");
    tree.set_box_sizing(nested, BoxSizing::ContentBox)
        .expect("set nested box sizing");
    tree.set_width(nested, Length::points(20.0))
        .expect("set nested width");
    tree.set_height(nested, Length::points(10.0))
        .expect("set nested height");

    let fixed = tree.create_default_measured_node(Size::new(200.0, 100.0));
    tree.set_display(fixed, Display::Block)
        .expect("set fixed display");
    tree.set_box_sizing(fixed, BoxSizing::ContentBox)
        .expect("set fixed box sizing");
    tree.set_position_type(fixed, PositionType::Fixed)
        .expect("set fixed position");
    tree.set_position(fixed, StandaloneEdge::Left, Length::points(10.0))
        .expect("set fixed left");
    tree.set_position(fixed, StandaloneEdge::Top, Length::points(15.0))
        .expect("set fixed top");
    tree.set_margin(fixed, StandaloneEdge::Left, Length::points(3.0))
        .expect("set fixed left margin");
    tree.set_margin(fixed, StandaloneEdge::Right, Length::points(7.0))
        .expect("set fixed right margin");
    tree.set_margin(fixed, StandaloneEdge::Top, Length::points(4.0))
        .expect("set fixed top margin");
    tree.set_margin(fixed, StandaloneEdge::Bottom, Length::points(6.0))
        .expect("set fixed bottom margin");

    tree.append_child(root, nested).expect("append nested");
    tree.append_child(nested, fixed).expect("append fixed");

    run_standalone_rust(tree, root, Constraints::definite(100.0, 50.0))
        .expect("linear fixed single insets strip AtMost parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_fixed_root_padding_box_offset() {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_width(root, Length::points(100.0))
        .expect("set root width");
    tree.set_height(root, Length::points(80.0))
        .expect("set root height");
    tree.set_padding(root, StandaloneEdge::All, Length::points(3.0))
        .expect("set root padding");

    let nested = tree.create_default_node();
    tree.set_display(nested, Display::Block)
        .expect("set nested display");
    tree.set_box_sizing(nested, BoxSizing::ContentBox)
        .expect("set nested box sizing");
    tree.set_width(nested, Length::points(20.0))
        .expect("set nested width");
    tree.set_height(nested, Length::points(20.0))
        .expect("set nested height");

    let fixed = tree.create_default_node();
    tree.set_display(fixed, Display::Block)
        .expect("set fixed display");
    tree.set_box_sizing(fixed, BoxSizing::ContentBox)
        .expect("set fixed box sizing");
    tree.set_position_type(fixed, PositionType::Fixed)
        .expect("set fixed position");
    tree.set_width(fixed, Length::points(10.0))
        .expect("set fixed width");
    tree.set_height(fixed, Length::points(10.0))
        .expect("set fixed height");
    tree.set_position(fixed, StandaloneEdge::Left, Length::points(5.0))
        .expect("set fixed left");
    tree.set_position(fixed, StandaloneEdge::Top, Length::points(7.0))
        .expect("set fixed top");

    tree.append_child(root, nested).expect("append nested");
    tree.append_child(nested, fixed).expect("append fixed");

    run_standalone_rust(tree, root, Constraints::definite(100.0, 80.0))
        .expect("linear fixed root padding-box offset parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_absolute_percent_insets_and_size() {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_width(root, Length::points(200.0))
        .expect("set root width");
    tree.set_height(root, Length::points(100.0))
        .expect("set root height");

    let absolute = tree.create_default_node();
    tree.set_display(absolute, Display::Block)
        .expect("set absolute display");
    tree.set_box_sizing(absolute, BoxSizing::ContentBox)
        .expect("set absolute box sizing");
    tree.set_position_type(absolute, PositionType::Absolute)
        .expect("set absolute position");
    tree.set_position(absolute, StandaloneEdge::Left, Length::percent(10.0))
        .expect("set absolute left");
    tree.set_position(absolute, StandaloneEdge::Top, Length::percent(25.0))
        .expect("set absolute top");
    tree.set_width(absolute, Length::percent(50.0))
        .expect("set absolute width");
    tree.set_height(absolute, Length::percent(20.0))
        .expect("set absolute height");
    tree.append_child(root, absolute)
        .expect("append absolute child");

    run_standalone_rust(tree, root, Constraints::definite(200.0, 100.0))
        .expect("linear absolute percent inset and size parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_absolute_percent_end_insets() {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_width(root, Length::points(200.0))
        .expect("set root width");
    tree.set_height(root, Length::points(100.0))
        .expect("set root height");

    let absolute = tree.create_default_node();
    tree.set_display(absolute, Display::Block)
        .expect("set absolute display");
    tree.set_box_sizing(absolute, BoxSizing::ContentBox)
        .expect("set absolute box sizing");
    tree.set_position_type(absolute, PositionType::Absolute)
        .expect("set absolute position");
    tree.set_position(absolute, StandaloneEdge::Right, Length::percent(10.0))
        .expect("set absolute right");
    tree.set_position(absolute, StandaloneEdge::Bottom, Length::percent(25.0))
        .expect("set absolute bottom");
    tree.set_width(absolute, Length::percent(50.0))
        .expect("set absolute width");
    tree.set_height(absolute, Length::percent(20.0))
        .expect("set absolute height");
    tree.append_child(root, absolute)
        .expect("append absolute child");

    run_standalone_rust(tree, root, Constraints::definite(200.0, 100.0))
        .expect("linear absolute percent end inset parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_absolute_auto_size_between_insets() {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root linear orientation");
    tree.set_linear_gravity(root, LinearGravity::Center)
        .expect("set root linear gravity");
    tree.set_width(root, Length::points(200.0))
        .expect("set root width");
    tree.set_height(root, Length::points(100.0))
        .expect("set root height");

    let absolute = tree.create_default_node();
    tree.set_display(absolute, Display::Block)
        .expect("set absolute display");
    tree.set_box_sizing(absolute, BoxSizing::ContentBox)
        .expect("set absolute box sizing");
    tree.set_position_type(absolute, PositionType::Absolute)
        .expect("set absolute position");
    tree.set_position(absolute, StandaloneEdge::Left, Length::points(10.0))
        .expect("set absolute left");
    tree.set_position(absolute, StandaloneEdge::Right, Length::points(30.0))
        .expect("set absolute right");
    tree.set_position(absolute, StandaloneEdge::Top, Length::points(20.0))
        .expect("set absolute top");
    tree.set_position(absolute, StandaloneEdge::Bottom, Length::points(25.0))
        .expect("set absolute bottom");
    tree.append_child(root, absolute)
        .expect("append absolute child");

    run_standalone_rust(tree, root, Constraints::definite(200.0, 100.0))
        .expect("linear absolute auto size between insets parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_absolute_auto_size_between_insets_with_margins() {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root linear orientation");
    tree.set_linear_gravity(root, LinearGravity::Center)
        .expect("set root linear gravity");
    tree.set_width(root, Length::points(200.0))
        .expect("set root width");
    tree.set_height(root, Length::points(100.0))
        .expect("set root height");

    let absolute = tree.create_default_node();
    tree.set_display(absolute, Display::Block)
        .expect("set absolute display");
    tree.set_box_sizing(absolute, BoxSizing::ContentBox)
        .expect("set absolute box sizing");
    tree.set_position_type(absolute, PositionType::Absolute)
        .expect("set absolute position");
    tree.set_position(absolute, StandaloneEdge::Left, Length::points(10.0))
        .expect("set absolute left");
    tree.set_position(absolute, StandaloneEdge::Right, Length::points(30.0))
        .expect("set absolute right");
    tree.set_position(absolute, StandaloneEdge::Top, Length::points(20.0))
        .expect("set absolute top");
    tree.set_position(absolute, StandaloneEdge::Bottom, Length::points(25.0))
        .expect("set absolute bottom");
    tree.set_margin(absolute, StandaloneEdge::Left, Length::points(3.0))
        .expect("set absolute left margin");
    tree.set_margin(absolute, StandaloneEdge::Right, Length::points(7.0))
        .expect("set absolute right margin");
    tree.set_margin(absolute, StandaloneEdge::Top, Length::points(4.0))
        .expect("set absolute top margin");
    tree.set_margin(absolute, StandaloneEdge::Bottom, Length::points(6.0))
        .expect("set absolute bottom margin");
    tree.append_child(root, absolute)
        .expect("append absolute child");

    run_standalone_rust(tree, root, Constraints::definite(200.0, 100.0))
        .expect("linear absolute auto size between insets with margins parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_absolute_single_insets_strip_at_most() {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root linear orientation");
    tree.set_linear_gravity(root, LinearGravity::Center)
        .expect("set root linear gravity");
    tree.set_width(root, Length::points(100.0))
        .expect("set root width");
    tree.set_height(root, Length::points(50.0))
        .expect("set root height");

    let absolute = tree.create_default_measured_node(Size::new(200.0, 100.0));
    tree.set_display(absolute, Display::Block)
        .expect("set absolute display");
    tree.set_box_sizing(absolute, BoxSizing::ContentBox)
        .expect("set absolute box sizing");
    tree.set_position_type(absolute, PositionType::Absolute)
        .expect("set absolute position");
    tree.set_position(absolute, StandaloneEdge::Left, Length::points(10.0))
        .expect("set absolute left");
    tree.set_position(absolute, StandaloneEdge::Top, Length::points(15.0))
        .expect("set absolute top");
    tree.set_margin(absolute, StandaloneEdge::Left, Length::points(3.0))
        .expect("set absolute left margin");
    tree.set_margin(absolute, StandaloneEdge::Right, Length::points(7.0))
        .expect("set absolute right margin");
    tree.set_margin(absolute, StandaloneEdge::Top, Length::points(4.0))
        .expect("set absolute top margin");
    tree.set_margin(absolute, StandaloneEdge::Bottom, Length::points(6.0))
        .expect("set absolute bottom margin");
    tree.append_child(root, absolute)
        .expect("append absolute child");

    run_standalone_rust(tree, root, Constraints::definite(100.0, 50.0))
        .expect("linear absolute single insets strip AtMost parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_absolute_paired_insets_fill_padding_box() {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root linear orientation");
    tree.set_linear_gravity(root, LinearGravity::Center)
        .expect("set root linear gravity");
    tree.set_width(root, Length::points(100.0))
        .expect("set root width");
    tree.set_height(root, Length::points(50.0))
        .expect("set root height");
    tree.set_padding(root, StandaloneEdge::All, Length::points(10.0))
        .expect("set root padding");

    let absolute = tree.create_default_measured_node(Size::new(200.0, 200.0));
    tree.set_display(absolute, Display::Block)
        .expect("set absolute display");
    tree.set_box_sizing(absolute, BoxSizing::ContentBox)
        .expect("set absolute box sizing");
    tree.set_position_type(absolute, PositionType::Absolute)
        .expect("set absolute position");
    tree.set_position(absolute, StandaloneEdge::Left, Length::points(10.0))
        .expect("set absolute left");
    tree.set_position(absolute, StandaloneEdge::Right, Length::points(15.0))
        .expect("set absolute right");
    tree.set_position(absolute, StandaloneEdge::Top, Length::points(4.0))
        .expect("set absolute top");
    tree.set_position(absolute, StandaloneEdge::Bottom, Length::points(6.0))
        .expect("set absolute bottom");
    tree.set_margin(absolute, StandaloneEdge::Left, Length::points(2.0))
        .expect("set absolute left margin");
    tree.set_margin(absolute, StandaloneEdge::Right, Length::points(3.0))
        .expect("set absolute right margin");
    tree.set_margin(absolute, StandaloneEdge::Top, Length::points(1.0))
        .expect("set absolute top margin");
    tree.set_margin(absolute, StandaloneEdge::Bottom, Length::points(2.0))
        .expect("set absolute bottom margin");
    tree.append_child(root, absolute)
        .expect("append absolute child");

    run_standalone_rust(tree, root, Constraints::definite(120.0, 70.0))
        .expect("linear absolute paired insets fill padding box parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_absolute_start_insets_override_static_alignment() {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root linear orientation");
    tree.set_linear_gravity(root, LinearGravity::Center)
        .expect("set root linear gravity");
    tree.set_width(root, Length::points(200.0))
        .expect("set root width");
    tree.set_height(root, Length::points(100.0))
        .expect("set root height");

    let absolute = tree.create_default_node();
    tree.set_display(absolute, Display::Block)
        .expect("set absolute display");
    tree.set_box_sizing(absolute, BoxSizing::ContentBox)
        .expect("set absolute box sizing");
    tree.set_position_type(absolute, PositionType::Absolute)
        .expect("set absolute position");
    tree.set_position(absolute, StandaloneEdge::Left, Length::points(12.0))
        .expect("set absolute left");
    tree.set_position(absolute, StandaloneEdge::Top, Length::points(9.0))
        .expect("set absolute top");
    tree.set_width(absolute, Length::points(20.0))
        .expect("set absolute width");
    tree.set_height(absolute, Length::points(10.0))
        .expect("set absolute height");
    tree.set_linear_layout_gravity(absolute, LinearLayoutGravity::End)
        .expect("set absolute layout gravity");
    tree.append_child(root, absolute)
        .expect("append absolute child");

    run_standalone_rust(tree, root, Constraints::definite(200.0, 100.0))
        .expect("linear absolute start insets override static alignment parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_absolute_start_insets_with_margins() {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root linear orientation");
    tree.set_linear_gravity(root, LinearGravity::Center)
        .expect("set root linear gravity");
    tree.set_width(root, Length::points(200.0))
        .expect("set root width");
    tree.set_height(root, Length::points(100.0))
        .expect("set root height");

    let absolute = tree.create_default_node();
    tree.set_display(absolute, Display::Block)
        .expect("set absolute display");
    tree.set_box_sizing(absolute, BoxSizing::ContentBox)
        .expect("set absolute box sizing");
    tree.set_position_type(absolute, PositionType::Absolute)
        .expect("set absolute position");
    tree.set_position(absolute, StandaloneEdge::Left, Length::points(12.0))
        .expect("set absolute left");
    tree.set_position(absolute, StandaloneEdge::Top, Length::points(9.0))
        .expect("set absolute top");
    tree.set_width(absolute, Length::points(20.0))
        .expect("set absolute width");
    tree.set_height(absolute, Length::points(10.0))
        .expect("set absolute height");
    tree.set_linear_layout_gravity(absolute, LinearLayoutGravity::End)
        .expect("set absolute layout gravity");
    tree.set_margin(absolute, StandaloneEdge::Left, Length::points(3.0))
        .expect("set absolute left margin");
    tree.set_margin(absolute, StandaloneEdge::Right, Length::points(7.0))
        .expect("set absolute right margin");
    tree.set_margin(absolute, StandaloneEdge::Top, Length::points(4.0))
        .expect("set absolute top margin");
    tree.set_margin(absolute, StandaloneEdge::Bottom, Length::points(6.0))
        .expect("set absolute bottom margin");
    tree.append_child(root, absolute)
        .expect("append absolute child");

    run_standalone_rust(tree, root, Constraints::definite(200.0, 100.0))
        .expect("linear absolute start insets with margins parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_absolute_end_insets_with_margins() {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root linear orientation");
    tree.set_linear_gravity(root, LinearGravity::Center)
        .expect("set root linear gravity");
    tree.set_width(root, Length::points(200.0))
        .expect("set root width");
    tree.set_height(root, Length::points(100.0))
        .expect("set root height");

    let absolute = tree.create_default_node();
    tree.set_display(absolute, Display::Block)
        .expect("set absolute display");
    tree.set_box_sizing(absolute, BoxSizing::ContentBox)
        .expect("set absolute box sizing");
    tree.set_position_type(absolute, PositionType::Absolute)
        .expect("set absolute position");
    tree.set_linear_layout_gravity(absolute, LinearLayoutGravity::End)
        .expect("set absolute layout gravity");
    tree.set_position(absolute, StandaloneEdge::Right, Length::points(30.0))
        .expect("set absolute right");
    tree.set_position(absolute, StandaloneEdge::Bottom, Length::points(25.0))
        .expect("set absolute bottom");
    tree.set_width(absolute, Length::points(20.0))
        .expect("set absolute width");
    tree.set_height(absolute, Length::points(10.0))
        .expect("set absolute height");
    tree.set_margin(absolute, StandaloneEdge::Left, Length::points(3.0))
        .expect("set absolute left margin");
    tree.set_margin(absolute, StandaloneEdge::Right, Length::points(7.0))
        .expect("set absolute right margin");
    tree.set_margin(absolute, StandaloneEdge::Top, Length::points(4.0))
        .expect("set absolute top margin");
    tree.set_margin(absolute, StandaloneEdge::Bottom, Length::points(6.0))
        .expect("set absolute bottom margin");
    tree.append_child(root, absolute)
        .expect("append absolute child");

    run_standalone_rust(tree, root, Constraints::definite(200.0, 100.0))
        .expect("linear absolute end insets with margins parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_absolute_start_insets_with_percent_margins() {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root linear orientation");
    tree.set_linear_gravity(root, LinearGravity::Center)
        .expect("set root linear gravity");
    tree.set_width(root, Length::points(200.0))
        .expect("set root width");
    tree.set_height(root, Length::points(100.0))
        .expect("set root height");

    let absolute = tree.create_default_node();
    tree.set_display(absolute, Display::Block)
        .expect("set absolute display");
    tree.set_box_sizing(absolute, BoxSizing::ContentBox)
        .expect("set absolute box sizing");
    tree.set_position_type(absolute, PositionType::Absolute)
        .expect("set absolute position");
    tree.set_position(absolute, StandaloneEdge::Left, Length::points(12.0))
        .expect("set absolute left");
    tree.set_position(absolute, StandaloneEdge::Top, Length::points(9.0))
        .expect("set absolute top");
    tree.set_width(absolute, Length::points(20.0))
        .expect("set absolute width");
    tree.set_height(absolute, Length::points(10.0))
        .expect("set absolute height");
    tree.set_linear_layout_gravity(absolute, LinearLayoutGravity::End)
        .expect("set absolute layout gravity");
    tree.set_margin(absolute, StandaloneEdge::Left, Length::percent(10.0))
        .expect("set absolute left margin");
    tree.set_margin(absolute, StandaloneEdge::Right, Length::percent(5.0))
        .expect("set absolute right margin");
    tree.set_margin(absolute, StandaloneEdge::Top, Length::percent(8.0))
        .expect("set absolute top margin");
    tree.set_margin(absolute, StandaloneEdge::Bottom, Length::percent(4.0))
        .expect("set absolute bottom margin");
    tree.append_child(root, absolute)
        .expect("append absolute child");

    run_standalone_rust(tree, root, Constraints::definite(200.0, 100.0))
        .expect("linear absolute start insets with percent margins parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_absolute_end_insets_with_percent_margins() {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root linear orientation");
    tree.set_linear_gravity(root, LinearGravity::Center)
        .expect("set root linear gravity");
    tree.set_width(root, Length::points(200.0))
        .expect("set root width");
    tree.set_height(root, Length::points(100.0))
        .expect("set root height");

    let absolute = tree.create_default_node();
    tree.set_display(absolute, Display::Block)
        .expect("set absolute display");
    tree.set_box_sizing(absolute, BoxSizing::ContentBox)
        .expect("set absolute box sizing");
    tree.set_position_type(absolute, PositionType::Absolute)
        .expect("set absolute position");
    tree.set_linear_layout_gravity(absolute, LinearLayoutGravity::End)
        .expect("set absolute layout gravity");
    tree.set_position(absolute, StandaloneEdge::Right, Length::points(30.0))
        .expect("set absolute right");
    tree.set_position(absolute, StandaloneEdge::Bottom, Length::points(25.0))
        .expect("set absolute bottom");
    tree.set_width(absolute, Length::points(20.0))
        .expect("set absolute width");
    tree.set_height(absolute, Length::points(10.0))
        .expect("set absolute height");
    tree.set_margin(absolute, StandaloneEdge::Left, Length::percent(3.0))
        .expect("set absolute left margin");
    tree.set_margin(absolute, StandaloneEdge::Right, Length::percent(7.0))
        .expect("set absolute right margin");
    tree.set_margin(absolute, StandaloneEdge::Top, Length::percent(4.0))
        .expect("set absolute top margin");
    tree.set_margin(absolute, StandaloneEdge::Bottom, Length::percent(6.0))
        .expect("set absolute bottom margin");
    tree.append_child(root, absolute)
        .expect("append absolute child");

    run_standalone_rust(tree, root, Constraints::definite(200.0, 100.0))
        .expect("linear absolute end insets with percent margins parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_absolute_auto_size_between_insets_with_percent_margins()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root linear orientation");
    tree.set_linear_gravity(root, LinearGravity::Center)
        .expect("set root linear gravity");
    tree.set_width(root, Length::points(200.0))
        .expect("set root width");
    tree.set_height(root, Length::points(100.0))
        .expect("set root height");

    let absolute = tree.create_default_node();
    tree.set_display(absolute, Display::Block)
        .expect("set absolute display");
    tree.set_box_sizing(absolute, BoxSizing::ContentBox)
        .expect("set absolute box sizing");
    tree.set_position_type(absolute, PositionType::Absolute)
        .expect("set absolute position");
    tree.set_position(absolute, StandaloneEdge::Left, Length::points(10.0))
        .expect("set absolute left");
    tree.set_position(absolute, StandaloneEdge::Right, Length::points(30.0))
        .expect("set absolute right");
    tree.set_position(absolute, StandaloneEdge::Top, Length::points(20.0))
        .expect("set absolute top");
    tree.set_position(absolute, StandaloneEdge::Bottom, Length::points(25.0))
        .expect("set absolute bottom");
    tree.set_margin(absolute, StandaloneEdge::Left, Length::percent(4.0))
        .expect("set absolute left margin");
    tree.set_margin(absolute, StandaloneEdge::Right, Length::percent(6.0))
        .expect("set absolute right margin");
    tree.set_margin(absolute, StandaloneEdge::Top, Length::percent(5.0))
        .expect("set absolute top margin");
    tree.set_margin(absolute, StandaloneEdge::Bottom, Length::percent(7.0))
        .expect("set absolute bottom margin");
    tree.append_child(root, absolute)
        .expect("append absolute child");

    run_standalone_rust(tree, root, Constraints::definite(200.0, 100.0))
        .expect("linear absolute auto-size between insets with percent margins parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_absolute_paired_insets_explicit_size_start_wins() {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root linear orientation");
    tree.set_linear_gravity(root, LinearGravity::Center)
        .expect("set root linear gravity");
    tree.set_width(root, Length::points(200.0))
        .expect("set root width");
    tree.set_height(root, Length::points(100.0))
        .expect("set root height");

    let absolute = tree.create_default_node();
    tree.set_display(absolute, Display::Block)
        .expect("set absolute display");
    tree.set_box_sizing(absolute, BoxSizing::ContentBox)
        .expect("set absolute box sizing");
    tree.set_position_type(absolute, PositionType::Absolute)
        .expect("set absolute position");
    tree.set_linear_layout_gravity(absolute, LinearLayoutGravity::End)
        .expect("set absolute layout gravity");
    tree.set_position(absolute, StandaloneEdge::Left, Length::points(12.0))
        .expect("set absolute left");
    tree.set_position(absolute, StandaloneEdge::Right, Length::points(30.0))
        .expect("set absolute right");
    tree.set_position(absolute, StandaloneEdge::Top, Length::points(9.0))
        .expect("set absolute top");
    tree.set_position(absolute, StandaloneEdge::Bottom, Length::points(25.0))
        .expect("set absolute bottom");
    tree.set_width(absolute, Length::points(20.0))
        .expect("set absolute width");
    tree.set_height(absolute, Length::points(10.0))
        .expect("set absolute height");
    tree.append_child(root, absolute)
        .expect("append absolute child");

    run_standalone_rust(tree, root, Constraints::definite(200.0, 100.0))
        .expect("linear absolute paired insets explicit size start wins parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_absolute_paired_insets_explicit_size_with_margins_start_wins()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root linear orientation");
    tree.set_linear_gravity(root, LinearGravity::Center)
        .expect("set root linear gravity");
    tree.set_width(root, Length::points(200.0))
        .expect("set root width");
    tree.set_height(root, Length::points(100.0))
        .expect("set root height");

    let absolute = tree.create_default_node();
    tree.set_display(absolute, Display::Block)
        .expect("set absolute display");
    tree.set_box_sizing(absolute, BoxSizing::ContentBox)
        .expect("set absolute box sizing");
    tree.set_position_type(absolute, PositionType::Absolute)
        .expect("set absolute position");
    tree.set_linear_layout_gravity(absolute, LinearLayoutGravity::End)
        .expect("set absolute layout gravity");
    tree.set_position(absolute, StandaloneEdge::Left, Length::points(12.0))
        .expect("set absolute left");
    tree.set_position(absolute, StandaloneEdge::Right, Length::points(30.0))
        .expect("set absolute right");
    tree.set_position(absolute, StandaloneEdge::Top, Length::points(9.0))
        .expect("set absolute top");
    tree.set_position(absolute, StandaloneEdge::Bottom, Length::points(25.0))
        .expect("set absolute bottom");
    tree.set_width(absolute, Length::points(20.0))
        .expect("set absolute width");
    tree.set_height(absolute, Length::points(10.0))
        .expect("set absolute height");
    tree.set_margin(absolute, StandaloneEdge::Left, Length::points(3.0))
        .expect("set absolute left margin");
    tree.set_margin(absolute, StandaloneEdge::Right, Length::points(7.0))
        .expect("set absolute right margin");
    tree.set_margin(absolute, StandaloneEdge::Top, Length::points(4.0))
        .expect("set absolute top margin");
    tree.set_margin(absolute, StandaloneEdge::Bottom, Length::points(6.0))
        .expect("set absolute bottom margin");
    tree.append_child(root, absolute)
        .expect("append absolute child");

    run_standalone_rust(tree, root, Constraints::definite(200.0, 100.0))
        .expect("linear absolute paired insets explicit size with margins start wins parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_absolute_percent_paired_insets_explicit_size_start_wins()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root linear orientation");
    tree.set_linear_gravity(root, LinearGravity::Center)
        .expect("set root linear gravity");
    tree.set_width(root, Length::points(200.0))
        .expect("set root width");
    tree.set_height(root, Length::points(100.0))
        .expect("set root height");

    let absolute = tree.create_default_node();
    tree.set_display(absolute, Display::Block)
        .expect("set absolute display");
    tree.set_box_sizing(absolute, BoxSizing::ContentBox)
        .expect("set absolute box sizing");
    tree.set_position_type(absolute, PositionType::Absolute)
        .expect("set absolute position");
    tree.set_linear_layout_gravity(absolute, LinearLayoutGravity::End)
        .expect("set absolute layout gravity");
    tree.set_position(absolute, StandaloneEdge::Left, Length::percent(10.0))
        .expect("set absolute left");
    tree.set_position(absolute, StandaloneEdge::Right, Length::percent(20.0))
        .expect("set absolute right");
    tree.set_position(absolute, StandaloneEdge::Top, Length::percent(15.0))
        .expect("set absolute top");
    tree.set_position(absolute, StandaloneEdge::Bottom, Length::percent(25.0))
        .expect("set absolute bottom");
    tree.set_width(absolute, Length::points(20.0))
        .expect("set absolute width");
    tree.set_height(absolute, Length::points(10.0))
        .expect("set absolute height");
    tree.append_child(root, absolute)
        .expect("append absolute child");

    run_standalone_rust(tree, root, Constraints::definite(200.0, 100.0))
        .expect("linear absolute percent paired insets explicit size start wins parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_absolute_calc_start_insets_with_percent_margins() {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root linear orientation");
    tree.set_linear_gravity(root, LinearGravity::Center)
        .expect("set root linear gravity");
    tree.set_width(root, Length::points(210.0))
        .expect("set root width");
    tree.set_height(root, Length::points(120.0))
        .expect("set root height");

    let absolute = tree.create_default_node();
    tree.set_display(absolute, Display::Block)
        .expect("set absolute display");
    tree.set_box_sizing(absolute, BoxSizing::ContentBox)
        .expect("set absolute box sizing");
    tree.set_position_type(absolute, PositionType::Absolute)
        .expect("set absolute position");
    tree.set_linear_layout_gravity(absolute, LinearLayoutGravity::End)
        .expect("set absolute layout gravity");
    tree.set_position(absolute, StandaloneEdge::Left, Length::calc(6.0, 10.0))
        .expect("set absolute left");
    tree.set_position(absolute, StandaloneEdge::Top, Length::calc(4.0, 20.0))
        .expect("set absolute top");
    tree.set_width(absolute, Length::points(28.0))
        .expect("set absolute width");
    tree.set_height(absolute, Length::points(18.0))
        .expect("set absolute height");
    tree.set_margin(absolute, StandaloneEdge::Left, Length::percent(4.0))
        .expect("set absolute left margin");
    tree.set_margin(absolute, StandaloneEdge::Right, Length::percent(6.0))
        .expect("set absolute right margin");
    tree.set_margin(absolute, StandaloneEdge::Top, Length::percent(5.0))
        .expect("set absolute top margin");
    tree.set_margin(absolute, StandaloneEdge::Bottom, Length::percent(7.0))
        .expect("set absolute bottom margin");
    tree.append_child(root, absolute)
        .expect("append absolute child");

    run_standalone_rust(tree, root, Constraints::definite(210.0, 120.0))
        .expect("linear absolute calc start insets with percent margins parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_absolute_calc_paired_insets_explicit_size_start_wins()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root linear orientation");
    tree.set_linear_gravity(root, LinearGravity::Center)
        .expect("set root linear gravity");
    tree.set_width(root, Length::points(210.0))
        .expect("set root width");
    tree.set_height(root, Length::points(120.0))
        .expect("set root height");

    let absolute = tree.create_default_node();
    tree.set_display(absolute, Display::Block)
        .expect("set absolute display");
    tree.set_box_sizing(absolute, BoxSizing::ContentBox)
        .expect("set absolute box sizing");
    tree.set_position_type(absolute, PositionType::Absolute)
        .expect("set absolute position");
    tree.set_linear_layout_gravity(absolute, LinearLayoutGravity::End)
        .expect("set absolute layout gravity");
    tree.set_position(absolute, StandaloneEdge::Left, Length::calc(6.0, 10.0))
        .expect("set absolute left");
    tree.set_position(absolute, StandaloneEdge::Right, Length::calc(8.0, 12.0))
        .expect("set absolute right");
    tree.set_position(absolute, StandaloneEdge::Top, Length::calc(4.0, 20.0))
        .expect("set absolute top");
    tree.set_position(absolute, StandaloneEdge::Bottom, Length::calc(3.0, 15.0))
        .expect("set absolute bottom");
    tree.set_width(absolute, Length::points(28.0))
        .expect("set absolute width");
    tree.set_height(absolute, Length::points(18.0))
        .expect("set absolute height");
    tree.set_margin(absolute, StandaloneEdge::Left, Length::points(2.0))
        .expect("set absolute left margin");
    tree.set_margin(absolute, StandaloneEdge::Right, Length::points(5.0))
        .expect("set absolute right margin");
    tree.set_margin(absolute, StandaloneEdge::Top, Length::points(3.0))
        .expect("set absolute top margin");
    tree.set_margin(absolute, StandaloneEdge::Bottom, Length::points(4.0))
        .expect("set absolute bottom margin");
    tree.append_child(root, absolute)
        .expect("append absolute child");

    run_standalone_rust(tree, root, Constraints::definite(210.0, 120.0))
        .expect("linear absolute calc paired insets explicit size start wins parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_absolute_auto_size_between_calc_insets_with_margins()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root linear orientation");
    tree.set_linear_gravity(root, LinearGravity::Center)
        .expect("set root linear gravity");
    tree.set_width(root, Length::points(220.0))
        .expect("set root width");
    tree.set_height(root, Length::points(130.0))
        .expect("set root height");

    let absolute = tree.create_default_node();
    tree.set_display(absolute, Display::Block)
        .expect("set absolute display");
    tree.set_box_sizing(absolute, BoxSizing::ContentBox)
        .expect("set absolute box sizing");
    tree.set_position_type(absolute, PositionType::Absolute)
        .expect("set absolute position");
    tree.set_position(absolute, StandaloneEdge::Left, Length::calc(7.0, 5.0))
        .expect("set absolute left");
    tree.set_position(absolute, StandaloneEdge::Right, Length::calc(9.0, 10.0))
        .expect("set absolute right");
    tree.set_position(absolute, StandaloneEdge::Top, Length::calc(6.0, 8.0))
        .expect("set absolute top");
    tree.set_position(absolute, StandaloneEdge::Bottom, Length::calc(5.0, 12.0))
        .expect("set absolute bottom");
    tree.set_margin(absolute, StandaloneEdge::Left, Length::points(4.0))
        .expect("set absolute left margin");
    tree.set_margin(absolute, StandaloneEdge::Right, Length::points(6.0))
        .expect("set absolute right margin");
    tree.set_margin(absolute, StandaloneEdge::Top, Length::points(3.0))
        .expect("set absolute top margin");
    tree.set_margin(absolute, StandaloneEdge::Bottom, Length::points(7.0))
        .expect("set absolute bottom margin");
    tree.append_child(root, absolute)
        .expect("append absolute child");

    run_standalone_rust(tree, root, Constraints::definite(220.0, 130.0))
        .expect("linear absolute auto-size between calc insets with margins parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_absolute_calc_end_insets_with_percent_margins() {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root linear orientation");
    tree.set_linear_gravity(root, LinearGravity::Center)
        .expect("set root linear gravity");
    tree.set_width(root, Length::points(210.0))
        .expect("set root width");
    tree.set_height(root, Length::points(120.0))
        .expect("set root height");

    let absolute = tree.create_default_node();
    tree.set_display(absolute, Display::Block)
        .expect("set absolute display");
    tree.set_box_sizing(absolute, BoxSizing::ContentBox)
        .expect("set absolute box sizing");
    tree.set_position_type(absolute, PositionType::Absolute)
        .expect("set absolute position");
    tree.set_position(absolute, StandaloneEdge::Right, Length::calc(8.0, 12.0))
        .expect("set absolute right");
    tree.set_position(absolute, StandaloneEdge::Bottom, Length::calc(3.0, 15.0))
        .expect("set absolute bottom");
    tree.set_width(absolute, Length::points(28.0))
        .expect("set absolute width");
    tree.set_height(absolute, Length::points(18.0))
        .expect("set absolute height");
    tree.set_margin(absolute, StandaloneEdge::Left, Length::percent(3.0))
        .expect("set absolute left margin");
    tree.set_margin(absolute, StandaloneEdge::Right, Length::percent(7.0))
        .expect("set absolute right margin");
    tree.set_margin(absolute, StandaloneEdge::Top, Length::percent(4.0))
        .expect("set absolute top margin");
    tree.set_margin(absolute, StandaloneEdge::Bottom, Length::percent(6.0))
        .expect("set absolute bottom margin");
    tree.append_child(root, absolute)
        .expect("append absolute child");

    run_standalone_rust(tree, root, Constraints::definite(210.0, 120.0))
        .expect("linear absolute calc end insets with percent margins parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_absolute_auto_size_between_calc_insets_with_percent_margins()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root linear orientation");
    tree.set_linear_gravity(root, LinearGravity::Center)
        .expect("set root linear gravity");
    tree.set_width(root, Length::points(220.0))
        .expect("set root width");
    tree.set_height(root, Length::points(130.0))
        .expect("set root height");

    let absolute = tree.create_default_measured_node(Size::new(300.0, 160.0));
    tree.set_display(absolute, Display::Block)
        .expect("set absolute display");
    tree.set_box_sizing(absolute, BoxSizing::ContentBox)
        .expect("set absolute box sizing");
    tree.set_position_type(absolute, PositionType::Absolute)
        .expect("set absolute position");
    tree.set_position(absolute, StandaloneEdge::Left, Length::calc(7.0, 5.0))
        .expect("set absolute left");
    tree.set_position(absolute, StandaloneEdge::Right, Length::calc(9.0, 10.0))
        .expect("set absolute right");
    tree.set_position(absolute, StandaloneEdge::Top, Length::calc(6.0, 8.0))
        .expect("set absolute top");
    tree.set_position(absolute, StandaloneEdge::Bottom, Length::calc(5.0, 12.0))
        .expect("set absolute bottom");
    tree.set_margin(absolute, StandaloneEdge::Left, Length::percent(4.0))
        .expect("set absolute left margin");
    tree.set_margin(absolute, StandaloneEdge::Right, Length::percent(6.0))
        .expect("set absolute right margin");
    tree.set_margin(absolute, StandaloneEdge::Top, Length::percent(5.0))
        .expect("set absolute top margin");
    tree.set_margin(absolute, StandaloneEdge::Bottom, Length::percent(7.0))
        .expect("set absolute bottom margin");
    tree.append_child(root, absolute)
        .expect("append absolute child");

    run_standalone_rust(tree, root, Constraints::definite(220.0, 130.0))
        .expect("linear absolute auto-size between calc insets with percent margins parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_absolute_calc_end_insets_with_percent_margins_and_padding_border_origin()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root linear orientation");
    tree.set_linear_gravity(root, LinearGravity::Center)
        .expect("set root linear gravity");
    tree.set_width(root, Length::points(214.0))
        .expect("set root width");
    tree.set_height(root, Length::points(126.0))
        .expect("set root height");
    tree.set_padding(root, StandaloneEdge::Left, Length::points(6.0))
        .expect("set root left padding");
    tree.set_padding(root, StandaloneEdge::Right, Length::points(8.0))
        .expect("set root right padding");
    tree.set_padding(root, StandaloneEdge::Top, Length::points(4.0))
        .expect("set root top padding");
    tree.set_padding(root, StandaloneEdge::Bottom, Length::points(5.0))
        .expect("set root bottom padding");
    tree.set_border(root, StandaloneEdge::Left, 2.0)
        .expect("set root left border");
    tree.set_border(root, StandaloneEdge::Right, 3.0)
        .expect("set root right border");
    tree.set_border(root, StandaloneEdge::Top, 1.0)
        .expect("set root top border");
    tree.set_border(root, StandaloneEdge::Bottom, 2.0)
        .expect("set root bottom border");

    let absolute = tree.create_default_node();
    tree.set_display(absolute, Display::Block)
        .expect("set absolute display");
    tree.set_box_sizing(absolute, BoxSizing::ContentBox)
        .expect("set absolute box sizing");
    tree.set_position_type(absolute, PositionType::Absolute)
        .expect("set absolute position");
    tree.set_position(absolute, StandaloneEdge::Right, Length::calc(8.0, 12.0))
        .expect("set absolute right");
    tree.set_position(absolute, StandaloneEdge::Bottom, Length::calc(3.0, 15.0))
        .expect("set absolute bottom");
    tree.set_width(absolute, Length::points(28.0))
        .expect("set absolute width");
    tree.set_height(absolute, Length::points(18.0))
        .expect("set absolute height");
    tree.set_margin(absolute, StandaloneEdge::Left, Length::percent(3.0))
        .expect("set absolute left margin");
    tree.set_margin(absolute, StandaloneEdge::Right, Length::percent(7.0))
        .expect("set absolute right margin");
    tree.set_margin(absolute, StandaloneEdge::Top, Length::percent(4.0))
        .expect("set absolute top margin");
    tree.set_margin(absolute, StandaloneEdge::Bottom, Length::percent(6.0))
        .expect("set absolute bottom margin");
    tree.append_child(root, absolute)
        .expect("append absolute child");

    run_standalone_rust(tree, root, Constraints::definite(214.0, 126.0)).expect(
        "linear absolute calc end insets with percent margins and padding/border origin parity",
    );
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_absolute_horizontal_percent_paired_insets_with_percent_margins_start_wins()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root linear orientation");
    tree.set_linear_gravity(root, LinearGravity::Center)
        .expect("set root linear gravity");
    tree.set_width(root, Length::points(240.0))
        .expect("set root width");
    tree.set_height(root, Length::points(120.0))
        .expect("set root height");

    let absolute = tree.create_default_node();
    tree.set_display(absolute, Display::Block)
        .expect("set absolute display");
    tree.set_box_sizing(absolute, BoxSizing::ContentBox)
        .expect("set absolute box sizing");
    tree.set_position_type(absolute, PositionType::Absolute)
        .expect("set absolute position");
    tree.set_linear_layout_gravity(absolute, LinearLayoutGravity::End)
        .expect("set absolute layout gravity");
    tree.set_position(absolute, StandaloneEdge::Left, Length::percent(12.0))
        .expect("set absolute left");
    tree.set_position(absolute, StandaloneEdge::Right, Length::percent(18.0))
        .expect("set absolute right");
    tree.set_position(absolute, StandaloneEdge::Top, Length::points(11.0))
        .expect("set absolute top");
    tree.set_width(absolute, Length::points(34.0))
        .expect("set absolute width");
    tree.set_height(absolute, Length::points(16.0))
        .expect("set absolute height");
    tree.set_margin(absolute, StandaloneEdge::Left, Length::percent(4.0))
        .expect("set absolute left margin");
    tree.set_margin(absolute, StandaloneEdge::Right, Length::percent(6.0))
        .expect("set absolute right margin");
    tree.set_margin(absolute, StandaloneEdge::Top, Length::points(5.0))
        .expect("set absolute top margin");
    tree.append_child(root, absolute)
        .expect("append absolute child");

    run_standalone_rust(tree, root, Constraints::definite(240.0, 120.0))
        .expect("linear absolute horizontal percent paired insets with percent margins parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_absolute_vertical_percent_paired_insets_with_percent_margins_start_wins()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Vertical)
        .expect("set root linear orientation");
    tree.set_linear_gravity(root, LinearGravity::Center)
        .expect("set root linear gravity");
    tree.set_width(root, Length::points(236.0))
        .expect("set root width");
    tree.set_height(root, Length::points(128.0))
        .expect("set root height");

    let absolute = tree.create_default_node();
    tree.set_display(absolute, Display::Block)
        .expect("set absolute display");
    tree.set_box_sizing(absolute, BoxSizing::ContentBox)
        .expect("set absolute box sizing");
    tree.set_position_type(absolute, PositionType::Absolute)
        .expect("set absolute position");
    tree.set_linear_layout_gravity(absolute, LinearLayoutGravity::End)
        .expect("set absolute layout gravity");
    tree.set_position(absolute, StandaloneEdge::Left, Length::points(17.0))
        .expect("set absolute left");
    tree.set_position(absolute, StandaloneEdge::Top, Length::percent(14.0))
        .expect("set absolute top");
    tree.set_position(absolute, StandaloneEdge::Bottom, Length::percent(22.0))
        .expect("set absolute bottom");
    tree.set_width(absolute, Length::points(32.0))
        .expect("set absolute width");
    tree.set_height(absolute, Length::points(18.0))
        .expect("set absolute height");
    tree.set_margin(absolute, StandaloneEdge::Top, Length::percent(5.0))
        .expect("set absolute top margin");
    tree.set_margin(absolute, StandaloneEdge::Bottom, Length::percent(7.0))
        .expect("set absolute bottom margin");
    tree.set_margin(absolute, StandaloneEdge::Left, Length::points(4.0))
        .expect("set absolute left margin");
    tree.append_child(root, absolute)
        .expect("append absolute child");

    run_standalone_rust(tree, root, Constraints::definite(236.0, 128.0))
        .expect("linear absolute vertical percent paired insets with percent margins parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_fixed_horizontal_percent_paired_insets_with_percent_margins_start_wins()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root linear orientation");
    tree.set_linear_gravity(root, LinearGravity::Center)
        .expect("set root linear gravity");
    tree.set_width(root, Length::points(244.0))
        .expect("set root width");
    tree.set_height(root, Length::points(132.0))
        .expect("set root height");

    let nested = tree.create_default_node();
    tree.set_display(nested, Display::Block)
        .expect("set nested display");
    tree.set_box_sizing(nested, BoxSizing::ContentBox)
        .expect("set nested box sizing");
    tree.set_width(nested, Length::points(44.0))
        .expect("set nested width");
    tree.set_height(nested, Length::points(26.0))
        .expect("set nested height");

    let fixed = tree.create_default_node();
    tree.set_display(fixed, Display::Block)
        .expect("set fixed display");
    tree.set_box_sizing(fixed, BoxSizing::ContentBox)
        .expect("set fixed box sizing");
    tree.set_position_type(fixed, PositionType::Fixed)
        .expect("set fixed position");
    tree.set_linear_layout_gravity(fixed, LinearLayoutGravity::End)
        .expect("set fixed layout gravity");
    tree.set_position(fixed, StandaloneEdge::Left, Length::percent(11.0))
        .expect("set fixed left");
    tree.set_position(fixed, StandaloneEdge::Right, Length::percent(19.0))
        .expect("set fixed right");
    tree.set_position(fixed, StandaloneEdge::Top, Length::points(13.0))
        .expect("set fixed top");
    tree.set_width(fixed, Length::points(36.0))
        .expect("set fixed width");
    tree.set_height(fixed, Length::points(18.0))
        .expect("set fixed height");
    tree.set_margin(fixed, StandaloneEdge::Left, Length::percent(5.0))
        .expect("set fixed left margin");
    tree.set_margin(fixed, StandaloneEdge::Right, Length::percent(8.0))
        .expect("set fixed right margin");
    tree.set_margin(fixed, StandaloneEdge::Top, Length::points(3.0))
        .expect("set fixed top margin");

    tree.append_child(root, nested).expect("append nested");
    tree.append_child(nested, fixed)
        .expect("append fixed child");

    run_standalone_rust(tree, root, Constraints::definite(244.0, 132.0))
        .expect("linear fixed horizontal percent paired insets with percent margins parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_fixed_vertical_percent_paired_insets_with_percent_margins_start_wins()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Vertical)
        .expect("set root linear orientation");
    tree.set_linear_gravity(root, LinearGravity::Center)
        .expect("set root linear gravity");
    tree.set_width(root, Length::points(232.0))
        .expect("set root width");
    tree.set_height(root, Length::points(136.0))
        .expect("set root height");

    let nested = tree.create_default_node();
    tree.set_display(nested, Display::Block)
        .expect("set nested display");
    tree.set_box_sizing(nested, BoxSizing::ContentBox)
        .expect("set nested box sizing");
    tree.set_width(nested, Length::points(42.0))
        .expect("set nested width");
    tree.set_height(nested, Length::points(28.0))
        .expect("set nested height");

    let fixed = tree.create_default_node();
    tree.set_display(fixed, Display::Block)
        .expect("set fixed display");
    tree.set_box_sizing(fixed, BoxSizing::ContentBox)
        .expect("set fixed box sizing");
    tree.set_position_type(fixed, PositionType::Fixed)
        .expect("set fixed position");
    tree.set_linear_layout_gravity(fixed, LinearLayoutGravity::End)
        .expect("set fixed layout gravity");
    tree.set_position(fixed, StandaloneEdge::Left, Length::points(15.0))
        .expect("set fixed left");
    tree.set_position(fixed, StandaloneEdge::Top, Length::percent(13.0))
        .expect("set fixed top");
    tree.set_position(fixed, StandaloneEdge::Bottom, Length::percent(21.0))
        .expect("set fixed bottom");
    tree.set_width(fixed, Length::points(30.0))
        .expect("set fixed width");
    tree.set_height(fixed, Length::points(20.0))
        .expect("set fixed height");
    tree.set_margin(fixed, StandaloneEdge::Top, Length::percent(6.0))
        .expect("set fixed top margin");
    tree.set_margin(fixed, StandaloneEdge::Bottom, Length::percent(9.0))
        .expect("set fixed bottom margin");
    tree.set_margin(fixed, StandaloneEdge::Left, Length::points(5.0))
        .expect("set fixed left margin");

    tree.append_child(root, nested).expect("append nested");
    tree.append_child(nested, fixed)
        .expect("append fixed child");

    run_standalone_rust(tree, root, Constraints::definite(232.0, 136.0))
        .expect("linear fixed vertical percent paired insets with percent margins parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_absolute_bottom_inset_only_with_percent_vertical_margins()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Vertical)
        .expect("set root linear orientation");
    tree.set_linear_gravity(root, LinearGravity::Center)
        .expect("set root linear gravity");
    tree.set_width(root, Length::points(216.0))
        .expect("set root width");
    tree.set_height(root, Length::points(118.0))
        .expect("set root height");

    let absolute = tree.create_default_node();
    tree.set_display(absolute, Display::Block)
        .expect("set absolute display");
    tree.set_box_sizing(absolute, BoxSizing::ContentBox)
        .expect("set absolute box sizing");
    tree.set_position_type(absolute, PositionType::Absolute)
        .expect("set absolute position");
    tree.set_linear_layout_gravity(absolute, LinearLayoutGravity::Start)
        .expect("set absolute layout gravity");
    tree.set_position(absolute, StandaloneEdge::Left, Length::points(18.0))
        .expect("set absolute left");
    tree.set_position(absolute, StandaloneEdge::Bottom, Length::percent(17.0))
        .expect("set absolute bottom");
    tree.set_width(absolute, Length::points(30.0))
        .expect("set absolute width");
    tree.set_height(absolute, Length::points(22.0))
        .expect("set absolute height");
    tree.set_margin(absolute, StandaloneEdge::Top, Length::percent(4.0))
        .expect("set absolute top margin");
    tree.set_margin(absolute, StandaloneEdge::Bottom, Length::percent(8.0))
        .expect("set absolute bottom margin");
    tree.append_child(root, absolute)
        .expect("append absolute child");

    run_standalone_rust(tree, root, Constraints::definite(216.0, 118.0))
        .expect("linear absolute bottom inset only with percent margins parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_fixed_right_inset_only_with_calc_horizontal_margins()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root linear orientation");
    tree.set_linear_gravity(root, LinearGravity::Center)
        .expect("set root linear gravity");
    tree.set_width(root, Length::points(218.0))
        .expect("set root width");
    tree.set_height(root, Length::points(122.0))
        .expect("set root height");
    tree.set_padding(root, StandaloneEdge::Left, Length::points(5.0))
        .expect("set root left padding");
    tree.set_padding(root, StandaloneEdge::Right, Length::points(7.0))
        .expect("set root right padding");
    tree.set_border(root, StandaloneEdge::Left, 2.0)
        .expect("set root left border");
    tree.set_border(root, StandaloneEdge::Right, 3.0)
        .expect("set root right border");

    let nested = tree.create_default_node();
    tree.set_display(nested, Display::Block)
        .expect("set nested display");
    tree.set_box_sizing(nested, BoxSizing::ContentBox)
        .expect("set nested box sizing");
    tree.set_width(nested, Length::points(40.0))
        .expect("set nested width");
    tree.set_height(nested, Length::points(24.0))
        .expect("set nested height");

    let fixed = tree.create_default_node();
    tree.set_display(fixed, Display::Block)
        .expect("set fixed display");
    tree.set_box_sizing(fixed, BoxSizing::ContentBox)
        .expect("set fixed box sizing");
    tree.set_position_type(fixed, PositionType::Fixed)
        .expect("set fixed position");
    tree.set_linear_layout_gravity(fixed, LinearLayoutGravity::Start)
        .expect("set fixed layout gravity");
    tree.set_position(fixed, StandaloneEdge::Right, Length::calc(6.0, 12.0))
        .expect("set fixed right");
    tree.set_position(fixed, StandaloneEdge::Top, Length::points(14.0))
        .expect("set fixed top");
    tree.set_width(fixed, Length::points(34.0))
        .expect("set fixed width");
    tree.set_height(fixed, Length::points(18.0))
        .expect("set fixed height");
    tree.set_margin(fixed, StandaloneEdge::Left, Length::calc(3.0, 5.0))
        .expect("set fixed left margin");
    tree.set_margin(fixed, StandaloneEdge::Right, Length::calc(4.0, 7.0))
        .expect("set fixed right margin");

    tree.append_child(root, nested).expect("append nested");
    tree.append_child(nested, fixed)
        .expect("append fixed child");

    run_standalone_rust(tree, root, Constraints::definite(218.0, 122.0))
        .expect("linear fixed right inset only with calc margins parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_out_of_flow_intrinsic_sizing() {
    for (case_name, tree, root, constraints) in [
        out_of_flow_intrinsic_sizing_tree(
            "absolute block fit-content uses latest linear sizing",
            PositionType::Absolute,
            false,
            Length::fit_content(Some(BaseLength::fixed(80.0))),
            Length::fit_content(Some(BaseLength::fixed(20.0))),
            OutOfFlowNaturalSize::Subtree(Size::new(120.0, 30.0)),
        ),
        out_of_flow_intrinsic_sizing_tree(
            "fixed block fit-content uses latest linear sizing",
            PositionType::Fixed,
            true,
            Length::fit_content(Some(BaseLength::fixed(80.0))),
            Length::fit_content(Some(BaseLength::fixed(20.0))),
            OutOfFlowNaturalSize::Subtree(Size::new(120.0, 30.0)),
        ),
        out_of_flow_intrinsic_sizing_tree(
            "absolute measured fit-content uses measured natural size",
            PositionType::Absolute,
            false,
            Length::fit_content(Some(BaseLength::fixed(80.0))),
            Length::fit_content(Some(BaseLength::fixed(20.0))),
            OutOfFlowNaturalSize::Measured(Size::new(120.0, 30.0)),
        ),
        out_of_flow_intrinsic_sizing_tree(
            "fixed measured fit-content uses measured natural size",
            PositionType::Fixed,
            true,
            Length::fit_content(Some(BaseLength::fixed(80.0))),
            Length::fit_content(Some(BaseLength::fixed(20.0))),
            OutOfFlowNaturalSize::Measured(Size::new(120.0, 30.0)),
        ),
        out_of_flow_intrinsic_sizing_tree(
            "absolute block max-content uses latest linear natural size",
            PositionType::Absolute,
            false,
            Length::MaxContent,
            Length::MaxContent,
            OutOfFlowNaturalSize::Subtree(Size::new(250.0, 130.0)),
        ),
        out_of_flow_intrinsic_sizing_tree(
            "fixed block max-content uses latest linear natural size",
            PositionType::Fixed,
            true,
            Length::MaxContent,
            Length::MaxContent,
            OutOfFlowNaturalSize::Subtree(Size::new(250.0, 130.0)),
        ),
        out_of_flow_intrinsic_sizing_tree(
            "absolute measured max-content uses measured natural size",
            PositionType::Absolute,
            false,
            Length::MaxContent,
            Length::MaxContent,
            OutOfFlowNaturalSize::Measured(Size::new(250.0, 130.0)),
        ),
        out_of_flow_intrinsic_sizing_tree(
            "fixed measured max-content uses measured natural size",
            PositionType::Fixed,
            true,
            Length::MaxContent,
            Length::MaxContent,
            OutOfFlowNaturalSize::Measured(Size::new(250.0, 130.0)),
        ),
    ] {
        run_standalone_rust(tree, root, constraints)
            .unwrap_or_else(|error| panic!("{case_name} parity failed: {error}"));
    }
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_absolute_display_none_does_not_participate_in_ordered_stack()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root orientation");

    let later = tree.create_default_measured_node(Size::new(32.0, 18.0));
    tree.set_order(later, 3).expect("set later order");

    let hidden_absolute = tree.create_default_node();
    tree.set_display(hidden_absolute, Display::None)
        .expect("set hidden absolute display");
    tree.set_position_type(hidden_absolute, PositionType::Absolute)
        .expect("set hidden absolute position");
    tree.set_width(hidden_absolute, Length::points(260.0))
        .expect("set hidden absolute width");
    tree.set_height(hidden_absolute, Length::points(180.0))
        .expect("set hidden absolute height");
    tree.set_linear_layout_gravity(hidden_absolute, LinearLayoutGravity::End)
        .expect("set hidden absolute layout gravity");
    tree.set_order(hidden_absolute, -5)
        .expect("set hidden absolute order");

    let earlier = tree.create_default_measured_node(Size::new(42.0, 20.0));
    tree.set_order(earlier, -1).expect("set earlier order");

    tree.append_child(root, later).expect("append later");
    tree.append_child(root, hidden_absolute)
        .expect("append hidden absolute");
    tree.append_child(root, earlier).expect("append earlier");

    run_standalone_rust(tree, root, Constraints::indefinite())
        .expect("linear absolute display-none does not participate in ordered stack parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_fixed_display_none_descendant_skips_root_containing_block_measure()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Vertical)
        .expect("set root orientation");
    tree.set_width(root, Length::points(120.0))
        .expect("set root width");
    tree.set_padding(root, StandaloneEdge::All, Length::points(4.0))
        .expect("set root padding");
    tree.set_border(root, StandaloneEdge::All, 1.0)
        .expect("set root border");

    let nested = tree.create_default_measured_node(Size::new(44.0, 24.0));
    let hidden_fixed = tree.create_default_node();
    tree.set_display(hidden_fixed, Display::None)
        .expect("set hidden fixed display");
    tree.set_position_type(hidden_fixed, PositionType::Fixed)
        .expect("set hidden fixed position");
    tree.set_position(hidden_fixed, StandaloneEdge::Left, Length::percent(8.0))
        .expect("set hidden fixed left");
    tree.set_position(hidden_fixed, StandaloneEdge::Right, Length::percent(10.0))
        .expect("set hidden fixed right");
    tree.set_position(hidden_fixed, StandaloneEdge::Top, Length::calc(3.0, 5.0))
        .expect("set hidden fixed top");
    tree.set_position(hidden_fixed, StandaloneEdge::Bottom, Length::calc(4.0, 7.0))
        .expect("set hidden fixed bottom");
    tree.set_margin(
        hidden_fixed,
        StandaloneEdge::Horizontal,
        Length::percent(4.0),
    )
    .expect("set hidden fixed horizontal margin");
    tree.set_margin(
        hidden_fixed,
        StandaloneEdge::Vertical,
        Length::calc(2.0, 6.0),
    )
    .expect("set hidden fixed vertical margin");

    let visible = tree.create_default_measured_node(Size::new(36.0, 20.0));
    tree.append_child(root, nested).expect("append nested");
    tree.append_child(nested, hidden_fixed)
        .expect("append hidden fixed");
    tree.append_child(root, visible).expect("append visible");

    run_standalone_rust(
        tree,
        root,
        Constraints::new(
            SideConstraint::definite(130.0),
            SideConstraint::indefinite(),
        ),
    )
    .expect("linear fixed display-none descendant skips root containing-block measure parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_display_none_subtree_hides_measured_descendant_with_order()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root orientation");
    tree.set_padding(root, StandaloneEdge::All, Length::points(4.0))
        .expect("set root padding");
    tree.set_border(root, StandaloneEdge::All, 1.0)
        .expect("set root border");

    let visible_later = tree.create_default_measured_node(Size::new(34.0, 18.0));
    tree.set_order(visible_later, 4)
        .expect("set visible later order");

    let hidden_parent = tree.create_default_node();
    tree.set_display(hidden_parent, Display::None)
        .expect("set hidden parent display");
    tree.set_width(hidden_parent, Length::points(120.0))
        .expect("set hidden parent width");
    tree.set_height(hidden_parent, Length::points(90.0))
        .expect("set hidden parent height");
    tree.set_padding(hidden_parent, StandaloneEdge::All, Length::points(3.0))
        .expect("set hidden parent padding");
    tree.set_border(hidden_parent, StandaloneEdge::All, 2.0)
        .expect("set hidden parent border");
    tree.set_order(hidden_parent, -5)
        .expect("set hidden parent order");

    let hidden_descendant = tree.create_default_measured_node(Size::new(80.0, 30.0));
    tree.set_order(hidden_descendant, -9)
        .expect("set hidden descendant order");

    let visible_earlier = tree.create_default_measured_node(Size::new(42.0, 20.0));
    tree.set_order(visible_earlier, -1)
        .expect("set visible earlier order");

    tree.append_child(root, visible_later)
        .expect("append visible later");
    tree.append_child(root, hidden_parent)
        .expect("append hidden parent");
    tree.append_child(hidden_parent, hidden_descendant)
        .expect("append hidden descendant");
    tree.append_child(root, visible_earlier)
        .expect("append visible earlier");

    run_standalone_rust(tree, root, Constraints::indefinite())
        .expect("linear display-none subtree hides measured descendant with order parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_display_none_subtree_hides_absolute_descendant() {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Vertical)
        .expect("set root orientation");
    tree.set_width(root, Length::points(118.0))
        .expect("set root width");
    tree.set_padding(root, StandaloneEdge::All, Length::points(5.0))
        .expect("set root padding");
    tree.set_border(root, StandaloneEdge::All, 1.0)
        .expect("set root border");

    let hidden_parent = tree.create_default_node();
    tree.set_display(hidden_parent, Display::None)
        .expect("set hidden parent display");
    tree.set_padding(hidden_parent, StandaloneEdge::All, Length::points(2.0))
        .expect("set hidden parent padding");
    tree.set_border(hidden_parent, StandaloneEdge::All, 3.0)
        .expect("set hidden parent border");

    let hidden_absolute = tree.create_default_node();
    tree.set_display(hidden_absolute, Display::Block)
        .expect("set hidden absolute display");
    tree.set_box_sizing(hidden_absolute, BoxSizing::ContentBox)
        .expect("set hidden absolute box sizing");
    tree.set_position_type(hidden_absolute, PositionType::Absolute)
        .expect("set hidden absolute position");
    tree.set_position(hidden_absolute, StandaloneEdge::Top, Length::percent(20.0))
        .expect("set hidden absolute top");
    tree.set_position(
        hidden_absolute,
        StandaloneEdge::Bottom,
        Length::calc(4.0, 8.0),
    )
    .expect("set hidden absolute bottom");
    tree.set_width(hidden_absolute, Length::points(240.0))
        .expect("set hidden absolute width");
    tree.set_height(hidden_absolute, Length::points(150.0))
        .expect("set hidden absolute height");

    let visible = tree.create_default_measured_node(Size::new(44.0, 22.0));
    tree.append_child(root, hidden_parent)
        .expect("append hidden parent");
    tree.append_child(hidden_parent, hidden_absolute)
        .expect("append hidden absolute");
    tree.append_child(root, visible).expect("append visible");

    run_standalone_rust(
        tree,
        root,
        Constraints::new(
            SideConstraint::definite(130.0),
            SideConstraint::indefinite(),
        ),
    )
    .expect("linear display-none subtree hides absolute descendant parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_display_none_subtree_hides_fixed_descendant_before_root_measure()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Vertical)
        .expect("set root orientation");
    tree.set_width(root, Length::points(128.0))
        .expect("set root width");
    tree.set_padding(root, StandaloneEdge::All, Length::points(4.0))
        .expect("set root padding");
    tree.set_border(root, StandaloneEdge::All, 1.0)
        .expect("set root border");

    let visible = tree.create_default_measured_node(Size::new(46.0, 24.0));

    let hidden_parent = tree.create_default_node();
    tree.set_display(hidden_parent, Display::None)
        .expect("set hidden parent display");
    tree.set_padding(hidden_parent, StandaloneEdge::All, Length::points(3.0))
        .expect("set hidden parent padding");
    tree.set_border(hidden_parent, StandaloneEdge::All, 2.0)
        .expect("set hidden parent border");

    let hidden_fixed = tree.create_default_node();
    tree.set_display(hidden_fixed, Display::Block)
        .expect("set hidden fixed display");
    tree.set_position_type(hidden_fixed, PositionType::Fixed)
        .expect("set hidden fixed position");
    tree.set_position(hidden_fixed, StandaloneEdge::Left, Length::percent(10.0))
        .expect("set hidden fixed left");
    tree.set_position(hidden_fixed, StandaloneEdge::Right, Length::percent(14.0))
        .expect("set hidden fixed right");
    tree.set_position(hidden_fixed, StandaloneEdge::Top, Length::calc(3.0, 6.0))
        .expect("set hidden fixed top");
    tree.set_width(hidden_fixed, Length::points(260.0))
        .expect("set hidden fixed width");
    tree.set_height(hidden_fixed, Length::points(170.0))
        .expect("set hidden fixed height");

    tree.append_child(root, visible).expect("append visible");
    tree.append_child(root, hidden_parent)
        .expect("append hidden parent");
    tree.append_child(hidden_parent, hidden_fixed)
        .expect("append hidden fixed");

    run_standalone_rust(
        tree,
        root,
        Constraints::new(
            SideConstraint::definite(140.0),
            SideConstraint::indefinite(),
        ),
    )
    .expect("linear display-none subtree hides fixed descendant before root measure parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_display_none_and_ordered_stack() {
    let (tree, root, constraints) = linear_display_none_and_ordered_stack_tree();

    run_standalone_rust(tree, root, constraints)
        .expect("linear display-none and ordered stack parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_horizontal_linear_order_skips_display_none_with_padding_border()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root orientation");
    tree.set_align_items(root, AlignItems::FlexStart)
        .expect("set root align items");
    tree.set_width(root, Length::points(142.0))
        .expect("set root width");
    tree.set_height(root, Length::points(48.0))
        .expect("set root height");
    tree.set_padding(root, StandaloneEdge::Left, Length::points(6.0))
        .expect("set root left padding");
    tree.set_padding(root, StandaloneEdge::Right, Length::points(4.0))
        .expect("set root right padding");
    tree.set_border(root, StandaloneEdge::Left, 1.0)
        .expect("set root left border");
    tree.set_border(root, StandaloneEdge::Right, 2.0)
        .expect("set root right border");

    let later = tree.create_default_node();
    tree.set_display(later, Display::Block)
        .expect("set later display");
    tree.set_width(later, Length::points(18.0))
        .expect("set later width");
    tree.set_height(later, Length::points(12.0))
        .expect("set later height");
    tree.set_order(later, 2).expect("set later order");

    let hidden = tree.create_default_node();
    tree.set_display(hidden, Display::None)
        .expect("set hidden display");
    tree.set_width(hidden, Length::points(90.0))
        .expect("set hidden width");
    tree.set_height(hidden, Length::points(20.0))
        .expect("set hidden height");
    tree.set_order(hidden, -3).expect("set hidden order");

    let earlier = tree.create_default_node();
    tree.set_display(earlier, Display::Block)
        .expect("set earlier display");
    tree.set_width(earlier, Length::points(24.0))
        .expect("set earlier width");
    tree.set_height(earlier, Length::points(16.0))
        .expect("set earlier height");
    tree.set_order(earlier, -1).expect("set earlier order");

    let middle = tree.create_default_node();
    tree.set_display(middle, Display::Block)
        .expect("set middle display");
    tree.set_width(middle, Length::points(14.0))
        .expect("set middle width");
    tree.set_height(middle, Length::points(18.0))
        .expect("set middle height");

    tree.append_child(root, later).expect("append later");
    tree.append_child(root, hidden).expect("append hidden");
    tree.append_child(root, middle).expect("append middle");
    tree.append_child(root, earlier).expect("append earlier");

    run_standalone_rust(tree, root, Constraints::definite(142.0, 48.0))
        .expect("horizontal linear order skips display-none with padding/border parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_horizontal_reverse_linear_order_skips_display_none_with_percent_main_margins()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::HorizontalReverse)
        .expect("set root orientation");
    tree.set_align_items(root, AlignItems::FlexStart)
        .expect("set root align items");
    tree.set_width(root, Length::points(150.0))
        .expect("set root width");
    tree.set_height(root, Length::points(50.0))
        .expect("set root height");

    let trailing = tree.create_default_node();
    tree.set_display(trailing, Display::Block)
        .expect("set trailing display");
    tree.set_width(trailing, Length::points(16.0))
        .expect("set trailing width");
    tree.set_height(trailing, Length::points(12.0))
        .expect("set trailing height");
    tree.set_margin(trailing, StandaloneEdge::Left, Length::percent(3.0))
        .expect("set trailing left margin");
    tree.set_order(trailing, 3).expect("set trailing order");

    let hidden = tree.create_default_node();
    tree.set_display(hidden, Display::None)
        .expect("set hidden display");
    tree.set_width(hidden, Length::points(80.0))
        .expect("set hidden width");
    tree.set_height(hidden, Length::points(30.0))
        .expect("set hidden height");
    tree.set_order(hidden, -2).expect("set hidden order");

    let leading = tree.create_default_node();
    tree.set_display(leading, Display::Block)
        .expect("set leading display");
    tree.set_width(leading, Length::points(22.0))
        .expect("set leading width");
    tree.set_height(leading, Length::points(14.0))
        .expect("set leading height");
    tree.set_margin(leading, StandaloneEdge::Right, Length::percent(5.0))
        .expect("set leading right margin");
    tree.set_order(leading, -1).expect("set leading order");

    let middle = tree.create_default_node();
    tree.set_display(middle, Display::Block)
        .expect("set middle display");
    tree.set_width(middle, Length::points(18.0))
        .expect("set middle width");
    tree.set_height(middle, Length::points(16.0))
        .expect("set middle height");
    tree.set_margin(middle, StandaloneEdge::Horizontal, Length::percent(2.0))
        .expect("set middle horizontal margins");

    tree.append_child(root, trailing).expect("append trailing");
    tree.append_child(root, hidden).expect("append hidden");
    tree.append_child(root, middle).expect("append middle");
    tree.append_child(root, leading).expect("append leading");

    run_standalone_rust(tree, root, Constraints::definite(150.0, 50.0))
        .expect("horizontal-reverse linear order skips display-none with percent margins parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_rtl_horizontal_linear_order_skips_display_none_with_center_gravity()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_direction(root, Direction::Rtl)
        .expect("set root direction");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::Center)
        .expect("set root linear gravity");
    tree.set_align_items(root, AlignItems::FlexStart)
        .expect("set root align items");
    tree.set_width(root, Length::points(156.0))
        .expect("set root width");
    tree.set_height(root, Length::points(54.0))
        .expect("set root height");

    let later = tree.create_default_node();
    tree.set_display(later, Display::Block)
        .expect("set later display");
    tree.set_width(later, Length::points(20.0))
        .expect("set later width");
    tree.set_height(later, Length::points(12.0))
        .expect("set later height");
    tree.set_order(later, 4).expect("set later order");

    let hidden = tree.create_default_node();
    tree.set_display(hidden, Display::None)
        .expect("set hidden display");
    tree.set_width(hidden, Length::points(70.0))
        .expect("set hidden width");
    tree.set_height(hidden, Length::points(22.0))
        .expect("set hidden height");
    tree.set_order(hidden, -4).expect("set hidden order");

    let first = tree.create_default_node();
    tree.set_display(first, Display::Block)
        .expect("set first display");
    tree.set_width(first, Length::points(26.0))
        .expect("set first width");
    tree.set_height(first, Length::points(18.0))
        .expect("set first height");
    tree.set_margin(first, StandaloneEdge::Left, Length::calc(3.0, 4.0))
        .expect("set first left margin");
    tree.set_order(first, -1).expect("set first order");

    let middle = tree.create_default_node();
    tree.set_display(middle, Display::Block)
        .expect("set middle display");
    tree.set_width(middle, Length::points(18.0))
        .expect("set middle width");
    tree.set_height(middle, Length::points(16.0))
        .expect("set middle height");
    tree.set_margin(middle, StandaloneEdge::Right, Length::calc(2.0, 5.0))
        .expect("set middle right margin");

    tree.append_child(root, later).expect("append later");
    tree.append_child(root, hidden).expect("append hidden");
    tree.append_child(root, middle).expect("append middle");
    tree.append_child(root, first).expect("append first");

    run_standalone_rust(tree, root, Constraints::definite(156.0, 54.0))
        .expect("RTL horizontal linear order skips display-none with center gravity parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_rtl_horizontal_reverse_linear_order_skips_display_none_with_padding_border()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_direction(root, Direction::Rtl)
        .expect("set root direction");
    tree.set_linear_orientation(root, LinearOrientation::HorizontalReverse)
        .expect("set root orientation");
    tree.set_align_items(root, AlignItems::FlexStart)
        .expect("set root align items");
    tree.set_width(root, Length::points(148.0))
        .expect("set root width");
    tree.set_height(root, Length::points(52.0))
        .expect("set root height");
    tree.set_padding(root, StandaloneEdge::Horizontal, Length::points(5.0))
        .expect("set root horizontal padding");
    tree.set_border(root, StandaloneEdge::Horizontal, 1.0)
        .expect("set root horizontal border");

    let late = tree.create_default_node();
    tree.set_display(late, Display::Block)
        .expect("set late display");
    tree.set_width(late, Length::points(18.0))
        .expect("set late width");
    tree.set_height(late, Length::points(12.0))
        .expect("set late height");
    tree.set_order(late, 2).expect("set late order");

    let hidden = tree.create_default_node();
    tree.set_display(hidden, Display::None)
        .expect("set hidden display");
    tree.set_width(hidden, Length::points(64.0))
        .expect("set hidden width");
    tree.set_height(hidden, Length::points(18.0))
        .expect("set hidden height");
    tree.set_order(hidden, -2).expect("set hidden order");

    let early = tree.create_default_node();
    tree.set_display(early, Display::Block)
        .expect("set early display");
    tree.set_width(early, Length::points(24.0))
        .expect("set early width");
    tree.set_height(early, Length::points(16.0))
        .expect("set early height");
    tree.set_order(early, -1).expect("set early order");

    let middle = tree.create_default_node();
    tree.set_display(middle, Display::Block)
        .expect("set middle display");
    tree.set_width(middle, Length::points(20.0))
        .expect("set middle width");
    tree.set_height(middle, Length::points(14.0))
        .expect("set middle height");

    tree.append_child(root, late).expect("append late");
    tree.append_child(root, hidden).expect("append hidden");
    tree.append_child(root, middle).expect("append middle");
    tree.append_child(root, early).expect("append early");

    run_standalone_rust(tree, root, Constraints::definite(148.0, 52.0)).expect(
        "RTL horizontal-reverse linear order skips display-none with padding/border parity",
    );
}

#[test]
fn standalone_owned_tree_matches_cpp_for_vertical_reverse_linear_order_skips_display_none_with_calc_main_margins()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::VerticalReverse)
        .expect("set root orientation");
    tree.set_align_items(root, AlignItems::FlexStart)
        .expect("set root align items");
    tree.set_width(root, Length::points(80.0))
        .expect("set root width");
    tree.set_height(root, Length::points(158.0))
        .expect("set root height");

    let later = tree.create_default_node();
    tree.set_display(later, Display::Block)
        .expect("set later display");
    tree.set_width(later, Length::points(20.0))
        .expect("set later width");
    tree.set_height(later, Length::points(18.0))
        .expect("set later height");
    tree.set_margin(later, StandaloneEdge::Top, Length::calc(2.0, 4.0))
        .expect("set later top margin");
    tree.set_order(later, 2).expect("set later order");

    let hidden = tree.create_default_node();
    tree.set_display(hidden, Display::None)
        .expect("set hidden display");
    tree.set_width(hidden, Length::points(60.0))
        .expect("set hidden width");
    tree.set_height(hidden, Length::points(90.0))
        .expect("set hidden height");
    tree.set_order(hidden, -3).expect("set hidden order");

    let earlier = tree.create_default_node();
    tree.set_display(earlier, Display::Block)
        .expect("set earlier display");
    tree.set_width(earlier, Length::points(26.0))
        .expect("set earlier width");
    tree.set_height(earlier, Length::points(24.0))
        .expect("set earlier height");
    tree.set_margin(earlier, StandaloneEdge::Bottom, Length::calc(3.0, 5.0))
        .expect("set earlier bottom margin");
    tree.set_order(earlier, -1).expect("set earlier order");

    let middle = tree.create_default_node();
    tree.set_display(middle, Display::Block)
        .expect("set middle display");
    tree.set_width(middle, Length::points(22.0))
        .expect("set middle width");
    tree.set_height(middle, Length::points(20.0))
        .expect("set middle height");
    tree.set_margin(middle, StandaloneEdge::Vertical, Length::calc(1.0, 3.0))
        .expect("set middle vertical margins");

    tree.append_child(root, later).expect("append later");
    tree.append_child(root, hidden).expect("append hidden");
    tree.append_child(root, middle).expect("append middle");
    tree.append_child(root, earlier).expect("append earlier");

    run_standalone_rust(tree, root, Constraints::definite(80.0, 158.0))
        .expect("vertical-reverse linear order skips display-none with calc margins parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_vertical_linear_order_skips_display_none_weight_distribution()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Vertical)
        .expect("set root orientation");
    tree.set_align_items(root, AlignItems::FlexStart)
        .expect("set root align items");
    tree.set_width(root, Length::points(84.0))
        .expect("set root width");
    tree.set_height(root, Length::points(170.0))
        .expect("set root height");
    tree.set_padding(root, StandaloneEdge::Vertical, Length::points(4.0))
        .expect("set root vertical padding");
    tree.set_border(root, StandaloneEdge::Vertical, 1.0)
        .expect("set root vertical border");

    let fixed_late = tree.create_default_node();
    tree.set_display(fixed_late, Display::Block)
        .expect("set fixed late display");
    tree.set_width(fixed_late, Length::points(20.0))
        .expect("set fixed late width");
    tree.set_height(fixed_late, Length::points(18.0))
        .expect("set fixed late height");
    tree.set_order(fixed_late, 3).expect("set fixed late order");

    let hidden_weighted = tree.create_default_node();
    tree.set_display(hidden_weighted, Display::None)
        .expect("set hidden weighted display");
    tree.set_width(hidden_weighted, Length::points(80.0))
        .expect("set hidden weighted width");
    tree.set_height(hidden_weighted, Length::points(60.0))
        .expect("set hidden weighted height");
    tree.set_linear_weight(hidden_weighted, 5.0)
        .expect("set hidden weighted weight");
    tree.set_order(hidden_weighted, -4)
        .expect("set hidden weighted order");

    let weighted_first = tree.create_default_node();
    tree.set_display(weighted_first, Display::Block)
        .expect("set weighted first display");
    tree.set_width(weighted_first, Length::points(24.0))
        .expect("set weighted first width");
    tree.set_linear_weight(weighted_first, 1.0)
        .expect("set weighted first weight");
    tree.set_min_height(weighted_first, Length::points(20.0))
        .expect("set weighted first min height");
    tree.set_order(weighted_first, -1)
        .expect("set weighted first order");

    let weighted_second = tree.create_default_node();
    tree.set_display(weighted_second, Display::Block)
        .expect("set weighted second display");
    tree.set_width(weighted_second, Length::points(22.0))
        .expect("set weighted second width");
    tree.set_linear_weight(weighted_second, 2.0)
        .expect("set weighted second weight");
    tree.set_max_height(weighted_second, Length::points(70.0))
        .expect("set weighted second max height");

    tree.append_child(root, fixed_late)
        .expect("append fixed late");
    tree.append_child(root, hidden_weighted)
        .expect("append hidden weighted");
    tree.append_child(root, weighted_second)
        .expect("append weighted second");
    tree.append_child(root, weighted_first)
        .expect("append weighted first");

    run_standalone_rust(tree, root, Constraints::definite(84.0, 170.0))
        .expect("vertical linear order skips display-none weight distribution parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_horizontal_linear_indefinite_container_order_skips_display_none_with_percent_margins()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root orientation");
    tree.set_align_items(root, AlignItems::FlexStart)
        .expect("set root align items");
    tree.set_padding(root, StandaloneEdge::Left, Length::points(5.0))
        .expect("set root left padding");
    tree.set_padding(root, StandaloneEdge::Right, Length::points(7.0))
        .expect("set root right padding");
    tree.set_padding(root, StandaloneEdge::Top, Length::points(3.0))
        .expect("set root top padding");
    tree.set_padding(root, StandaloneEdge::Bottom, Length::points(4.0))
        .expect("set root bottom padding");
    tree.set_border(root, StandaloneEdge::Horizontal, 1.0)
        .expect("set root horizontal border");

    let later = tree.create_default_measured_node(Size::new(22.0, 14.0));
    tree.set_display(later, Display::Block)
        .expect("set later display");
    tree.set_box_sizing(later, BoxSizing::ContentBox)
        .expect("set later box sizing");
    tree.set_margin(later, StandaloneEdge::Left, Length::percent(4.0))
        .expect("set later left percent margin");
    tree.set_margin(later, StandaloneEdge::Right, Length::percent(2.0))
        .expect("set later right percent margin");
    tree.set_order(later, 3).expect("set later order");

    let hidden = tree.create_default_node();
    tree.set_display(hidden, Display::None)
        .expect("set hidden display");
    tree.set_width(hidden, Length::points(120.0))
        .expect("set hidden width");
    tree.set_height(hidden, Length::points(60.0))
        .expect("set hidden height");
    tree.set_margin(hidden, StandaloneEdge::Horizontal, Length::percent(8.0))
        .expect("set hidden horizontal margin");
    tree.set_order(hidden, -4).expect("set hidden order");

    let earlier = tree.create_default_measured_node(Size::new(34.0, 18.0));
    tree.set_display(earlier, Display::Block)
        .expect("set earlier display");
    tree.set_box_sizing(earlier, BoxSizing::ContentBox)
        .expect("set earlier box sizing");
    tree.set_margin(earlier, StandaloneEdge::Left, Length::percent(3.0))
        .expect("set earlier left percent margin");
    tree.set_margin(earlier, StandaloneEdge::Right, Length::percent(5.0))
        .expect("set earlier right percent margin");
    tree.set_order(earlier, -1).expect("set earlier order");

    let middle = tree.create_default_measured_node(Size::new(18.0, 16.0));
    tree.set_display(middle, Display::Block)
        .expect("set middle display");
    tree.set_box_sizing(middle, BoxSizing::ContentBox)
        .expect("set middle box sizing");
    tree.set_margin(middle, StandaloneEdge::Horizontal, Length::points(2.0))
        .expect("set middle horizontal margin");

    tree.append_child(root, later).expect("append later");
    tree.append_child(root, hidden).expect("append hidden");
    tree.append_child(root, middle).expect("append middle");
    tree.append_child(root, earlier).expect("append earlier");

    run_standalone_rust(tree, root, Constraints::indefinite()).expect(
        "horizontal linear indefinite container order skips display-none with percent margins parity",
    );
}

#[test]
fn standalone_owned_tree_matches_cpp_for_vertical_linear_indefinite_container_order_skips_display_none_with_calc_margins()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Vertical)
        .expect("set root orientation");
    tree.set_align_items(root, AlignItems::FlexStart)
        .expect("set root align items");
    tree.set_padding(root, StandaloneEdge::Top, Length::points(6.0))
        .expect("set root top padding");
    tree.set_padding(root, StandaloneEdge::Bottom, Length::points(5.0))
        .expect("set root bottom padding");
    tree.set_padding(root, StandaloneEdge::Left, Length::points(4.0))
        .expect("set root left padding");
    tree.set_border(root, StandaloneEdge::Vertical, 1.0)
        .expect("set root vertical border");

    let later = tree.create_default_measured_node(Size::new(18.0, 22.0));
    tree.set_display(later, Display::Block)
        .expect("set later display");
    tree.set_box_sizing(later, BoxSizing::ContentBox)
        .expect("set later box sizing");
    tree.set_margin(later, StandaloneEdge::Top, Length::calc(3.0, 4.0))
        .expect("set later top margin");
    tree.set_margin(later, StandaloneEdge::Bottom, Length::calc(2.0, 5.0))
        .expect("set later bottom margin");
    tree.set_order(later, 4).expect("set later order");

    let hidden = tree.create_default_node();
    tree.set_display(hidden, Display::None)
        .expect("set hidden display");
    tree.set_width(hidden, Length::points(80.0))
        .expect("set hidden width");
    tree.set_height(hidden, Length::points(120.0))
        .expect("set hidden height");
    tree.set_margin(hidden, StandaloneEdge::Vertical, Length::calc(4.0, 8.0))
        .expect("set hidden vertical margin");
    tree.set_order(hidden, -3).expect("set hidden order");

    let earlier = tree.create_default_measured_node(Size::new(26.0, 34.0));
    tree.set_display(earlier, Display::Block)
        .expect("set earlier display");
    tree.set_box_sizing(earlier, BoxSizing::ContentBox)
        .expect("set earlier box sizing");
    tree.set_margin(earlier, StandaloneEdge::Top, Length::calc(2.0, 6.0))
        .expect("set earlier top margin");
    tree.set_margin(earlier, StandaloneEdge::Bottom, Length::calc(3.0, 4.0))
        .expect("set earlier bottom margin");
    tree.set_order(earlier, -1).expect("set earlier order");

    let middle = tree.create_default_measured_node(Size::new(20.0, 18.0));
    tree.set_display(middle, Display::Block)
        .expect("set middle display");
    tree.set_box_sizing(middle, BoxSizing::ContentBox)
        .expect("set middle box sizing");
    tree.set_margin(middle, StandaloneEdge::Vertical, Length::points(2.0))
        .expect("set middle vertical margin");

    tree.append_child(root, later).expect("append later");
    tree.append_child(root, hidden).expect("append hidden");
    tree.append_child(root, earlier).expect("append earlier");
    tree.append_child(root, middle).expect("append middle");

    run_standalone_rust(tree, root, Constraints::indefinite()).expect(
        "vertical linear indefinite container order skips display-none with calc margins parity",
    );
}

#[test]
fn standalone_owned_tree_matches_cpp_for_horizontal_linear_at_most_main_axis_space_between_order_skips_display_none()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::SpaceBetween)
        .expect("set root linear gravity");
    tree.set_align_items(root, AlignItems::FlexStart)
        .expect("set root align items");
    tree.set_height(root, Length::points(42.0))
        .expect("set root height");
    tree.set_padding(root, StandaloneEdge::Horizontal, Length::points(3.0))
        .expect("set root horizontal padding");
    tree.set_border(root, StandaloneEdge::Horizontal, 1.0)
        .expect("set root horizontal border");

    let last = tree.create_default_measured_node(Size::new(24.0, 12.0));
    tree.set_display(last, Display::Block)
        .expect("set last display");
    tree.set_box_sizing(last, BoxSizing::ContentBox)
        .expect("set last box sizing");
    tree.set_margin(last, StandaloneEdge::Left, Length::points(2.0))
        .expect("set last left margin");
    tree.set_order(last, 5).expect("set last order");

    let hidden = tree.create_default_node();
    tree.set_display(hidden, Display::None)
        .expect("set hidden display");
    tree.set_width(hidden, Length::points(90.0))
        .expect("set hidden width");
    tree.set_height(hidden, Length::points(22.0))
        .expect("set hidden height");
    tree.set_order(hidden, -5).expect("set hidden order");

    let first = tree.create_default_measured_node(Size::new(32.0, 18.0));
    tree.set_display(first, Display::Block)
        .expect("set first display");
    tree.set_box_sizing(first, BoxSizing::ContentBox)
        .expect("set first box sizing");
    tree.set_margin(first, StandaloneEdge::Right, Length::points(4.0))
        .expect("set first right margin");
    tree.set_order(first, -1).expect("set first order");

    let middle = tree.create_default_measured_node(Size::new(20.0, 16.0));
    tree.set_display(middle, Display::Block)
        .expect("set middle display");
    tree.set_box_sizing(middle, BoxSizing::ContentBox)
        .expect("set middle box sizing");
    tree.set_margin(middle, StandaloneEdge::Horizontal, Length::points(1.0))
        .expect("set middle horizontal margin");

    tree.append_child(root, last).expect("append last");
    tree.append_child(root, hidden).expect("append hidden");
    tree.append_child(root, middle).expect("append middle");
    tree.append_child(root, first).expect("append first");

    run_standalone_rust(
        tree,
        root,
        Constraints::new(
            SideConstraint::at_most(116.0),
            SideConstraint::definite(42.0),
        ),
    )
    .expect("horizontal linear AtMost main-axis space-between order skips display-none parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_vertical_reverse_linear_at_most_main_axis_center_order_skips_display_none()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::VerticalReverse)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::Center)
        .expect("set root linear gravity");
    tree.set_align_items(root, AlignItems::FlexStart)
        .expect("set root align items");
    tree.set_width(root, Length::points(72.0))
        .expect("set root width");
    tree.set_padding(root, StandaloneEdge::Vertical, Length::points(4.0))
        .expect("set root vertical padding");
    tree.set_border(root, StandaloneEdge::Vertical, 1.0)
        .expect("set root vertical border");

    let later = tree.create_default_measured_node(Size::new(22.0, 20.0));
    tree.set_display(later, Display::Block)
        .expect("set later display");
    tree.set_box_sizing(later, BoxSizing::ContentBox)
        .expect("set later box sizing");
    tree.set_margin(later, StandaloneEdge::Top, Length::calc(2.0, 4.0))
        .expect("set later top margin");
    tree.set_order(later, 4).expect("set later order");

    let hidden = tree.create_default_node();
    tree.set_display(hidden, Display::None)
        .expect("set hidden display");
    tree.set_width(hidden, Length::points(60.0))
        .expect("set hidden width");
    tree.set_height(hidden, Length::points(100.0))
        .expect("set hidden height");
    tree.set_order(hidden, -4).expect("set hidden order");

    let earlier = tree.create_default_measured_node(Size::new(28.0, 26.0));
    tree.set_display(earlier, Display::Block)
        .expect("set earlier display");
    tree.set_box_sizing(earlier, BoxSizing::ContentBox)
        .expect("set earlier box sizing");
    tree.set_margin(earlier, StandaloneEdge::Bottom, Length::calc(3.0, 5.0))
        .expect("set earlier bottom margin");
    tree.set_order(earlier, -1).expect("set earlier order");

    let middle = tree.create_default_measured_node(Size::new(18.0, 18.0));
    tree.set_display(middle, Display::Block)
        .expect("set middle display");
    tree.set_box_sizing(middle, BoxSizing::ContentBox)
        .expect("set middle box sizing");
    tree.set_margin(middle, StandaloneEdge::Vertical, Length::points(2.0))
        .expect("set middle vertical margin");

    tree.append_child(root, later).expect("append later");
    tree.append_child(root, hidden).expect("append hidden");
    tree.append_child(root, middle).expect("append middle");
    tree.append_child(root, earlier).expect("append earlier");

    run_standalone_rust(
        tree,
        root,
        Constraints::new(
            SideConstraint::definite(72.0),
            SideConstraint::at_most(120.0),
        ),
    )
    .expect("vertical-reverse linear AtMost main-axis center order skips display-none parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_rtl_horizontal_linear_auto_cross_axis_order_skips_display_none_with_auto_margins()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_direction(root, Direction::Rtl)
        .expect("set root direction");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root orientation");
    tree.set_align_items(root, AlignItems::FlexEnd)
        .expect("set root align items");
    tree.set_width(root, Length::points(128.0))
        .expect("set root width");
    tree.set_padding(root, StandaloneEdge::Top, Length::points(4.0))
        .expect("set root top padding");
    tree.set_padding(root, StandaloneEdge::Bottom, Length::points(6.0))
        .expect("set root bottom padding");
    tree.set_border(root, StandaloneEdge::Vertical, 1.0)
        .expect("set root vertical border");

    let later = tree.create_default_measured_node(Size::new(20.0, 18.0));
    tree.set_display(later, Display::Block)
        .expect("set later display");
    tree.set_box_sizing(later, BoxSizing::ContentBox)
        .expect("set later box sizing");
    tree.set_margin(later, StandaloneEdge::Top, Length::Auto)
        .expect("set later top auto margin");
    tree.set_order(later, 3).expect("set later order");

    let hidden = tree.create_default_node();
    tree.set_display(hidden, Display::None)
        .expect("set hidden display");
    tree.set_width(hidden, Length::points(88.0))
        .expect("set hidden width");
    tree.set_height(hidden, Length::points(70.0))
        .expect("set hidden height");
    tree.set_margin(hidden, StandaloneEdge::Vertical, Length::Auto)
        .expect("set hidden auto margin");
    tree.set_order(hidden, -5).expect("set hidden order");

    let earlier = tree.create_default_measured_node(Size::new(26.0, 22.0));
    tree.set_display(earlier, Display::Block)
        .expect("set earlier display");
    tree.set_box_sizing(earlier, BoxSizing::ContentBox)
        .expect("set earlier box sizing");
    tree.set_margin(earlier, StandaloneEdge::Bottom, Length::Auto)
        .expect("set earlier bottom auto margin");
    tree.set_order(earlier, -1).expect("set earlier order");

    let middle = tree.create_default_measured_node(Size::new(18.0, 16.0));
    tree.set_display(middle, Display::Block)
        .expect("set middle display");
    tree.set_box_sizing(middle, BoxSizing::ContentBox)
        .expect("set middle box sizing");
    tree.set_margin(middle, StandaloneEdge::Top, Length::percent(4.0))
        .expect("set middle top margin");
    tree.set_margin(middle, StandaloneEdge::Bottom, Length::percent(3.0))
        .expect("set middle bottom margin");

    tree.append_child(root, later).expect("append later");
    tree.append_child(root, hidden).expect("append hidden");
    tree.append_child(root, middle).expect("append middle");
    tree.append_child(root, earlier).expect("append earlier");

    run_standalone_rust(tree, root, Constraints::definite(128.0, 90.0)).expect(
        "RTL horizontal linear auto cross-axis order skips display-none with auto margins parity",
    );
}

#[test]
fn standalone_owned_tree_matches_cpp_for_vertical_linear_auto_cross_axis_order_skips_display_none_with_layout_gravity_end()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Vertical)
        .expect("set root orientation");
    tree.set_linear_cross_gravity(root, LinearCrossGravity::Center)
        .expect("set root cross gravity");
    tree.set_width(root, Length::points(96.0))
        .expect("set root width");
    tree.set_height(root, Length::points(150.0))
        .expect("set root height");
    tree.set_padding(root, StandaloneEdge::Horizontal, Length::points(5.0))
        .expect("set root horizontal padding");
    tree.set_border(root, StandaloneEdge::Horizontal, 1.0)
        .expect("set root horizontal border");

    let later = tree.create_default_measured_node(Size::new(22.0, 20.0));
    tree.set_display(later, Display::Block)
        .expect("set later display");
    tree.set_box_sizing(later, BoxSizing::ContentBox)
        .expect("set later box sizing");
    tree.set_linear_layout_gravity(later, LinearLayoutGravity::End)
        .expect("set later layout gravity");
    tree.set_margin(later, StandaloneEdge::Left, Length::calc(2.0, 5.0))
        .expect("set later left margin");
    tree.set_order(later, 4).expect("set later order");

    let hidden = tree.create_default_node();
    tree.set_display(hidden, Display::None)
        .expect("set hidden display");
    tree.set_width(hidden, Length::points(120.0))
        .expect("set hidden width");
    tree.set_height(hidden, Length::points(80.0))
        .expect("set hidden height");
    tree.set_linear_layout_gravity(hidden, LinearLayoutGravity::End)
        .expect("set hidden layout gravity");
    tree.set_order(hidden, -4).expect("set hidden order");

    let earlier = tree.create_default_measured_node(Size::new(34.0, 26.0));
    tree.set_display(earlier, Display::Block)
        .expect("set earlier display");
    tree.set_box_sizing(earlier, BoxSizing::ContentBox)
        .expect("set earlier box sizing");
    tree.set_linear_layout_gravity(earlier, LinearLayoutGravity::End)
        .expect("set earlier layout gravity");
    tree.set_margin(earlier, StandaloneEdge::Right, Length::calc(3.0, 4.0))
        .expect("set earlier right margin");
    tree.set_order(earlier, -1).expect("set earlier order");

    let middle = tree.create_default_measured_node(Size::new(20.0, 18.0));
    tree.set_display(middle, Display::Block)
        .expect("set middle display");
    tree.set_box_sizing(middle, BoxSizing::ContentBox)
        .expect("set middle box sizing");
    tree.set_margin(middle, StandaloneEdge::Horizontal, Length::percent(3.0))
        .expect("set middle horizontal margin");

    tree.append_child(root, later).expect("append later");
    tree.append_child(root, hidden).expect("append hidden");
    tree.append_child(root, middle).expect("append middle");
    tree.append_child(root, earlier).expect("append earlier");

    run_standalone_rust(tree, root, Constraints::definite(96.0, 150.0)).expect(
        "vertical linear auto cross-axis order skips display-none with layout-gravity end parity",
    );
}

#[test]
fn standalone_owned_tree_matches_cpp_for_horizontal_reverse_linear_cross_gravity_start_order_skips_display_none_with_percent_margins()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::HorizontalReverse)
        .expect("set root orientation");
    tree.set_linear_cross_gravity(root, LinearCrossGravity::Start)
        .expect("set root cross gravity");
    tree.set_width(root, Length::points(136.0))
        .expect("set root width");
    tree.set_height(root, Length::points(94.0))
        .expect("set root height");
    tree.set_padding(root, StandaloneEdge::Top, Length::points(4.0))
        .expect("set root top padding");
    tree.set_padding(root, StandaloneEdge::Bottom, Length::points(5.0))
        .expect("set root bottom padding");
    tree.set_border(root, StandaloneEdge::Vertical, 1.0)
        .expect("set root vertical border");

    let later = tree.create_default_measured_node(Size::new(24.0, 18.0));
    tree.set_display(later, Display::Block)
        .expect("set later display");
    tree.set_box_sizing(later, BoxSizing::ContentBox)
        .expect("set later box sizing");
    tree.set_margin(later, StandaloneEdge::Top, Length::percent(5.0))
        .expect("set later top margin");
    tree.set_order(later, 3).expect("set later order");

    let hidden = tree.create_default_node();
    tree.set_display(hidden, Display::None)
        .expect("set hidden display");
    tree.set_width(hidden, Length::points(90.0))
        .expect("set hidden width");
    tree.set_height(hidden, Length::points(70.0))
        .expect("set hidden height");
    tree.set_linear_layout_gravity(hidden, LinearLayoutGravity::Bottom)
        .expect("set hidden layout gravity");
    tree.set_order(hidden, -5).expect("set hidden order");

    let earlier = tree.create_default_measured_node(Size::new(30.0, 22.0));
    tree.set_display(earlier, Display::Block)
        .expect("set earlier display");
    tree.set_box_sizing(earlier, BoxSizing::ContentBox)
        .expect("set earlier box sizing");
    tree.set_margin(earlier, StandaloneEdge::Bottom, Length::percent(4.0))
        .expect("set earlier bottom margin");
    tree.set_order(earlier, -1).expect("set earlier order");

    let middle = tree.create_default_measured_node(Size::new(18.0, 16.0));
    tree.set_display(middle, Display::Block)
        .expect("set middle display");
    tree.set_box_sizing(middle, BoxSizing::ContentBox)
        .expect("set middle box sizing");
    tree.set_margin(middle, StandaloneEdge::Vertical, Length::percent(3.0))
        .expect("set middle vertical margin");

    tree.append_child(root, later).expect("append later");
    tree.append_child(root, hidden).expect("append hidden");
    tree.append_child(root, middle).expect("append middle");
    tree.append_child(root, earlier).expect("append earlier");

    run_standalone_rust(tree, root, Constraints::definite(136.0, 94.0)).expect(
        "horizontal-reverse linear cross-gravity start order skips display-none with percent margins parity",
    );
}

#[test]
fn standalone_owned_tree_matches_cpp_for_horizontal_reverse_linear_layout_gravity_top_order_skips_display_none_with_calc_margins()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::HorizontalReverse)
        .expect("set root orientation");
    tree.set_linear_cross_gravity(root, LinearCrossGravity::Center)
        .expect("set root cross gravity");
    tree.set_align_items(root, AlignItems::FlexEnd)
        .expect("set root align items");
    tree.set_width(root, Length::points(142.0))
        .expect("set root width");
    tree.set_height(root, Length::points(92.0))
        .expect("set root height");
    tree.set_padding(root, StandaloneEdge::Vertical, Length::points(3.0))
        .expect("set root vertical padding");
    tree.set_border(root, StandaloneEdge::Vertical, 1.0)
        .expect("set root vertical border");

    let later = tree.create_default_measured_node(Size::new(22.0, 18.0));
    tree.set_display(later, Display::Block)
        .expect("set later display");
    tree.set_box_sizing(later, BoxSizing::ContentBox)
        .expect("set later box sizing");
    tree.set_linear_layout_gravity(later, LinearLayoutGravity::Top)
        .expect("set later layout gravity");
    tree.set_margin(later, StandaloneEdge::Top, Length::calc(3.0, 6.0))
        .expect("set later top margin");
    tree.set_order(later, 4).expect("set later order");

    let hidden = tree.create_default_node();
    tree.set_display(hidden, Display::None)
        .expect("set hidden display");
    tree.set_width(hidden, Length::points(100.0))
        .expect("set hidden width");
    tree.set_height(hidden, Length::points(74.0))
        .expect("set hidden height");
    tree.set_linear_layout_gravity(hidden, LinearLayoutGravity::Bottom)
        .expect("set hidden layout gravity");
    tree.set_margin(hidden, StandaloneEdge::Vertical, Length::calc(8.0, 8.0))
        .expect("set hidden vertical margin");
    tree.set_order(hidden, -6).expect("set hidden order");

    let earlier = tree.create_default_measured_node(Size::new(28.0, 20.0));
    tree.set_display(earlier, Display::Block)
        .expect("set earlier display");
    tree.set_box_sizing(earlier, BoxSizing::ContentBox)
        .expect("set earlier box sizing");
    tree.set_align_self(earlier, Some(AlignItems::FlexEnd))
        .expect("set earlier align-self");
    tree.set_linear_layout_gravity(earlier, LinearLayoutGravity::Top)
        .expect("set earlier layout gravity");
    tree.set_margin(earlier, StandaloneEdge::Bottom, Length::calc(4.0, 5.0))
        .expect("set earlier bottom margin");
    tree.set_order(earlier, -1).expect("set earlier order");

    let middle = tree.create_default_measured_node(Size::new(20.0, 16.0));
    tree.set_display(middle, Display::Block)
        .expect("set middle display");
    tree.set_box_sizing(middle, BoxSizing::ContentBox)
        .expect("set middle box sizing");
    tree.set_linear_layout_gravity(middle, LinearLayoutGravity::Top)
        .expect("set middle layout gravity");
    tree.set_margin(middle, StandaloneEdge::Vertical, Length::calc(2.0, 4.0))
        .expect("set middle vertical margin");

    tree.append_child(root, later).expect("append later");
    tree.append_child(root, hidden).expect("append hidden");
    tree.append_child(root, middle).expect("append middle");
    tree.append_child(root, earlier).expect("append earlier");

    run_standalone_rust(tree, root, Constraints::definite(142.0, 92.0)).expect(
        "horizontal-reverse linear layout-gravity top order skips display-none with calc margins parity",
    );
}

#[test]
fn standalone_owned_tree_matches_cpp_for_vertical_reverse_linear_cross_gravity_end_order_skips_display_none_with_percent_margins()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::VerticalReverse)
        .expect("set root orientation");
    tree.set_linear_cross_gravity(root, LinearCrossGravity::End)
        .expect("set root cross gravity");
    tree.set_width(root, Length::points(118.0))
        .expect("set root width");
    tree.set_height(root, Length::points(156.0))
        .expect("set root height");
    tree.set_padding(root, StandaloneEdge::Horizontal, Length::points(4.0))
        .expect("set root horizontal padding");
    tree.set_border(root, StandaloneEdge::Horizontal, 1.0)
        .expect("set root horizontal border");

    let later = tree.create_default_measured_node(Size::new(24.0, 20.0));
    tree.set_display(later, Display::Block)
        .expect("set later display");
    tree.set_box_sizing(later, BoxSizing::ContentBox)
        .expect("set later box sizing");
    tree.set_margin(later, StandaloneEdge::Left, Length::percent(4.0))
        .expect("set later left margin");
    tree.set_order(later, 5).expect("set later order");

    let hidden = tree.create_default_node();
    tree.set_display(hidden, Display::None)
        .expect("set hidden display");
    tree.set_width(hidden, Length::points(120.0))
        .expect("set hidden width");
    tree.set_height(hidden, Length::points(80.0))
        .expect("set hidden height");
    tree.set_linear_cross_gravity(hidden, LinearCrossGravity::Start)
        .expect("set hidden cross gravity");
    tree.set_order(hidden, -7).expect("set hidden order");

    let earlier = tree.create_default_measured_node(Size::new(34.0, 24.0));
    tree.set_display(earlier, Display::Block)
        .expect("set earlier display");
    tree.set_box_sizing(earlier, BoxSizing::ContentBox)
        .expect("set earlier box sizing");
    tree.set_margin(earlier, StandaloneEdge::Right, Length::percent(5.0))
        .expect("set earlier right margin");
    tree.set_order(earlier, -1).expect("set earlier order");

    let middle = tree.create_default_measured_node(Size::new(22.0, 18.0));
    tree.set_display(middle, Display::Block)
        .expect("set middle display");
    tree.set_box_sizing(middle, BoxSizing::ContentBox)
        .expect("set middle box sizing");
    tree.set_margin(middle, StandaloneEdge::Horizontal, Length::percent(3.0))
        .expect("set middle horizontal margin");

    tree.append_child(root, later).expect("append later");
    tree.append_child(root, hidden).expect("append hidden");
    tree.append_child(root, middle).expect("append middle");
    tree.append_child(root, earlier).expect("append earlier");

    run_standalone_rust(tree, root, Constraints::definite(118.0, 156.0)).expect(
        "vertical-reverse linear cross-gravity end order skips display-none with percent margins parity",
    );
}

#[test]
fn standalone_owned_tree_matches_cpp_for_vertical_reverse_linear_align_self_center_order_skips_display_none_with_calc_margins()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::VerticalReverse)
        .expect("set root orientation");
    tree.set_linear_cross_gravity(root, LinearCrossGravity::End)
        .expect("set root cross gravity");
    tree.set_width(root, Length::points(122.0))
        .expect("set root width");
    tree.set_height(root, Length::points(158.0))
        .expect("set root height");
    tree.set_padding(root, StandaloneEdge::Horizontal, Length::points(5.0))
        .expect("set root horizontal padding");
    tree.set_border(root, StandaloneEdge::Horizontal, 1.0)
        .expect("set root horizontal border");

    let later = tree.create_default_measured_node(Size::new(26.0, 18.0));
    tree.set_display(later, Display::Block)
        .expect("set later display");
    tree.set_box_sizing(later, BoxSizing::ContentBox)
        .expect("set later box sizing");
    tree.set_align_self(later, Some(AlignItems::Center))
        .expect("set later align-self");
    tree.set_margin(later, StandaloneEdge::Left, Length::calc(2.0, 7.0))
        .expect("set later left margin");
    tree.set_order(later, 4).expect("set later order");

    let hidden = tree.create_default_node();
    tree.set_display(hidden, Display::None)
        .expect("set hidden display");
    tree.set_width(hidden, Length::points(116.0))
        .expect("set hidden width");
    tree.set_height(hidden, Length::points(90.0))
        .expect("set hidden height");
    tree.set_align_self(hidden, Some(AlignItems::FlexEnd))
        .expect("set hidden align-self");
    tree.set_order(hidden, -6).expect("set hidden order");

    let earlier = tree.create_default_measured_node(Size::new(32.0, 22.0));
    tree.set_display(earlier, Display::Block)
        .expect("set earlier display");
    tree.set_box_sizing(earlier, BoxSizing::ContentBox)
        .expect("set earlier box sizing");
    tree.set_align_self(earlier, Some(AlignItems::Center))
        .expect("set earlier align-self");
    tree.set_margin(earlier, StandaloneEdge::Right, Length::calc(3.0, 5.0))
        .expect("set earlier right margin");
    tree.set_order(earlier, -1).expect("set earlier order");

    let middle = tree.create_default_measured_node(Size::new(20.0, 16.0));
    tree.set_display(middle, Display::Block)
        .expect("set middle display");
    tree.set_box_sizing(middle, BoxSizing::ContentBox)
        .expect("set middle box sizing");
    tree.set_align_self(middle, Some(AlignItems::Center))
        .expect("set middle align-self");
    tree.set_margin(middle, StandaloneEdge::Horizontal, Length::calc(2.0, 4.0))
        .expect("set middle horizontal margin");

    tree.append_child(root, later).expect("append later");
    tree.append_child(root, hidden).expect("append hidden");
    tree.append_child(root, middle).expect("append middle");
    tree.append_child(root, earlier).expect("append earlier");

    run_standalone_rust(tree, root, Constraints::definite(122.0, 158.0)).expect(
        "vertical-reverse linear align-self center order skips display-none with calc margins parity",
    );
}

#[test]
fn standalone_owned_tree_matches_cpp_for_rtl_vertical_linear_layout_gravity_left_order_skips_display_none_with_percent_margins()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_direction(root, Direction::Rtl)
        .expect("set root direction");
    tree.set_linear_orientation(root, LinearOrientation::Vertical)
        .expect("set root orientation");
    tree.set_align_items(root, AlignItems::FlexStart)
        .expect("set root align items");
    tree.set_width(root, Length::points(126.0))
        .expect("set root width");
    tree.set_height(root, Length::points(150.0))
        .expect("set root height");
    tree.set_padding(root, StandaloneEdge::Horizontal, Length::points(4.0))
        .expect("set root horizontal padding");
    tree.set_border(root, StandaloneEdge::Horizontal, 1.0)
        .expect("set root horizontal border");

    let later = tree.create_default_measured_node(Size::new(26.0, 18.0));
    tree.set_display(later, Display::Block)
        .expect("set later display");
    tree.set_box_sizing(later, BoxSizing::ContentBox)
        .expect("set later box sizing");
    tree.set_linear_layout_gravity(later, LinearLayoutGravity::Left)
        .expect("set later layout gravity");
    tree.set_margin(later, StandaloneEdge::Left, Length::percent(3.0))
        .expect("set later left margin");
    tree.set_order(later, 4).expect("set later order");

    let hidden = tree.create_default_node();
    tree.set_display(hidden, Display::None)
        .expect("set hidden display");
    tree.set_width(hidden, Length::points(112.0))
        .expect("set hidden width");
    tree.set_height(hidden, Length::points(82.0))
        .expect("set hidden height");
    tree.set_linear_layout_gravity(hidden, LinearLayoutGravity::Right)
        .expect("set hidden layout gravity");
    tree.set_order(hidden, -5).expect("set hidden order");

    let earlier = tree.create_default_measured_node(Size::new(34.0, 24.0));
    tree.set_display(earlier, Display::Block)
        .expect("set earlier display");
    tree.set_box_sizing(earlier, BoxSizing::ContentBox)
        .expect("set earlier box sizing");
    tree.set_linear_layout_gravity(earlier, LinearLayoutGravity::Left)
        .expect("set earlier layout gravity");
    tree.set_margin(earlier, StandaloneEdge::Right, Length::percent(5.0))
        .expect("set earlier right margin");
    tree.set_order(earlier, -1).expect("set earlier order");

    let middle = tree.create_default_measured_node(Size::new(22.0, 20.0));
    tree.set_display(middle, Display::Block)
        .expect("set middle display");
    tree.set_box_sizing(middle, BoxSizing::ContentBox)
        .expect("set middle box sizing");
    tree.set_linear_layout_gravity(middle, LinearLayoutGravity::Left)
        .expect("set middle layout gravity");
    tree.set_margin(middle, StandaloneEdge::Horizontal, Length::percent(4.0))
        .expect("set middle horizontal margin");

    tree.append_child(root, later).expect("append later");
    tree.append_child(root, hidden).expect("append hidden");
    tree.append_child(root, middle).expect("append middle");
    tree.append_child(root, earlier).expect("append earlier");

    run_standalone_rust(tree, root, Constraints::definite(126.0, 150.0)).expect(
        "RTL vertical linear layout-gravity left order skips display-none with percent margins parity",
    );
}

#[test]
fn standalone_owned_tree_matches_cpp_for_rtl_vertical_reverse_linear_left_auto_cross_margin_order_skips_display_none_with_layout_gravity_right()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_direction(root, Direction::Rtl)
        .expect("set root direction");
    tree.set_linear_orientation(root, LinearOrientation::VerticalReverse)
        .expect("set root orientation");
    tree.set_align_items(root, AlignItems::FlexEnd)
        .expect("set root align items");
    tree.set_width(root, Length::points(130.0))
        .expect("set root width");
    tree.set_height(root, Length::points(152.0))
        .expect("set root height");
    tree.set_padding(root, StandaloneEdge::Horizontal, Length::points(5.0))
        .expect("set root horizontal padding");
    tree.set_border(root, StandaloneEdge::Horizontal, 1.0)
        .expect("set root horizontal border");

    let later = tree.create_default_measured_node(Size::new(24.0, 18.0));
    tree.set_display(later, Display::Block)
        .expect("set later display");
    tree.set_box_sizing(later, BoxSizing::ContentBox)
        .expect("set later box sizing");
    tree.set_linear_layout_gravity(later, LinearLayoutGravity::Right)
        .expect("set later layout gravity");
    tree.set_margin(later, StandaloneEdge::Left, Length::Auto)
        .expect("set later left auto margin");
    tree.set_order(later, 5).expect("set later order");

    let hidden = tree.create_default_node();
    tree.set_display(hidden, Display::None)
        .expect("set hidden display");
    tree.set_width(hidden, Length::points(118.0))
        .expect("set hidden width");
    tree.set_height(hidden, Length::points(88.0))
        .expect("set hidden height");
    tree.set_linear_layout_gravity(hidden, LinearLayoutGravity::Left)
        .expect("set hidden layout gravity");
    tree.set_margin(hidden, StandaloneEdge::Horizontal, Length::Auto)
        .expect("set hidden horizontal auto margins");
    tree.set_order(hidden, -7).expect("set hidden order");

    let earlier = tree.create_default_measured_node(Size::new(36.0, 24.0));
    tree.set_display(earlier, Display::Block)
        .expect("set earlier display");
    tree.set_box_sizing(earlier, BoxSizing::ContentBox)
        .expect("set earlier box sizing");
    tree.set_linear_layout_gravity(earlier, LinearLayoutGravity::Right)
        .expect("set earlier layout gravity");
    tree.set_margin(earlier, StandaloneEdge::Left, Length::Auto)
        .expect("set earlier left auto margin");
    tree.set_margin(earlier, StandaloneEdge::Right, Length::percent(4.0))
        .expect("set earlier right margin");
    tree.set_order(earlier, -1).expect("set earlier order");

    let middle = tree.create_default_measured_node(Size::new(22.0, 20.0));
    tree.set_display(middle, Display::Block)
        .expect("set middle display");
    tree.set_box_sizing(middle, BoxSizing::ContentBox)
        .expect("set middle box sizing");
    tree.set_linear_layout_gravity(middle, LinearLayoutGravity::Right)
        .expect("set middle layout gravity");
    tree.set_margin(middle, StandaloneEdge::Left, Length::Auto)
        .expect("set middle left auto margin");

    tree.append_child(root, later).expect("append later");
    tree.append_child(root, hidden).expect("append hidden");
    tree.append_child(root, middle).expect("append middle");
    tree.append_child(root, earlier).expect("append earlier");

    run_standalone_rust(tree, root, Constraints::definite(130.0, 152.0)).expect(
        "RTL vertical-reverse linear left auto cross-margin order skips display-none with layout-gravity right parity",
    );
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_at_most_main_axis_sizing() {
    let (tree, root, constraints) = linear_at_most_main_axis_sizing_tree();

    run_standalone_rust(tree, root, constraints).expect("linear AtMost main-axis sizing parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_at_most_cross_axis_stretch_suppression() {
    let (tree, root, constraints) = linear_at_most_cross_axis_stretch_suppression_tree();

    run_standalone_rust(tree, root, constraints)
        .expect("linear AtMost cross-axis stretch suppression parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_container_clamp_matrix() {
    for (case_name, tree, root, constraints) in [
        linear_container_min_width_max_height_clamp_tree(),
        linear_container_max_width_min_height_clamp_tree(),
        linear_container_padding_border_tight_constraint_clamp_tree(),
    ] {
        run_standalone_rust(tree, root, constraints)
            .unwrap_or_else(|error| panic!("{case_name} parity failed: {error}"));
    }
}

#[test]
fn standalone_owned_tree_matches_cpp_for_horizontal_linear_at_most_width_center_percent_main_margins()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::Center)
        .expect("set root gravity");
    tree.set_height(root, Length::points(36.0))
        .expect("set root height");
    tree.set_padding(root, StandaloneEdge::Horizontal, Length::points(4.0))
        .expect("set root horizontal padding");
    tree.set_border(root, StandaloneEdge::Horizontal, 1.0)
        .expect("set root horizontal border");

    let first = tree.create_default_node();
    tree.set_display(first, Display::Block)
        .expect("set first display");
    tree.set_width(first, Length::points(30.0))
        .expect("set first width");
    tree.set_height(first, Length::points(18.0))
        .expect("set first height");
    tree.set_margin(first, StandaloneEdge::Left, Length::percent(6.0))
        .expect("set first left margin");
    tree.set_margin(first, StandaloneEdge::Right, Length::percent(4.0))
        .expect("set first right margin");

    let second = tree.create_default_node();
    tree.set_display(second, Display::Block)
        .expect("set second display");
    tree.set_width(second, Length::points(24.0))
        .expect("set second width");
    tree.set_height(second, Length::points(20.0))
        .expect("set second height");
    tree.set_margin(second, StandaloneEdge::Left, Length::points(5.0))
        .expect("set second left margin");
    tree.set_margin(second, StandaloneEdge::Right, Length::points(7.0))
        .expect("set second right margin");

    tree.append_child(root, first).expect("append first");
    tree.append_child(root, second).expect("append second");

    run_standalone_rust(
        tree,
        root,
        Constraints::new(
            SideConstraint::at_most(180.0),
            SideConstraint::definite(36.0),
        ),
    )
    .expect("horizontal linear AtMost-width center percent main margins parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_horizontal_reverse_linear_at_most_width_end_calc_main_margins()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::HorizontalReverse)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::End)
        .expect("set root gravity");
    tree.set_height(root, Length::points(40.0))
        .expect("set root height");
    tree.set_padding(root, StandaloneEdge::Horizontal, Length::points(5.0))
        .expect("set root horizontal padding");
    tree.set_border(root, StandaloneEdge::Horizontal, 1.0)
        .expect("set root horizontal border");

    let first = tree.create_default_node();
    tree.set_display(first, Display::Block)
        .expect("set first display");
    tree.set_width(first, Length::points(28.0))
        .expect("set first width");
    tree.set_height(first, Length::points(18.0))
        .expect("set first height");
    tree.set_margin(first, StandaloneEdge::Left, Length::calc(4.0, 6.0))
        .expect("set first left margin");
    tree.set_margin(first, StandaloneEdge::Right, Length::calc(3.0, 4.0))
        .expect("set first right margin");

    let second = tree.create_default_node();
    tree.set_display(second, Display::Block)
        .expect("set second display");
    tree.set_width(second, Length::points(34.0))
        .expect("set second width");
    tree.set_height(second, Length::points(24.0))
        .expect("set second height");
    tree.set_margin(second, StandaloneEdge::Left, Length::points(6.0))
        .expect("set second left margin");
    tree.set_margin(second, StandaloneEdge::Right, Length::points(8.0))
        .expect("set second right margin");

    tree.append_child(root, first).expect("append first");
    tree.append_child(root, second).expect("append second");

    run_standalone_rust(
        tree,
        root,
        Constraints::new(
            SideConstraint::at_most(190.0),
            SideConstraint::definite(40.0),
        ),
    )
    .expect("horizontal-reverse linear AtMost-width end calc main margins parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_vertical_linear_at_most_height_center_percent_main_margins()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Vertical)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::Center)
        .expect("set root gravity");
    tree.set_width(root, Length::points(48.0))
        .expect("set root width");
    tree.set_padding(root, StandaloneEdge::Vertical, Length::points(4.0))
        .expect("set root vertical padding");
    tree.set_border(root, StandaloneEdge::Vertical, 1.0)
        .expect("set root vertical border");

    let first = tree.create_default_node();
    tree.set_display(first, Display::Block)
        .expect("set first display");
    tree.set_width(first, Length::points(30.0))
        .expect("set first width");
    tree.set_height(first, Length::points(26.0))
        .expect("set first height");
    tree.set_margin(first, StandaloneEdge::Top, Length::percent(5.0))
        .expect("set first top margin");
    tree.set_margin(first, StandaloneEdge::Bottom, Length::percent(7.0))
        .expect("set first bottom margin");

    let second = tree.create_default_node();
    tree.set_display(second, Display::Block)
        .expect("set second display");
    tree.set_width(second, Length::points(34.0))
        .expect("set second width");
    tree.set_height(second, Length::points(22.0))
        .expect("set second height");
    tree.set_margin(second, StandaloneEdge::Top, Length::points(6.0))
        .expect("set second top margin");
    tree.set_margin(second, StandaloneEdge::Bottom, Length::points(8.0))
        .expect("set second bottom margin");

    tree.append_child(root, first).expect("append first");
    tree.append_child(root, second).expect("append second");

    run_standalone_rust(
        tree,
        root,
        Constraints::new(
            SideConstraint::definite(48.0),
            SideConstraint::at_most(180.0),
        ),
    )
    .expect("vertical linear AtMost-height center percent main margins parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_vertical_reverse_linear_at_most_height_bottom_calc_main_margins()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::VerticalReverse)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::Bottom)
        .expect("set root gravity");
    tree.set_width(root, Length::points(54.0))
        .expect("set root width");
    tree.set_padding(root, StandaloneEdge::Vertical, Length::points(5.0))
        .expect("set root vertical padding");
    tree.set_border(root, StandaloneEdge::Vertical, 1.0)
        .expect("set root vertical border");

    let first = tree.create_default_node();
    tree.set_display(first, Display::Block)
        .expect("set first display");
    tree.set_width(first, Length::points(32.0))
        .expect("set first width");
    tree.set_height(first, Length::points(24.0))
        .expect("set first height");
    tree.set_margin(first, StandaloneEdge::Top, Length::calc(3.0, 5.0))
        .expect("set first top margin");
    tree.set_margin(first, StandaloneEdge::Bottom, Length::calc(4.0, 6.0))
        .expect("set first bottom margin");

    let second = tree.create_default_node();
    tree.set_display(second, Display::Block)
        .expect("set second display");
    tree.set_width(second, Length::points(42.0))
        .expect("set second width");
    tree.set_height(second, Length::points(30.0))
        .expect("set second height");
    tree.set_margin(second, StandaloneEdge::Top, Length::points(5.0))
        .expect("set second top margin");
    tree.set_margin(second, StandaloneEdge::Bottom, Length::points(7.0))
        .expect("set second bottom margin");

    tree.append_child(root, first).expect("append first");
    tree.append_child(root, second).expect("append second");

    run_standalone_rust(
        tree,
        root,
        Constraints::new(
            SideConstraint::definite(54.0),
            SideConstraint::at_most(190.0),
        ),
    )
    .expect("vertical-reverse linear AtMost-height bottom calc main margins parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_horizontal_linear_at_most_cross_axis_center_percent_margins()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root orientation");
    tree.set_align_items(root, AlignItems::FlexStart)
        .expect("set root align-items");
    tree.set_width(root, Length::points(96.0))
        .expect("set root width");
    tree.set_linear_cross_gravity(root, LinearCrossGravity::Center)
        .expect("set root cross gravity");
    tree.set_padding(root, StandaloneEdge::Vertical, Length::points(4.0))
        .expect("set root vertical padding");
    tree.set_border(root, StandaloneEdge::Vertical, 1.0)
        .expect("set root vertical border");

    let tall = tree.create_default_node();
    tree.set_display(tall, Display::Block)
        .expect("set tall display");
    tree.set_width(tall, Length::points(24.0))
        .expect("set tall width");
    tree.set_height(tall, Length::points(40.0))
        .expect("set tall height");

    let centered = tree.create_default_node();
    tree.set_display(centered, Display::Block)
        .expect("set centered display");
    tree.set_width(centered, Length::points(26.0))
        .expect("set centered width");
    tree.set_height(centered, Length::points(18.0))
        .expect("set centered height");
    tree.set_margin(centered, StandaloneEdge::Top, Length::percent(8.0))
        .expect("set centered top margin");
    tree.set_margin(centered, StandaloneEdge::Bottom, Length::percent(6.0))
        .expect("set centered bottom margin");

    tree.append_child(root, tall).expect("append tall");
    tree.append_child(root, centered).expect("append centered");

    run_standalone_rust(
        tree,
        root,
        Constraints::new(
            SideConstraint::definite(96.0),
            SideConstraint::at_most(120.0),
        ),
    )
    .expect("horizontal linear AtMost cross-axis center percent margins parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_vertical_linear_at_most_cross_axis_center_calc_margins() {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Vertical)
        .expect("set root orientation");
    tree.set_align_items(root, AlignItems::FlexStart)
        .expect("set root align-items");
    tree.set_height(root, Length::points(96.0))
        .expect("set root height");
    tree.set_linear_cross_gravity(root, LinearCrossGravity::Center)
        .expect("set root cross gravity");
    tree.set_padding(root, StandaloneEdge::Horizontal, Length::points(5.0))
        .expect("set root horizontal padding");
    tree.set_border(root, StandaloneEdge::Horizontal, 1.0)
        .expect("set root horizontal border");

    let wide = tree.create_default_node();
    tree.set_display(wide, Display::Block)
        .expect("set wide display");
    tree.set_width(wide, Length::points(44.0))
        .expect("set wide width");
    tree.set_height(wide, Length::points(24.0))
        .expect("set wide height");

    let centered = tree.create_default_node();
    tree.set_display(centered, Display::Block)
        .expect("set centered display");
    tree.set_width(centered, Length::points(20.0))
        .expect("set centered width");
    tree.set_height(centered, Length::points(26.0))
        .expect("set centered height");
    tree.set_margin(centered, StandaloneEdge::Left, Length::calc(3.0, 6.0))
        .expect("set centered left margin");
    tree.set_margin(centered, StandaloneEdge::Right, Length::calc(4.0, 5.0))
        .expect("set centered right margin");

    tree.append_child(root, wide).expect("append wide");
    tree.append_child(root, centered).expect("append centered");

    run_standalone_rust(
        tree,
        root,
        Constraints::new(
            SideConstraint::at_most(128.0),
            SideConstraint::definite(96.0),
        ),
    )
    .expect("vertical linear AtMost cross-axis center calc margins parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_horizontal_reverse_linear_at_most_main_axis_shrink_wraps_content()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_linear_orientation(root, LinearOrientation::HorizontalReverse)
        .expect("set root orientation");
    tree.set_height(root, Length::points(20.0))
        .expect("set root height");

    let first = tree.create_default_node();
    tree.set_display(first, Display::Block)
        .expect("set first display");
    tree.set_width(first, Length::points(10.0))
        .expect("set first width");
    tree.set_height(first, Length::points(20.0))
        .expect("set first height");

    let second = tree.create_default_node();
    tree.set_display(second, Display::Block)
        .expect("set second display");
    tree.set_width(second, Length::points(20.0))
        .expect("set second width");
    tree.set_height(second, Length::points(20.0))
        .expect("set second height");

    tree.append_child(root, first).expect("append first");
    tree.append_child(root, second).expect("append second");

    run_standalone_rust(
        tree,
        root,
        Constraints::new(
            SideConstraint::at_most(100.0),
            SideConstraint::definite(20.0),
        ),
    )
    .expect("horizontal-reverse linear AtMost main-axis shrink-wrap parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_vertical_reverse_linear_at_most_main_axis_overflow_content_size()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_linear_orientation(root, LinearOrientation::VerticalReverse)
        .expect("set root orientation");
    tree.set_width(root, Length::points(20.0))
        .expect("set root width");

    let first = tree.create_default_node();
    tree.set_display(first, Display::Block)
        .expect("set first display");
    tree.set_width(first, Length::points(20.0))
        .expect("set first width");
    tree.set_height(first, Length::points(80.0))
        .expect("set first height");

    let second = tree.create_default_node();
    tree.set_display(second, Display::Block)
        .expect("set second display");
    tree.set_width(second, Length::points(20.0))
        .expect("set second width");
    tree.set_height(second, Length::points(70.0))
        .expect("set second height");

    tree.append_child(root, first).expect("append first");
    tree.append_child(root, second).expect("append second");

    run_standalone_rust(
        tree,
        root,
        Constraints::new(
            SideConstraint::definite(20.0),
            SideConstraint::at_most(100.0),
        ),
    )
    .expect("vertical-reverse linear AtMost main-axis overflow sizing parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_horizontal_reverse_linear_auto_main_percent_margins_keep_initial_size()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_linear_orientation(root, LinearOrientation::HorizontalReverse)
        .expect("set root orientation");
    tree.set_height(root, Length::points(10.0))
        .expect("set root height");

    let child = tree.create_default_node();
    tree.set_display(child, Display::Block)
        .expect("set child display");
    tree.set_width(child, Length::points(100.0))
        .expect("set child width");
    tree.set_height(child, Length::points(10.0))
        .expect("set child height");
    tree.set_margin(child, StandaloneEdge::Left, Length::percent(10.0))
        .expect("set child left margin");
    tree.set_margin(child, StandaloneEdge::Right, Length::percent(10.0))
        .expect("set child right margin");

    tree.append_child(root, child).expect("append child");

    run_standalone_rust(tree, root, Constraints::indefinite())
        .expect("horizontal-reverse linear auto main percent-margin sizing parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_vertical_reverse_linear_auto_main_percent_margins_keep_initial_size()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_linear_orientation(root, LinearOrientation::VerticalReverse)
        .expect("set root orientation");

    let child = tree.create_default_node();
    tree.set_display(child, Display::Block)
        .expect("set child display");
    tree.set_width(child, Length::points(100.0))
        .expect("set child width");
    tree.set_height(child, Length::points(100.0))
        .expect("set child height");
    tree.set_margin(child, StandaloneEdge::Top, Length::percent(10.0))
        .expect("set child top margin");
    tree.set_margin(child, StandaloneEdge::Bottom, Length::percent(10.0))
        .expect("set child bottom margin");

    tree.append_child(root, child).expect("append child");

    run_standalone_rust(tree, root, Constraints::indefinite())
        .expect("vertical-reverse linear auto main percent-margin sizing parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_horizontal_linear_auto_cross_percent_margins_keep_initial_size()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root orientation");
    tree.set_width(root, Length::points(80.0))
        .expect("set root width");

    let child = tree.create_default_node();
    tree.set_display(child, Display::Block)
        .expect("set child display");
    tree.set_width(child, Length::points(30.0))
        .expect("set child width");
    tree.set_height(child, Length::points(40.0))
        .expect("set child height");
    tree.set_margin(child, StandaloneEdge::Top, Length::percent(25.0))
        .expect("set child top margin");
    tree.set_margin(child, StandaloneEdge::Bottom, Length::percent(25.0))
        .expect("set child bottom margin");

    tree.append_child(root, child).expect("append child");

    run_standalone_rust(
        tree,
        root,
        Constraints::new(SideConstraint::definite(80.0), SideConstraint::indefinite()),
    )
    .expect("horizontal linear auto cross percent-margin sizing parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_vertical_linear_auto_cross_percent_margins_keep_initial_size()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_linear_orientation(root, LinearOrientation::Vertical)
        .expect("set root orientation");
    tree.set_height(root, Length::points(80.0))
        .expect("set root height");

    let child = tree.create_default_node();
    tree.set_display(child, Display::Block)
        .expect("set child display");
    tree.set_width(child, Length::points(40.0))
        .expect("set child width");
    tree.set_height(child, Length::points(30.0))
        .expect("set child height");
    tree.set_margin(child, StandaloneEdge::Left, Length::percent(25.0))
        .expect("set child left margin");
    tree.set_margin(child, StandaloneEdge::Right, Length::percent(25.0))
        .expect("set child right margin");

    tree.append_child(root, child).expect("append child");

    run_standalone_rust(
        tree,
        root,
        Constraints::new(SideConstraint::indefinite(), SideConstraint::definite(80.0)),
    )
    .expect("vertical linear auto cross percent-margin sizing parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_horizontal_linear_indefinite_axes_padding_border_percent_margins()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root orientation");
    tree.set_padding(root, StandaloneEdge::Left, Length::points(6.0))
        .expect("set root left padding");
    tree.set_padding(root, StandaloneEdge::Right, Length::points(8.0))
        .expect("set root right padding");
    tree.set_padding(root, StandaloneEdge::Top, Length::points(4.0))
        .expect("set root top padding");
    tree.set_padding(root, StandaloneEdge::Bottom, Length::points(5.0))
        .expect("set root bottom padding");
    tree.set_border(root, StandaloneEdge::Left, 1.0)
        .expect("set root left border");
    tree.set_border(root, StandaloneEdge::Right, 2.0)
        .expect("set root right border");
    tree.set_border(root, StandaloneEdge::Top, 1.0)
        .expect("set root top border");
    tree.set_border(root, StandaloneEdge::Bottom, 2.0)
        .expect("set root bottom border");

    let first = tree.create_default_node();
    tree.set_display(first, Display::Block)
        .expect("set first display");
    tree.set_width(first, Length::points(50.0))
        .expect("set first width");
    tree.set_height(first, Length::points(20.0))
        .expect("set first height");
    tree.set_margin(first, StandaloneEdge::Left, Length::percent(10.0))
        .expect("set first left margin");
    tree.set_margin(first, StandaloneEdge::Right, Length::percent(5.0))
        .expect("set first right margin");
    tree.set_margin(first, StandaloneEdge::Top, Length::percent(12.0))
        .expect("set first top margin");
    tree.set_margin(first, StandaloneEdge::Bottom, Length::percent(8.0))
        .expect("set first bottom margin");

    let second = tree.create_default_node();
    tree.set_display(second, Display::Block)
        .expect("set second display");
    tree.set_width(second, Length::points(30.0))
        .expect("set second width");
    tree.set_height(second, Length::points(35.0))
        .expect("set second height");
    tree.set_margin(second, StandaloneEdge::Left, Length::points(4.0))
        .expect("set second left margin");
    tree.set_margin(second, StandaloneEdge::Right, Length::points(6.0))
        .expect("set second right margin");
    tree.set_margin(second, StandaloneEdge::Top, Length::points(2.0))
        .expect("set second top margin");
    tree.set_margin(second, StandaloneEdge::Bottom, Length::points(3.0))
        .expect("set second bottom margin");

    tree.append_child(root, first).expect("append first");
    tree.append_child(root, second).expect("append second");

    run_standalone_rust(tree, root, Constraints::indefinite()).expect(
        "horizontal linear indefinite axes padding/border percent-margin container sizing parity",
    );
}

#[test]
fn standalone_owned_tree_matches_cpp_for_vertical_reverse_linear_indefinite_axes_padding_border_percent_margins()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::VerticalReverse)
        .expect("set root orientation");
    tree.set_padding(root, StandaloneEdge::Left, Length::points(7.0))
        .expect("set root left padding");
    tree.set_padding(root, StandaloneEdge::Right, Length::points(5.0))
        .expect("set root right padding");
    tree.set_padding(root, StandaloneEdge::Top, Length::points(6.0))
        .expect("set root top padding");
    tree.set_padding(root, StandaloneEdge::Bottom, Length::points(4.0))
        .expect("set root bottom padding");
    tree.set_border(root, StandaloneEdge::Left, 2.0)
        .expect("set root left border");
    tree.set_border(root, StandaloneEdge::Right, 1.0)
        .expect("set root right border");
    tree.set_border(root, StandaloneEdge::Top, 2.0)
        .expect("set root top border");
    tree.set_border(root, StandaloneEdge::Bottom, 1.0)
        .expect("set root bottom border");

    let first = tree.create_default_node();
    tree.set_display(first, Display::Block)
        .expect("set first display");
    tree.set_width(first, Length::points(36.0))
        .expect("set first width");
    tree.set_height(first, Length::points(44.0))
        .expect("set first height");
    tree.set_margin(first, StandaloneEdge::Top, Length::percent(10.0))
        .expect("set first top margin");
    tree.set_margin(first, StandaloneEdge::Bottom, Length::percent(6.0))
        .expect("set first bottom margin");
    tree.set_margin(first, StandaloneEdge::Left, Length::percent(8.0))
        .expect("set first left margin");
    tree.set_margin(first, StandaloneEdge::Right, Length::percent(4.0))
        .expect("set first right margin");

    let second = tree.create_default_node();
    tree.set_display(second, Display::Block)
        .expect("set second display");
    tree.set_width(second, Length::points(48.0))
        .expect("set second width");
    tree.set_height(second, Length::points(18.0))
        .expect("set second height");
    tree.set_margin(second, StandaloneEdge::Top, Length::points(3.0))
        .expect("set second top margin");
    tree.set_margin(second, StandaloneEdge::Bottom, Length::points(5.0))
        .expect("set second bottom margin");
    tree.set_margin(second, StandaloneEdge::Left, Length::points(2.0))
        .expect("set second left margin");
    tree.set_margin(second, StandaloneEdge::Right, Length::points(4.0))
        .expect("set second right margin");

    tree.append_child(root, first).expect("append first");
    tree.append_child(root, second).expect("append second");

    run_standalone_rust(tree, root, Constraints::indefinite()).expect(
        "vertical-reverse linear indefinite axes padding/border percent-margin container sizing parity",
    );
}

#[test]
fn standalone_owned_tree_matches_cpp_for_rtl_horizontal_linear_at_most_axes_padding_border_overflow_content_size()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_direction(root, Direction::Rtl)
        .expect("set root direction");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root orientation");
    tree.set_padding(root, StandaloneEdge::Horizontal, Length::points(5.0))
        .expect("set root horizontal padding");
    tree.set_padding(root, StandaloneEdge::Vertical, Length::points(3.0))
        .expect("set root vertical padding");
    tree.set_border(root, StandaloneEdge::All, 1.0)
        .expect("set root border");

    let first = tree.create_default_node();
    tree.set_display(first, Display::Block)
        .expect("set first display");
    tree.set_width(first, Length::points(64.0))
        .expect("set first width");
    tree.set_height(first, Length::points(34.0))
        .expect("set first height");
    tree.set_margin(first, StandaloneEdge::Left, Length::percent(12.0))
        .expect("set first left margin");
    tree.set_margin(first, StandaloneEdge::Right, Length::points(5.0))
        .expect("set first right margin");

    let second = tree.create_default_node();
    tree.set_display(second, Display::Block)
        .expect("set second display");
    tree.set_width(second, Length::points(58.0))
        .expect("set second width");
    tree.set_height(second, Length::points(46.0))
        .expect("set second height");
    tree.set_margin(second, StandaloneEdge::Left, Length::points(7.0))
        .expect("set second left margin");
    tree.set_margin(second, StandaloneEdge::Right, Length::percent(6.0))
        .expect("set second right margin");

    tree.append_child(root, first).expect("append first");
    tree.append_child(root, second).expect("append second");

    run_standalone_rust(
        tree,
        root,
        Constraints::new(SideConstraint::at_most(90.0), SideConstraint::at_most(40.0)),
    )
    .expect("RTL horizontal linear AtMost axes overflow container sizing parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_vertical_reverse_linear_at_most_axes_padding_border_overflow_content_size()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::VerticalReverse)
        .expect("set root orientation");
    tree.set_padding(root, StandaloneEdge::Horizontal, Length::points(4.0))
        .expect("set root horizontal padding");
    tree.set_padding(root, StandaloneEdge::Vertical, Length::points(6.0))
        .expect("set root vertical padding");
    tree.set_border(root, StandaloneEdge::All, 2.0)
        .expect("set root border");

    let first = tree.create_default_node();
    tree.set_display(first, Display::Block)
        .expect("set first display");
    tree.set_width(first, Length::points(42.0))
        .expect("set first width");
    tree.set_height(first, Length::points(66.0))
        .expect("set first height");
    tree.set_margin(first, StandaloneEdge::Top, Length::percent(9.0))
        .expect("set first top margin");
    tree.set_margin(first, StandaloneEdge::Bottom, Length::points(6.0))
        .expect("set first bottom margin");

    let second = tree.create_default_node();
    tree.set_display(second, Display::Block)
        .expect("set second display");
    tree.set_width(second, Length::points(70.0))
        .expect("set second width");
    tree.set_height(second, Length::points(54.0))
        .expect("set second height");
    tree.set_margin(second, StandaloneEdge::Top, Length::points(8.0))
        .expect("set second top margin");
    tree.set_margin(second, StandaloneEdge::Bottom, Length::percent(7.0))
        .expect("set second bottom margin");

    tree.append_child(root, first).expect("append first");
    tree.append_child(root, second).expect("append second");

    run_standalone_rust(
        tree,
        root,
        Constraints::new(
            SideConstraint::at_most(60.0),
            SideConstraint::at_most(100.0),
        ),
    )
    .expect("vertical-reverse linear AtMost axes overflow container sizing parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_horizontal_reverse_linear_min_max_clamp_percent_margins() {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::HorizontalReverse)
        .expect("set root orientation");
    tree.set_min_width(root, Length::points(128.0))
        .expect("set root min width");
    tree.set_max_height(root, Length::points(44.0))
        .expect("set root max height");
    tree.set_padding(root, StandaloneEdge::Horizontal, Length::points(5.0))
        .expect("set root horizontal padding");
    tree.set_padding(root, StandaloneEdge::Vertical, Length::points(4.0))
        .expect("set root vertical padding");
    tree.set_border(root, StandaloneEdge::All, 1.0)
        .expect("set root border");

    let first = tree.create_default_node();
    tree.set_display(first, Display::Block)
        .expect("set first display");
    tree.set_width(first, Length::points(38.0))
        .expect("set first width");
    tree.set_height(first, Length::points(30.0))
        .expect("set first height");
    tree.set_margin(first, StandaloneEdge::Left, Length::percent(10.0))
        .expect("set first left margin");
    tree.set_margin(first, StandaloneEdge::Right, Length::percent(4.0))
        .expect("set first right margin");

    let second = tree.create_default_node();
    tree.set_display(second, Display::Block)
        .expect("set second display");
    tree.set_width(second, Length::points(44.0))
        .expect("set second width");
    tree.set_height(second, Length::points(52.0))
        .expect("set second height");
    tree.set_margin(second, StandaloneEdge::Left, Length::points(6.0))
        .expect("set second left margin");
    tree.set_margin(second, StandaloneEdge::Right, Length::points(8.0))
        .expect("set second right margin");

    tree.append_child(root, first).expect("append first");
    tree.append_child(root, second).expect("append second");

    run_standalone_rust(tree, root, Constraints::indefinite()).expect(
        "horizontal-reverse linear min/max clamp with percent margins container sizing parity",
    );
}

#[test]
fn standalone_owned_tree_matches_cpp_for_vertical_linear_min_max_clamp_calc_margins() {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Vertical)
        .expect("set root orientation");
    tree.set_max_width(root, Length::points(72.0))
        .expect("set root max width");
    tree.set_min_height(root, Length::points(118.0))
        .expect("set root min height");
    tree.set_padding(root, StandaloneEdge::Horizontal, Length::points(6.0))
        .expect("set root horizontal padding");
    tree.set_padding(root, StandaloneEdge::Vertical, Length::points(5.0))
        .expect("set root vertical padding");
    tree.set_border(root, StandaloneEdge::All, 2.0)
        .expect("set root border");

    let first = tree.create_default_node();
    tree.set_display(first, Display::Block)
        .expect("set first display");
    tree.set_width(first, Length::points(90.0))
        .expect("set first width");
    tree.set_height(first, Length::points(36.0))
        .expect("set first height");
    tree.set_margin(first, StandaloneEdge::Top, Length::calc(4.0, 8.0))
        .expect("set first top margin");
    tree.set_margin(first, StandaloneEdge::Bottom, Length::calc(2.0, 5.0))
        .expect("set first bottom margin");

    let second = tree.create_default_node();
    tree.set_display(second, Display::Block)
        .expect("set second display");
    tree.set_width(second, Length::points(40.0))
        .expect("set second width");
    tree.set_height(second, Length::points(42.0))
        .expect("set second height");
    tree.set_margin(second, StandaloneEdge::Left, Length::calc(3.0, 7.0))
        .expect("set second left margin");
    tree.set_margin(second, StandaloneEdge::Right, Length::calc(5.0, 4.0))
        .expect("set second right margin");

    tree.append_child(root, first).expect("append first");
    tree.append_child(root, second).expect("append second");

    run_standalone_rust(tree, root, Constraints::indefinite())
        .expect("vertical linear min/max clamp with calc margins container sizing parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_measured_cross_axis_constraints() {
    let (tree, root, constraints) = linear_measured_cross_axis_constraint_tree();

    run_standalone_rust(tree, root, constraints)
        .expect("linear measured cross-axis constraint parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_vertical_linear_measured_center_at_most_cross_axis() {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_linear_orientation(root, LinearOrientation::Vertical)
        .expect("set root orientation");
    tree.set_align_items(root, AlignItems::Center)
        .expect("set root align items");

    let child = tree.create_default_node();
    tree.set_display(child, Display::Block)
        .expect("set child display");
    tree.set_measure_func(child, Some(cross_axis_bounded_measure))
        .expect("set child measure func");
    tree.append_child(root, child).expect("append child");

    run_standalone_rust(
        tree,
        root,
        Constraints::new(SideConstraint::at_most(90.0), SideConstraint::indefinite()),
    )
    .expect("vertical measured center AtMost cross-axis parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_vertical_linear_measured_stretch_definite_cross_axis() {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_linear_orientation(root, LinearOrientation::Vertical)
        .expect("set root orientation");
    tree.set_width(root, Length::points(80.0))
        .expect("set root width");

    let child = tree.create_default_node();
    tree.set_display(child, Display::Block)
        .expect("set child display");
    tree.set_margin(child, StandaloneEdge::Left, Length::points(3.0))
        .expect("set child left margin");
    tree.set_margin(child, StandaloneEdge::Right, Length::points(5.0))
        .expect("set child right margin");
    tree.set_measure_func(child, Some(callback_measure))
        .expect("set child measure func");
    tree.append_child(root, child).expect("append child");

    run_standalone_rust(tree, root, Constraints::indefinite())
        .expect("vertical measured stretch definite cross-axis parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_horizontal_linear_measured_stretch_definite_cross_axis() {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root orientation");
    tree.set_height(root, Length::points(70.0))
        .expect("set root height");

    let child = tree.create_default_node();
    tree.set_display(child, Display::Block)
        .expect("set child display");
    tree.set_margin(child, StandaloneEdge::Top, Length::points(4.0))
        .expect("set child top margin");
    tree.set_margin(child, StandaloneEdge::Bottom, Length::points(6.0))
        .expect("set child bottom margin");
    tree.set_measure_func(child, Some(callback_measure))
        .expect("set child measure func");
    tree.append_child(root, child).expect("append child");

    run_standalone_rust(tree, root, Constraints::indefinite())
        .expect("horizontal measured stretch definite cross-axis parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_vertical_linear_measured_layout_gravity_end_at_most_cross_axis()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_linear_orientation(root, LinearOrientation::Vertical)
        .expect("set root orientation");
    tree.set_align_items(root, AlignItems::FlexStart)
        .expect("set root align items");

    let child = tree.create_default_node();
    tree.set_display(child, Display::Block)
        .expect("set child display");
    tree.set_linear_layout_gravity(child, LinearLayoutGravity::End)
        .expect("set child layout gravity");
    tree.set_measure_func(child, Some(cross_axis_bounded_measure))
        .expect("set child measure func");
    tree.append_child(root, child).expect("append child");

    run_standalone_rust(
        tree,
        root,
        Constraints::new(SideConstraint::at_most(90.0), SideConstraint::indefinite()),
    )
    .expect("vertical measured layout-gravity end AtMost cross-axis parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_horizontal_linear_measured_auto_cross_axis_parent_constraint()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root orientation");
    tree.set_width(root, Length::points(100.0))
        .expect("set root width");
    tree.set_height(root, Length::Auto)
        .expect("set root height");

    let child = tree.create_default_node();
    tree.set_display(child, Display::Block)
        .expect("set child display");
    tree.set_width(child, Length::points(10.0))
        .expect("set child width");
    tree.set_measure_func(child, Some(callback_measure))
        .expect("set child measure func");
    tree.append_child(root, child).expect("append child");

    run_standalone_rust(tree, root, Constraints::definite(100.0, 80.0))
        .expect("horizontal measured auto cross-axis parent constraint parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_horizontal_linear_measured_percent_cross_axis_final_remeasure()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root orientation");
    tree.set_width(root, Length::points(100.0))
        .expect("set root width");
    tree.set_height(root, Length::points(60.0))
        .expect("set root height");

    let child = tree.create_default_node();
    tree.set_display(child, Display::Block)
        .expect("set child display");
    tree.set_width(child, Length::points(10.0))
        .expect("set child width");
    tree.set_height(child, Length::percent(50.0))
        .expect("set child percent height");
    tree.set_measure_func(child, Some(callback_measure))
        .expect("set child measure func");
    tree.append_child(root, child).expect("append child");

    run_standalone_rust(tree, root, Constraints::definite(100.0, 60.0))
        .expect("horizontal measured percent cross-axis final remeasure parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_percent_cross_size_final_remeasure() {
    let (tree, root, constraints) = linear_percent_cross_size_final_remeasure_tree();

    run_standalone_rust(tree, root, constraints)
        .expect("linear percent cross-size final remeasure parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_vertical_linear_measured_percent_calc_min_max_container_base()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_linear_orientation(root, LinearOrientation::Vertical)
        .expect("set root orientation");
    tree.set_width(root, Length::points(126.0))
        .expect("set root width");
    tree.set_height(root, Length::points(92.0))
        .expect("set root height");
    tree.set_justify_content(root, JustifyContent::Center)
        .expect("set root justify content");

    let child = tree.create_default_measured_node(Size::new(80.0, 9.0));
    tree.set_min_width(child, Length::percent(25.0))
        .expect("set child min width");
    tree.set_max_width(child, Length::calc(8.0, 35.0))
        .expect("set child max width");
    tree.set_min_height(child, Length::calc(3.0, 18.0))
        .expect("set child min height");
    tree.set_max_height(child, Length::percent(70.0))
        .expect("set child max height");
    tree.set_padding(child, StandaloneEdge::All, Length::points(1.0))
        .expect("set child padding");
    tree.set_border(child, StandaloneEdge::All, 1.0)
        .expect("set child border");
    tree.append_child(root, child).expect("append child");

    run_standalone_rust(tree, root, Constraints::definite(126.0, 92.0))
        .expect("vertical measured percent/calc min-max container base parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_horizontal_linear_measured_at_most_main_axis_is_indefinite()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root orientation");
    tree.set_height(root, Length::points(40.0))
        .expect("set root height");

    let child = tree.create_default_node();
    tree.set_display(child, Display::Block)
        .expect("set child display");
    tree.set_measure_func(child, Some(callback_measure))
        .expect("set child measure func");
    tree.append_child(root, child).expect("append child");

    run_standalone_rust(
        tree,
        root,
        Constraints::new(
            SideConstraint::at_most(120.0),
            SideConstraint::definite(40.0),
        ),
    )
    .expect("horizontal measured AtMost main-axis indefinite constraint parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_vertical_linear_measured_at_most_main_axis_is_indefinite()
{
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_linear_orientation(root, LinearOrientation::Vertical)
        .expect("set root orientation");
    tree.set_width(root, Length::points(44.0))
        .expect("set root width");

    let child = tree.create_default_node();
    tree.set_display(child, Display::Block)
        .expect("set child display");
    tree.set_measure_func(child, Some(callback_measure))
        .expect("set child measure func");
    tree.append_child(root, child).expect("append child");

    run_standalone_rust(
        tree,
        root,
        Constraints::new(
            SideConstraint::definite(44.0),
            SideConstraint::at_most(130.0),
        ),
    )
    .expect("vertical measured AtMost main-axis indefinite constraint parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_horizontal_linear_measured_definite_cross_stretch_percent_margins()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root orientation");
    tree.set_width(root, Length::points(110.0))
        .expect("set root width");
    tree.set_height(root, Length::points(80.0))
        .expect("set root height");

    let child = tree.create_default_node();
    tree.set_display(child, Display::Block)
        .expect("set child display");
    tree.set_width(child, Length::points(20.0))
        .expect("set child width");
    tree.set_margin(child, StandaloneEdge::Top, Length::percent(10.0))
        .expect("set child top margin");
    tree.set_margin(child, StandaloneEdge::Bottom, Length::percent(5.0))
        .expect("set child bottom margin");
    tree.set_measure_func(child, Some(callback_measure))
        .expect("set child measure func");
    tree.append_child(root, child).expect("append child");

    run_standalone_rust(tree, root, Constraints::definite(110.0, 80.0))
        .expect("horizontal measured definite cross stretch with percent margins parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_vertical_linear_measured_layout_gravity_end_suppresses_definite_cross_stretch()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_linear_orientation(root, LinearOrientation::Vertical)
        .expect("set root orientation");
    tree.set_width(root, Length::points(90.0))
        .expect("set root width");
    tree.set_height(root, Length::points(100.0))
        .expect("set root height");

    let child = tree.create_default_node();
    tree.set_display(child, Display::Block)
        .expect("set child display");
    tree.set_height(child, Length::points(18.0))
        .expect("set child height");
    tree.set_linear_layout_gravity(child, LinearLayoutGravity::End)
        .expect("set child layout gravity");
    tree.set_margin(child, StandaloneEdge::Left, Length::calc(3.0, 8.0))
        .expect("set child left margin");
    tree.set_margin(child, StandaloneEdge::Right, Length::percent(7.0))
        .expect("set child right margin");
    tree.set_measure_func(child, Some(callback_measure))
        .expect("set child measure func");
    tree.append_child(root, child).expect("append child");

    run_standalone_rust(tree, root, Constraints::definite(90.0, 100.0))
        .expect("vertical measured layout-gravity end suppresses definite cross stretch parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_horizontal_linear_measured_fit_content_cross_axis_avoids_default_stretch()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root orientation");
    tree.set_width(root, Length::points(100.0))
        .expect("set root width");
    tree.set_height(root, Length::points(70.0))
        .expect("set root height");

    let child = tree.create_default_node();
    tree.set_display(child, Display::Block)
        .expect("set child display");
    tree.set_width(child, Length::points(22.0))
        .expect("set child width");
    tree.set_height(child, Length::fit_content(Some(BaseLength::fixed(42.0))))
        .expect("set child fit-content height");
    tree.set_measure_func(child, Some(callback_measure))
        .expect("set child measure func");
    tree.append_child(root, child).expect("append child");

    run_standalone_rust(tree, root, Constraints::definite(100.0, 70.0))
        .expect("horizontal measured fit-content cross-axis avoids default stretch parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_horizontal_linear_measured_fit_content_cross_axis_stretch_gravity_overrides_intrinsic()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root orientation");
    tree.set_width(root, Length::points(100.0))
        .expect("set root width");
    tree.set_height(root, Length::points(72.0))
        .expect("set root height");

    let child = tree.create_default_node();
    tree.set_display(child, Display::Block)
        .expect("set child display");
    tree.set_width(child, Length::points(24.0))
        .expect("set child width");
    tree.set_height(child, Length::fit_content(Some(BaseLength::fixed(30.0))))
        .expect("set child fit-content height");
    tree.set_linear_layout_gravity(child, LinearLayoutGravity::Stretch)
        .expect("set child layout gravity");
    tree.set_margin(child, StandaloneEdge::Top, Length::points(4.0))
        .expect("set child top margin");
    tree.set_margin(child, StandaloneEdge::Bottom, Length::points(6.0))
        .expect("set child bottom margin");
    tree.set_measure_func(child, Some(callback_measure))
        .expect("set child measure func");
    tree.append_child(root, child).expect("append child");

    run_standalone_rust(tree, root, Constraints::definite(100.0, 72.0)).expect(
        "horizontal measured fit-content cross-axis stretch gravity overrides intrinsic parity",
    );
}

#[test]
fn standalone_owned_tree_matches_cpp_for_horizontal_linear_weighted_measured_child_receives_definite_main_axis()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root orientation");
    tree.set_linear_weight_sum(root, 2.0)
        .expect("set root weight sum");
    tree.set_width(root, Length::points(120.0))
        .expect("set root width");
    tree.set_height(root, Length::points(50.0))
        .expect("set root height");

    let fixed = tree.create_default_node();
    tree.set_display(fixed, Display::Block)
        .expect("set fixed display");
    tree.set_width(fixed, Length::points(24.0))
        .expect("set fixed width");
    tree.set_height(fixed, Length::points(12.0))
        .expect("set fixed height");

    let weighted = tree.create_default_node();
    tree.set_display(weighted, Display::Block)
        .expect("set weighted display");
    tree.set_linear_weight(weighted, 1.0)
        .expect("set weighted child weight");
    tree.set_height(weighted, Length::points(14.0))
        .expect("set weighted child height");
    tree.set_margin(weighted, StandaloneEdge::Left, Length::points(3.0))
        .expect("set weighted child left margin");
    tree.set_margin(weighted, StandaloneEdge::Right, Length::points(5.0))
        .expect("set weighted child right margin");
    tree.set_measure_func(weighted, Some(callback_measure))
        .expect("set weighted child measure func");

    tree.append_child(root, fixed).expect("append fixed");
    tree.append_child(root, weighted).expect("append weighted");

    run_standalone_rust(tree, root, Constraints::definite(120.0, 50.0))
        .expect("horizontal weighted measured child receives definite main-axis parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_horizontal_linear_ordered_weighted_measured_skips_display_none_with_weight_sum()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root orientation");
    tree.set_linear_weight_sum(root, 4.0)
        .expect("set root weight sum");
    tree.set_align_items(root, AlignItems::FlexStart)
        .expect("set root align items");
    tree.set_width(root, Length::points(180.0))
        .expect("set root width");
    tree.set_height(root, Length::points(64.0))
        .expect("set root height");

    let fixed_late = tree.create_default_node();
    tree.set_display(fixed_late, Display::Block)
        .expect("set fixed late display");
    tree.set_width(fixed_late, Length::points(28.0))
        .expect("set fixed late width");
    tree.set_height(fixed_late, Length::points(12.0))
        .expect("set fixed late height");
    tree.set_order(fixed_late, 4).expect("set fixed late order");

    let hidden_weighted = tree.create_default_node();
    tree.set_display(hidden_weighted, Display::None)
        .expect("set hidden weighted display");
    tree.set_width(hidden_weighted, Length::points(120.0))
        .expect("set hidden weighted width");
    tree.set_height(hidden_weighted, Length::points(40.0))
        .expect("set hidden weighted height");
    tree.set_linear_weight(hidden_weighted, 8.0)
        .expect("set hidden weighted weight");
    tree.set_order(hidden_weighted, -5)
        .expect("set hidden weighted order");

    let weighted_first = tree.create_default_node();
    tree.set_display(weighted_first, Display::Block)
        .expect("set weighted first display");
    tree.set_linear_weight(weighted_first, 1.0)
        .expect("set weighted first weight");
    tree.set_height(weighted_first, Length::points(15.0))
        .expect("set weighted first height");
    tree.set_margin(weighted_first, StandaloneEdge::Left, Length::points(3.0))
        .expect("set weighted first left margin");
    tree.set_margin(weighted_first, StandaloneEdge::Right, Length::points(5.0))
        .expect("set weighted first right margin");
    tree.set_measure_func(weighted_first, Some(callback_measure))
        .expect("set weighted first measure func");
    tree.set_order(weighted_first, -1)
        .expect("set weighted first order");

    let weighted_second = tree.create_default_node();
    tree.set_display(weighted_second, Display::Block)
        .expect("set weighted second display");
    tree.set_linear_weight(weighted_second, 2.0)
        .expect("set weighted second weight");
    tree.set_height(weighted_second, Length::points(17.0))
        .expect("set weighted second height");
    tree.set_margin(weighted_second, StandaloneEdge::Left, Length::points(2.0))
        .expect("set weighted second left margin");
    tree.set_margin(weighted_second, StandaloneEdge::Right, Length::points(4.0))
        .expect("set weighted second right margin");
    tree.set_measure_func(weighted_second, Some(callback_measure))
        .expect("set weighted second measure func");

    tree.append_child(root, fixed_late)
        .expect("append fixed late");
    tree.append_child(root, hidden_weighted)
        .expect("append hidden weighted");
    tree.append_child(root, weighted_second)
        .expect("append weighted second");
    tree.append_child(root, weighted_first)
        .expect("append weighted first");

    run_standalone_rust(tree, root, Constraints::definite(180.0, 64.0)).expect(
        "horizontal linear ordered weighted measured skips display-none with weight-sum parity",
    );
}

#[test]
fn standalone_owned_tree_matches_cpp_for_horizontal_reverse_linear_ordered_weighted_measured_skips_display_none_with_percent_margins()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_linear_orientation(root, LinearOrientation::HorizontalReverse)
        .expect("set root orientation");
    tree.set_align_items(root, AlignItems::FlexStart)
        .expect("set root align items");
    tree.set_width(root, Length::points(172.0))
        .expect("set root width");
    tree.set_height(root, Length::points(62.0))
        .expect("set root height");

    let fixed_late = tree.create_default_node();
    tree.set_display(fixed_late, Display::Block)
        .expect("set fixed late display");
    tree.set_width(fixed_late, Length::points(24.0))
        .expect("set fixed late width");
    tree.set_height(fixed_late, Length::points(14.0))
        .expect("set fixed late height");
    tree.set_order(fixed_late, 3).expect("set fixed late order");

    let hidden_weighted = tree.create_default_node();
    tree.set_display(hidden_weighted, Display::None)
        .expect("set hidden weighted display");
    tree.set_width(hidden_weighted, Length::points(96.0))
        .expect("set hidden weighted width");
    tree.set_height(hidden_weighted, Length::points(32.0))
        .expect("set hidden weighted height");
    tree.set_linear_weight(hidden_weighted, 6.0)
        .expect("set hidden weighted weight");
    tree.set_order(hidden_weighted, -4)
        .expect("set hidden weighted order");

    let weighted_first = tree.create_default_node();
    tree.set_display(weighted_first, Display::Block)
        .expect("set weighted first display");
    tree.set_linear_weight(weighted_first, 1.0)
        .expect("set weighted first weight");
    tree.set_height(weighted_first, Length::points(16.0))
        .expect("set weighted first height");
    tree.set_margin(weighted_first, StandaloneEdge::Left, Length::percent(3.0))
        .expect("set weighted first left margin");
    tree.set_margin(weighted_first, StandaloneEdge::Right, Length::percent(4.0))
        .expect("set weighted first right margin");
    tree.set_measure_func(weighted_first, Some(callback_measure))
        .expect("set weighted first measure func");
    tree.set_order(weighted_first, -1)
        .expect("set weighted first order");

    let weighted_second = tree.create_default_node();
    tree.set_display(weighted_second, Display::Block)
        .expect("set weighted second display");
    tree.set_linear_weight(weighted_second, 2.0)
        .expect("set weighted second weight");
    tree.set_height(weighted_second, Length::points(18.0))
        .expect("set weighted second height");
    tree.set_margin(weighted_second, StandaloneEdge::Left, Length::percent(2.0))
        .expect("set weighted second left margin");
    tree.set_margin(weighted_second, StandaloneEdge::Right, Length::percent(5.0))
        .expect("set weighted second right margin");
    tree.set_measure_func(weighted_second, Some(callback_measure))
        .expect("set weighted second measure func");

    tree.append_child(root, weighted_second)
        .expect("append weighted second");
    tree.append_child(root, fixed_late)
        .expect("append fixed late");
    tree.append_child(root, hidden_weighted)
        .expect("append hidden weighted");
    tree.append_child(root, weighted_first)
        .expect("append weighted first");

    run_standalone_rust(tree, root, Constraints::definite(172.0, 62.0)).expect(
        "horizontal-reverse linear ordered weighted measured skips display-none with percent margins parity",
    );
}

#[test]
fn standalone_owned_tree_matches_cpp_for_rtl_horizontal_linear_ordered_weighted_measured_skips_display_none_with_calc_margins()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_direction(root, Direction::Rtl)
        .expect("set root direction");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root orientation");
    tree.set_align_items(root, AlignItems::FlexStart)
        .expect("set root align items");
    tree.set_width(root, Length::points(168.0))
        .expect("set root width");
    tree.set_height(root, Length::points(60.0))
        .expect("set root height");

    let fixed_late = tree.create_default_node();
    tree.set_display(fixed_late, Display::Block)
        .expect("set fixed late display");
    tree.set_width(fixed_late, Length::points(26.0))
        .expect("set fixed late width");
    tree.set_height(fixed_late, Length::points(12.0))
        .expect("set fixed late height");
    tree.set_order(fixed_late, 4).expect("set fixed late order");

    let hidden_weighted = tree.create_default_node();
    tree.set_display(hidden_weighted, Display::None)
        .expect("set hidden weighted display");
    tree.set_width(hidden_weighted, Length::points(110.0))
        .expect("set hidden weighted width");
    tree.set_height(hidden_weighted, Length::points(42.0))
        .expect("set hidden weighted height");
    tree.set_linear_weight(hidden_weighted, 7.0)
        .expect("set hidden weighted weight");
    tree.set_order(hidden_weighted, -5)
        .expect("set hidden weighted order");

    let weighted_first = tree.create_default_node();
    tree.set_display(weighted_first, Display::Block)
        .expect("set weighted first display");
    tree.set_linear_weight(weighted_first, 1.0)
        .expect("set weighted first weight");
    tree.set_height(weighted_first, Length::points(15.0))
        .expect("set weighted first height");
    tree.set_margin(weighted_first, StandaloneEdge::Left, Length::calc(2.0, 4.0))
        .expect("set weighted first left margin");
    tree.set_margin(
        weighted_first,
        StandaloneEdge::Right,
        Length::calc(3.0, 5.0),
    )
    .expect("set weighted first right margin");
    tree.set_measure_func(weighted_first, Some(callback_measure))
        .expect("set weighted first measure func");
    tree.set_order(weighted_first, -1)
        .expect("set weighted first order");

    let weighted_second = tree.create_default_node();
    tree.set_display(weighted_second, Display::Block)
        .expect("set weighted second display");
    tree.set_linear_weight(weighted_second, 2.0)
        .expect("set weighted second weight");
    tree.set_height(weighted_second, Length::points(17.0))
        .expect("set weighted second height");
    tree.set_margin(
        weighted_second,
        StandaloneEdge::Left,
        Length::calc(1.0, 6.0),
    )
    .expect("set weighted second left margin");
    tree.set_margin(
        weighted_second,
        StandaloneEdge::Right,
        Length::calc(2.0, 4.0),
    )
    .expect("set weighted second right margin");
    tree.set_measure_func(weighted_second, Some(callback_measure))
        .expect("set weighted second measure func");

    tree.append_child(root, hidden_weighted)
        .expect("append hidden weighted");
    tree.append_child(root, weighted_second)
        .expect("append weighted second");
    tree.append_child(root, fixed_late)
        .expect("append fixed late");
    tree.append_child(root, weighted_first)
        .expect("append weighted first");

    run_standalone_rust(tree, root, Constraints::definite(168.0, 60.0)).expect(
        "RTL horizontal linear ordered weighted measured skips display-none with calc margins parity",
    );
}

#[test]
fn standalone_owned_tree_matches_cpp_for_vertical_linear_ordered_weighted_measured_skips_display_none_with_weight_sum()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_linear_orientation(root, LinearOrientation::Vertical)
        .expect("set root orientation");
    tree.set_linear_weight_sum(root, 5.0)
        .expect("set root weight sum");
    tree.set_align_items(root, AlignItems::FlexStart)
        .expect("set root align items");
    tree.set_width(root, Length::points(84.0))
        .expect("set root width");
    tree.set_height(root, Length::points(190.0))
        .expect("set root height");

    let fixed_late = tree.create_default_node();
    tree.set_display(fixed_late, Display::Block)
        .expect("set fixed late display");
    tree.set_width(fixed_late, Length::points(22.0))
        .expect("set fixed late width");
    tree.set_height(fixed_late, Length::points(28.0))
        .expect("set fixed late height");
    tree.set_order(fixed_late, 4).expect("set fixed late order");

    let hidden_weighted = tree.create_default_node();
    tree.set_display(hidden_weighted, Display::None)
        .expect("set hidden weighted display");
    tree.set_width(hidden_weighted, Length::points(70.0))
        .expect("set hidden weighted width");
    tree.set_height(hidden_weighted, Length::points(120.0))
        .expect("set hidden weighted height");
    tree.set_linear_weight(hidden_weighted, 9.0)
        .expect("set hidden weighted weight");
    tree.set_order(hidden_weighted, -5)
        .expect("set hidden weighted order");

    let weighted_first = tree.create_default_node();
    tree.set_display(weighted_first, Display::Block)
        .expect("set weighted first display");
    tree.set_linear_weight(weighted_first, 1.0)
        .expect("set weighted first weight");
    tree.set_width(weighted_first, Length::points(24.0))
        .expect("set weighted first width");
    tree.set_margin(weighted_first, StandaloneEdge::Top, Length::points(4.0))
        .expect("set weighted first top margin");
    tree.set_margin(weighted_first, StandaloneEdge::Bottom, Length::points(6.0))
        .expect("set weighted first bottom margin");
    tree.set_measure_func(weighted_first, Some(callback_measure))
        .expect("set weighted first measure func");
    tree.set_order(weighted_first, -1)
        .expect("set weighted first order");

    let weighted_second = tree.create_default_node();
    tree.set_display(weighted_second, Display::Block)
        .expect("set weighted second display");
    tree.set_linear_weight(weighted_second, 2.0)
        .expect("set weighted second weight");
    tree.set_width(weighted_second, Length::points(26.0))
        .expect("set weighted second width");
    tree.set_margin(weighted_second, StandaloneEdge::Top, Length::points(3.0))
        .expect("set weighted second top margin");
    tree.set_margin(weighted_second, StandaloneEdge::Bottom, Length::points(5.0))
        .expect("set weighted second bottom margin");
    tree.set_measure_func(weighted_second, Some(callback_measure))
        .expect("set weighted second measure func");

    tree.append_child(root, fixed_late)
        .expect("append fixed late");
    tree.append_child(root, weighted_second)
        .expect("append weighted second");
    tree.append_child(root, hidden_weighted)
        .expect("append hidden weighted");
    tree.append_child(root, weighted_first)
        .expect("append weighted first");

    run_standalone_rust(tree, root, Constraints::definite(84.0, 190.0)).expect(
        "vertical linear ordered weighted measured skips display-none with weight-sum parity",
    );
}

#[test]
fn standalone_owned_tree_matches_cpp_for_vertical_reverse_linear_ordered_weighted_measured_skips_display_none_with_percent_margins()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_linear_orientation(root, LinearOrientation::VerticalReverse)
        .expect("set root orientation");
    tree.set_align_items(root, AlignItems::FlexStart)
        .expect("set root align items");
    tree.set_width(root, Length::points(82.0))
        .expect("set root width");
    tree.set_height(root, Length::points(184.0))
        .expect("set root height");

    let fixed_late = tree.create_default_node();
    tree.set_display(fixed_late, Display::Block)
        .expect("set fixed late display");
    tree.set_width(fixed_late, Length::points(20.0))
        .expect("set fixed late width");
    tree.set_height(fixed_late, Length::points(24.0))
        .expect("set fixed late height");
    tree.set_order(fixed_late, 3).expect("set fixed late order");

    let hidden_weighted = tree.create_default_node();
    tree.set_display(hidden_weighted, Display::None)
        .expect("set hidden weighted display");
    tree.set_width(hidden_weighted, Length::points(64.0))
        .expect("set hidden weighted width");
    tree.set_height(hidden_weighted, Length::points(100.0))
        .expect("set hidden weighted height");
    tree.set_linear_weight(hidden_weighted, 6.0)
        .expect("set hidden weighted weight");
    tree.set_order(hidden_weighted, -4)
        .expect("set hidden weighted order");

    let weighted_first = tree.create_default_node();
    tree.set_display(weighted_first, Display::Block)
        .expect("set weighted first display");
    tree.set_linear_weight(weighted_first, 1.0)
        .expect("set weighted first weight");
    tree.set_width(weighted_first, Length::points(23.0))
        .expect("set weighted first width");
    tree.set_margin(weighted_first, StandaloneEdge::Top, Length::percent(3.0))
        .expect("set weighted first top margin");
    tree.set_margin(weighted_first, StandaloneEdge::Bottom, Length::percent(4.0))
        .expect("set weighted first bottom margin");
    tree.set_measure_func(weighted_first, Some(callback_measure))
        .expect("set weighted first measure func");
    tree.set_order(weighted_first, -1)
        .expect("set weighted first order");

    let weighted_second = tree.create_default_node();
    tree.set_display(weighted_second, Display::Block)
        .expect("set weighted second display");
    tree.set_linear_weight(weighted_second, 2.0)
        .expect("set weighted second weight");
    tree.set_width(weighted_second, Length::points(25.0))
        .expect("set weighted second width");
    tree.set_margin(weighted_second, StandaloneEdge::Top, Length::percent(2.0))
        .expect("set weighted second top margin");
    tree.set_margin(
        weighted_second,
        StandaloneEdge::Bottom,
        Length::percent(5.0),
    )
    .expect("set weighted second bottom margin");
    tree.set_measure_func(weighted_second, Some(callback_measure))
        .expect("set weighted second measure func");

    tree.append_child(root, weighted_second)
        .expect("append weighted second");
    tree.append_child(root, fixed_late)
        .expect("append fixed late");
    tree.append_child(root, hidden_weighted)
        .expect("append hidden weighted");
    tree.append_child(root, weighted_first)
        .expect("append weighted first");

    run_standalone_rust(tree, root, Constraints::definite(82.0, 184.0)).expect(
        "vertical-reverse linear ordered weighted measured skips display-none with percent margins parity",
    );
}

#[test]
fn standalone_owned_tree_matches_cpp_for_horizontal_linear_at_most_ordered_weighted_measured_skips_display_none_weight_disabled()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root orientation");
    tree.set_align_items(root, AlignItems::FlexStart)
        .expect("set root align items");
    tree.set_height(root, Length::points(48.0))
        .expect("set root height");

    let fixed_late = tree.create_default_node();
    tree.set_display(fixed_late, Display::Block)
        .expect("set fixed late display");
    tree.set_width(fixed_late, Length::points(22.0))
        .expect("set fixed late width");
    tree.set_height(fixed_late, Length::points(12.0))
        .expect("set fixed late height");
    tree.set_order(fixed_late, 4).expect("set fixed late order");

    let hidden_weighted = tree.create_default_node();
    tree.set_display(hidden_weighted, Display::None)
        .expect("set hidden weighted display");
    tree.set_width(hidden_weighted, Length::points(90.0))
        .expect("set hidden weighted width");
    tree.set_height(hidden_weighted, Length::points(30.0))
        .expect("set hidden weighted height");
    tree.set_linear_weight(hidden_weighted, 5.0)
        .expect("set hidden weighted weight");
    tree.set_order(hidden_weighted, -5)
        .expect("set hidden weighted order");

    let weighted_first = tree.create_default_node();
    tree.set_display(weighted_first, Display::Block)
        .expect("set weighted first display");
    tree.set_linear_weight(weighted_first, 1.0)
        .expect("set weighted first weight");
    tree.set_height(weighted_first, Length::points(14.0))
        .expect("set weighted first height");
    tree.set_margin(weighted_first, StandaloneEdge::Left, Length::points(3.0))
        .expect("set weighted first left margin");
    tree.set_margin(weighted_first, StandaloneEdge::Right, Length::points(4.0))
        .expect("set weighted first right margin");
    tree.set_measure_func(weighted_first, Some(callback_measure))
        .expect("set weighted first measure func");
    tree.set_order(weighted_first, -1)
        .expect("set weighted first order");

    let weighted_second = tree.create_default_node();
    tree.set_display(weighted_second, Display::Block)
        .expect("set weighted second display");
    tree.set_linear_weight(weighted_second, 2.0)
        .expect("set weighted second weight");
    tree.set_height(weighted_second, Length::points(16.0))
        .expect("set weighted second height");
    tree.set_margin(
        weighted_second,
        StandaloneEdge::Horizontal,
        Length::points(2.0),
    )
    .expect("set weighted second horizontal margin");
    tree.set_measure_func(weighted_second, Some(callback_measure))
        .expect("set weighted second measure func");

    tree.append_child(root, hidden_weighted)
        .expect("append hidden weighted");
    tree.append_child(root, fixed_late)
        .expect("append fixed late");
    tree.append_child(root, weighted_second)
        .expect("append weighted second");
    tree.append_child(root, weighted_first)
        .expect("append weighted first");

    run_standalone_rust(
        tree,
        root,
        Constraints::new(
            SideConstraint::at_most(128.0),
            SideConstraint::definite(48.0),
        ),
    )
    .expect(
        "horizontal linear AtMost ordered weighted measured skips display-none weight-disabled parity",
    );
}

#[test]
fn standalone_owned_tree_matches_cpp_for_horizontal_linear_auto_main_percent_margins_keep_initial_size()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root orientation");
    tree.set_height(root, Length::points(10.0))
        .expect("set root height");

    let child = tree.create_default_node();
    tree.set_display(child, Display::Block)
        .expect("set child display");
    tree.set_width(child, Length::points(100.0))
        .expect("set child width");
    tree.set_height(child, Length::points(10.0))
        .expect("set child height");
    tree.set_margin(child, StandaloneEdge::Left, Length::percent(10.0))
        .expect("set child left margin");
    tree.set_margin(child, StandaloneEdge::Right, Length::percent(10.0))
        .expect("set child right margin");
    tree.append_child(root, child).expect("append child");

    run_standalone_rust(tree, root, Constraints::indefinite())
        .expect("horizontal linear auto main percent margins keep initial size parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_vertical_linear_auto_main_percent_margins_keep_initial_size()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_linear_orientation(root, LinearOrientation::Vertical)
        .expect("set root orientation");

    let child = tree.create_default_node();
    tree.set_display(child, Display::Block)
        .expect("set child display");
    tree.set_width(child, Length::points(100.0))
        .expect("set child width");
    tree.set_height(child, Length::points(100.0))
        .expect("set child height");
    tree.set_margin(child, StandaloneEdge::Top, Length::percent(10.0))
        .expect("set child top margin");
    tree.set_margin(child, StandaloneEdge::Bottom, Length::percent(10.0))
        .expect("set child bottom margin");
    tree.append_child(root, child).expect("append child");

    run_standalone_rust(tree, root, Constraints::indefinite())
        .expect("vertical linear auto main percent margins keep initial size parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_vertical_linear_at_most_cross_axis_auto_child_no_stretch()
{
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_linear_orientation(root, LinearOrientation::Vertical)
        .expect("set root orientation");

    let child = tree.create_default_node();
    tree.set_display(child, Display::Block)
        .expect("set child display");
    tree.set_width(child, Length::Auto)
        .expect("set child auto width");
    tree.set_height(child, Length::points(10.0))
        .expect("set child height");
    tree.append_child(root, child).expect("append child");

    run_standalone_rust(
        tree,
        root,
        Constraints::new(SideConstraint::at_most(100.0), SideConstraint::indefinite()),
    )
    .expect("vertical linear AtMost cross-axis auto child no-stretch parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_horizontal_linear_fit_content_cross_axis_measured_child() {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root orientation");
    tree.set_width(root, Length::points(100.0))
        .expect("set root width");
    tree.set_height(root, Length::fit_content(Some(BaseLength::fixed(30.0))))
        .expect("set root fit-content height");

    let child = tree.create_default_measured_node(Size::new(20.0, 50.0));
    tree.append_child(root, child).expect("append child");

    run_standalone_rust(
        tree,
        root,
        Constraints::new(
            SideConstraint::definite(100.0),
            SideConstraint::indefinite(),
        ),
    )
    .expect("horizontal linear fit-content cross-axis measured child parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_horizontal_linear_percent_cross_size_with_stretch_remeasure()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root orientation");
    tree.set_width(root, Length::points(100.0))
        .expect("set root width");
    tree.set_height(root, Length::points(80.0))
        .expect("set root height");

    let child = tree.create_default_node();
    tree.set_display(child, Display::Block)
        .expect("set child display");
    tree.set_width(child, Length::points(20.0))
        .expect("set child width");
    tree.set_height(child, Length::percent(50.0))
        .expect("set child percent height");
    tree.set_linear_layout_gravity(child, LinearLayoutGravity::Stretch)
        .expect("set child layout gravity");
    tree.append_child(root, child).expect("append child");

    run_standalone_rust(tree, root, Constraints::definite(100.0, 80.0))
        .expect("horizontal percent cross-size with stretch remeasure parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_auto_cross_axis_parent_constraint() {
    let (tree, root, constraints) = linear_auto_cross_axis_parent_constraint_tree();

    run_standalone_rust(tree, root, constraints)
        .expect("linear auto cross-axis parent constraint parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_weight_and_gravity() {
    let (tree, root, constraints) = linear_weight_and_gravity_tree();

    run_standalone_rust(tree, root, constraints).expect("linear weight and gravity parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_weight_ratio_distribution() {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root orientation");
    tree.set_width(root, Length::points(90.0))
        .expect("set root width");
    tree.set_height(root, Length::points(20.0))
        .expect("set root height");

    let first = tree.create_default_node();
    tree.set_display(first, Display::Block)
        .expect("set first display");
    tree.set_linear_weight(first, 1.0)
        .expect("set first weight");

    let second = tree.create_default_node();
    tree.set_display(second, Display::Block)
        .expect("set second display");
    tree.set_linear_weight(second, 2.0)
        .expect("set second weight");

    tree.append_child(root, first).expect("append first");
    tree.append_child(root, second).expect("append second");

    run_standalone_rust(tree, root, Constraints::definite(90.0, 20.0))
        .expect("linear weight ratio distribution parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_weight_rtl_horizontal_ratio_distribution() {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_direction(root, Direction::Rtl)
        .expect("set root direction");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root orientation");
    tree.set_width(root, Length::points(90.0))
        .expect("set root width");
    tree.set_height(root, Length::points(20.0))
        .expect("set root height");

    let first = tree.create_default_node();
    tree.set_display(first, Display::Block)
        .expect("set first display");
    tree.set_linear_weight(first, 1.0)
        .expect("set first weight");

    let second = tree.create_default_node();
    tree.set_display(second, Display::Block)
        .expect("set second display");
    tree.set_linear_weight(second, 2.0)
        .expect("set second weight");

    tree.append_child(root, first).expect("append first");
    tree.append_child(root, second).expect("append second");

    run_standalone_rust(tree, root, Constraints::definite(90.0, 20.0))
        .expect("linear RTL horizontal weight ratio distribution parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_weight_horizontal_reverse_ratio_distribution() {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_linear_orientation(root, LinearOrientation::HorizontalReverse)
        .expect("set root orientation");
    tree.set_width(root, Length::points(90.0))
        .expect("set root width");
    tree.set_height(root, Length::points(20.0))
        .expect("set root height");

    let first = tree.create_default_node();
    tree.set_display(first, Display::Block)
        .expect("set first display");
    tree.set_linear_weight(first, 1.0)
        .expect("set first weight");

    let second = tree.create_default_node();
    tree.set_display(second, Display::Block)
        .expect("set second display");
    tree.set_linear_weight(second, 2.0)
        .expect("set second weight");

    tree.append_child(root, first).expect("append first");
    tree.append_child(root, second).expect("append second");

    run_standalone_rust(tree, root, Constraints::definite(90.0, 20.0))
        .expect("linear horizontal-reverse weight ratio distribution parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_weight_vertical_reverse_ratio_distribution() {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_linear_orientation(root, LinearOrientation::VerticalReverse)
        .expect("set root orientation");
    tree.set_width(root, Length::points(20.0))
        .expect("set root width");
    tree.set_height(root, Length::points(90.0))
        .expect("set root height");

    let first = tree.create_default_node();
    tree.set_display(first, Display::Block)
        .expect("set first display");
    tree.set_linear_weight(first, 1.0)
        .expect("set first weight");

    let second = tree.create_default_node();
    tree.set_display(second, Display::Block)
        .expect("set second display");
    tree.set_linear_weight(second, 2.0)
        .expect("set second weight");

    tree.append_child(root, first).expect("append first");
    tree.append_child(root, second).expect("append second");

    run_standalone_rust(tree, root, Constraints::definite(20.0, 90.0))
        .expect("linear vertical-reverse weight ratio distribution parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_horizontal_linear_weight_aspect_ratio_derives_cross_size()
{
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root orientation");
    tree.set_align_items(root, AlignItems::FlexStart)
        .expect("set root align items");
    tree.set_width(root, Length::points(120.0))
        .expect("set root width");
    tree.set_height(root, Length::points(90.0))
        .expect("set root height");

    let fixed = tree.create_default_node();
    tree.set_display(fixed, Display::Block)
        .expect("set fixed display");
    tree.set_width(fixed, Length::points(20.0))
        .expect("set fixed width");
    tree.set_height(fixed, Length::points(10.0))
        .expect("set fixed height");

    let weighted = tree.create_default_node();
    tree.set_display(weighted, Display::Block)
        .expect("set weighted display");
    tree.set_linear_weight(weighted, 1.0)
        .expect("set weighted weight");
    tree.set_aspect_ratio(weighted, Some(2.0))
        .expect("set weighted aspect ratio");

    tree.append_child(root, fixed).expect("append fixed");
    tree.append_child(root, weighted).expect("append weighted");

    run_standalone_rust(tree, root, Constraints::definite(120.0, 90.0))
        .expect("horizontal linear weighted aspect-ratio cross-size parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_vertical_linear_weight_aspect_ratio_derives_cross_size() {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_linear_orientation(root, LinearOrientation::Vertical)
        .expect("set root orientation");
    tree.set_align_items(root, AlignItems::FlexStart)
        .expect("set root align items");
    tree.set_width(root, Length::points(100.0))
        .expect("set root width");
    tree.set_height(root, Length::points(140.0))
        .expect("set root height");

    let fixed = tree.create_default_node();
    tree.set_display(fixed, Display::Block)
        .expect("set fixed display");
    tree.set_width(fixed, Length::points(10.0))
        .expect("set fixed width");
    tree.set_height(fixed, Length::points(20.0))
        .expect("set fixed height");

    let weighted = tree.create_default_node();
    tree.set_display(weighted, Display::Block)
        .expect("set weighted display");
    tree.set_linear_weight(weighted, 1.0)
        .expect("set weighted weight");
    tree.set_aspect_ratio(weighted, Some(1.5))
        .expect("set weighted aspect ratio");

    tree.append_child(root, fixed).expect("append fixed");
    tree.append_child(root, weighted).expect("append weighted");

    run_standalone_rust(tree, root, Constraints::definite(100.0, 140.0))
        .expect("vertical linear weighted aspect-ratio cross-size parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_horizontal_reverse_linear_weight_aspect_ratio_content_box_edges()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_linear_orientation(root, LinearOrientation::HorizontalReverse)
        .expect("set root orientation");
    tree.set_align_items(root, AlignItems::FlexStart)
        .expect("set root align items");
    tree.set_width(root, Length::points(130.0))
        .expect("set root width");
    tree.set_height(root, Length::points(96.0))
        .expect("set root height");

    let weighted = tree.create_default_node();
    tree.set_display(weighted, Display::Block)
        .expect("set weighted display");
    tree.set_box_sizing(weighted, BoxSizing::ContentBox)
        .expect("set weighted box sizing");
    tree.set_linear_weight(weighted, 1.0)
        .expect("set weighted weight");
    tree.set_aspect_ratio(weighted, Some(2.0))
        .expect("set weighted aspect ratio");
    tree.set_padding(weighted, StandaloneEdge::Left, Length::points(4.0))
        .expect("set weighted left padding");
    tree.set_padding(weighted, StandaloneEdge::Right, Length::points(6.0))
        .expect("set weighted right padding");
    tree.set_padding(weighted, StandaloneEdge::Top, Length::points(3.0))
        .expect("set weighted top padding");
    tree.set_padding(weighted, StandaloneEdge::Bottom, Length::points(5.0))
        .expect("set weighted bottom padding");
    tree.set_border(weighted, StandaloneEdge::Left, 1.0)
        .expect("set weighted left border");
    tree.set_border(weighted, StandaloneEdge::Right, 2.0)
        .expect("set weighted right border");
    tree.set_border(weighted, StandaloneEdge::Top, 1.0)
        .expect("set weighted top border");
    tree.set_border(weighted, StandaloneEdge::Bottom, 2.0)
        .expect("set weighted bottom border");

    let fixed = tree.create_default_node();
    tree.set_display(fixed, Display::Block)
        .expect("set fixed display");
    tree.set_width(fixed, Length::points(24.0))
        .expect("set fixed width");
    tree.set_height(fixed, Length::points(12.0))
        .expect("set fixed height");

    tree.append_child(root, weighted).expect("append weighted");
    tree.append_child(root, fixed).expect("append fixed");

    run_standalone_rust(tree, root, Constraints::definite(130.0, 96.0))
        .expect("horizontal-reverse linear weighted aspect-ratio content-box edge parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_rtl_horizontal_linear_weight_aspect_ratio_with_margins() {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_direction(root, Direction::Rtl)
        .expect("set root direction");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root orientation");
    tree.set_align_items(root, AlignItems::FlexStart)
        .expect("set root align items");
    tree.set_width(root, Length::points(140.0))
        .expect("set root width");
    tree.set_height(root, Length::points(100.0))
        .expect("set root height");

    let weighted = tree.create_default_node();
    tree.set_display(weighted, Display::Block)
        .expect("set weighted display");
    tree.set_linear_weight(weighted, 1.0)
        .expect("set weighted weight");
    tree.set_aspect_ratio(weighted, Some(1.25))
        .expect("set weighted aspect ratio");
    tree.set_margin(weighted, StandaloneEdge::Left, Length::points(5.0))
        .expect("set weighted left margin");
    tree.set_margin(weighted, StandaloneEdge::Right, Length::points(7.0))
        .expect("set weighted right margin");
    tree.set_margin(weighted, StandaloneEdge::Top, Length::points(3.0))
        .expect("set weighted top margin");
    tree.set_margin(weighted, StandaloneEdge::Bottom, Length::points(4.0))
        .expect("set weighted bottom margin");

    let fixed = tree.create_default_node();
    tree.set_display(fixed, Display::Block)
        .expect("set fixed display");
    tree.set_width(fixed, Length::points(20.0))
        .expect("set fixed width");
    tree.set_height(fixed, Length::points(10.0))
        .expect("set fixed height");

    tree.append_child(root, weighted).expect("append weighted");
    tree.append_child(root, fixed).expect("append fixed");

    run_standalone_rust(tree, root, Constraints::definite(140.0, 100.0))
        .expect("RTL horizontal linear weighted aspect-ratio margin parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_horizontal_linear_weight_stretch_overrides_aspect_ratio_cross_size()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root orientation");
    tree.set_align_items(root, AlignItems::Stretch)
        .expect("set root align items");
    tree.set_width(root, Length::points(120.0))
        .expect("set root width");
    tree.set_height(root, Length::points(80.0))
        .expect("set root height");

    let weighted = tree.create_default_node();
    tree.set_display(weighted, Display::Block)
        .expect("set weighted display");
    tree.set_linear_weight(weighted, 1.0)
        .expect("set weighted weight");
    tree.set_aspect_ratio(weighted, Some(3.0))
        .expect("set weighted aspect ratio");
    tree.set_margin(weighted, StandaloneEdge::Top, Length::points(4.0))
        .expect("set weighted top margin");
    tree.set_margin(weighted, StandaloneEdge::Bottom, Length::points(6.0))
        .expect("set weighted bottom margin");
    tree.append_child(root, weighted).expect("append weighted");

    run_standalone_rust(tree, root, Constraints::definite(120.0, 80.0))
        .expect("horizontal linear weighted stretch overrides aspect-ratio parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_vertical_linear_weight_layout_gravity_stretch_overrides_aspect_ratio_cross_size()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_linear_orientation(root, LinearOrientation::Vertical)
        .expect("set root orientation");
    tree.set_align_items(root, AlignItems::FlexStart)
        .expect("set root align items");
    tree.set_width(root, Length::points(90.0))
        .expect("set root width");
    tree.set_height(root, Length::points(130.0))
        .expect("set root height");

    let weighted = tree.create_default_node();
    tree.set_display(weighted, Display::Block)
        .expect("set weighted display");
    tree.set_linear_weight(weighted, 1.0)
        .expect("set weighted weight");
    tree.set_linear_layout_gravity(weighted, LinearLayoutGravity::Stretch)
        .expect("set weighted layout gravity");
    tree.set_aspect_ratio(weighted, Some(0.5))
        .expect("set weighted aspect ratio");
    tree.set_margin(weighted, StandaloneEdge::Left, Length::points(5.0))
        .expect("set weighted left margin");
    tree.set_margin(weighted, StandaloneEdge::Right, Length::points(7.0))
        .expect("set weighted right margin");
    tree.append_child(root, weighted).expect("append weighted");

    run_standalone_rust(tree, root, Constraints::definite(90.0, 130.0))
        .expect("vertical linear weighted layout-gravity stretch overrides aspect-ratio parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_weight_horizontal_at_most_main_axis_disabled() {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root orientation");
    tree.set_height(root, Length::points(20.0))
        .expect("set root height");

    let child = tree.create_default_node();
    tree.set_display(child, Display::Block)
        .expect("set child display");
    tree.set_linear_weight(child, 1.0)
        .expect("set child weight");
    tree.append_child(root, child).expect("append child");

    run_standalone_rust(
        tree,
        root,
        Constraints::new(
            SideConstraint::at_most(100.0),
            SideConstraint::definite(20.0),
        ),
    )
    .expect("linear horizontal AtMost main-axis weight-disabled parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_weight_vertical_at_most_main_axis_disabled() {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_linear_orientation(root, LinearOrientation::Vertical)
        .expect("set root orientation");
    tree.set_width(root, Length::points(20.0))
        .expect("set root width");

    let child = tree.create_default_node();
    tree.set_display(child, Display::Block)
        .expect("set child display");
    tree.set_linear_weight(child, 1.0)
        .expect("set child weight");
    tree.append_child(root, child).expect("append child");

    run_standalone_rust(
        tree,
        root,
        Constraints::new(
            SideConstraint::definite(20.0),
            SideConstraint::at_most(100.0),
        ),
    )
    .expect("linear vertical AtMost main-axis weight-disabled parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_weight_rtl_percent_min_width_freeze() {
    let mut tree = StandaloneTree::new();
    let root = weighted_linear_freeze_root(
        &mut tree,
        LinearOrientation::Horizontal,
        Size::new(100.0, 20.0),
    );
    tree.set_direction(root, Direction::Rtl)
        .expect("set root direction");

    let floor = weighted_linear_child(&mut tree, true);
    tree.set_min_width(floor, Length::percent(70.0))
        .expect("set floor percent min width");
    let flexible = weighted_linear_child(&mut tree, true);
    tree.append_child(root, floor).expect("append floor");
    tree.append_child(root, flexible).expect("append flexible");

    run_standalone_rust(tree, root, Constraints::definite(100.0, 20.0))
        .expect("linear RTL percent min-width freeze parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_weight_horizontal_reverse_percent_max_width_freeze()
{
    let mut tree = StandaloneTree::new();
    let root = weighted_linear_freeze_root(
        &mut tree,
        LinearOrientation::HorizontalReverse,
        Size::new(100.0, 20.0),
    );

    let capped = weighted_linear_child(&mut tree, true);
    tree.set_max_width(capped, Length::percent(30.0))
        .expect("set capped percent max width");
    let flexible = weighted_linear_child(&mut tree, true);
    tree.append_child(root, capped).expect("append capped");
    tree.append_child(root, flexible).expect("append flexible");

    run_standalone_rust(tree, root, Constraints::definite(100.0, 20.0))
        .expect("linear horizontal-reverse percent max-width freeze parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_weight_vertical_percent_min_height_freeze() {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_linear_orientation(root, LinearOrientation::Vertical)
        .expect("set root orientation");
    tree.set_align_items(root, AlignItems::FlexStart)
        .expect("set root align items");
    tree.set_width(root, Length::points(20.0))
        .expect("set root width");
    tree.set_height(root, Length::points(100.0))
        .expect("set root height");

    let floor = tree.create_default_node();
    tree.set_display(floor, Display::Block)
        .expect("set floor display");
    tree.set_linear_weight(floor, 1.0)
        .expect("set floor weight");
    tree.set_width(floor, Length::points(10.0))
        .expect("set floor width");
    tree.set_min_height(floor, Length::percent(70.0))
        .expect("set floor percent min height");

    let flexible = tree.create_default_node();
    tree.set_display(flexible, Display::Block)
        .expect("set flexible display");
    tree.set_linear_weight(flexible, 1.0)
        .expect("set flexible weight");
    tree.set_width(flexible, Length::points(10.0))
        .expect("set flexible width");

    tree.append_child(root, floor).expect("append floor");
    tree.append_child(root, flexible).expect("append flexible");

    run_standalone_rust(tree, root, Constraints::definite(20.0, 100.0))
        .expect("linear vertical percent min-height freeze parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_weight_vertical_reverse_percent_max_height_freeze()
{
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_linear_orientation(root, LinearOrientation::VerticalReverse)
        .expect("set root orientation");
    tree.set_align_items(root, AlignItems::FlexStart)
        .expect("set root align items");
    tree.set_width(root, Length::points(20.0))
        .expect("set root width");
    tree.set_height(root, Length::points(100.0))
        .expect("set root height");

    let capped = tree.create_default_node();
    tree.set_display(capped, Display::Block)
        .expect("set capped display");
    tree.set_linear_weight(capped, 1.0)
        .expect("set capped weight");
    tree.set_width(capped, Length::points(10.0))
        .expect("set capped width");
    tree.set_max_height(capped, Length::percent(30.0))
        .expect("set capped percent max height");

    let flexible = tree.create_default_node();
    tree.set_display(flexible, Display::Block)
        .expect("set flexible display");
    tree.set_linear_weight(flexible, 1.0)
        .expect("set flexible weight");
    tree.set_width(flexible, Length::points(10.0))
        .expect("set flexible width");

    tree.append_child(root, capped).expect("append capped");
    tree.append_child(root, flexible).expect("append flexible");

    run_standalone_rust(tree, root, Constraints::definite(20.0, 100.0))
        .expect("linear vertical-reverse percent max-height freeze parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_weight_horizontal_calc_min_width_freeze() {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root orientation");
    tree.set_align_items(root, AlignItems::FlexStart)
        .expect("set root align items");
    tree.set_width(root, Length::points(100.0))
        .expect("set root width");
    tree.set_height(root, Length::points(20.0))
        .expect("set root height");

    let floor = tree.create_default_node();
    tree.set_display(floor, Display::Block)
        .expect("set floor display");
    tree.set_linear_weight(floor, 1.0)
        .expect("set floor weight");
    tree.set_height(floor, Length::points(10.0))
        .expect("set floor height");
    tree.set_min_width(floor, Length::calc(10.0, 55.0))
        .expect("set floor calc min width");

    let flexible = tree.create_default_node();
    tree.set_display(flexible, Display::Block)
        .expect("set flexible display");
    tree.set_linear_weight(flexible, 1.0)
        .expect("set flexible weight");
    tree.set_height(flexible, Length::points(10.0))
        .expect("set flexible height");

    tree.append_child(root, floor).expect("append floor");
    tree.append_child(root, flexible).expect("append flexible");

    run_standalone_rust(tree, root, Constraints::definite(100.0, 20.0))
        .expect("linear horizontal calc min-width freeze parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_weight_vertical_calc_max_height_freeze() {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_linear_orientation(root, LinearOrientation::Vertical)
        .expect("set root orientation");
    tree.set_align_items(root, AlignItems::FlexStart)
        .expect("set root align items");
    tree.set_width(root, Length::points(20.0))
        .expect("set root width");
    tree.set_height(root, Length::points(100.0))
        .expect("set root height");

    let capped = tree.create_default_node();
    tree.set_display(capped, Display::Block)
        .expect("set capped display");
    tree.set_linear_weight(capped, 1.0)
        .expect("set capped weight");
    tree.set_width(capped, Length::points(10.0))
        .expect("set capped width");
    tree.set_max_height(capped, Length::calc(5.0, 25.0))
        .expect("set capped calc max height");

    let flexible = tree.create_default_node();
    tree.set_display(flexible, Display::Block)
        .expect("set flexible display");
    tree.set_linear_weight(flexible, 1.0)
        .expect("set flexible weight");
    tree.set_width(flexible, Length::points(10.0))
        .expect("set flexible width");

    tree.append_child(root, capped).expect("append capped");
    tree.append_child(root, flexible).expect("append flexible");

    run_standalone_rust(tree, root, Constraints::definite(20.0, 100.0))
        .expect("linear vertical calc max-height freeze parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_weight_rtl_horizontal_reverse_percent_min_width_freeze()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_direction(root, Direction::Rtl)
        .expect("set root direction");
    tree.set_linear_orientation(root, LinearOrientation::HorizontalReverse)
        .expect("set root orientation");
    tree.set_align_items(root, AlignItems::FlexStart)
        .expect("set root align items");
    tree.set_width(root, Length::points(100.0))
        .expect("set root width");
    tree.set_height(root, Length::points(20.0))
        .expect("set root height");

    let floor = tree.create_default_node();
    tree.set_display(floor, Display::Block)
        .expect("set floor display");
    tree.set_linear_weight(floor, 1.0)
        .expect("set floor weight");
    tree.set_height(floor, Length::points(10.0))
        .expect("set floor height");
    tree.set_min_width(floor, Length::percent(70.0))
        .expect("set floor percent min width");

    let flexible = tree.create_default_node();
    tree.set_display(flexible, Display::Block)
        .expect("set flexible display");
    tree.set_linear_weight(flexible, 1.0)
        .expect("set flexible weight");
    tree.set_height(flexible, Length::points(10.0))
        .expect("set flexible height");

    tree.append_child(root, floor).expect("append floor");
    tree.append_child(root, flexible).expect("append flexible");

    run_standalone_rust(tree, root, Constraints::definite(100.0, 20.0))
        .expect("linear RTL horizontal-reverse percent min-width freeze parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_weight_vertical_reverse_calc_min_height_freeze() {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_linear_orientation(root, LinearOrientation::VerticalReverse)
        .expect("set root orientation");
    tree.set_align_items(root, AlignItems::FlexStart)
        .expect("set root align items");
    tree.set_width(root, Length::points(20.0))
        .expect("set root width");
    tree.set_height(root, Length::points(100.0))
        .expect("set root height");

    let floor = tree.create_default_node();
    tree.set_display(floor, Display::Block)
        .expect("set floor display");
    tree.set_linear_weight(floor, 1.0)
        .expect("set floor weight");
    tree.set_width(floor, Length::points(10.0))
        .expect("set floor width");
    tree.set_min_height(floor, Length::calc(10.0, 55.0))
        .expect("set floor calc min height");

    let flexible = tree.create_default_node();
    tree.set_display(flexible, Display::Block)
        .expect("set flexible display");
    tree.set_linear_weight(flexible, 1.0)
        .expect("set flexible weight");
    tree.set_width(flexible, Length::points(10.0))
        .expect("set flexible width");

    tree.append_child(root, floor).expect("append floor");
    tree.append_child(root, flexible).expect("append flexible");

    run_standalone_rust(tree, root, Constraints::definite(20.0, 100.0))
        .expect("linear vertical-reverse calc min-height freeze parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_positive_weight_space() {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_width(root, Length::points(100.0))
        .expect("set root width");
    tree.set_height(root, Length::points(100.0))
        .expect("set root height");

    let fixed = tree.create_default_node();
    tree.set_display(fixed, Display::Block)
        .expect("set fixed display");
    tree.set_height(fixed, Length::points(10.0))
        .expect("set fixed height");

    let weighted = tree.create_default_node();
    tree.set_display(weighted, Display::Block)
        .expect("set weighted display");
    tree.set_linear_weight(weighted, 1.0)
        .expect("set weighted weight");

    tree.append_child(root, fixed).expect("append fixed");
    tree.append_child(root, weighted).expect("append weighted");

    run_standalone_rust(tree, root, Constraints::definite(100.0, 100.0))
        .expect("linear positive weight space parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_exhausted_weight_space() {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_width(root, Length::points(100.0))
        .expect("set root width");
    tree.set_height(root, Length::points(20.0))
        .expect("set root height");

    let fixed = tree.create_default_node();
    tree.set_display(fixed, Display::Block)
        .expect("set fixed display");
    tree.set_width(fixed, Length::points(20.0))
        .expect("set fixed width");
    tree.set_height(fixed, Length::points(30.0))
        .expect("set fixed height");

    let weighted = tree.create_default_node();
    tree.set_display(weighted, Display::Block)
        .expect("set weighted display");
    tree.set_width(weighted, Length::points(20.0))
        .expect("set weighted width");
    tree.set_height(weighted, Length::Auto)
        .expect("set weighted auto height");
    tree.set_linear_weight(weighted, 1.0)
        .expect("set weighted weight");

    tree.append_child(root, fixed).expect("append fixed");
    tree.append_child(root, weighted).expect("append weighted");

    run_standalone_rust(tree, root, Constraints::definite(100.0, 20.0))
        .expect("linear exhausted weight space parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_weight_min_max_freeze_distribution() {
    for (case_name, tree, root, constraints) in [
        linear_weight_max_width_freeze_tree(),
        linear_weight_percent_max_width_freeze_tree(),
        linear_weight_min_width_freeze_tree(),
        linear_weight_percent_min_width_freeze_tree(),
        linear_weight_max_height_freeze_tree(),
        linear_weight_percent_max_height_freeze_tree(),
        linear_weight_min_height_freeze_tree(),
        linear_weight_percent_min_height_freeze_tree(),
    ] {
        run_standalone_rust(tree, root, constraints)
            .unwrap_or_else(|error| panic!("{case_name} parity failed: {error}"));
    }
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_unallocated_weight_space() {
    for (case_name, tree, root, constraints) in [
        linear_weight_sum_unallocated_space_tree(),
        linear_total_weight_below_one_unallocated_space_tree(),
        linear_weight_sub_epsilon_min_violation_tree(),
    ] {
        run_standalone_rust(tree, root, constraints)
            .unwrap_or_else(|error| panic!("{case_name} parity failed: {error}"));
    }
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_gravity_mapping() {
    let mut cases = Vec::new();
    for gravity in [
        LinearGravity::None,
        LinearGravity::Top,
        LinearGravity::Bottom,
        LinearGravity::Left,
        LinearGravity::Right,
        LinearGravity::CenterVertical,
        LinearGravity::CenterHorizontal,
        LinearGravity::SpaceBetween,
        LinearGravity::Start,
        LinearGravity::End,
        LinearGravity::Center,
    ] {
        cases.push(vertical_linear_gravity_mapping_tree(gravity));
    }
    cases.push(horizontal_linear_gravity_overrides_justify_content_tree());
    for gravity in [LinearGravity::Left, LinearGravity::Right] {
        cases.push(rtl_horizontal_linear_gravity_tree(gravity));
    }

    for (case_name, tree, root, constraints) in cases {
        run_standalone_rust(tree, root, constraints)
            .unwrap_or_else(|error| panic!("{case_name} parity failed: {error}"));
    }
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_horizontal_ltr_fronts() {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_direction(root, Direction::Ltr)
        .expect("set root direction");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root orientation");
    tree.set_align_items(root, AlignItems::FlexStart)
        .expect("set root align items");
    tree.set_width(root, Length::points(100.0))
        .expect("set root width");
    tree.set_height(root, Length::points(80.0))
        .expect("set root height");

    let first = fixed_linear_gravity_child(&mut tree, 10.0, 12.0);
    tree.set_margin(first, StandaloneEdge::Left, Length::points(2.0))
        .expect("set first left margin");
    tree.set_margin(first, StandaloneEdge::Right, Length::points(3.0))
        .expect("set first right margin");
    tree.set_margin(first, StandaloneEdge::Top, Length::points(4.0))
        .expect("set first top margin");
    tree.set_margin(first, StandaloneEdge::Bottom, Length::points(5.0))
        .expect("set first bottom margin");
    let second = fixed_linear_gravity_child(&mut tree, 20.0, 8.0);
    tree.set_margin(second, StandaloneEdge::Left, Length::points(1.0))
        .expect("set second left margin");
    tree.set_margin(second, StandaloneEdge::Right, Length::points(2.0))
        .expect("set second right margin");
    tree.set_margin(second, StandaloneEdge::Top, Length::points(3.0))
        .expect("set second top margin");
    tree.set_margin(second, StandaloneEdge::Bottom, Length::points(4.0))
        .expect("set second bottom margin");
    tree.append_child(root, first).expect("append first");
    tree.append_child(root, second).expect("append second");

    run_standalone_rust(tree, root, Constraints::definite(100.0, 80.0))
        .expect("linear horizontal LTR fronts parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_horizontal_rtl_fronts() {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_direction(root, Direction::Rtl)
        .expect("set root direction");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root orientation");
    tree.set_align_items(root, AlignItems::FlexStart)
        .expect("set root align items");
    tree.set_width(root, Length::points(100.0))
        .expect("set root width");
    tree.set_height(root, Length::points(80.0))
        .expect("set root height");

    let first = fixed_linear_gravity_child(&mut tree, 10.0, 12.0);
    tree.set_margin(first, StandaloneEdge::Left, Length::points(2.0))
        .expect("set first left margin");
    tree.set_margin(first, StandaloneEdge::Right, Length::points(3.0))
        .expect("set first right margin");
    tree.set_margin(first, StandaloneEdge::Top, Length::points(4.0))
        .expect("set first top margin");
    tree.set_margin(first, StandaloneEdge::Bottom, Length::points(5.0))
        .expect("set first bottom margin");
    let second = fixed_linear_gravity_child(&mut tree, 20.0, 8.0);
    tree.set_margin(second, StandaloneEdge::Left, Length::points(1.0))
        .expect("set second left margin");
    tree.set_margin(second, StandaloneEdge::Right, Length::points(2.0))
        .expect("set second right margin");
    tree.set_margin(second, StandaloneEdge::Top, Length::points(3.0))
        .expect("set second top margin");
    tree.set_margin(second, StandaloneEdge::Bottom, Length::points(4.0))
        .expect("set second bottom margin");
    tree.append_child(root, first).expect("append first");
    tree.append_child(root, second).expect("append second");

    run_standalone_rust(tree, root, Constraints::definite(100.0, 80.0))
        .expect("linear horizontal RTL fronts parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_horizontal_reverse_ltr_fronts() {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_direction(root, Direction::Ltr)
        .expect("set root direction");
    tree.set_linear_orientation(root, LinearOrientation::HorizontalReverse)
        .expect("set root orientation");
    tree.set_align_items(root, AlignItems::FlexStart)
        .expect("set root align items");
    tree.set_width(root, Length::points(100.0))
        .expect("set root width");
    tree.set_height(root, Length::points(80.0))
        .expect("set root height");

    let first = fixed_linear_gravity_child(&mut tree, 10.0, 12.0);
    tree.set_margin(first, StandaloneEdge::Left, Length::points(2.0))
        .expect("set first left margin");
    tree.set_margin(first, StandaloneEdge::Right, Length::points(3.0))
        .expect("set first right margin");
    tree.set_margin(first, StandaloneEdge::Top, Length::points(4.0))
        .expect("set first top margin");
    tree.set_margin(first, StandaloneEdge::Bottom, Length::points(5.0))
        .expect("set first bottom margin");
    let second = fixed_linear_gravity_child(&mut tree, 20.0, 8.0);
    tree.set_margin(second, StandaloneEdge::Left, Length::points(1.0))
        .expect("set second left margin");
    tree.set_margin(second, StandaloneEdge::Right, Length::points(2.0))
        .expect("set second right margin");
    tree.set_margin(second, StandaloneEdge::Top, Length::points(3.0))
        .expect("set second top margin");
    tree.set_margin(second, StandaloneEdge::Bottom, Length::points(4.0))
        .expect("set second bottom margin");
    tree.append_child(root, first).expect("append first");
    tree.append_child(root, second).expect("append second");

    run_standalone_rust(tree, root, Constraints::definite(100.0, 80.0))
        .expect("linear horizontal-reverse LTR fronts parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_horizontal_reverse_rtl_fronts() {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_direction(root, Direction::Rtl)
        .expect("set root direction");
    tree.set_linear_orientation(root, LinearOrientation::HorizontalReverse)
        .expect("set root orientation");
    tree.set_align_items(root, AlignItems::FlexStart)
        .expect("set root align items");
    tree.set_width(root, Length::points(100.0))
        .expect("set root width");
    tree.set_height(root, Length::points(80.0))
        .expect("set root height");

    let first = fixed_linear_gravity_child(&mut tree, 10.0, 12.0);
    tree.set_margin(first, StandaloneEdge::Left, Length::points(2.0))
        .expect("set first left margin");
    tree.set_margin(first, StandaloneEdge::Right, Length::points(3.0))
        .expect("set first right margin");
    tree.set_margin(first, StandaloneEdge::Top, Length::points(4.0))
        .expect("set first top margin");
    tree.set_margin(first, StandaloneEdge::Bottom, Length::points(5.0))
        .expect("set first bottom margin");
    let second = fixed_linear_gravity_child(&mut tree, 20.0, 8.0);
    tree.set_margin(second, StandaloneEdge::Left, Length::points(1.0))
        .expect("set second left margin");
    tree.set_margin(second, StandaloneEdge::Right, Length::points(2.0))
        .expect("set second right margin");
    tree.set_margin(second, StandaloneEdge::Top, Length::points(3.0))
        .expect("set second top margin");
    tree.set_margin(second, StandaloneEdge::Bottom, Length::points(4.0))
        .expect("set second bottom margin");
    tree.append_child(root, first).expect("append first");
    tree.append_child(root, second).expect("append second");

    run_standalone_rust(tree, root, Constraints::definite(100.0, 80.0))
        .expect("linear horizontal-reverse RTL fronts parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_vertical_ltr_fronts() {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_direction(root, Direction::Ltr)
        .expect("set root direction");
    tree.set_linear_orientation(root, LinearOrientation::Vertical)
        .expect("set root orientation");
    tree.set_align_items(root, AlignItems::FlexStart)
        .expect("set root align items");
    tree.set_width(root, Length::points(100.0))
        .expect("set root width");
    tree.set_height(root, Length::points(80.0))
        .expect("set root height");

    let first = fixed_linear_gravity_child(&mut tree, 10.0, 12.0);
    tree.set_margin(first, StandaloneEdge::Left, Length::points(2.0))
        .expect("set first left margin");
    tree.set_margin(first, StandaloneEdge::Right, Length::points(3.0))
        .expect("set first right margin");
    tree.set_margin(first, StandaloneEdge::Top, Length::points(4.0))
        .expect("set first top margin");
    tree.set_margin(first, StandaloneEdge::Bottom, Length::points(5.0))
        .expect("set first bottom margin");
    let second = fixed_linear_gravity_child(&mut tree, 20.0, 8.0);
    tree.set_margin(second, StandaloneEdge::Left, Length::points(1.0))
        .expect("set second left margin");
    tree.set_margin(second, StandaloneEdge::Right, Length::points(2.0))
        .expect("set second right margin");
    tree.set_margin(second, StandaloneEdge::Top, Length::points(3.0))
        .expect("set second top margin");
    tree.set_margin(second, StandaloneEdge::Bottom, Length::points(4.0))
        .expect("set second bottom margin");
    tree.append_child(root, first).expect("append first");
    tree.append_child(root, second).expect("append second");

    run_standalone_rust(tree, root, Constraints::definite(100.0, 80.0))
        .expect("linear vertical LTR fronts parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_vertical_rtl_fronts() {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_direction(root, Direction::Rtl)
        .expect("set root direction");
    tree.set_linear_orientation(root, LinearOrientation::Vertical)
        .expect("set root orientation");
    tree.set_align_items(root, AlignItems::FlexStart)
        .expect("set root align items");
    tree.set_width(root, Length::points(100.0))
        .expect("set root width");
    tree.set_height(root, Length::points(80.0))
        .expect("set root height");

    let first = fixed_linear_gravity_child(&mut tree, 10.0, 12.0);
    tree.set_margin(first, StandaloneEdge::Left, Length::points(2.0))
        .expect("set first left margin");
    tree.set_margin(first, StandaloneEdge::Right, Length::points(3.0))
        .expect("set first right margin");
    tree.set_margin(first, StandaloneEdge::Top, Length::points(4.0))
        .expect("set first top margin");
    tree.set_margin(first, StandaloneEdge::Bottom, Length::points(5.0))
        .expect("set first bottom margin");
    let second = fixed_linear_gravity_child(&mut tree, 20.0, 8.0);
    tree.set_margin(second, StandaloneEdge::Left, Length::points(1.0))
        .expect("set second left margin");
    tree.set_margin(second, StandaloneEdge::Right, Length::points(2.0))
        .expect("set second right margin");
    tree.set_margin(second, StandaloneEdge::Top, Length::points(3.0))
        .expect("set second top margin");
    tree.set_margin(second, StandaloneEdge::Bottom, Length::points(4.0))
        .expect("set second bottom margin");
    tree.append_child(root, first).expect("append first");
    tree.append_child(root, second).expect("append second");

    run_standalone_rust(tree, root, Constraints::definite(100.0, 80.0))
        .expect("linear vertical RTL fronts parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_vertical_reverse_ltr_fronts() {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_direction(root, Direction::Ltr)
        .expect("set root direction");
    tree.set_linear_orientation(root, LinearOrientation::VerticalReverse)
        .expect("set root orientation");
    tree.set_align_items(root, AlignItems::FlexStart)
        .expect("set root align items");
    tree.set_width(root, Length::points(100.0))
        .expect("set root width");
    tree.set_height(root, Length::points(80.0))
        .expect("set root height");

    let first = fixed_linear_gravity_child(&mut tree, 10.0, 12.0);
    tree.set_margin(first, StandaloneEdge::Left, Length::points(2.0))
        .expect("set first left margin");
    tree.set_margin(first, StandaloneEdge::Right, Length::points(3.0))
        .expect("set first right margin");
    tree.set_margin(first, StandaloneEdge::Top, Length::points(4.0))
        .expect("set first top margin");
    tree.set_margin(first, StandaloneEdge::Bottom, Length::points(5.0))
        .expect("set first bottom margin");
    let second = fixed_linear_gravity_child(&mut tree, 20.0, 8.0);
    tree.set_margin(second, StandaloneEdge::Left, Length::points(1.0))
        .expect("set second left margin");
    tree.set_margin(second, StandaloneEdge::Right, Length::points(2.0))
        .expect("set second right margin");
    tree.set_margin(second, StandaloneEdge::Top, Length::points(3.0))
        .expect("set second top margin");
    tree.set_margin(second, StandaloneEdge::Bottom, Length::points(4.0))
        .expect("set second bottom margin");
    tree.append_child(root, first).expect("append first");
    tree.append_child(root, second).expect("append second");

    run_standalone_rust(tree, root, Constraints::definite(100.0, 80.0))
        .expect("linear vertical-reverse LTR fronts parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_vertical_reverse_rtl_fronts() {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_direction(root, Direction::Rtl)
        .expect("set root direction");
    tree.set_linear_orientation(root, LinearOrientation::VerticalReverse)
        .expect("set root orientation");
    tree.set_align_items(root, AlignItems::FlexStart)
        .expect("set root align items");
    tree.set_width(root, Length::points(100.0))
        .expect("set root width");
    tree.set_height(root, Length::points(80.0))
        .expect("set root height");

    let first = fixed_linear_gravity_child(&mut tree, 10.0, 12.0);
    tree.set_margin(first, StandaloneEdge::Left, Length::points(2.0))
        .expect("set first left margin");
    tree.set_margin(first, StandaloneEdge::Right, Length::points(3.0))
        .expect("set first right margin");
    tree.set_margin(first, StandaloneEdge::Top, Length::points(4.0))
        .expect("set first top margin");
    tree.set_margin(first, StandaloneEdge::Bottom, Length::points(5.0))
        .expect("set first bottom margin");
    let second = fixed_linear_gravity_child(&mut tree, 20.0, 8.0);
    tree.set_margin(second, StandaloneEdge::Left, Length::points(1.0))
        .expect("set second left margin");
    tree.set_margin(second, StandaloneEdge::Right, Length::points(2.0))
        .expect("set second right margin");
    tree.set_margin(second, StandaloneEdge::Top, Length::points(3.0))
        .expect("set second top margin");
    tree.set_margin(second, StandaloneEdge::Bottom, Length::points(4.0))
        .expect("set second bottom margin");
    tree.append_child(root, first).expect("append first");
    tree.append_child(root, second).expect("append second");

    run_standalone_rust(tree, root, Constraints::definite(100.0, 80.0))
        .expect("linear vertical-reverse RTL fronts parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_space_between_single_item() {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_linear_gravity(root, LinearGravity::SpaceBetween)
        .expect("set root linear gravity");
    tree.set_width(root, Length::points(100.0))
        .expect("set root width");
    tree.set_height(root, Length::points(100.0))
        .expect("set root height");

    let child = tree.create_default_node();
    tree.set_display(child, Display::Block)
        .expect("set child display");
    tree.set_width(child, Length::points(10.0))
        .expect("set child width");
    tree.set_height(child, Length::points(10.0))
        .expect("set child height");
    tree.append_child(root, child).expect("append child");

    run_standalone_rust(tree, root, Constraints::definite(100.0, 100.0))
        .expect("linear space-between single item parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_space_between_multi_item() {
    {
        let mut tree = StandaloneTree::new();
        let root = tree.create_default_node();
        tree.set_display(root, Display::Linear)
            .expect("set root display");
        tree.set_linear_gravity(root, LinearGravity::SpaceBetween)
            .expect("set root linear gravity");
        tree.set_width(root, Length::points(100.0))
            .expect("set root width");
        tree.set_height(root, Length::points(100.0))
            .expect("set root height");

        let first = tree.create_default_node();
        tree.set_display(first, Display::Block)
            .expect("set first display");
        tree.set_height(first, Length::points(10.0))
            .expect("set first height");

        let second = tree.create_default_node();
        tree.set_display(second, Display::Block)
            .expect("set second display");
        tree.set_height(second, Length::points(10.0))
            .expect("set second height");

        tree.append_child(root, first).expect("append first");
        tree.append_child(root, second).expect("append second");

        run_standalone_rust(tree, root, Constraints::definite(100.0, 100.0))
            .expect("linear space-between multi-item positive free-space parity");
    }

    {
        let mut tree = StandaloneTree::new();
        let root = tree.create_default_node();
        tree.set_display(root, Display::Linear)
            .expect("set root display");
        tree.set_linear_gravity(root, LinearGravity::SpaceBetween)
            .expect("set root linear gravity");
        tree.set_width(root, Length::points(100.0))
            .expect("set root width");
        tree.set_height(root, Length::points(100.0))
            .expect("set root height");

        let first = tree.create_default_node();
        tree.set_display(first, Display::Block)
            .expect("set first display");
        tree.set_height(first, Length::points(70.0))
            .expect("set first height");

        let second = tree.create_default_node();
        tree.set_display(second, Display::Block)
            .expect("set second display");
        tree.set_height(second, Length::points(70.0))
            .expect("set second height");

        tree.append_child(root, first).expect("append first");
        tree.append_child(root, second).expect("append second");

        run_standalone_rust(tree, root, Constraints::definite(100.0, 100.0))
            .expect("linear space-between multi-item overflow parity");
    }
}

#[test]
fn standalone_owned_tree_matches_cpp_for_horizontal_linear_center_overflow_with_root_padding_border_and_percent_margins()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::Center)
        .expect("set root gravity");
    tree.set_width(root, Length::points(70.0))
        .expect("set root width");
    tree.set_height(root, Length::points(40.0))
        .expect("set root height");
    tree.set_padding(root, StandaloneEdge::Left, Length::points(4.0))
        .expect("set root left padding");
    tree.set_padding(root, StandaloneEdge::Right, Length::points(6.0))
        .expect("set root right padding");
    tree.set_border(root, StandaloneEdge::Left, 1.0)
        .expect("set root left border");
    tree.set_border(root, StandaloneEdge::Right, 2.0)
        .expect("set root right border");

    let first = tree.create_default_node();
    tree.set_display(first, Display::Block)
        .expect("set first display");
    tree.set_width(first, Length::points(48.0))
        .expect("set first width");
    tree.set_height(first, Length::points(12.0))
        .expect("set first height");
    tree.set_margin(first, StandaloneEdge::Left, Length::percent(12.0))
        .expect("set first left margin");
    tree.set_margin(first, StandaloneEdge::Right, Length::percent(8.0))
        .expect("set first right margin");

    let second = tree.create_default_node();
    tree.set_display(second, Display::Block)
        .expect("set second display");
    tree.set_width(second, Length::points(42.0))
        .expect("set second width");
    tree.set_height(second, Length::points(14.0))
        .expect("set second height");
    tree.set_margin(second, StandaloneEdge::Left, Length::points(6.0))
        .expect("set second left margin");
    tree.set_margin(second, StandaloneEdge::Right, Length::points(5.0))
        .expect("set second right margin");

    tree.append_child(root, first).expect("append first");
    tree.append_child(root, second).expect("append second");

    run_standalone_rust(tree, root, Constraints::definite(83.0, 40.0))
        .expect("horizontal linear center overflow with root padding/border parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_horizontal_reverse_linear_center_overflow_with_calc_main_margins()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::HorizontalReverse)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::Center)
        .expect("set root gravity");
    tree.set_width(root, Length::points(80.0))
        .expect("set root width");
    tree.set_height(root, Length::points(44.0))
        .expect("set root height");

    let first = tree.create_default_node();
    tree.set_display(first, Display::Block)
        .expect("set first display");
    tree.set_width(first, Length::points(52.0))
        .expect("set first width");
    tree.set_height(first, Length::points(16.0))
        .expect("set first height");
    tree.set_margin(first, StandaloneEdge::Left, Length::calc(5.0, 8.0))
        .expect("set first left margin");
    tree.set_margin(first, StandaloneEdge::Right, Length::calc(3.0, 7.0))
        .expect("set first right margin");

    let second = tree.create_default_node();
    tree.set_display(second, Display::Block)
        .expect("set second display");
    tree.set_width(second, Length::points(46.0))
        .expect("set second width");
    tree.set_height(second, Length::points(12.0))
        .expect("set second height");
    tree.set_margin(second, StandaloneEdge::Left, Length::calc(4.0, 6.0))
        .expect("set second left margin");
    tree.set_margin(second, StandaloneEdge::Right, Length::calc(2.0, 5.0))
        .expect("set second right margin");

    tree.append_child(root, first).expect("append first");
    tree.append_child(root, second).expect("append second");

    run_standalone_rust(tree, root, Constraints::definite(80.0, 44.0))
        .expect("horizontal-reverse linear center overflow with calc margins parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_vertical_linear_end_overflow_with_calc_main_margins_and_root_padding_border()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Vertical)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::End)
        .expect("set root gravity");
    tree.set_width(root, Length::points(52.0))
        .expect("set root width");
    tree.set_height(root, Length::points(72.0))
        .expect("set root height");
    tree.set_padding(root, StandaloneEdge::Top, Length::points(5.0))
        .expect("set root top padding");
    tree.set_padding(root, StandaloneEdge::Bottom, Length::points(3.0))
        .expect("set root bottom padding");
    tree.set_border(root, StandaloneEdge::Top, 2.0)
        .expect("set root top border");
    tree.set_border(root, StandaloneEdge::Bottom, 1.0)
        .expect("set root bottom border");

    let first = tree.create_default_node();
    tree.set_display(first, Display::Block)
        .expect("set first display");
    tree.set_width(first, Length::points(24.0))
        .expect("set first width");
    tree.set_height(first, Length::points(48.0))
        .expect("set first height");
    tree.set_margin(first, StandaloneEdge::Top, Length::calc(4.0, 5.0))
        .expect("set first top margin");
    tree.set_margin(first, StandaloneEdge::Bottom, Length::calc(3.0, 6.0))
        .expect("set first bottom margin");

    let second = tree.create_default_node();
    tree.set_display(second, Display::Block)
        .expect("set second display");
    tree.set_width(second, Length::points(26.0))
        .expect("set second width");
    tree.set_height(second, Length::points(42.0))
        .expect("set second height");
    tree.set_margin(second, StandaloneEdge::Top, Length::points(5.0))
        .expect("set second top margin");
    tree.set_margin(second, StandaloneEdge::Bottom, Length::points(7.0))
        .expect("set second bottom margin");

    tree.append_child(root, first).expect("append first");
    tree.append_child(root, second).expect("append second");

    run_standalone_rust(tree, root, Constraints::definite(52.0, 83.0))
        .expect("vertical linear end overflow with root padding/border parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_vertical_reverse_linear_bottom_overflow_with_percent_main_margins()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::VerticalReverse)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::Bottom)
        .expect("set root gravity");
    tree.set_width(root, Length::points(58.0))
        .expect("set root width");
    tree.set_height(root, Length::points(82.0))
        .expect("set root height");

    let first = tree.create_default_node();
    tree.set_display(first, Display::Block)
        .expect("set first display");
    tree.set_width(first, Length::points(28.0))
        .expect("set first width");
    tree.set_height(first, Length::points(54.0))
        .expect("set first height");
    tree.set_margin(first, StandaloneEdge::Top, Length::percent(8.0))
        .expect("set first top margin");
    tree.set_margin(first, StandaloneEdge::Bottom, Length::percent(6.0))
        .expect("set first bottom margin");

    let second = tree.create_default_node();
    tree.set_display(second, Display::Block)
        .expect("set second display");
    tree.set_width(second, Length::points(30.0))
        .expect("set second width");
    tree.set_height(second, Length::points(44.0))
        .expect("set second height");
    tree.set_margin(second, StandaloneEdge::Top, Length::points(4.0))
        .expect("set second top margin");
    tree.set_margin(second, StandaloneEdge::Bottom, Length::points(5.0))
        .expect("set second bottom margin");

    tree.append_child(root, first).expect("append first");
    tree.append_child(root, second).expect("append second");

    run_standalone_rust(tree, root, Constraints::definite(58.0, 82.0))
        .expect("vertical-reverse linear bottom overflow with percent margins parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_rtl_horizontal_linear_left_gravity_overflow_with_auto_and_calc_main_margins()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_direction(root, Direction::Rtl)
        .expect("set root direction");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::Left)
        .expect("set root gravity");
    tree.set_width(root, Length::points(76.0))
        .expect("set root width");
    tree.set_height(root, Length::points(42.0))
        .expect("set root height");

    let first = tree.create_default_node();
    tree.set_display(first, Display::Block)
        .expect("set first display");
    tree.set_width(first, Length::points(50.0))
        .expect("set first width");
    tree.set_height(first, Length::points(14.0))
        .expect("set first height");
    tree.set_margin(first, StandaloneEdge::Left, Length::Auto)
        .expect("set first left auto margin");
    tree.set_margin(first, StandaloneEdge::Right, Length::calc(4.0, 8.0))
        .expect("set first right margin");

    let second = tree.create_default_node();
    tree.set_display(second, Display::Block)
        .expect("set second display");
    tree.set_width(second, Length::points(44.0))
        .expect("set second width");
    tree.set_height(second, Length::points(12.0))
        .expect("set second height");
    tree.set_margin(second, StandaloneEdge::Left, Length::calc(3.0, 7.0))
        .expect("set second left margin");
    tree.set_margin(second, StandaloneEdge::Right, Length::points(5.0))
        .expect("set second right margin");

    tree.append_child(root, first).expect("append first");
    tree.append_child(root, second).expect("append second");

    run_standalone_rust(tree, root, Constraints::definite(76.0, 42.0))
        .expect("RTL horizontal linear left-gravity overflow with auto/calc margins parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_horizontal_linear_space_between_overflow_three_items_with_percent_and_calc_margins()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::SpaceBetween)
        .expect("set root gravity");
    tree.set_width(root, Length::points(92.0))
        .expect("set root width");
    tree.set_height(root, Length::points(44.0))
        .expect("set root height");
    tree.set_padding(root, StandaloneEdge::Horizontal, Length::points(3.0))
        .expect("set root horizontal padding");
    tree.set_border(root, StandaloneEdge::Horizontal, 1.0)
        .expect("set root horizontal border");

    let first = tree.create_default_node();
    tree.set_display(first, Display::Block)
        .expect("set first display");
    tree.set_width(first, Length::points(34.0))
        .expect("set first width");
    tree.set_height(first, Length::points(10.0))
        .expect("set first height");
    tree.set_margin(first, StandaloneEdge::Left, Length::percent(6.0))
        .expect("set first left margin");
    tree.set_margin(first, StandaloneEdge::Right, Length::calc(3.0, 5.0))
        .expect("set first right margin");

    let second = tree.create_default_node();
    tree.set_display(second, Display::Block)
        .expect("set second display");
    tree.set_width(second, Length::points(30.0))
        .expect("set second width");
    tree.set_height(second, Length::points(12.0))
        .expect("set second height");
    tree.set_margin(second, StandaloneEdge::Left, Length::calc(4.0, 4.0))
        .expect("set second left margin");
    tree.set_margin(second, StandaloneEdge::Right, Length::percent(5.0))
        .expect("set second right margin");

    let third = tree.create_default_node();
    tree.set_display(third, Display::Block)
        .expect("set third display");
    tree.set_width(third, Length::points(28.0))
        .expect("set third width");
    tree.set_height(third, Length::points(14.0))
        .expect("set third height");
    tree.set_margin(third, StandaloneEdge::Left, Length::points(5.0))
        .expect("set third left margin");
    tree.set_margin(third, StandaloneEdge::Right, Length::calc(2.0, 6.0))
        .expect("set third right margin");

    tree.append_child(root, first).expect("append first");
    tree.append_child(root, second).expect("append second");
    tree.append_child(root, third).expect("append third");

    run_standalone_rust(tree, root, Constraints::definite(100.0, 44.0))
        .expect("horizontal linear space-between overflow with percent/calc margins parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_horizontal_linear_center_with_fixed_main_margins() {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::Center)
        .expect("set root gravity");
    tree.set_width(root, Length::points(120.0))
        .expect("set root width");
    tree.set_height(root, Length::points(40.0))
        .expect("set root height");

    let first = fixed_linear_gravity_child(&mut tree, 20.0, 10.0);
    tree.set_margin(first, StandaloneEdge::Left, Length::points(4.0))
        .expect("set first left margin");
    tree.set_margin(first, StandaloneEdge::Right, Length::points(6.0))
        .expect("set first right margin");

    let second = fixed_linear_gravity_child(&mut tree, 10.0, 10.0);
    tree.set_margin(second, StandaloneEdge::Left, Length::points(3.0))
        .expect("set second left margin");
    tree.set_margin(second, StandaloneEdge::Right, Length::points(5.0))
        .expect("set second right margin");

    tree.append_child(root, first).expect("append first");
    tree.append_child(root, second).expect("append second");

    run_standalone_rust(tree, root, Constraints::definite(120.0, 40.0))
        .expect("horizontal linear center with fixed main margins parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_horizontal_reverse_linear_end_with_fixed_main_margins() {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_linear_orientation(root, LinearOrientation::HorizontalReverse)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::End)
        .expect("set root gravity");
    tree.set_width(root, Length::points(130.0))
        .expect("set root width");
    tree.set_height(root, Length::points(40.0))
        .expect("set root height");

    let first = fixed_linear_gravity_child(&mut tree, 18.0, 10.0);
    tree.set_margin(first, StandaloneEdge::Left, Length::points(4.0))
        .expect("set first left margin");
    tree.set_margin(first, StandaloneEdge::Right, Length::points(8.0))
        .expect("set first right margin");

    let second = fixed_linear_gravity_child(&mut tree, 14.0, 10.0);
    tree.set_margin(second, StandaloneEdge::Left, Length::points(2.0))
        .expect("set second left margin");
    tree.set_margin(second, StandaloneEdge::Right, Length::points(5.0))
        .expect("set second right margin");

    tree.append_child(root, first).expect("append first");
    tree.append_child(root, second).expect("append second");

    run_standalone_rust(tree, root, Constraints::definite(130.0, 40.0))
        .expect("horizontal-reverse linear end with fixed main margins parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_vertical_linear_space_between_with_fixed_main_margins() {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_linear_orientation(root, LinearOrientation::Vertical)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::SpaceBetween)
        .expect("set root gravity");
    tree.set_width(root, Length::points(60.0))
        .expect("set root width");
    tree.set_height(root, Length::points(150.0))
        .expect("set root height");

    let first = fixed_linear_gravity_child(&mut tree, 20.0, 18.0);
    tree.set_margin(first, StandaloneEdge::Top, Length::points(3.0))
        .expect("set first top margin");
    tree.set_margin(first, StandaloneEdge::Bottom, Length::points(7.0))
        .expect("set first bottom margin");

    let second = fixed_linear_gravity_child(&mut tree, 20.0, 16.0);
    tree.set_margin(second, StandaloneEdge::Top, Length::points(5.0))
        .expect("set second top margin");
    tree.set_margin(second, StandaloneEdge::Bottom, Length::points(9.0))
        .expect("set second bottom margin");

    tree.append_child(root, first).expect("append first");
    tree.append_child(root, second).expect("append second");

    run_standalone_rust(tree, root, Constraints::definite(60.0, 150.0))
        .expect("vertical linear space-between with fixed main margins parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_horizontal_linear_center_with_percent_main_margins() {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::Center)
        .expect("set root gravity");
    tree.set_width(root, Length::points(160.0))
        .expect("set root width");
    tree.set_height(root, Length::points(40.0))
        .expect("set root height");

    let child = fixed_linear_gravity_child(&mut tree, 40.0, 10.0);
    tree.set_margin(child, StandaloneEdge::Left, Length::percent(10.0))
        .expect("set child left margin");
    tree.set_margin(child, StandaloneEdge::Right, Length::percent(5.0))
        .expect("set child right margin");
    tree.append_child(root, child).expect("append child");

    run_standalone_rust(tree, root, Constraints::definite(160.0, 40.0))
        .expect("horizontal linear center with percent main margins parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_horizontal_linear_end_with_main_start_auto_margin() {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::End)
        .expect("set root gravity");
    tree.set_width(root, Length::points(120.0))
        .expect("set root width");
    tree.set_height(root, Length::points(40.0))
        .expect("set root height");

    let child = fixed_linear_gravity_child(&mut tree, 20.0, 10.0);
    tree.set_margin(child, StandaloneEdge::Left, Length::Auto)
        .expect("set child left auto margin");
    tree.set_margin(child, StandaloneEdge::Right, Length::points(7.0))
        .expect("set child right margin");
    tree.append_child(root, child).expect("append child");

    run_standalone_rust(tree, root, Constraints::definite(120.0, 40.0))
        .expect("horizontal linear end with main-start auto margin parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_vertical_linear_center_with_main_end_auto_margin() {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_linear_orientation(root, LinearOrientation::Vertical)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::Center)
        .expect("set root gravity");
    tree.set_width(root, Length::points(60.0))
        .expect("set root width");
    tree.set_height(root, Length::points(120.0))
        .expect("set root height");

    let child = fixed_linear_gravity_child(&mut tree, 20.0, 18.0);
    tree.set_margin(child, StandaloneEdge::Top, Length::points(5.0))
        .expect("set child top margin");
    tree.set_margin(child, StandaloneEdge::Bottom, Length::Auto)
        .expect("set child bottom auto margin");
    tree.append_child(root, child).expect("append child");

    run_standalone_rust(tree, root, Constraints::definite(60.0, 120.0))
        .expect("vertical linear center with main-end auto margin parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_horizontal_linear_center_with_calc_main_margins() {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::Center)
        .expect("set root gravity");
    tree.set_width(root, Length::points(180.0))
        .expect("set root width");
    tree.set_height(root, Length::points(50.0))
        .expect("set root height");

    let first = tree.create_default_node();
    tree.set_display(first, Display::Block)
        .expect("set first display");
    tree.set_box_sizing(first, BoxSizing::ContentBox)
        .expect("set first box sizing");
    tree.set_width(first, Length::points(30.0))
        .expect("set first width");
    tree.set_height(first, Length::points(10.0))
        .expect("set first height");
    tree.set_margin(first, StandaloneEdge::Left, Length::calc(4.0, 10.0))
        .expect("set first left margin");
    tree.set_margin(first, StandaloneEdge::Right, Length::calc(2.0, 5.0))
        .expect("set first right margin");

    let second = tree.create_default_node();
    tree.set_display(second, Display::Block)
        .expect("set second display");
    tree.set_box_sizing(second, BoxSizing::ContentBox)
        .expect("set second box sizing");
    tree.set_width(second, Length::points(20.0))
        .expect("set second width");
    tree.set_height(second, Length::points(12.0))
        .expect("set second height");
    tree.set_margin(second, StandaloneEdge::Left, Length::calc(3.0, 0.0))
        .expect("set second left margin");
    tree.set_margin(second, StandaloneEdge::Right, Length::calc(1.0, 15.0))
        .expect("set second right margin");

    tree.append_child(root, first).expect("append first");
    tree.append_child(root, second).expect("append second");

    run_standalone_rust(tree, root, Constraints::definite(180.0, 50.0))
        .expect("horizontal linear center with calc main margins parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_horizontal_reverse_linear_end_with_calc_main_margins() {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::HorizontalReverse)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::End)
        .expect("set root gravity");
    tree.set_width(root, Length::points(170.0))
        .expect("set root width");
    tree.set_height(root, Length::points(50.0))
        .expect("set root height");

    let first = tree.create_default_node();
    tree.set_display(first, Display::Block)
        .expect("set first display");
    tree.set_box_sizing(first, BoxSizing::ContentBox)
        .expect("set first box sizing");
    tree.set_width(first, Length::points(24.0))
        .expect("set first width");
    tree.set_height(first, Length::points(12.0))
        .expect("set first height");
    tree.set_margin(first, StandaloneEdge::Left, Length::calc(5.0, 4.0))
        .expect("set first left margin");
    tree.set_margin(first, StandaloneEdge::Right, Length::calc(7.0, 3.0))
        .expect("set first right margin");

    let second = tree.create_default_node();
    tree.set_display(second, Display::Block)
        .expect("set second display");
    tree.set_box_sizing(second, BoxSizing::ContentBox)
        .expect("set second box sizing");
    tree.set_width(second, Length::points(18.0))
        .expect("set second width");
    tree.set_height(second, Length::points(10.0))
        .expect("set second height");
    tree.set_margin(second, StandaloneEdge::Left, Length::calc(2.0, 8.0))
        .expect("set second left margin");
    tree.set_margin(second, StandaloneEdge::Right, Length::calc(6.0, 1.0))
        .expect("set second right margin");

    tree.append_child(root, first).expect("append first");
    tree.append_child(root, second).expect("append second");

    run_standalone_rust(tree, root, Constraints::definite(170.0, 50.0))
        .expect("horizontal-reverse linear end with calc main margins parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_vertical_linear_center_with_calc_main_margins() {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Vertical)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::Center)
        .expect("set root gravity");
    tree.set_width(root, Length::points(80.0))
        .expect("set root width");
    tree.set_height(root, Length::points(160.0))
        .expect("set root height");

    let first = tree.create_default_node();
    tree.set_display(first, Display::Block)
        .expect("set first display");
    tree.set_box_sizing(first, BoxSizing::ContentBox)
        .expect("set first box sizing");
    tree.set_width(first, Length::points(20.0))
        .expect("set first width");
    tree.set_height(first, Length::points(18.0))
        .expect("set first height");
    tree.set_margin(first, StandaloneEdge::Top, Length::calc(4.0, 10.0))
        .expect("set first top margin");
    tree.set_margin(first, StandaloneEdge::Bottom, Length::calc(2.0, 5.0))
        .expect("set first bottom margin");

    let second = tree.create_default_node();
    tree.set_display(second, Display::Block)
        .expect("set second display");
    tree.set_box_sizing(second, BoxSizing::ContentBox)
        .expect("set second box sizing");
    tree.set_width(second, Length::points(22.0))
        .expect("set second width");
    tree.set_height(second, Length::points(16.0))
        .expect("set second height");
    tree.set_margin(second, StandaloneEdge::Top, Length::calc(3.0, 8.0))
        .expect("set second top margin");
    tree.set_margin(second, StandaloneEdge::Bottom, Length::calc(1.0, 12.0))
        .expect("set second bottom margin");

    tree.append_child(root, first).expect("append first");
    tree.append_child(root, second).expect("append second");

    run_standalone_rust(tree, root, Constraints::definite(80.0, 160.0))
        .expect("vertical linear center with calc main margins parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_vertical_reverse_linear_space_between_with_calc_main_margins()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::VerticalReverse)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::SpaceBetween)
        .expect("set root gravity");
    tree.set_width(root, Length::points(80.0))
        .expect("set root width");
    tree.set_height(root, Length::points(180.0))
        .expect("set root height");

    let first = tree.create_default_node();
    tree.set_display(first, Display::Block)
        .expect("set first display");
    tree.set_box_sizing(first, BoxSizing::ContentBox)
        .expect("set first box sizing");
    tree.set_width(first, Length::points(20.0))
        .expect("set first width");
    tree.set_height(first, Length::points(18.0))
        .expect("set first height");
    tree.set_margin(first, StandaloneEdge::Top, Length::calc(2.0, 5.0))
        .expect("set first top margin");
    tree.set_margin(first, StandaloneEdge::Bottom, Length::calc(3.0, 4.0))
        .expect("set first bottom margin");

    let second = tree.create_default_node();
    tree.set_display(second, Display::Block)
        .expect("set second display");
    tree.set_box_sizing(second, BoxSizing::ContentBox)
        .expect("set second box sizing");
    tree.set_width(second, Length::points(24.0))
        .expect("set second width");
    tree.set_height(second, Length::points(16.0))
        .expect("set second height");
    tree.set_margin(second, StandaloneEdge::Top, Length::calc(1.0, 7.0))
        .expect("set second top margin");
    tree.set_margin(second, StandaloneEdge::Bottom, Length::calc(4.0, 3.0))
        .expect("set second bottom margin");

    tree.append_child(root, first).expect("append first");
    tree.append_child(root, second).expect("append second");

    run_standalone_rust(tree, root, Constraints::definite(80.0, 180.0))
        .expect("vertical-reverse linear space-between with calc main margins parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_horizontal_linear_space_between_three_items_with_calc_main_margins()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::SpaceBetween)
        .expect("set root gravity");
    tree.set_width(root, Length::points(220.0))
        .expect("set root width");
    tree.set_height(root, Length::points(60.0))
        .expect("set root height");

    let first = tree.create_default_node();
    tree.set_display(first, Display::Block)
        .expect("set first display");
    tree.set_box_sizing(first, BoxSizing::ContentBox)
        .expect("set first box sizing");
    tree.set_width(first, Length::points(24.0))
        .expect("set first width");
    tree.set_height(first, Length::points(12.0))
        .expect("set first height");
    tree.set_margin(first, StandaloneEdge::Left, Length::calc(2.0, 4.0))
        .expect("set first left margin");
    tree.set_margin(first, StandaloneEdge::Right, Length::calc(3.0, 2.0))
        .expect("set first right margin");

    let second = tree.create_default_node();
    tree.set_display(second, Display::Block)
        .expect("set second display");
    tree.set_box_sizing(second, BoxSizing::ContentBox)
        .expect("set second box sizing");
    tree.set_width(second, Length::points(18.0))
        .expect("set second width");
    tree.set_height(second, Length::points(14.0))
        .expect("set second height");
    tree.set_margin(second, StandaloneEdge::Left, Length::calc(5.0, 1.0))
        .expect("set second left margin");
    tree.set_margin(second, StandaloneEdge::Right, Length::calc(1.0, 3.0))
        .expect("set second right margin");

    let third = tree.create_default_node();
    tree.set_display(third, Display::Block)
        .expect("set third display");
    tree.set_box_sizing(third, BoxSizing::ContentBox)
        .expect("set third box sizing");
    tree.set_width(third, Length::points(22.0))
        .expect("set third width");
    tree.set_height(third, Length::points(10.0))
        .expect("set third height");
    tree.set_margin(third, StandaloneEdge::Left, Length::calc(4.0, 2.0))
        .expect("set third left margin");
    tree.set_margin(third, StandaloneEdge::Right, Length::calc(2.0, 4.0))
        .expect("set third right margin");

    tree.append_child(root, first).expect("append first");
    tree.append_child(root, second).expect("append second");
    tree.append_child(root, third).expect("append third");

    run_standalone_rust(tree, root, Constraints::definite(220.0, 60.0))
        .expect("horizontal linear space-between three items with calc main margins parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_vertical_linear_end_with_main_start_auto_and_calc_main_end_margin()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Vertical)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::End)
        .expect("set root gravity");
    tree.set_width(root, Length::points(70.0))
        .expect("set root width");
    tree.set_height(root, Length::points(140.0))
        .expect("set root height");

    let child = tree.create_default_node();
    tree.set_display(child, Display::Block)
        .expect("set child display");
    tree.set_box_sizing(child, BoxSizing::ContentBox)
        .expect("set child box sizing");
    tree.set_width(child, Length::points(24.0))
        .expect("set child width");
    tree.set_height(child, Length::points(18.0))
        .expect("set child height");
    tree.set_margin(child, StandaloneEdge::Top, Length::Auto)
        .expect("set child top auto margin");
    tree.set_margin(child, StandaloneEdge::Bottom, Length::calc(4.0, 10.0))
        .expect("set child bottom margin");
    tree.append_child(root, child).expect("append child");

    run_standalone_rust(tree, root, Constraints::definite(70.0, 140.0))
        .expect("vertical linear end with main-start auto and calc main-end margin parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_rtl_horizontal_linear_left_gravity_with_calc_main_margins()
{
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_direction(root, Direction::Rtl)
        .expect("set root direction");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::Left)
        .expect("set root gravity");
    tree.set_width(root, Length::points(170.0))
        .expect("set root width");
    tree.set_height(root, Length::points(50.0))
        .expect("set root height");

    let first = tree.create_default_node();
    tree.set_display(first, Display::Block)
        .expect("set first display");
    tree.set_box_sizing(first, BoxSizing::ContentBox)
        .expect("set first box sizing");
    tree.set_width(first, Length::points(24.0))
        .expect("set first width");
    tree.set_height(first, Length::points(12.0))
        .expect("set first height");
    tree.set_margin(first, StandaloneEdge::Left, Length::calc(3.0, 8.0))
        .expect("set first left margin");
    tree.set_margin(first, StandaloneEdge::Right, Length::calc(5.0, 4.0))
        .expect("set first right margin");

    let second = tree.create_default_node();
    tree.set_display(second, Display::Block)
        .expect("set second display");
    tree.set_box_sizing(second, BoxSizing::ContentBox)
        .expect("set second box sizing");
    tree.set_width(second, Length::points(18.0))
        .expect("set second width");
    tree.set_height(second, Length::points(10.0))
        .expect("set second height");
    tree.set_margin(second, StandaloneEdge::Left, Length::calc(2.0, 5.0))
        .expect("set second left margin");
    tree.set_margin(second, StandaloneEdge::Right, Length::calc(4.0, 7.0))
        .expect("set second right margin");

    tree.append_child(root, first).expect("append first");
    tree.append_child(root, second).expect("append second");

    run_standalone_rust(tree, root, Constraints::definite(170.0, 50.0))
        .expect("rtl horizontal linear left gravity with calc main margins parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_rtl_horizontal_linear_right_gravity_with_percent_main_margins()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_direction(root, Direction::Rtl)
        .expect("set root direction");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::Right)
        .expect("set root gravity");
    tree.set_width(root, Length::points(180.0))
        .expect("set root width");
    tree.set_height(root, Length::points(50.0))
        .expect("set root height");

    let first = tree.create_default_node();
    tree.set_display(first, Display::Block)
        .expect("set first display");
    tree.set_width(first, Length::points(26.0))
        .expect("set first width");
    tree.set_height(first, Length::points(12.0))
        .expect("set first height");
    tree.set_margin(first, StandaloneEdge::Left, Length::percent(6.0))
        .expect("set first left margin");
    tree.set_margin(first, StandaloneEdge::Right, Length::percent(4.0))
        .expect("set first right margin");

    let second = tree.create_default_node();
    tree.set_display(second, Display::Block)
        .expect("set second display");
    tree.set_width(second, Length::points(20.0))
        .expect("set second width");
    tree.set_height(second, Length::points(10.0))
        .expect("set second height");
    tree.set_margin(second, StandaloneEdge::Left, Length::percent(3.0))
        .expect("set second left margin");
    tree.set_margin(second, StandaloneEdge::Right, Length::percent(5.0))
        .expect("set second right margin");

    tree.append_child(root, first).expect("append first");
    tree.append_child(root, second).expect("append second");

    run_standalone_rust(tree, root, Constraints::definite(180.0, 50.0))
        .expect("rtl horizontal linear right gravity with percent main margins parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_rtl_horizontal_reverse_linear_left_gravity_with_calc_main_margins()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_direction(root, Direction::Rtl)
        .expect("set root direction");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::HorizontalReverse)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::Left)
        .expect("set root gravity");
    tree.set_width(root, Length::points(190.0))
        .expect("set root width");
    tree.set_height(root, Length::points(50.0))
        .expect("set root height");

    let first = tree.create_default_node();
    tree.set_display(first, Display::Block)
        .expect("set first display");
    tree.set_box_sizing(first, BoxSizing::ContentBox)
        .expect("set first box sizing");
    tree.set_width(first, Length::points(30.0))
        .expect("set first width");
    tree.set_height(first, Length::points(12.0))
        .expect("set first height");
    tree.set_margin(first, StandaloneEdge::Left, Length::calc(4.0, 6.0))
        .expect("set first left margin");
    tree.set_margin(first, StandaloneEdge::Right, Length::calc(2.0, 9.0))
        .expect("set first right margin");

    let second = tree.create_default_node();
    tree.set_display(second, Display::Block)
        .expect("set second display");
    tree.set_box_sizing(second, BoxSizing::ContentBox)
        .expect("set second box sizing");
    tree.set_width(second, Length::points(22.0))
        .expect("set second width");
    tree.set_height(second, Length::points(10.0))
        .expect("set second height");
    tree.set_margin(second, StandaloneEdge::Left, Length::calc(5.0, 3.0))
        .expect("set second left margin");
    tree.set_margin(second, StandaloneEdge::Right, Length::calc(1.0, 7.0))
        .expect("set second right margin");

    tree.append_child(root, first).expect("append first");
    tree.append_child(root, second).expect("append second");

    run_standalone_rust(tree, root, Constraints::definite(190.0, 50.0))
        .expect("rtl horizontal-reverse linear left gravity with calc main margins parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_rtl_horizontal_reverse_linear_right_gravity_with_auto_and_calc_main_margins()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_direction(root, Direction::Rtl)
        .expect("set root direction");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::HorizontalReverse)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::Right)
        .expect("set root gravity");
    tree.set_width(root, Length::points(170.0))
        .expect("set root width");
    tree.set_height(root, Length::points(50.0))
        .expect("set root height");

    let child = tree.create_default_node();
    tree.set_display(child, Display::Block)
        .expect("set child display");
    tree.set_box_sizing(child, BoxSizing::ContentBox)
        .expect("set child box sizing");
    tree.set_width(child, Length::points(32.0))
        .expect("set child width");
    tree.set_height(child, Length::points(12.0))
        .expect("set child height");
    tree.set_margin(child, StandaloneEdge::Left, Length::Auto)
        .expect("set child left auto margin");
    tree.set_margin(child, StandaloneEdge::Right, Length::calc(6.0, 5.0))
        .expect("set child right margin");
    tree.append_child(root, child).expect("append child");

    run_standalone_rust(tree, root, Constraints::definite(170.0, 50.0)).expect(
        "rtl horizontal-reverse linear right gravity with auto and calc main margins parity",
    );
}

#[test]
fn standalone_owned_tree_matches_cpp_for_vertical_reverse_linear_top_gravity_with_calc_main_margins()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::VerticalReverse)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::Top)
        .expect("set root gravity");
    tree.set_width(root, Length::points(80.0))
        .expect("set root width");
    tree.set_height(root, Length::points(180.0))
        .expect("set root height");

    let first = tree.create_default_node();
    tree.set_display(first, Display::Block)
        .expect("set first display");
    tree.set_box_sizing(first, BoxSizing::ContentBox)
        .expect("set first box sizing");
    tree.set_width(first, Length::points(24.0))
        .expect("set first width");
    tree.set_height(first, Length::points(20.0))
        .expect("set first height");
    tree.set_margin(first, StandaloneEdge::Top, Length::calc(3.0, 4.0))
        .expect("set first top margin");
    tree.set_margin(first, StandaloneEdge::Bottom, Length::calc(5.0, 2.0))
        .expect("set first bottom margin");

    let second = tree.create_default_node();
    tree.set_display(second, Display::Block)
        .expect("set second display");
    tree.set_box_sizing(second, BoxSizing::ContentBox)
        .expect("set second box sizing");
    tree.set_width(second, Length::points(20.0))
        .expect("set second width");
    tree.set_height(second, Length::points(16.0))
        .expect("set second height");
    tree.set_margin(second, StandaloneEdge::Top, Length::calc(2.0, 6.0))
        .expect("set second top margin");
    tree.set_margin(second, StandaloneEdge::Bottom, Length::calc(4.0, 3.0))
        .expect("set second bottom margin");

    tree.append_child(root, first).expect("append first");
    tree.append_child(root, second).expect("append second");

    run_standalone_rust(tree, root, Constraints::definite(80.0, 180.0))
        .expect("vertical-reverse linear top gravity with calc main margins parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_vertical_reverse_linear_bottom_gravity_with_percent_main_margins()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_linear_orientation(root, LinearOrientation::VerticalReverse)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::Bottom)
        .expect("set root gravity");
    tree.set_width(root, Length::points(80.0))
        .expect("set root width");
    tree.set_height(root, Length::points(190.0))
        .expect("set root height");

    let first = tree.create_default_node();
    tree.set_display(first, Display::Block)
        .expect("set first display");
    tree.set_width(first, Length::points(24.0))
        .expect("set first width");
    tree.set_height(first, Length::points(18.0))
        .expect("set first height");
    tree.set_margin(first, StandaloneEdge::Top, Length::percent(4.0))
        .expect("set first top margin");
    tree.set_margin(first, StandaloneEdge::Bottom, Length::percent(6.0))
        .expect("set first bottom margin");

    let second = tree.create_default_node();
    tree.set_display(second, Display::Block)
        .expect("set second display");
    tree.set_width(second, Length::points(20.0))
        .expect("set second width");
    tree.set_height(second, Length::points(14.0))
        .expect("set second height");
    tree.set_margin(second, StandaloneEdge::Top, Length::percent(3.0))
        .expect("set second top margin");
    tree.set_margin(second, StandaloneEdge::Bottom, Length::percent(5.0))
        .expect("set second bottom margin");

    tree.append_child(root, first).expect("append first");
    tree.append_child(root, second).expect("append second");

    run_standalone_rust(tree, root, Constraints::definite(80.0, 190.0))
        .expect("vertical-reverse linear bottom gravity with percent main margins parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_layout_gravity_mapping() {
    let mut cases = Vec::new();
    cases.push(linear_layout_gravity_end_overrides_stretch_tree());
    cases.push(linear_layout_gravity_stretch_overrides_explicit_cross_size_tree());
    cases.push(linear_layout_gravity_stretch_overrides_weighted_cross_size_tree());
    for gravity in linear_layout_gravity_variants() {
        cases.push(vertical_linear_layout_gravity_mapping_tree(gravity));
        cases.push(horizontal_linear_layout_gravity_mapping_tree(gravity));
    }
    for gravity in [LinearLayoutGravity::Left, LinearLayoutGravity::Right] {
        cases.push(rtl_vertical_linear_layout_gravity_tree(gravity));
    }

    for (case_name, tree, root, constraints) in cases {
        run_standalone_rust(tree, root, constraints)
            .unwrap_or_else(|error| panic!("{case_name} parity failed: {error}"));
    }
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_cross_gravity_mapping() {
    let mut cases = Vec::new();
    for cross_gravity in linear_cross_gravity_variants() {
        cases.push(vertical_linear_cross_gravity_mapping_tree(cross_gravity));
        cases.push(horizontal_linear_cross_gravity_mapping_tree(cross_gravity));
    }

    for (case_name, tree, root, constraints) in cases {
        run_standalone_rust(tree, root, constraints)
            .unwrap_or_else(|error| panic!("{case_name} parity failed: {error}"));
    }
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_align_items_stretch_fallback() {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_align_items(root, AlignItems::Stretch)
        .expect("set root align items");
    tree.set_width(root, Length::points(100.0))
        .expect("set root width");
    tree.set_height(root, Length::points(100.0))
        .expect("set root height");

    let child = tree.create_default_node();
    tree.set_display(child, Display::Block)
        .expect("set child display");
    tree.set_box_sizing(child, BoxSizing::ContentBox)
        .expect("set child box sizing");
    tree.set_width(child, Length::points(20.0))
        .expect("set child width");
    tree.set_height(child, Length::points(10.0))
        .expect("set child height");
    tree.append_child(root, child).expect("append child");

    run_standalone_rust(tree, root, Constraints::definite(100.0, 100.0))
        .expect("linear align-items stretch fallback parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_align_self_center_overrides_align_items_end() {
    let mut tree = StandaloneTree::new();
    let root = linear_cross_gravity_root(
        &mut tree,
        LinearOrientation::Vertical,
        LinearCrossGravity::None,
        AlignItems::FlexEnd,
    );

    let child = fixed_linear_gravity_child(&mut tree, 20.0, 10.0);
    tree.set_align_self(child, Some(AlignItems::Center))
        .expect("set child align self");
    tree.append_child(root, child).expect("append child");

    run_standalone_rust(tree, root, Constraints::definite(100.0, 100.0))
        .expect("linear align-self center overrides align-items end parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_align_self_stretch_overrides_align_items_start() {
    let mut tree = StandaloneTree::new();
    let root = linear_cross_gravity_root(
        &mut tree,
        LinearOrientation::Vertical,
        LinearCrossGravity::None,
        AlignItems::FlexStart,
    );

    let child = fixed_linear_gravity_child(&mut tree, 20.0, 10.0);
    tree.set_align_self(child, Some(AlignItems::Stretch))
        .expect("set child align self");
    tree.append_child(root, child).expect("append child");

    run_standalone_rust(tree, root, Constraints::definite(100.0, 100.0))
        .expect("linear align-self stretch overrides align-items start parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_layout_gravity_overrides_align_self_and_cross_gravity()
 {
    let mut tree = StandaloneTree::new();
    let root = linear_cross_gravity_root(
        &mut tree,
        LinearOrientation::Vertical,
        LinearCrossGravity::Center,
        AlignItems::FlexEnd,
    );

    let child = fixed_linear_gravity_child(&mut tree, 20.0, 10.0);
    tree.set_align_self(child, Some(AlignItems::FlexEnd))
        .expect("set child align self");
    tree.set_linear_layout_gravity(child, LinearLayoutGravity::Start)
        .expect("set child linear layout gravity");
    tree.append_child(root, child).expect("append child");

    run_standalone_rust(tree, root, Constraints::definite(100.0, 100.0))
        .expect("linear layout-gravity overrides align-self and cross-gravity parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_cross_gravity_precedes_align_items() {
    let mut tree = StandaloneTree::new();
    let root = linear_cross_gravity_root(
        &mut tree,
        LinearOrientation::Vertical,
        LinearCrossGravity::End,
        AlignItems::Center,
    );

    let child = fixed_linear_gravity_child(&mut tree, 20.0, 10.0);
    tree.append_child(root, child).expect("append child");

    run_standalone_rust(tree, root, Constraints::definite(100.0, 100.0))
        .expect("linear cross-gravity precedes align-items parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_align_items_flex_end_fallback_without_cross_gravity()
 {
    let mut tree = StandaloneTree::new();
    let root = linear_cross_gravity_root(
        &mut tree,
        LinearOrientation::Vertical,
        LinearCrossGravity::None,
        AlignItems::FlexEnd,
    );

    let child = fixed_linear_gravity_child(&mut tree, 20.0, 10.0);
    tree.append_child(root, child).expect("append child");

    run_standalone_rust(tree, root, Constraints::definite(100.0, 100.0))
        .expect("linear align-items flex-end fallback without cross-gravity parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_horizontal_align_items_center_fallback_without_cross_gravity()
 {
    let mut tree = StandaloneTree::new();
    let root = linear_cross_gravity_root(
        &mut tree,
        LinearOrientation::Horizontal,
        LinearCrossGravity::None,
        AlignItems::Center,
    );

    let child = fixed_linear_gravity_child(&mut tree, 20.0, 10.0);
    tree.append_child(root, child).expect("append child");

    run_standalone_rust(tree, root, Constraints::definite(100.0, 100.0))
        .expect("horizontal linear align-items center fallback without cross-gravity parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_vertical_linear_align_self_center_precedes_cross_gravity_end_with_calc_margins()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Vertical)
        .expect("set root orientation");
    tree.set_align_items(root, AlignItems::FlexStart)
        .expect("set root align items");
    tree.set_linear_cross_gravity(root, LinearCrossGravity::End)
        .expect("set root cross gravity");
    tree.set_width(root, Length::points(140.0))
        .expect("set root width");
    tree.set_height(root, Length::points(100.0))
        .expect("set root height");

    let child = tree.create_default_node();
    tree.set_display(child, Display::Block)
        .expect("set child display");
    tree.set_box_sizing(child, BoxSizing::ContentBox)
        .expect("set child box sizing");
    tree.set_width(child, Length::points(24.0))
        .expect("set child width");
    tree.set_height(child, Length::points(14.0))
        .expect("set child height");
    tree.set_align_self(child, Some(AlignItems::Center))
        .expect("set child align self");
    tree.set_margin(child, StandaloneEdge::Left, Length::calc(4.0, 6.0))
        .expect("set child left margin");
    tree.set_margin(child, StandaloneEdge::Right, Length::calc(3.0, 8.0))
        .expect("set child right margin");
    tree.append_child(root, child).expect("append child");

    run_standalone_rust(tree, root, Constraints::definite(140.0, 100.0))
        .expect("vertical linear align-self center before cross-gravity parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_horizontal_linear_align_self_stretch_precedes_cross_gravity_end()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root orientation");
    tree.set_align_items(root, AlignItems::FlexStart)
        .expect("set root align items");
    tree.set_linear_cross_gravity(root, LinearCrossGravity::End)
        .expect("set root cross gravity");
    tree.set_width(root, Length::points(120.0))
        .expect("set root width");
    tree.set_height(root, Length::points(90.0))
        .expect("set root height");

    let child = tree.create_default_node();
    tree.set_display(child, Display::Block)
        .expect("set child display");
    tree.set_box_sizing(child, BoxSizing::ContentBox)
        .expect("set child box sizing");
    tree.set_width(child, Length::points(30.0))
        .expect("set child width");
    tree.set_height(child, Length::points(16.0))
        .expect("set child height");
    tree.set_align_self(child, Some(AlignItems::Stretch))
        .expect("set child align self");
    tree.set_margin(child, StandaloneEdge::Top, Length::points(5.0))
        .expect("set child top margin");
    tree.set_margin(child, StandaloneEdge::Bottom, Length::points(7.0))
        .expect("set child bottom margin");
    tree.append_child(root, child).expect("append child");

    run_standalone_rust(tree, root, Constraints::definite(120.0, 90.0))
        .expect("horizontal linear align-self stretch before cross-gravity parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_horizontal_linear_layout_gravity_top_precedes_align_self_end_and_cross_gravity_center()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root orientation");
    tree.set_align_items(root, AlignItems::FlexEnd)
        .expect("set root align items");
    tree.set_linear_cross_gravity(root, LinearCrossGravity::Center)
        .expect("set root cross gravity");
    tree.set_width(root, Length::points(120.0))
        .expect("set root width");
    tree.set_height(root, Length::points(90.0))
        .expect("set root height");

    let child = tree.create_default_node();
    tree.set_display(child, Display::Block)
        .expect("set child display");
    tree.set_box_sizing(child, BoxSizing::ContentBox)
        .expect("set child box sizing");
    tree.set_width(child, Length::points(28.0))
        .expect("set child width");
    tree.set_height(child, Length::points(18.0))
        .expect("set child height");
    tree.set_align_self(child, Some(AlignItems::FlexEnd))
        .expect("set child align self");
    tree.set_linear_layout_gravity(child, LinearLayoutGravity::Top)
        .expect("set child layout gravity");
    tree.set_margin(child, StandaloneEdge::Top, Length::calc(3.0, 5.0))
        .expect("set child top margin");
    tree.set_margin(child, StandaloneEdge::Bottom, Length::calc(2.0, 7.0))
        .expect("set child bottom margin");
    tree.append_child(root, child).expect("append child");

    run_standalone_rust(tree, root, Constraints::definite(120.0, 90.0))
        .expect("horizontal linear layout-gravity top precedence parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_horizontal_linear_cross_auto_margin_precedes_align_self_center()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root orientation");
    tree.set_align_items(root, AlignItems::FlexStart)
        .expect("set root align items");
    tree.set_width(root, Length::points(100.0))
        .expect("set root width");
    tree.set_height(root, Length::points(90.0))
        .expect("set root height");

    let child = tree.create_default_node();
    tree.set_display(child, Display::Block)
        .expect("set child display");
    tree.set_box_sizing(child, BoxSizing::ContentBox)
        .expect("set child box sizing");
    tree.set_width(child, Length::points(24.0))
        .expect("set child width");
    tree.set_height(child, Length::points(14.0))
        .expect("set child height");
    tree.set_align_self(child, Some(AlignItems::Center))
        .expect("set child align self");
    tree.set_margin(child, StandaloneEdge::Top, Length::Auto)
        .expect("set child top auto margin");
    tree.set_margin(child, StandaloneEdge::Bottom, Length::calc(4.0, 5.0))
        .expect("set child bottom margin");
    tree.append_child(root, child).expect("append child");

    run_standalone_rust(tree, root, Constraints::definite(100.0, 90.0))
        .expect("horizontal linear auto cross margin before align-self parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_horizontal_linear_cross_auto_margin_precedes_layout_gravity_bottom()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root orientation");
    tree.set_align_items(root, AlignItems::FlexStart)
        .expect("set root align items");
    tree.set_width(root, Length::points(100.0))
        .expect("set root width");
    tree.set_height(root, Length::points(90.0))
        .expect("set root height");

    let child = tree.create_default_node();
    tree.set_display(child, Display::Block)
        .expect("set child display");
    tree.set_box_sizing(child, BoxSizing::ContentBox)
        .expect("set child box sizing");
    tree.set_width(child, Length::points(26.0))
        .expect("set child width");
    tree.set_height(child, Length::points(16.0))
        .expect("set child height");
    tree.set_linear_layout_gravity(child, LinearLayoutGravity::Bottom)
        .expect("set child layout gravity");
    tree.set_margin(child, StandaloneEdge::Top, Length::calc(3.0, 4.0))
        .expect("set child top margin");
    tree.set_margin(child, StandaloneEdge::Bottom, Length::Auto)
        .expect("set child bottom auto margin");
    tree.append_child(root, child).expect("append child");

    run_standalone_rust(tree, root, Constraints::definite(100.0, 90.0))
        .expect("horizontal linear auto cross margin before layout-gravity parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_vertical_linear_paired_cross_auto_margins_precede_cross_gravity_end()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Vertical)
        .expect("set root orientation");
    tree.set_align_items(root, AlignItems::FlexStart)
        .expect("set root align items");
    tree.set_linear_cross_gravity(root, LinearCrossGravity::End)
        .expect("set root cross gravity");
    tree.set_width(root, Length::points(140.0))
        .expect("set root width");
    tree.set_height(root, Length::points(100.0))
        .expect("set root height");

    let child = tree.create_default_node();
    tree.set_display(child, Display::Block)
        .expect("set child display");
    tree.set_box_sizing(child, BoxSizing::ContentBox)
        .expect("set child box sizing");
    tree.set_width(child, Length::points(28.0))
        .expect("set child width");
    tree.set_height(child, Length::points(18.0))
        .expect("set child height");
    tree.set_margin(child, StandaloneEdge::Left, Length::Auto)
        .expect("set child left auto margin");
    tree.set_margin(child, StandaloneEdge::Right, Length::Auto)
        .expect("set child right auto margin");
    tree.append_child(root, child).expect("append child");

    run_standalone_rust(tree, root, Constraints::definite(140.0, 100.0))
        .expect("vertical linear paired auto cross margins before cross-gravity parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_horizontal_reverse_linear_cross_axis_center_with_percent_margins()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::HorizontalReverse)
        .expect("set root orientation");
    tree.set_align_items(root, AlignItems::Center)
        .expect("set root align items");
    tree.set_width(root, Length::points(150.0))
        .expect("set root width");
    tree.set_height(root, Length::points(90.0))
        .expect("set root height");

    let child = tree.create_default_node();
    tree.set_display(child, Display::Block)
        .expect("set child display");
    tree.set_box_sizing(child, BoxSizing::ContentBox)
        .expect("set child box sizing");
    tree.set_width(child, Length::points(26.0))
        .expect("set child width");
    tree.set_height(child, Length::points(20.0))
        .expect("set child height");
    tree.set_margin(child, StandaloneEdge::Top, Length::percent(6.0))
        .expect("set child top margin");
    tree.set_margin(child, StandaloneEdge::Bottom, Length::percent(4.0))
        .expect("set child bottom margin");
    tree.append_child(root, child).expect("append child");

    run_standalone_rust(tree, root, Constraints::definite(150.0, 90.0))
        .expect("horizontal-reverse linear center cross-axis percent margin parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_horizontal_reverse_linear_cross_axis_end_with_calc_margins()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::HorizontalReverse)
        .expect("set root orientation");
    tree.set_align_items(root, AlignItems::FlexEnd)
        .expect("set root align items");
    tree.set_width(root, Length::points(146.0))
        .expect("set root width");
    tree.set_height(root, Length::points(88.0))
        .expect("set root height");

    let child = tree.create_default_node();
    tree.set_display(child, Display::Block)
        .expect("set child display");
    tree.set_box_sizing(child, BoxSizing::ContentBox)
        .expect("set child box sizing");
    tree.set_width(child, Length::points(24.0))
        .expect("set child width");
    tree.set_height(child, Length::points(18.0))
        .expect("set child height");
    tree.set_margin(child, StandaloneEdge::Top, Length::calc(3.0, 5.0))
        .expect("set child top margin");
    tree.set_margin(child, StandaloneEdge::Bottom, Length::calc(2.0, 3.0))
        .expect("set child bottom margin");
    tree.append_child(root, child).expect("append child");

    run_standalone_rust(tree, root, Constraints::definite(146.0, 88.0))
        .expect("horizontal-reverse linear end cross-axis calc margin parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_horizontal_reverse_linear_cross_axis_auto_margin_precedes_align_self_end()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::HorizontalReverse)
        .expect("set root orientation");
    tree.set_align_items(root, AlignItems::FlexStart)
        .expect("set root align items");
    tree.set_width(root, Length::points(152.0))
        .expect("set root width");
    tree.set_height(root, Length::points(92.0))
        .expect("set root height");

    let child = tree.create_default_node();
    tree.set_display(child, Display::Block)
        .expect("set child display");
    tree.set_box_sizing(child, BoxSizing::ContentBox)
        .expect("set child box sizing");
    tree.set_width(child, Length::points(28.0))
        .expect("set child width");
    tree.set_height(child, Length::points(22.0))
        .expect("set child height");
    tree.set_align_self(child, Some(AlignItems::FlexEnd))
        .expect("set child align-self");
    tree.set_margin(child, StandaloneEdge::Top, Length::Auto)
        .expect("set child top auto margin");
    tree.set_margin(child, StandaloneEdge::Bottom, Length::percent(5.0))
        .expect("set child bottom margin");
    tree.append_child(root, child).expect("append child");

    run_standalone_rust(tree, root, Constraints::definite(152.0, 92.0))
        .expect("horizontal-reverse linear auto cross margin before align-self parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_rtl_vertical_reverse_linear_cross_axis_center_with_percent_margins()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_direction(root, Direction::Rtl)
        .expect("set root direction");
    tree.set_linear_orientation(root, LinearOrientation::VerticalReverse)
        .expect("set root orientation");
    tree.set_align_items(root, AlignItems::Center)
        .expect("set root align items");
    tree.set_width(root, Length::points(120.0))
        .expect("set root width");
    tree.set_height(root, Length::points(150.0))
        .expect("set root height");

    let child = tree.create_default_node();
    tree.set_display(child, Display::Block)
        .expect("set child display");
    tree.set_box_sizing(child, BoxSizing::ContentBox)
        .expect("set child box sizing");
    tree.set_width(child, Length::points(30.0))
        .expect("set child width");
    tree.set_height(child, Length::points(20.0))
        .expect("set child height");
    tree.set_margin(child, StandaloneEdge::Left, Length::percent(4.0))
        .expect("set child left margin");
    tree.set_margin(child, StandaloneEdge::Right, Length::percent(6.0))
        .expect("set child right margin");
    tree.append_child(root, child).expect("append child");

    run_standalone_rust(tree, root, Constraints::definite(120.0, 150.0))
        .expect("rtl vertical-reverse linear center cross-axis percent margin parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_rtl_vertical_reverse_linear_layout_gravity_left_with_calc_cross_margins()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_direction(root, Direction::Rtl)
        .expect("set root direction");
    tree.set_linear_orientation(root, LinearOrientation::VerticalReverse)
        .expect("set root orientation");
    tree.set_align_items(root, AlignItems::FlexStart)
        .expect("set root align items");
    tree.set_width(root, Length::points(124.0))
        .expect("set root width");
    tree.set_height(root, Length::points(148.0))
        .expect("set root height");

    let child = tree.create_default_node();
    tree.set_display(child, Display::Block)
        .expect("set child display");
    tree.set_box_sizing(child, BoxSizing::ContentBox)
        .expect("set child box sizing");
    tree.set_linear_layout_gravity(child, LinearLayoutGravity::Left)
        .expect("set child layout gravity");
    tree.set_width(child, Length::points(32.0))
        .expect("set child width");
    tree.set_height(child, Length::points(18.0))
        .expect("set child height");
    tree.set_margin(child, StandaloneEdge::Left, Length::calc(2.0, 5.0))
        .expect("set child left margin");
    tree.set_margin(child, StandaloneEdge::Right, Length::calc(3.0, 4.0))
        .expect("set child right margin");
    tree.append_child(root, child).expect("append child");

    run_standalone_rust(tree, root, Constraints::definite(124.0, 148.0))
        .expect("rtl vertical-reverse linear left layout-gravity cross-axis parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_rtl_vertical_reverse_linear_cross_axis_right_auto_margin_precedes_layout_gravity_left()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_direction(root, Direction::Rtl)
        .expect("set root direction");
    tree.set_linear_orientation(root, LinearOrientation::VerticalReverse)
        .expect("set root orientation");
    tree.set_align_items(root, AlignItems::FlexStart)
        .expect("set root align items");
    tree.set_width(root, Length::points(128.0))
        .expect("set root width");
    tree.set_height(root, Length::points(152.0))
        .expect("set root height");

    let child = tree.create_default_node();
    tree.set_display(child, Display::Block)
        .expect("set child display");
    tree.set_box_sizing(child, BoxSizing::ContentBox)
        .expect("set child box sizing");
    tree.set_linear_layout_gravity(child, LinearLayoutGravity::Left)
        .expect("set child layout gravity");
    tree.set_width(child, Length::points(34.0))
        .expect("set child width");
    tree.set_height(child, Length::points(20.0))
        .expect("set child height");
    tree.set_margin(child, StandaloneEdge::Left, Length::percent(3.0))
        .expect("set child left margin");
    tree.set_margin(child, StandaloneEdge::Right, Length::Auto)
        .expect("set child right auto margin");
    tree.append_child(root, child).expect("append child");

    run_standalone_rust(tree, root, Constraints::definite(128.0, 152.0))
        .expect("rtl vertical-reverse linear auto cross margin before layout-gravity parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_justify_content_start_fallbacks() {
    for justify_content in [
        JustifyContent::SpaceAround,
        JustifyContent::SpaceEvenly,
        JustifyContent::Stretch,
    ] {
        let mut tree = StandaloneTree::new();
        let root = tree.create_default_node();
        tree.set_display(root, Display::Linear)
            .expect("set root display");
        tree.set_box_sizing(root, BoxSizing::ContentBox)
            .expect("set root box sizing");
        tree.set_linear_orientation(root, LinearOrientation::Horizontal)
            .expect("set root linear orientation");
        tree.set_justify_content(root, justify_content)
            .expect("set root justify content");
        tree.set_width(root, Length::points(100.0))
            .expect("set root width");
        tree.set_height(root, Length::points(10.0))
            .expect("set root height");

        for width in [10.0, 20.0] {
            let child = tree.create_default_node();
            tree.set_display(child, Display::Block)
                .expect("set child display");
            tree.set_box_sizing(child, BoxSizing::ContentBox)
                .expect("set child box sizing");
            tree.set_width(child, Length::points(width))
                .expect("set child width");
            tree.set_height(child, Length::points(10.0))
                .expect("set child height");
            tree.append_child(root, child).expect("append child");
        }

        run_standalone_rust(tree, root, Constraints::definite(100.0, 10.0))
            .unwrap_or_else(|error| panic!("{justify_content:?} fallback parity failed: {error}"));
    }
}

#[test]
fn standalone_owned_tree_matches_cpp_for_horizontal_linear_justify_space_around_start_fallback_order_skips_display_none()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::None)
        .expect("set root linear gravity");
    tree.set_justify_content(root, JustifyContent::SpaceAround)
        .expect("set root justify content");
    tree.set_align_items(root, AlignItems::FlexStart)
        .expect("set root align items");
    tree.set_width(root, Length::points(178.0))
        .expect("set root width");
    tree.set_height(root, Length::points(58.0))
        .expect("set root height");

    let late = tree.create_default_node();
    tree.set_display(late, Display::Block)
        .expect("set late display");
    tree.set_width(late, Length::points(26.0))
        .expect("set late width");
    tree.set_height(late, Length::points(12.0))
        .expect("set late height");
    tree.set_margin(late, StandaloneEdge::Left, Length::percent(3.0))
        .expect("set late left margin");
    tree.set_margin(late, StandaloneEdge::Right, Length::percent(4.0))
        .expect("set late right margin");
    tree.set_order(late, 3).expect("set late order");

    let hidden = tree.create_default_node();
    tree.set_display(hidden, Display::None)
        .expect("set hidden display");
    tree.set_width(hidden, Length::points(110.0))
        .expect("set hidden width");
    tree.set_height(hidden, Length::points(40.0))
        .expect("set hidden height");
    tree.set_order(hidden, -4).expect("set hidden order");

    let early = tree.create_default_node();
    tree.set_display(early, Display::Block)
        .expect("set early display");
    tree.set_width(early, Length::points(24.0))
        .expect("set early width");
    tree.set_height(early, Length::points(14.0))
        .expect("set early height");
    tree.set_margin(early, StandaloneEdge::Left, Length::percent(2.0))
        .expect("set early left margin");
    tree.set_margin(early, StandaloneEdge::Right, Length::percent(5.0))
        .expect("set early right margin");
    tree.set_order(early, -1).expect("set early order");

    tree.append_child(root, late).expect("append late");
    tree.append_child(root, hidden).expect("append hidden");
    tree.append_child(root, early).expect("append early");

    run_standalone_rust(tree, root, Constraints::definite(178.0, 58.0)).expect(
        "horizontal linear justify-content space-around start fallback order/display-none parity",
    );
}

#[test]
fn standalone_owned_tree_matches_cpp_for_horizontal_reverse_linear_justify_space_evenly_start_fallback_order_skips_display_none()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::HorizontalReverse)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::None)
        .expect("set root linear gravity");
    tree.set_justify_content(root, JustifyContent::SpaceEvenly)
        .expect("set root justify content");
    tree.set_align_items(root, AlignItems::FlexStart)
        .expect("set root align items");
    tree.set_width(root, Length::points(184.0))
        .expect("set root width");
    tree.set_height(root, Length::points(60.0))
        .expect("set root height");

    let late = tree.create_default_node();
    tree.set_display(late, Display::Block)
        .expect("set late display");
    tree.set_width(late, Length::points(28.0))
        .expect("set late width");
    tree.set_height(late, Length::points(13.0))
        .expect("set late height");
    tree.set_margin(late, StandaloneEdge::Left, Length::calc(3.0, 4.0))
        .expect("set late left margin");
    tree.set_margin(late, StandaloneEdge::Right, Length::calc(2.0, 6.0))
        .expect("set late right margin");
    tree.set_order(late, 4).expect("set late order");

    let hidden = tree.create_default_node();
    tree.set_display(hidden, Display::None)
        .expect("set hidden display");
    tree.set_width(hidden, Length::points(96.0))
        .expect("set hidden width");
    tree.set_height(hidden, Length::points(36.0))
        .expect("set hidden height");
    tree.set_order(hidden, -5).expect("set hidden order");

    let early = tree.create_default_node();
    tree.set_display(early, Display::Block)
        .expect("set early display");
    tree.set_width(early, Length::points(22.0))
        .expect("set early width");
    tree.set_height(early, Length::points(15.0))
        .expect("set early height");
    tree.set_margin(early, StandaloneEdge::Left, Length::calc(1.0, 5.0))
        .expect("set early left margin");
    tree.set_margin(early, StandaloneEdge::Right, Length::calc(4.0, 3.0))
        .expect("set early right margin");
    tree.set_order(early, -2).expect("set early order");

    tree.append_child(root, hidden).expect("append hidden");
    tree.append_child(root, late).expect("append late");
    tree.append_child(root, early).expect("append early");

    run_standalone_rust(tree, root, Constraints::definite(184.0, 60.0)).expect(
        "horizontal-reverse linear justify-content space-evenly start fallback order/display-none parity",
    );
}

#[test]
fn standalone_owned_tree_matches_cpp_for_rtl_horizontal_linear_justify_stretch_start_fallback_order_skips_display_none()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_direction(root, Direction::Rtl)
        .expect("set root direction");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::None)
        .expect("set root linear gravity");
    tree.set_justify_content(root, JustifyContent::Stretch)
        .expect("set root justify content");
    tree.set_align_items(root, AlignItems::FlexStart)
        .expect("set root align items");
    tree.set_width(root, Length::points(172.0))
        .expect("set root width");
    tree.set_height(root, Length::points(56.0))
        .expect("set root height");

    let late = tree.create_default_node();
    tree.set_display(late, Display::Block)
        .expect("set late display");
    tree.set_width(late, Length::points(25.0))
        .expect("set late width");
    tree.set_height(late, Length::points(12.0))
        .expect("set late height");
    tree.set_margin(late, StandaloneEdge::Left, Length::percent(4.0))
        .expect("set late left margin");
    tree.set_margin(late, StandaloneEdge::Right, Length::percent(2.0))
        .expect("set late right margin");
    tree.set_order(late, 3).expect("set late order");

    let hidden = tree.create_default_node();
    tree.set_display(hidden, Display::None)
        .expect("set hidden display");
    tree.set_width(hidden, Length::points(100.0))
        .expect("set hidden width");
    tree.set_height(hidden, Length::points(38.0))
        .expect("set hidden height");
    tree.set_order(hidden, -4).expect("set hidden order");

    let early = tree.create_default_node();
    tree.set_display(early, Display::Block)
        .expect("set early display");
    tree.set_width(early, Length::points(23.0))
        .expect("set early width");
    tree.set_height(early, Length::points(14.0))
        .expect("set early height");
    tree.set_margin(early, StandaloneEdge::Left, Length::percent(3.0))
        .expect("set early left margin");
    tree.set_margin(early, StandaloneEdge::Right, Length::percent(5.0))
        .expect("set early right margin");
    tree.set_order(early, -1).expect("set early order");

    tree.append_child(root, late).expect("append late");
    tree.append_child(root, early).expect("append early");
    tree.append_child(root, hidden).expect("append hidden");

    run_standalone_rust(tree, root, Constraints::definite(172.0, 56.0)).expect(
        "RTL horizontal linear justify-content stretch start fallback order/display-none parity",
    );
}

#[test]
fn standalone_owned_tree_matches_cpp_for_rtl_horizontal_reverse_linear_justify_space_around_start_fallback_order_skips_display_none()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_direction(root, Direction::Rtl)
        .expect("set root direction");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::HorizontalReverse)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::None)
        .expect("set root linear gravity");
    tree.set_justify_content(root, JustifyContent::SpaceAround)
        .expect("set root justify content");
    tree.set_align_items(root, AlignItems::FlexStart)
        .expect("set root align items");
    tree.set_width(root, Length::points(188.0))
        .expect("set root width");
    tree.set_height(root, Length::points(62.0))
        .expect("set root height");

    let late = tree.create_default_node();
    tree.set_display(late, Display::Block)
        .expect("set late display");
    tree.set_width(late, Length::points(30.0))
        .expect("set late width");
    tree.set_height(late, Length::points(13.0))
        .expect("set late height");
    tree.set_margin(late, StandaloneEdge::Left, Length::calc(4.0, 3.0))
        .expect("set late left margin");
    tree.set_margin(late, StandaloneEdge::Right, Length::calc(2.0, 5.0))
        .expect("set late right margin");
    tree.set_order(late, 2).expect("set late order");

    let hidden = tree.create_default_node();
    tree.set_display(hidden, Display::None)
        .expect("set hidden display");
    tree.set_width(hidden, Length::points(104.0))
        .expect("set hidden width");
    tree.set_height(hidden, Length::points(42.0))
        .expect("set hidden height");
    tree.set_order(hidden, -6).expect("set hidden order");

    let early = tree.create_default_node();
    tree.set_display(early, Display::Block)
        .expect("set early display");
    tree.set_width(early, Length::points(21.0))
        .expect("set early width");
    tree.set_height(early, Length::points(15.0))
        .expect("set early height");
    tree.set_margin(early, StandaloneEdge::Left, Length::calc(1.0, 6.0))
        .expect("set early left margin");
    tree.set_margin(early, StandaloneEdge::Right, Length::calc(3.0, 4.0))
        .expect("set early right margin");
    tree.set_order(early, -2).expect("set early order");

    tree.append_child(root, hidden).expect("append hidden");
    tree.append_child(root, early).expect("append early");
    tree.append_child(root, late).expect("append late");

    run_standalone_rust(tree, root, Constraints::definite(188.0, 62.0)).expect(
        "RTL horizontal-reverse linear justify-content space-around start fallback order/display-none parity",
    );
}

#[test]
fn standalone_owned_tree_matches_cpp_for_vertical_linear_justify_space_evenly_start_fallback_order_skips_display_none()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Vertical)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::None)
        .expect("set root linear gravity");
    tree.set_justify_content(root, JustifyContent::SpaceEvenly)
        .expect("set root justify content");
    tree.set_align_items(root, AlignItems::FlexStart)
        .expect("set root align items");
    tree.set_width(root, Length::points(72.0))
        .expect("set root width");
    tree.set_height(root, Length::points(194.0))
        .expect("set root height");

    let late = tree.create_default_node();
    tree.set_display(late, Display::Block)
        .expect("set late display");
    tree.set_width(late, Length::points(24.0))
        .expect("set late width");
    tree.set_height(late, Length::points(28.0))
        .expect("set late height");
    tree.set_margin(late, StandaloneEdge::Top, Length::percent(4.0))
        .expect("set late top margin");
    tree.set_margin(late, StandaloneEdge::Bottom, Length::percent(3.0))
        .expect("set late bottom margin");
    tree.set_order(late, 4).expect("set late order");

    let hidden = tree.create_default_node();
    tree.set_display(hidden, Display::None)
        .expect("set hidden display");
    tree.set_width(hidden, Length::points(60.0))
        .expect("set hidden width");
    tree.set_height(hidden, Length::points(120.0))
        .expect("set hidden height");
    tree.set_order(hidden, -5).expect("set hidden order");

    let early = tree.create_default_node();
    tree.set_display(early, Display::Block)
        .expect("set early display");
    tree.set_width(early, Length::points(20.0))
        .expect("set early width");
    tree.set_height(early, Length::points(22.0))
        .expect("set early height");
    tree.set_margin(early, StandaloneEdge::Top, Length::percent(2.0))
        .expect("set early top margin");
    tree.set_margin(early, StandaloneEdge::Bottom, Length::percent(5.0))
        .expect("set early bottom margin");
    tree.set_order(early, -1).expect("set early order");

    tree.append_child(root, late).expect("append late");
    tree.append_child(root, hidden).expect("append hidden");
    tree.append_child(root, early).expect("append early");

    run_standalone_rust(tree, root, Constraints::definite(72.0, 194.0)).expect(
        "vertical linear justify-content space-evenly start fallback order/display-none parity",
    );
}

#[test]
fn standalone_owned_tree_matches_cpp_for_vertical_reverse_linear_justify_stretch_start_fallback_order_skips_display_none()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::VerticalReverse)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::None)
        .expect("set root linear gravity");
    tree.set_justify_content(root, JustifyContent::Stretch)
        .expect("set root justify content");
    tree.set_align_items(root, AlignItems::FlexStart)
        .expect("set root align items");
    tree.set_width(root, Length::points(76.0))
        .expect("set root width");
    tree.set_height(root, Length::points(202.0))
        .expect("set root height");

    let late = tree.create_default_node();
    tree.set_display(late, Display::Block)
        .expect("set late display");
    tree.set_width(late, Length::points(26.0))
        .expect("set late width");
    tree.set_height(late, Length::points(30.0))
        .expect("set late height");
    tree.set_margin(late, StandaloneEdge::Top, Length::calc(4.0, 3.0))
        .expect("set late top margin");
    tree.set_margin(late, StandaloneEdge::Bottom, Length::calc(2.0, 5.0))
        .expect("set late bottom margin");
    tree.set_order(late, 3).expect("set late order");

    let hidden = tree.create_default_node();
    tree.set_display(hidden, Display::None)
        .expect("set hidden display");
    tree.set_width(hidden, Length::points(64.0))
        .expect("set hidden width");
    tree.set_height(hidden, Length::points(128.0))
        .expect("set hidden height");
    tree.set_order(hidden, -4).expect("set hidden order");

    let early = tree.create_default_node();
    tree.set_display(early, Display::Block)
        .expect("set early display");
    tree.set_width(early, Length::points(22.0))
        .expect("set early width");
    tree.set_height(early, Length::points(24.0))
        .expect("set early height");
    tree.set_margin(early, StandaloneEdge::Top, Length::calc(1.0, 6.0))
        .expect("set early top margin");
    tree.set_margin(early, StandaloneEdge::Bottom, Length::calc(3.0, 4.0))
        .expect("set early bottom margin");
    tree.set_order(early, -2).expect("set early order");

    tree.append_child(root, hidden).expect("append hidden");
    tree.append_child(root, late).expect("append late");
    tree.append_child(root, early).expect("append early");

    run_standalone_rust(tree, root, Constraints::definite(76.0, 202.0)).expect(
        "vertical-reverse linear justify-content stretch start fallback order/display-none parity",
    );
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_justify_content_center_and_flex_end() {
    {
        let mut tree = StandaloneTree::new();
        let root = tree.create_default_node();
        tree.set_display(root, Display::Linear)
            .expect("set root display");
        tree.set_box_sizing(root, BoxSizing::ContentBox)
            .expect("set root box sizing");
        tree.set_linear_orientation(root, LinearOrientation::Horizontal)
            .expect("set root linear orientation");
        tree.set_justify_content(root, JustifyContent::Center)
            .expect("set root justify content");
        tree.set_width(root, Length::points(100.0))
            .expect("set root width");
        tree.set_height(root, Length::points(10.0))
            .expect("set root height");

        let first = tree.create_default_node();
        tree.set_display(first, Display::Block)
            .expect("set first display");
        tree.set_box_sizing(first, BoxSizing::ContentBox)
            .expect("set first box sizing");
        tree.set_width(first, Length::points(10.0))
            .expect("set first width");
        tree.set_height(first, Length::points(10.0))
            .expect("set first height");

        let second = tree.create_default_node();
        tree.set_display(second, Display::Block)
            .expect("set second display");
        tree.set_box_sizing(second, BoxSizing::ContentBox)
            .expect("set second box sizing");
        tree.set_width(second, Length::points(20.0))
            .expect("set second width");
        tree.set_height(second, Length::points(10.0))
            .expect("set second height");

        tree.append_child(root, first).expect("append first");
        tree.append_child(root, second).expect("append second");

        run_standalone_rust(tree, root, Constraints::definite(100.0, 10.0))
            .expect("linear justify-content center parity");
    }

    {
        let mut tree = StandaloneTree::new();
        let root = tree.create_default_node();
        tree.set_display(root, Display::Linear)
            .expect("set root display");
        tree.set_box_sizing(root, BoxSizing::ContentBox)
            .expect("set root box sizing");
        tree.set_linear_orientation(root, LinearOrientation::Horizontal)
            .expect("set root linear orientation");
        tree.set_justify_content(root, JustifyContent::FlexEnd)
            .expect("set root justify content");
        tree.set_width(root, Length::points(100.0))
            .expect("set root width");
        tree.set_height(root, Length::points(10.0))
            .expect("set root height");

        let first = tree.create_default_node();
        tree.set_display(first, Display::Block)
            .expect("set first display");
        tree.set_box_sizing(first, BoxSizing::ContentBox)
            .expect("set first box sizing");
        tree.set_width(first, Length::points(10.0))
            .expect("set first width");
        tree.set_height(first, Length::points(10.0))
            .expect("set first height");

        let second = tree.create_default_node();
        tree.set_display(second, Display::Block)
            .expect("set second display");
        tree.set_box_sizing(second, BoxSizing::ContentBox)
            .expect("set second box sizing");
        tree.set_width(second, Length::points(20.0))
            .expect("set second width");
        tree.set_height(second, Length::points(10.0))
            .expect("set second height");

        tree.append_child(root, first).expect("append first");
        tree.append_child(root, second).expect("append second");

        run_standalone_rust(tree, root, Constraints::definite(100.0, 10.0))
            .expect("linear justify-content flex-end parity");
    }
}

#[test]
fn standalone_owned_tree_matches_cpp_for_vertical_linear_justify_content_start_with_percent_main_margins()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Vertical)
        .expect("set root orientation");
    tree.set_justify_content(root, JustifyContent::Start)
        .expect("set root justify content");
    tree.set_width(root, Length::points(50.0))
        .expect("set root width");
    tree.set_height(root, Length::points(140.0))
        .expect("set root height");

    let first = tree.create_default_node();
    tree.set_display(first, Display::Block)
        .expect("set first display");
    tree.set_box_sizing(first, BoxSizing::ContentBox)
        .expect("set first box sizing");
    tree.set_width(first, Length::points(20.0))
        .expect("set first width");
    tree.set_height(first, Length::points(14.0))
        .expect("set first height");
    tree.set_margin(first, StandaloneEdge::Top, Length::percent(5.0))
        .expect("set first top margin");
    tree.set_margin(first, StandaloneEdge::Bottom, Length::percent(3.0))
        .expect("set first bottom margin");

    let second = tree.create_default_node();
    tree.set_display(second, Display::Block)
        .expect("set second display");
    tree.set_box_sizing(second, BoxSizing::ContentBox)
        .expect("set second box sizing");
    tree.set_width(second, Length::points(18.0))
        .expect("set second width");
    tree.set_height(second, Length::points(16.0))
        .expect("set second height");
    tree.set_margin(second, StandaloneEdge::Top, Length::percent(4.0))
        .expect("set second top margin");
    tree.set_margin(second, StandaloneEdge::Bottom, Length::percent(2.0))
        .expect("set second bottom margin");

    tree.append_child(root, first).expect("append first");
    tree.append_child(root, second).expect("append second");

    run_standalone_rust(tree, root, Constraints::definite(50.0, 140.0))
        .expect("vertical linear justify-content start with percent main margins parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_vertical_linear_justify_content_flex_start_with_calc_main_margins()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Vertical)
        .expect("set root orientation");
    tree.set_justify_content(root, JustifyContent::FlexStart)
        .expect("set root justify content");
    tree.set_width(root, Length::points(52.0))
        .expect("set root width");
    tree.set_height(root, Length::points(150.0))
        .expect("set root height");

    let first = tree.create_default_node();
    tree.set_display(first, Display::Block)
        .expect("set first display");
    tree.set_box_sizing(first, BoxSizing::ContentBox)
        .expect("set first box sizing");
    tree.set_width(first, Length::points(22.0))
        .expect("set first width");
    tree.set_height(first, Length::points(15.0))
        .expect("set first height");
    tree.set_margin(first, StandaloneEdge::Top, Length::calc(2.0, 4.0))
        .expect("set first top margin");
    tree.set_margin(first, StandaloneEdge::Bottom, Length::calc(3.0, 2.0))
        .expect("set first bottom margin");

    let second = tree.create_default_node();
    tree.set_display(second, Display::Block)
        .expect("set second display");
    tree.set_box_sizing(second, BoxSizing::ContentBox)
        .expect("set second box sizing");
    tree.set_width(second, Length::points(17.0))
        .expect("set second width");
    tree.set_height(second, Length::points(19.0))
        .expect("set second height");
    tree.set_margin(second, StandaloneEdge::Top, Length::calc(1.0, 3.0))
        .expect("set second top margin");
    tree.set_margin(second, StandaloneEdge::Bottom, Length::calc(4.0, 1.0))
        .expect("set second bottom margin");

    tree.append_child(root, first).expect("append first");
    tree.append_child(root, second).expect("append second");

    run_standalone_rust(tree, root, Constraints::definite(52.0, 150.0))
        .expect("vertical linear justify-content flex-start with calc main margins parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_vertical_linear_justify_content_end_with_percent_main_margins()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Vertical)
        .expect("set root orientation");
    tree.set_justify_content(root, JustifyContent::End)
        .expect("set root justify content");
    tree.set_width(root, Length::points(48.0))
        .expect("set root width");
    tree.set_height(root, Length::points(160.0))
        .expect("set root height");

    let first = tree.create_default_node();
    tree.set_display(first, Display::Block)
        .expect("set first display");
    tree.set_box_sizing(first, BoxSizing::ContentBox)
        .expect("set first box sizing");
    tree.set_width(first, Length::points(16.0))
        .expect("set first width");
    tree.set_height(first, Length::points(18.0))
        .expect("set first height");
    tree.set_margin(first, StandaloneEdge::Top, Length::percent(3.0))
        .expect("set first top margin");
    tree.set_margin(first, StandaloneEdge::Bottom, Length::percent(4.0))
        .expect("set first bottom margin");

    let second = tree.create_default_node();
    tree.set_display(second, Display::Block)
        .expect("set second display");
    tree.set_box_sizing(second, BoxSizing::ContentBox)
        .expect("set second box sizing");
    tree.set_width(second, Length::points(21.0))
        .expect("set second width");
    tree.set_height(second, Length::points(13.0))
        .expect("set second height");
    tree.set_margin(second, StandaloneEdge::Top, Length::percent(2.0))
        .expect("set second top margin");
    tree.set_margin(second, StandaloneEdge::Bottom, Length::percent(5.0))
        .expect("set second bottom margin");

    tree.append_child(root, first).expect("append first");
    tree.append_child(root, second).expect("append second");

    run_standalone_rust(tree, root, Constraints::definite(48.0, 160.0))
        .expect("vertical linear justify-content end with percent main margins parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_vertical_linear_justify_content_center_with_calc_main_margins()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Vertical)
        .expect("set root orientation");
    tree.set_justify_content(root, JustifyContent::Center)
        .expect("set root justify content");
    tree.set_width(root, Length::points(54.0))
        .expect("set root width");
    tree.set_height(root, Length::points(170.0))
        .expect("set root height");

    let first = tree.create_default_node();
    tree.set_display(first, Display::Block)
        .expect("set first display");
    tree.set_box_sizing(first, BoxSizing::ContentBox)
        .expect("set first box sizing");
    tree.set_width(first, Length::points(19.0))
        .expect("set first width");
    tree.set_height(first, Length::points(17.0))
        .expect("set first height");
    tree.set_margin(first, StandaloneEdge::Top, Length::calc(3.0, 2.0))
        .expect("set first top margin");
    tree.set_margin(first, StandaloneEdge::Bottom, Length::calc(1.0, 4.0))
        .expect("set first bottom margin");

    let second = tree.create_default_node();
    tree.set_display(second, Display::Block)
        .expect("set second display");
    tree.set_box_sizing(second, BoxSizing::ContentBox)
        .expect("set second box sizing");
    tree.set_width(second, Length::points(23.0))
        .expect("set second width");
    tree.set_height(second, Length::points(11.0))
        .expect("set second height");
    tree.set_margin(second, StandaloneEdge::Top, Length::calc(4.0, 1.0))
        .expect("set second top margin");
    tree.set_margin(second, StandaloneEdge::Bottom, Length::calc(2.0, 3.0))
        .expect("set second bottom margin");

    tree.append_child(root, first).expect("append first");
    tree.append_child(root, second).expect("append second");

    run_standalone_rust(tree, root, Constraints::definite(54.0, 170.0))
        .expect("vertical linear justify-content center with calc main margins parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_vertical_linear_justify_content_space_around_falls_back_to_start_with_percent_main_margins()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Vertical)
        .expect("set root orientation");
    tree.set_justify_content(root, JustifyContent::SpaceAround)
        .expect("set root justify content");
    tree.set_width(root, Length::points(46.0))
        .expect("set root width");
    tree.set_height(root, Length::points(155.0))
        .expect("set root height");

    let first = tree.create_default_node();
    tree.set_display(first, Display::Block)
        .expect("set first display");
    tree.set_box_sizing(first, BoxSizing::ContentBox)
        .expect("set first box sizing");
    tree.set_width(first, Length::points(18.0))
        .expect("set first width");
    tree.set_height(first, Length::points(12.0))
        .expect("set first height");
    tree.set_margin(first, StandaloneEdge::Top, Length::percent(6.0))
        .expect("set first top margin");
    tree.set_margin(first, StandaloneEdge::Bottom, Length::percent(2.0))
        .expect("set first bottom margin");

    let second = tree.create_default_node();
    tree.set_display(second, Display::Block)
        .expect("set second display");
    tree.set_box_sizing(second, BoxSizing::ContentBox)
        .expect("set second box sizing");
    tree.set_width(second, Length::points(20.0))
        .expect("set second width");
    tree.set_height(second, Length::points(14.0))
        .expect("set second height");
    tree.set_margin(second, StandaloneEdge::Top, Length::percent(3.0))
        .expect("set second top margin");
    tree.set_margin(second, StandaloneEdge::Bottom, Length::percent(4.0))
        .expect("set second bottom margin");

    tree.append_child(root, first).expect("append first");
    tree.append_child(root, second).expect("append second");

    run_standalone_rust(tree, root, Constraints::definite(46.0, 155.0)).expect(
        "vertical linear justify-content space-around fallback with percent margins parity",
    );
}

#[test]
fn standalone_owned_tree_matches_cpp_for_vertical_linear_justify_content_space_evenly_falls_back_to_start_with_calc_main_margins()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Vertical)
        .expect("set root orientation");
    tree.set_justify_content(root, JustifyContent::SpaceEvenly)
        .expect("set root justify content");
    tree.set_width(root, Length::points(58.0))
        .expect("set root width");
    tree.set_height(root, Length::points(165.0))
        .expect("set root height");

    let first = tree.create_default_node();
    tree.set_display(first, Display::Block)
        .expect("set first display");
    tree.set_box_sizing(first, BoxSizing::ContentBox)
        .expect("set first box sizing");
    tree.set_width(first, Length::points(24.0))
        .expect("set first width");
    tree.set_height(first, Length::points(16.0))
        .expect("set first height");
    tree.set_margin(first, StandaloneEdge::Top, Length::calc(2.0, 5.0))
        .expect("set first top margin");
    tree.set_margin(first, StandaloneEdge::Bottom, Length::calc(1.0, 3.0))
        .expect("set first bottom margin");

    let second = tree.create_default_node();
    tree.set_display(second, Display::Block)
        .expect("set second display");
    tree.set_box_sizing(second, BoxSizing::ContentBox)
        .expect("set second box sizing");
    tree.set_width(second, Length::points(16.0))
        .expect("set second width");
    tree.set_height(second, Length::points(18.0))
        .expect("set second height");
    tree.set_margin(second, StandaloneEdge::Top, Length::calc(3.0, 2.0))
        .expect("set second top margin");
    tree.set_margin(second, StandaloneEdge::Bottom, Length::calc(4.0, 1.0))
        .expect("set second bottom margin");

    tree.append_child(root, first).expect("append first");
    tree.append_child(root, second).expect("append second");

    run_standalone_rust(tree, root, Constraints::definite(58.0, 165.0))
        .expect("vertical linear justify-content space-evenly fallback with calc margins parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_horizontal_linear_justify_content_space_between() {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root orientation");
    tree.set_justify_content(root, JustifyContent::SpaceBetween)
        .expect("set root justify content");
    tree.set_width(root, Length::points(140.0))
        .expect("set root width");
    tree.set_height(root, Length::points(30.0))
        .expect("set root height");

    let first = tree.create_default_node();
    tree.set_display(first, Display::Block)
        .expect("set first display");
    tree.set_box_sizing(first, BoxSizing::ContentBox)
        .expect("set first box sizing");
    tree.set_width(first, Length::points(20.0))
        .expect("set first width");
    tree.set_height(first, Length::points(10.0))
        .expect("set first height");

    let second = tree.create_default_node();
    tree.set_display(second, Display::Block)
        .expect("set second display");
    tree.set_box_sizing(second, BoxSizing::ContentBox)
        .expect("set second box sizing");
    tree.set_width(second, Length::points(30.0))
        .expect("set second width");
    tree.set_height(second, Length::points(12.0))
        .expect("set second height");

    tree.append_child(root, first).expect("append first");
    tree.append_child(root, second).expect("append second");

    run_standalone_rust(tree, root, Constraints::definite(140.0, 30.0))
        .expect("horizontal linear justify-content space-between parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_vertical_reverse_linear_justify_content_space_between_with_calc_main_margins()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::VerticalReverse)
        .expect("set root orientation");
    tree.set_justify_content(root, JustifyContent::SpaceBetween)
        .expect("set root justify content");
    tree.set_width(root, Length::points(90.0))
        .expect("set root width");
    tree.set_height(root, Length::points(190.0))
        .expect("set root height");

    let first = tree.create_default_node();
    tree.set_display(first, Display::Block)
        .expect("set first display");
    tree.set_box_sizing(first, BoxSizing::ContentBox)
        .expect("set first box sizing");
    tree.set_width(first, Length::points(20.0))
        .expect("set first width");
    tree.set_height(first, Length::points(18.0))
        .expect("set first height");
    tree.set_margin(first, StandaloneEdge::Top, Length::calc(2.0, 4.0))
        .expect("set first top margin");
    tree.set_margin(first, StandaloneEdge::Bottom, Length::calc(3.0, 5.0))
        .expect("set first bottom margin");

    let second = tree.create_default_node();
    tree.set_display(second, Display::Block)
        .expect("set second display");
    tree.set_box_sizing(second, BoxSizing::ContentBox)
        .expect("set second box sizing");
    tree.set_width(second, Length::points(22.0))
        .expect("set second width");
    tree.set_height(second, Length::points(16.0))
        .expect("set second height");
    tree.set_margin(second, StandaloneEdge::Top, Length::calc(4.0, 3.0))
        .expect("set second top margin");
    tree.set_margin(second, StandaloneEdge::Bottom, Length::calc(1.0, 6.0))
        .expect("set second bottom margin");

    let third = tree.create_default_node();
    tree.set_display(third, Display::Block)
        .expect("set third display");
    tree.set_box_sizing(third, BoxSizing::ContentBox)
        .expect("set third box sizing");
    tree.set_width(third, Length::points(24.0))
        .expect("set third width");
    tree.set_height(third, Length::points(14.0))
        .expect("set third height");
    tree.set_margin(third, StandaloneEdge::Top, Length::calc(5.0, 2.0))
        .expect("set third top margin");
    tree.set_margin(third, StandaloneEdge::Bottom, Length::calc(2.0, 4.0))
        .expect("set third bottom margin");

    tree.append_child(root, first).expect("append first");
    tree.append_child(root, second).expect("append second");
    tree.append_child(root, third).expect("append third");

    run_standalone_rust(tree, root, Constraints::definite(90.0, 190.0))
        .expect("vertical-reverse linear justify-content space-between parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_vertical_reverse_linear_justify_content_flex_start_with_percent_main_margins()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::VerticalReverse)
        .expect("set root orientation");
    tree.set_justify_content(root, JustifyContent::FlexStart)
        .expect("set root justify content");
    tree.set_align_items(root, AlignItems::FlexStart)
        .expect("set root align items");
    tree.set_width(root, Length::points(62.0))
        .expect("set root width");
    tree.set_height(root, Length::points(150.0))
        .expect("set root height");

    let first = tree.create_default_node();
    tree.set_display(first, Display::Block)
        .expect("set first display");
    tree.set_box_sizing(first, BoxSizing::ContentBox)
        .expect("set first box sizing");
    tree.set_width(first, Length::points(24.0))
        .expect("set first width");
    tree.set_height(first, Length::points(15.0))
        .expect("set first height");
    tree.set_margin(first, StandaloneEdge::Top, Length::percent(3.0))
        .expect("set first top margin");
    tree.set_margin(first, StandaloneEdge::Bottom, Length::percent(5.0))
        .expect("set first bottom margin");

    let second = tree.create_default_node();
    tree.set_display(second, Display::Block)
        .expect("set second display");
    tree.set_box_sizing(second, BoxSizing::ContentBox)
        .expect("set second box sizing");
    tree.set_width(second, Length::points(18.0))
        .expect("set second width");
    tree.set_height(second, Length::points(17.0))
        .expect("set second height");
    tree.set_margin(second, StandaloneEdge::Top, Length::percent(4.0))
        .expect("set second top margin");
    tree.set_margin(second, StandaloneEdge::Bottom, Length::percent(2.0))
        .expect("set second bottom margin");

    tree.append_child(root, first).expect("append first");
    tree.append_child(root, second).expect("append second");

    run_standalone_rust(tree, root, Constraints::definite(62.0, 150.0))
        .expect("vertical-reverse linear justify-content flex-start with percent margins parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_vertical_reverse_linear_justify_content_flex_end_with_calc_main_margins()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::VerticalReverse)
        .expect("set root orientation");
    tree.set_justify_content(root, JustifyContent::FlexEnd)
        .expect("set root justify content");
    tree.set_align_items(root, AlignItems::FlexStart)
        .expect("set root align items");
    tree.set_width(root, Length::points(66.0))
        .expect("set root width");
    tree.set_height(root, Length::points(164.0))
        .expect("set root height");

    let first = tree.create_default_node();
    tree.set_display(first, Display::Block)
        .expect("set first display");
    tree.set_box_sizing(first, BoxSizing::ContentBox)
        .expect("set first box sizing");
    tree.set_width(first, Length::points(22.0))
        .expect("set first width");
    tree.set_height(first, Length::points(18.0))
        .expect("set first height");
    tree.set_margin(first, StandaloneEdge::Top, Length::calc(2.0, 4.0))
        .expect("set first top margin");
    tree.set_margin(first, StandaloneEdge::Bottom, Length::calc(3.0, 2.0))
        .expect("set first bottom margin");

    let second = tree.create_default_node();
    tree.set_display(second, Display::Block)
        .expect("set second display");
    tree.set_box_sizing(second, BoxSizing::ContentBox)
        .expect("set second box sizing");
    tree.set_width(second, Length::points(20.0))
        .expect("set second width");
    tree.set_height(second, Length::points(14.0))
        .expect("set second height");
    tree.set_margin(second, StandaloneEdge::Top, Length::calc(1.0, 5.0))
        .expect("set second top margin");
    tree.set_margin(second, StandaloneEdge::Bottom, Length::calc(4.0, 1.0))
        .expect("set second bottom margin");

    tree.append_child(root, first).expect("append first");
    tree.append_child(root, second).expect("append second");

    run_standalone_rust(tree, root, Constraints::definite(66.0, 164.0))
        .expect("vertical-reverse linear justify-content flex-end with calc margins parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_vertical_reverse_linear_justify_content_center_with_percent_main_margins()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::VerticalReverse)
        .expect("set root orientation");
    tree.set_justify_content(root, JustifyContent::Center)
        .expect("set root justify content");
    tree.set_align_items(root, AlignItems::FlexStart)
        .expect("set root align items");
    tree.set_width(root, Length::points(70.0))
        .expect("set root width");
    tree.set_height(root, Length::points(172.0))
        .expect("set root height");

    let first = tree.create_default_node();
    tree.set_display(first, Display::Block)
        .expect("set first display");
    tree.set_box_sizing(first, BoxSizing::ContentBox)
        .expect("set first box sizing");
    tree.set_width(first, Length::points(26.0))
        .expect("set first width");
    tree.set_height(first, Length::points(16.0))
        .expect("set first height");
    tree.set_margin(first, StandaloneEdge::Top, Length::percent(2.0))
        .expect("set first top margin");
    tree.set_margin(first, StandaloneEdge::Bottom, Length::percent(6.0))
        .expect("set first bottom margin");

    let second = tree.create_default_node();
    tree.set_display(second, Display::Block)
        .expect("set second display");
    tree.set_box_sizing(second, BoxSizing::ContentBox)
        .expect("set second box sizing");
    tree.set_width(second, Length::points(19.0))
        .expect("set second width");
    tree.set_height(second, Length::points(13.0))
        .expect("set second height");
    tree.set_margin(second, StandaloneEdge::Top, Length::percent(5.0))
        .expect("set second top margin");
    tree.set_margin(second, StandaloneEdge::Bottom, Length::percent(3.0))
        .expect("set second bottom margin");

    tree.append_child(root, first).expect("append first");
    tree.append_child(root, second).expect("append second");

    run_standalone_rust(tree, root, Constraints::definite(70.0, 172.0))
        .expect("vertical-reverse linear justify-content center with percent margins parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_vertical_reverse_linear_justify_content_space_around_falls_back_to_start_with_calc_main_margins()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::VerticalReverse)
        .expect("set root orientation");
    tree.set_justify_content(root, JustifyContent::SpaceAround)
        .expect("set root justify content");
    tree.set_align_items(root, AlignItems::FlexStart)
        .expect("set root align items");
    tree.set_width(root, Length::points(68.0))
        .expect("set root width");
    tree.set_height(root, Length::points(158.0))
        .expect("set root height");

    let first = tree.create_default_node();
    tree.set_display(first, Display::Block)
        .expect("set first display");
    tree.set_box_sizing(first, BoxSizing::ContentBox)
        .expect("set first box sizing");
    tree.set_width(first, Length::points(23.0))
        .expect("set first width");
    tree.set_height(first, Length::points(17.0))
        .expect("set first height");
    tree.set_margin(first, StandaloneEdge::Top, Length::calc(2.0, 3.0))
        .expect("set first top margin");
    tree.set_margin(first, StandaloneEdge::Bottom, Length::calc(4.0, 2.0))
        .expect("set first bottom margin");

    let second = tree.create_default_node();
    tree.set_display(second, Display::Block)
        .expect("set second display");
    tree.set_box_sizing(second, BoxSizing::ContentBox)
        .expect("set second box sizing");
    tree.set_width(second, Length::points(21.0))
        .expect("set second width");
    tree.set_height(second, Length::points(15.0))
        .expect("set second height");
    tree.set_margin(second, StandaloneEdge::Top, Length::calc(3.0, 4.0))
        .expect("set second top margin");
    tree.set_margin(second, StandaloneEdge::Bottom, Length::calc(1.0, 5.0))
        .expect("set second bottom margin");

    tree.append_child(root, first).expect("append first");
    tree.append_child(root, second).expect("append second");

    run_standalone_rust(tree, root, Constraints::definite(68.0, 158.0))
        .expect("vertical-reverse linear justify-content space-around fallback parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_vertical_reverse_linear_justify_content_space_evenly_falls_back_to_start_with_percent_main_margins()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::VerticalReverse)
        .expect("set root orientation");
    tree.set_justify_content(root, JustifyContent::SpaceEvenly)
        .expect("set root justify content");
    tree.set_align_items(root, AlignItems::FlexStart)
        .expect("set root align items");
    tree.set_width(root, Length::points(64.0))
        .expect("set root width");
    tree.set_height(root, Length::points(166.0))
        .expect("set root height");

    let first = tree.create_default_node();
    tree.set_display(first, Display::Block)
        .expect("set first display");
    tree.set_box_sizing(first, BoxSizing::ContentBox)
        .expect("set first box sizing");
    tree.set_width(first, Length::points(20.0))
        .expect("set first width");
    tree.set_height(first, Length::points(14.0))
        .expect("set first height");
    tree.set_margin(first, StandaloneEdge::Top, Length::percent(7.0))
        .expect("set first top margin");
    tree.set_margin(first, StandaloneEdge::Bottom, Length::percent(2.0))
        .expect("set first bottom margin");

    let second = tree.create_default_node();
    tree.set_display(second, Display::Block)
        .expect("set second display");
    tree.set_box_sizing(second, BoxSizing::ContentBox)
        .expect("set second box sizing");
    tree.set_width(second, Length::points(25.0))
        .expect("set second width");
    tree.set_height(second, Length::points(18.0))
        .expect("set second height");
    tree.set_margin(second, StandaloneEdge::Top, Length::percent(3.0))
        .expect("set second top margin");
    tree.set_margin(second, StandaloneEdge::Bottom, Length::percent(4.0))
        .expect("set second bottom margin");

    tree.append_child(root, first).expect("append first");
    tree.append_child(root, second).expect("append second");

    run_standalone_rust(tree, root, Constraints::definite(64.0, 166.0))
        .expect("vertical-reverse linear justify-content space-evenly fallback parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_vertical_reverse_linear_justify_content_stretch_falls_back_to_start_with_calc_main_margins()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::VerticalReverse)
        .expect("set root orientation");
    tree.set_justify_content(root, JustifyContent::Stretch)
        .expect("set root justify content");
    tree.set_align_items(root, AlignItems::FlexStart)
        .expect("set root align items");
    tree.set_width(root, Length::points(72.0))
        .expect("set root width");
    tree.set_height(root, Length::points(176.0))
        .expect("set root height");

    let first = tree.create_default_node();
    tree.set_display(first, Display::Block)
        .expect("set first display");
    tree.set_box_sizing(first, BoxSizing::ContentBox)
        .expect("set first box sizing");
    tree.set_width(first, Length::points(27.0))
        .expect("set first width");
    tree.set_height(first, Length::points(19.0))
        .expect("set first height");
    tree.set_margin(first, StandaloneEdge::Top, Length::calc(4.0, 1.0))
        .expect("set first top margin");
    tree.set_margin(first, StandaloneEdge::Bottom, Length::calc(2.0, 6.0))
        .expect("set first bottom margin");

    let second = tree.create_default_node();
    tree.set_display(second, Display::Block)
        .expect("set second display");
    tree.set_box_sizing(second, BoxSizing::ContentBox)
        .expect("set second box sizing");
    tree.set_width(second, Length::points(18.0))
        .expect("set second width");
    tree.set_height(second, Length::points(16.0))
        .expect("set second height");
    tree.set_margin(second, StandaloneEdge::Top, Length::calc(3.0, 2.0))
        .expect("set second top margin");
    tree.set_margin(second, StandaloneEdge::Bottom, Length::calc(5.0, 3.0))
        .expect("set second bottom margin");

    tree.append_child(root, first).expect("append first");
    tree.append_child(root, second).expect("append second");

    run_standalone_rust(tree, root, Constraints::definite(72.0, 176.0))
        .expect("vertical-reverse linear justify-content stretch fallback parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_justify_content_space_between_single_item() {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root orientation");
    tree.set_justify_content(root, JustifyContent::SpaceBetween)
        .expect("set root justify content");
    tree.set_width(root, Length::points(120.0))
        .expect("set root width");
    tree.set_height(root, Length::points(40.0))
        .expect("set root height");

    let child = tree.create_default_node();
    tree.set_display(child, Display::Block)
        .expect("set child display");
    tree.set_box_sizing(child, BoxSizing::ContentBox)
        .expect("set child box sizing");
    tree.set_width(child, Length::points(24.0))
        .expect("set child width");
    tree.set_height(child, Length::points(12.0))
        .expect("set child height");
    tree.set_margin(child, StandaloneEdge::Left, Length::points(3.0))
        .expect("set child left margin");
    tree.set_margin(child, StandaloneEdge::Right, Length::points(5.0))
        .expect("set child right margin");
    tree.append_child(root, child).expect("append child");

    run_standalone_rust(tree, root, Constraints::definite(120.0, 40.0))
        .expect("linear justify-content space-between single-item parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_horizontal_reverse_linear_gravity_end_overrides_space_between_with_calc_main_margins()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::HorizontalReverse)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::End)
        .expect("set root gravity");
    tree.set_justify_content(root, JustifyContent::SpaceBetween)
        .expect("set root justify content");
    tree.set_width(root, Length::points(180.0))
        .expect("set root width");
    tree.set_height(root, Length::points(50.0))
        .expect("set root height");

    let first = tree.create_default_node();
    tree.set_display(first, Display::Block)
        .expect("set first display");
    tree.set_box_sizing(first, BoxSizing::ContentBox)
        .expect("set first box sizing");
    tree.set_width(first, Length::points(28.0))
        .expect("set first width");
    tree.set_height(first, Length::points(12.0))
        .expect("set first height");
    tree.set_margin(first, StandaloneEdge::Left, Length::calc(4.0, 6.0))
        .expect("set first left margin");
    tree.set_margin(first, StandaloneEdge::Right, Length::calc(3.0, 8.0))
        .expect("set first right margin");

    let second = tree.create_default_node();
    tree.set_display(second, Display::Block)
        .expect("set second display");
    tree.set_box_sizing(second, BoxSizing::ContentBox)
        .expect("set second box sizing");
    tree.set_width(second, Length::points(18.0))
        .expect("set second width");
    tree.set_height(second, Length::points(10.0))
        .expect("set second height");
    tree.set_margin(second, StandaloneEdge::Left, Length::calc(1.0, 5.0))
        .expect("set second left margin");
    tree.set_margin(second, StandaloneEdge::Right, Length::calc(5.0, 4.0))
        .expect("set second right margin");

    tree.append_child(root, first).expect("append first");
    tree.append_child(root, second).expect("append second");

    run_standalone_rust(tree, root, Constraints::definite(180.0, 50.0))
        .expect("horizontal-reverse linear gravity end override parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_vertical_linear_gravity_center_overrides_flex_end_with_percent_main_margins()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_linear_orientation(root, LinearOrientation::Vertical)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::Center)
        .expect("set root gravity");
    tree.set_justify_content(root, JustifyContent::FlexEnd)
        .expect("set root justify content");
    tree.set_width(root, Length::points(80.0))
        .expect("set root width");
    tree.set_height(root, Length::points(170.0))
        .expect("set root height");

    let first = tree.create_default_node();
    tree.set_display(first, Display::Block)
        .expect("set first display");
    tree.set_width(first, Length::points(26.0))
        .expect("set first width");
    tree.set_height(first, Length::points(20.0))
        .expect("set first height");
    tree.set_margin(first, StandaloneEdge::Top, Length::percent(4.0))
        .expect("set first top margin");
    tree.set_margin(first, StandaloneEdge::Bottom, Length::percent(6.0))
        .expect("set first bottom margin");

    let second = tree.create_default_node();
    tree.set_display(second, Display::Block)
        .expect("set second display");
    tree.set_width(second, Length::points(22.0))
        .expect("set second width");
    tree.set_height(second, Length::points(16.0))
        .expect("set second height");
    tree.set_margin(second, StandaloneEdge::Top, Length::percent(3.0))
        .expect("set second top margin");
    tree.set_margin(second, StandaloneEdge::Bottom, Length::percent(5.0))
        .expect("set second bottom margin");

    tree.append_child(root, first).expect("append first");
    tree.append_child(root, second).expect("append second");

    run_standalone_rust(tree, root, Constraints::definite(80.0, 170.0))
        .expect("vertical linear gravity center override parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_rtl_horizontal_linear_gravity_left_overrides_center_with_auto_main_margin()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_direction(root, Direction::Rtl)
        .expect("set root direction");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::Left)
        .expect("set root gravity");
    tree.set_justify_content(root, JustifyContent::Center)
        .expect("set root justify content");
    tree.set_width(root, Length::points(160.0))
        .expect("set root width");
    tree.set_height(root, Length::points(50.0))
        .expect("set root height");

    let child = tree.create_default_node();
    tree.set_display(child, Display::Block)
        .expect("set child display");
    tree.set_box_sizing(child, BoxSizing::ContentBox)
        .expect("set child box sizing");
    tree.set_width(child, Length::points(30.0))
        .expect("set child width");
    tree.set_height(child, Length::points(12.0))
        .expect("set child height");
    tree.set_margin(child, StandaloneEdge::Left, Length::Auto)
        .expect("set child left auto margin");
    tree.set_margin(child, StandaloneEdge::Right, Length::calc(4.0, 5.0))
        .expect("set child right margin");
    tree.append_child(root, child).expect("append child");

    run_standalone_rust(tree, root, Constraints::definite(160.0, 50.0))
        .expect("rtl horizontal linear gravity left override parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_cross_gravity_auto_margins_and_baseline() {
    let (tree, root, constraints) = linear_cross_gravity_auto_margins_and_baseline_tree();

    run_standalone_rust(tree, root, constraints)
        .expect("linear cross-gravity, auto-margin, and baseline parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_horizontal_linear_cross_axis_start_auto_margin() {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root linear orientation");
    tree.set_align_items(root, AlignItems::FlexStart)
        .expect("set root align items");
    tree.set_width(root, Length::points(100.0))
        .expect("set root width");
    tree.set_height(root, Length::points(100.0))
        .expect("set root height");

    let child = tree.create_default_node();
    tree.set_display(child, Display::Block)
        .expect("set child display");
    tree.set_box_sizing(child, BoxSizing::ContentBox)
        .expect("set child box sizing");
    tree.set_width(child, Length::points(20.0))
        .expect("set child width");
    tree.set_height(child, Length::points(10.0))
        .expect("set child height");
    tree.set_margin(child, StandaloneEdge::Top, Length::Auto)
        .expect("set child top auto margin");
    tree.append_child(root, child).expect("append child");

    run_standalone_rust(tree, root, Constraints::definite(100.0, 100.0))
        .expect("horizontal linear cross-axis start auto margin parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_horizontal_linear_cross_axis_end_auto_margin() {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root linear orientation");
    tree.set_align_items(root, AlignItems::FlexStart)
        .expect("set root align items");
    tree.set_width(root, Length::points(100.0))
        .expect("set root width");
    tree.set_height(root, Length::points(100.0))
        .expect("set root height");

    let child = tree.create_default_node();
    tree.set_display(child, Display::Block)
        .expect("set child display");
    tree.set_box_sizing(child, BoxSizing::ContentBox)
        .expect("set child box sizing");
    tree.set_width(child, Length::points(20.0))
        .expect("set child width");
    tree.set_height(child, Length::points(10.0))
        .expect("set child height");
    tree.set_margin(child, StandaloneEdge::Bottom, Length::Auto)
        .expect("set child bottom auto margin");
    tree.append_child(root, child).expect("append child");

    run_standalone_rust(tree, root, Constraints::definite(100.0, 100.0))
        .expect("horizontal linear cross-axis end auto margin parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_horizontal_linear_cross_axis_center_with_fixed_margins() {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root linear orientation");
    tree.set_align_items(root, AlignItems::Center)
        .expect("set root align items");
    tree.set_width(root, Length::points(100.0))
        .expect("set root width");
    tree.set_height(root, Length::points(80.0))
        .expect("set root height");

    let child = fixed_linear_gravity_child(&mut tree, 20.0, 10.0);
    tree.set_box_sizing(child, BoxSizing::ContentBox)
        .expect("set child box sizing");
    tree.set_margin(child, StandaloneEdge::Top, Length::points(4.0))
        .expect("set child top margin");
    tree.set_margin(child, StandaloneEdge::Bottom, Length::points(6.0))
        .expect("set child bottom margin");
    tree.append_child(root, child).expect("append child");

    run_standalone_rust(tree, root, Constraints::definite(100.0, 80.0))
        .expect("horizontal linear cross-axis center with fixed margins parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_horizontal_linear_cross_axis_end_with_percent_margins() {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root linear orientation");
    tree.set_align_items(root, AlignItems::FlexEnd)
        .expect("set root align items");
    tree.set_width(root, Length::points(100.0))
        .expect("set root width");
    tree.set_height(root, Length::points(80.0))
        .expect("set root height");

    let child = fixed_linear_gravity_child(&mut tree, 20.0, 10.0);
    tree.set_box_sizing(child, BoxSizing::ContentBox)
        .expect("set child box sizing");
    tree.set_margin(child, StandaloneEdge::Top, Length::percent(10.0))
        .expect("set child top margin");
    tree.set_margin(child, StandaloneEdge::Bottom, Length::percent(5.0))
        .expect("set child bottom margin");
    tree.append_child(root, child).expect("append child");

    run_standalone_rust(tree, root, Constraints::definite(100.0, 80.0))
        .expect("horizontal linear cross-axis end with percent margins parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_vertical_linear_cross_axis_center_with_fixed_margins() {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Vertical)
        .expect("set root linear orientation");
    tree.set_align_items(root, AlignItems::Center)
        .expect("set root align items");
    tree.set_width(root, Length::points(120.0))
        .expect("set root width");
    tree.set_height(root, Length::points(100.0))
        .expect("set root height");

    let child = fixed_linear_gravity_child(&mut tree, 20.0, 10.0);
    tree.set_box_sizing(child, BoxSizing::ContentBox)
        .expect("set child box sizing");
    tree.set_margin(child, StandaloneEdge::Left, Length::points(7.0))
        .expect("set child left margin");
    tree.set_margin(child, StandaloneEdge::Right, Length::points(3.0))
        .expect("set child right margin");
    tree.append_child(root, child).expect("append child");

    run_standalone_rust(tree, root, Constraints::definite(120.0, 100.0))
        .expect("vertical linear cross-axis center with fixed margins parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_rtl_vertical_linear_cross_axis_end_with_fixed_margins() {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_direction(root, Direction::Rtl)
        .expect("set root direction");
    tree.set_linear_orientation(root, LinearOrientation::Vertical)
        .expect("set root linear orientation");
    tree.set_align_items(root, AlignItems::FlexEnd)
        .expect("set root align items");
    tree.set_width(root, Length::points(120.0))
        .expect("set root width");
    tree.set_height(root, Length::points(100.0))
        .expect("set root height");

    let child = fixed_linear_gravity_child(&mut tree, 20.0, 10.0);
    tree.set_box_sizing(child, BoxSizing::ContentBox)
        .expect("set child box sizing");
    tree.set_margin(child, StandaloneEdge::Left, Length::points(7.0))
        .expect("set child left margin");
    tree.set_margin(child, StandaloneEdge::Right, Length::points(3.0))
        .expect("set child right margin");
    tree.append_child(root, child).expect("append child");

    run_standalone_rust(tree, root, Constraints::definite(120.0, 100.0))
        .expect("RTL vertical linear cross-axis end with fixed margins parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_vertical_linear_cross_axis_start_auto_margin() {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Vertical)
        .expect("set root linear orientation");
    tree.set_align_items(root, AlignItems::FlexEnd)
        .expect("set root align items");
    tree.set_width(root, Length::points(120.0))
        .expect("set root width");
    tree.set_height(root, Length::points(100.0))
        .expect("set root height");

    let child = fixed_linear_gravity_child(&mut tree, 20.0, 10.0);
    tree.set_box_sizing(child, BoxSizing::ContentBox)
        .expect("set child box sizing");
    tree.set_margin(child, StandaloneEdge::Left, Length::Auto)
        .expect("set child left auto margin");
    tree.append_child(root, child).expect("append child");

    run_standalone_rust(tree, root, Constraints::definite(120.0, 100.0))
        .expect("vertical linear cross-axis start auto margin parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_rtl_vertical_linear_cross_axis_end_auto_margin() {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_direction(root, Direction::Rtl)
        .expect("set root direction");
    tree.set_linear_orientation(root, LinearOrientation::Vertical)
        .expect("set root linear orientation");
    tree.set_align_items(root, AlignItems::FlexStart)
        .expect("set root align items");
    tree.set_width(root, Length::points(120.0))
        .expect("set root width");
    tree.set_height(root, Length::points(100.0))
        .expect("set root height");

    let child = fixed_linear_gravity_child(&mut tree, 20.0, 10.0);
    tree.set_box_sizing(child, BoxSizing::ContentBox)
        .expect("set child box sizing");
    tree.set_margin(child, StandaloneEdge::Left, Length::Auto)
        .expect("set child left auto margin");
    tree.append_child(root, child).expect("append child");

    run_standalone_rust(tree, root, Constraints::definite(120.0, 100.0))
        .expect("RTL vertical linear cross-axis end auto margin parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_horizontal_linear_cross_axis_center_with_calc_margins() {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root linear orientation");
    tree.set_align_items(root, AlignItems::Center)
        .expect("set root align items");
    tree.set_width(root, Length::points(120.0))
        .expect("set root width");
    tree.set_height(root, Length::points(90.0))
        .expect("set root height");

    let child = tree.create_default_node();
    tree.set_display(child, Display::Block)
        .expect("set child display");
    tree.set_box_sizing(child, BoxSizing::ContentBox)
        .expect("set child box sizing");
    tree.set_width(child, Length::points(20.0))
        .expect("set child width");
    tree.set_height(child, Length::points(12.0))
        .expect("set child height");
    tree.set_margin(child, StandaloneEdge::Top, Length::calc(4.0, 10.0))
        .expect("set child top margin");
    tree.set_margin(child, StandaloneEdge::Bottom, Length::calc(6.0, 5.0))
        .expect("set child bottom margin");
    tree.append_child(root, child).expect("append child");

    run_standalone_rust(tree, root, Constraints::definite(120.0, 90.0))
        .expect("horizontal linear cross-axis center with calc margins parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_horizontal_linear_cross_axis_end_with_calc_margins() {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root linear orientation");
    tree.set_align_items(root, AlignItems::FlexEnd)
        .expect("set root align items");
    tree.set_width(root, Length::points(120.0))
        .expect("set root width");
    tree.set_height(root, Length::points(90.0))
        .expect("set root height");

    let child = tree.create_default_node();
    tree.set_display(child, Display::Block)
        .expect("set child display");
    tree.set_box_sizing(child, BoxSizing::ContentBox)
        .expect("set child box sizing");
    tree.set_width(child, Length::points(24.0))
        .expect("set child width");
    tree.set_height(child, Length::points(14.0))
        .expect("set child height");
    tree.set_margin(child, StandaloneEdge::Top, Length::calc(5.0, 8.0))
        .expect("set child top margin");
    tree.set_margin(child, StandaloneEdge::Bottom, Length::calc(3.0, 12.0))
        .expect("set child bottom margin");
    tree.append_child(root, child).expect("append child");

    run_standalone_rust(tree, root, Constraints::definite(120.0, 90.0))
        .expect("horizontal linear cross-axis end with calc margins parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_horizontal_linear_cross_axis_center_overflow_with_root_padding_border_and_percent_margins()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root linear orientation");
    tree.set_align_items(root, AlignItems::Center)
        .expect("set root align items");
    tree.set_width(root, Length::points(110.0))
        .expect("set root width");
    tree.set_height(root, Length::points(42.0))
        .expect("set root height");
    tree.set_padding(root, StandaloneEdge::Top, Length::points(4.0))
        .expect("set root top padding");
    tree.set_padding(root, StandaloneEdge::Bottom, Length::points(6.0))
        .expect("set root bottom padding");
    tree.set_border(root, StandaloneEdge::Top, 1.0)
        .expect("set root top border");
    tree.set_border(root, StandaloneEdge::Bottom, 2.0)
        .expect("set root bottom border");

    let child = tree.create_default_node();
    tree.set_display(child, Display::Block)
        .expect("set child display");
    tree.set_box_sizing(child, BoxSizing::ContentBox)
        .expect("set child box sizing");
    tree.set_width(child, Length::points(24.0))
        .expect("set child width");
    tree.set_height(child, Length::points(58.0))
        .expect("set child height");
    tree.set_margin(child, StandaloneEdge::Top, Length::percent(8.0))
        .expect("set child top margin");
    tree.set_margin(child, StandaloneEdge::Bottom, Length::percent(6.0))
        .expect("set child bottom margin");
    tree.append_child(root, child).expect("append child");

    run_standalone_rust(tree, root, Constraints::definite(110.0, 55.0))
        .expect("horizontal linear cross-axis center overflow with root padding/border parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_horizontal_linear_cross_axis_end_overflow_with_root_padding_border_and_calc_margins()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root linear orientation");
    tree.set_align_items(root, AlignItems::FlexEnd)
        .expect("set root align items");
    tree.set_width(root, Length::points(112.0))
        .expect("set root width");
    tree.set_height(root, Length::points(40.0))
        .expect("set root height");
    tree.set_padding(root, StandaloneEdge::Vertical, Length::points(3.0))
        .expect("set root vertical padding");
    tree.set_border(root, StandaloneEdge::Vertical, 1.0)
        .expect("set root vertical border");

    let child = tree.create_default_node();
    tree.set_display(child, Display::Block)
        .expect("set child display");
    tree.set_box_sizing(child, BoxSizing::ContentBox)
        .expect("set child box sizing");
    tree.set_width(child, Length::points(26.0))
        .expect("set child width");
    tree.set_height(child, Length::points(54.0))
        .expect("set child height");
    tree.set_margin(child, StandaloneEdge::Top, Length::calc(5.0, 4.0))
        .expect("set child top margin");
    tree.set_margin(child, StandaloneEdge::Bottom, Length::calc(4.0, 3.0))
        .expect("set child bottom margin");
    tree.append_child(root, child).expect("append child");

    run_standalone_rust(tree, root, Constraints::definite(112.0, 48.0))
        .expect("horizontal linear cross-axis end overflow with root padding/border parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_vertical_linear_cross_axis_center_overflow_with_root_padding_border_and_percent_margins()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Vertical)
        .expect("set root linear orientation");
    tree.set_align_items(root, AlignItems::Center)
        .expect("set root align items");
    tree.set_width(root, Length::points(46.0))
        .expect("set root width");
    tree.set_height(root, Length::points(120.0))
        .expect("set root height");
    tree.set_padding(root, StandaloneEdge::Left, Length::points(4.0))
        .expect("set root left padding");
    tree.set_padding(root, StandaloneEdge::Right, Length::points(5.0))
        .expect("set root right padding");
    tree.set_border(root, StandaloneEdge::Left, 1.0)
        .expect("set root left border");
    tree.set_border(root, StandaloneEdge::Right, 2.0)
        .expect("set root right border");

    let child = tree.create_default_node();
    tree.set_display(child, Display::Block)
        .expect("set child display");
    tree.set_box_sizing(child, BoxSizing::ContentBox)
        .expect("set child box sizing");
    tree.set_width(child, Length::points(66.0))
        .expect("set child width");
    tree.set_height(child, Length::points(24.0))
        .expect("set child height");
    tree.set_margin(child, StandaloneEdge::Left, Length::percent(7.0))
        .expect("set child left margin");
    tree.set_margin(child, StandaloneEdge::Right, Length::percent(5.0))
        .expect("set child right margin");
    tree.append_child(root, child).expect("append child");

    run_standalone_rust(tree, root, Constraints::definite(58.0, 120.0))
        .expect("vertical linear cross-axis center overflow with root padding/border parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_rtl_vertical_linear_cross_axis_end_overflow_with_root_padding_border_and_calc_margins()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_direction(root, Direction::Rtl)
        .expect("set root direction");
    tree.set_linear_orientation(root, LinearOrientation::Vertical)
        .expect("set root linear orientation");
    tree.set_align_items(root, AlignItems::FlexEnd)
        .expect("set root align items");
    tree.set_width(root, Length::points(48.0))
        .expect("set root width");
    tree.set_height(root, Length::points(118.0))
        .expect("set root height");
    tree.set_padding(root, StandaloneEdge::Horizontal, Length::points(4.0))
        .expect("set root horizontal padding");
    tree.set_border(root, StandaloneEdge::Horizontal, 1.0)
        .expect("set root horizontal border");

    let child = tree.create_default_node();
    tree.set_display(child, Display::Block)
        .expect("set child display");
    tree.set_box_sizing(child, BoxSizing::ContentBox)
        .expect("set child box sizing");
    tree.set_width(child, Length::points(68.0))
        .expect("set child width");
    tree.set_height(child, Length::points(22.0))
        .expect("set child height");
    tree.set_margin(child, StandaloneEdge::Left, Length::calc(3.0, 6.0))
        .expect("set child left margin");
    tree.set_margin(child, StandaloneEdge::Right, Length::calc(5.0, 4.0))
        .expect("set child right margin");
    tree.append_child(root, child).expect("append child");

    run_standalone_rust(tree, root, Constraints::definite(58.0, 118.0))
        .expect("RTL vertical linear cross-axis end overflow with root padding/border parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_horizontal_reverse_linear_layout_gravity_bottom_cross_overflow_with_percent_margins()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::HorizontalReverse)
        .expect("set root linear orientation");
    tree.set_align_items(root, AlignItems::FlexStart)
        .expect("set root align items");
    tree.set_width(root, Length::points(108.0))
        .expect("set root width");
    tree.set_height(root, Length::points(44.0))
        .expect("set root height");

    let child = tree.create_default_node();
    tree.set_display(child, Display::Block)
        .expect("set child display");
    tree.set_box_sizing(child, BoxSizing::ContentBox)
        .expect("set child box sizing");
    tree.set_width(child, Length::points(28.0))
        .expect("set child width");
    tree.set_height(child, Length::points(62.0))
        .expect("set child height");
    tree.set_linear_layout_gravity(child, LinearLayoutGravity::Bottom)
        .expect("set child layout gravity");
    tree.set_margin(child, StandaloneEdge::Top, Length::percent(6.0))
        .expect("set child top margin");
    tree.set_margin(child, StandaloneEdge::Bottom, Length::percent(4.0))
        .expect("set child bottom margin");
    tree.append_child(root, child).expect("append child");

    run_standalone_rust(tree, root, Constraints::definite(108.0, 44.0)).expect(
        "horizontal-reverse linear layout-gravity bottom cross overflow with percent margins parity",
    );
}

#[test]
fn standalone_owned_tree_matches_cpp_for_horizontal_linear_cross_auto_margin_overflow_with_root_padding_border()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root linear orientation");
    tree.set_align_items(root, AlignItems::FlexEnd)
        .expect("set root align items");
    tree.set_width(root, Length::points(106.0))
        .expect("set root width");
    tree.set_height(root, Length::points(38.0))
        .expect("set root height");
    tree.set_padding(root, StandaloneEdge::Vertical, Length::points(4.0))
        .expect("set root vertical padding");
    tree.set_border(root, StandaloneEdge::Vertical, 1.0)
        .expect("set root vertical border");

    let child = tree.create_default_node();
    tree.set_display(child, Display::Block)
        .expect("set child display");
    tree.set_box_sizing(child, BoxSizing::ContentBox)
        .expect("set child box sizing");
    tree.set_width(child, Length::points(24.0))
        .expect("set child width");
    tree.set_height(child, Length::points(58.0))
        .expect("set child height");
    tree.set_margin(child, StandaloneEdge::Top, Length::Auto)
        .expect("set child top auto margin");
    tree.set_margin(child, StandaloneEdge::Bottom, Length::points(5.0))
        .expect("set child bottom margin");
    tree.append_child(root, child).expect("append child");

    run_standalone_rust(tree, root, Constraints::definite(106.0, 48.0))
        .expect("horizontal linear cross auto-margin overflow with root padding/border parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_horizontal_linear_cross_gravity_center_with_calc_margins()
{
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root linear orientation");
    tree.set_align_items(root, AlignItems::FlexEnd)
        .expect("set root align items");
    tree.set_linear_cross_gravity(root, LinearCrossGravity::Center)
        .expect("set root cross gravity");
    tree.set_width(root, Length::points(120.0))
        .expect("set root width");
    tree.set_height(root, Length::points(90.0))
        .expect("set root height");

    let child = tree.create_default_node();
    tree.set_display(child, Display::Block)
        .expect("set child display");
    tree.set_box_sizing(child, BoxSizing::ContentBox)
        .expect("set child box sizing");
    tree.set_width(child, Length::points(22.0))
        .expect("set child width");
    tree.set_height(child, Length::points(16.0))
        .expect("set child height");
    tree.set_margin(child, StandaloneEdge::Top, Length::calc(2.0, 15.0))
        .expect("set child top margin");
    tree.set_margin(child, StandaloneEdge::Bottom, Length::calc(4.0, 6.0))
        .expect("set child bottom margin");
    tree.append_child(root, child).expect("append child");

    run_standalone_rust(tree, root, Constraints::definite(120.0, 90.0))
        .expect("horizontal linear cross-gravity center with calc margins parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_vertical_linear_cross_axis_center_with_calc_margins() {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Vertical)
        .expect("set root linear orientation");
    tree.set_align_items(root, AlignItems::Center)
        .expect("set root align items");
    tree.set_width(root, Length::points(140.0))
        .expect("set root width");
    tree.set_height(root, Length::points(100.0))
        .expect("set root height");

    let child = tree.create_default_node();
    tree.set_display(child, Display::Block)
        .expect("set child display");
    tree.set_box_sizing(child, BoxSizing::ContentBox)
        .expect("set child box sizing");
    tree.set_width(child, Length::points(24.0))
        .expect("set child width");
    tree.set_height(child, Length::points(14.0))
        .expect("set child height");
    tree.set_margin(child, StandaloneEdge::Left, Length::calc(7.0, 5.0))
        .expect("set child left margin");
    tree.set_margin(child, StandaloneEdge::Right, Length::calc(3.0, 10.0))
        .expect("set child right margin");
    tree.append_child(root, child).expect("append child");

    run_standalone_rust(tree, root, Constraints::definite(140.0, 100.0))
        .expect("vertical linear cross-axis center with calc margins parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_vertical_linear_cross_gravity_end_with_calc_margins() {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Vertical)
        .expect("set root linear orientation");
    tree.set_align_items(root, AlignItems::FlexStart)
        .expect("set root align items");
    tree.set_linear_cross_gravity(root, LinearCrossGravity::End)
        .expect("set root cross gravity");
    tree.set_width(root, Length::points(140.0))
        .expect("set root width");
    tree.set_height(root, Length::points(100.0))
        .expect("set root height");

    let child = tree.create_default_node();
    tree.set_display(child, Display::Block)
        .expect("set child display");
    tree.set_box_sizing(child, BoxSizing::ContentBox)
        .expect("set child box sizing");
    tree.set_width(child, Length::points(26.0))
        .expect("set child width");
    tree.set_height(child, Length::points(12.0))
        .expect("set child height");
    tree.set_margin(child, StandaloneEdge::Left, Length::calc(5.0, 8.0))
        .expect("set child left margin");
    tree.set_margin(child, StandaloneEdge::Right, Length::calc(9.0, 4.0))
        .expect("set child right margin");
    tree.append_child(root, child).expect("append child");

    run_standalone_rust(tree, root, Constraints::definite(140.0, 100.0))
        .expect("vertical linear cross-gravity end with calc margins parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_rtl_vertical_linear_layout_gravity_left_with_calc_margins()
{
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_direction(root, Direction::Rtl)
        .expect("set root direction");
    tree.set_linear_orientation(root, LinearOrientation::Vertical)
        .expect("set root linear orientation");
    tree.set_align_items(root, AlignItems::FlexStart)
        .expect("set root align items");
    tree.set_width(root, Length::points(140.0))
        .expect("set root width");
    tree.set_height(root, Length::points(100.0))
        .expect("set root height");

    let child = tree.create_default_node();
    tree.set_display(child, Display::Block)
        .expect("set child display");
    tree.set_box_sizing(child, BoxSizing::ContentBox)
        .expect("set child box sizing");
    tree.set_linear_layout_gravity(child, LinearLayoutGravity::Left)
        .expect("set child layout gravity");
    tree.set_width(child, Length::points(28.0))
        .expect("set child width");
    tree.set_height(child, Length::points(16.0))
        .expect("set child height");
    tree.set_margin(child, StandaloneEdge::Left, Length::calc(6.0, 6.0))
        .expect("set child left margin");
    tree.set_margin(child, StandaloneEdge::Right, Length::calc(4.0, 9.0))
        .expect("set child right margin");
    tree.append_child(root, child).expect("append child");

    run_standalone_rust(tree, root, Constraints::definite(140.0, 100.0))
        .expect("RTL vertical linear layout-gravity left with calc margins parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_horizontal_linear_layout_gravity_top_with_calc_cross_margins()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root orientation");
    tree.set_align_items(root, AlignItems::FlexEnd)
        .expect("set root align items");
    tree.set_width(root, Length::points(130.0))
        .expect("set root width");
    tree.set_height(root, Length::points(90.0))
        .expect("set root height");

    let child = tree.create_default_node();
    tree.set_display(child, Display::Block)
        .expect("set child display");
    tree.set_box_sizing(child, BoxSizing::ContentBox)
        .expect("set child box sizing");
    tree.set_linear_layout_gravity(child, LinearLayoutGravity::Top)
        .expect("set child layout gravity");
    tree.set_width(child, Length::points(24.0))
        .expect("set child width");
    tree.set_height(child, Length::points(12.0))
        .expect("set child height");
    tree.set_margin(child, StandaloneEdge::Top, Length::calc(3.0, 8.0))
        .expect("set child top margin");
    tree.set_margin(child, StandaloneEdge::Bottom, Length::calc(5.0, 4.0))
        .expect("set child bottom margin");
    tree.append_child(root, child).expect("append child");

    run_standalone_rust(tree, root, Constraints::definite(130.0, 90.0))
        .expect("horizontal linear layout-gravity top with calc cross margins parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_horizontal_linear_layout_gravity_bottom_with_percent_cross_margins()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root orientation");
    tree.set_align_items(root, AlignItems::FlexStart)
        .expect("set root align items");
    tree.set_width(root, Length::points(130.0))
        .expect("set root width");
    tree.set_height(root, Length::points(90.0))
        .expect("set root height");

    let child = tree.create_default_node();
    tree.set_display(child, Display::Block)
        .expect("set child display");
    tree.set_linear_layout_gravity(child, LinearLayoutGravity::Bottom)
        .expect("set child layout gravity");
    tree.set_width(child, Length::points(24.0))
        .expect("set child width");
    tree.set_height(child, Length::points(12.0))
        .expect("set child height");
    tree.set_margin(child, StandaloneEdge::Top, Length::percent(6.0))
        .expect("set child top margin");
    tree.set_margin(child, StandaloneEdge::Bottom, Length::percent(4.0))
        .expect("set child bottom margin");
    tree.append_child(root, child).expect("append child");

    run_standalone_rust(tree, root, Constraints::definite(130.0, 90.0))
        .expect("horizontal linear layout-gravity bottom with percent cross margins parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_horizontal_linear_layout_gravity_bottom_with_top_auto_cross_margin()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root orientation");
    tree.set_align_items(root, AlignItems::FlexStart)
        .expect("set root align items");
    tree.set_width(root, Length::points(130.0))
        .expect("set root width");
    tree.set_height(root, Length::points(90.0))
        .expect("set root height");

    let child = tree.create_default_node();
    tree.set_display(child, Display::Block)
        .expect("set child display");
    tree.set_box_sizing(child, BoxSizing::ContentBox)
        .expect("set child box sizing");
    tree.set_linear_layout_gravity(child, LinearLayoutGravity::Bottom)
        .expect("set child layout gravity");
    tree.set_width(child, Length::points(24.0))
        .expect("set child width");
    tree.set_height(child, Length::points(12.0))
        .expect("set child height");
    tree.set_margin(child, StandaloneEdge::Top, Length::Auto)
        .expect("set child top auto margin");
    tree.set_margin(child, StandaloneEdge::Bottom, Length::calc(5.0, 5.0))
        .expect("set child bottom margin");
    tree.append_child(root, child).expect("append child");

    run_standalone_rust(tree, root, Constraints::definite(130.0, 90.0))
        .expect("horizontal linear layout-gravity bottom with top auto cross margin parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_vertical_linear_layout_gravity_left_with_calc_cross_margins()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Vertical)
        .expect("set root orientation");
    tree.set_align_items(root, AlignItems::FlexEnd)
        .expect("set root align items");
    tree.set_width(root, Length::points(140.0))
        .expect("set root width");
    tree.set_height(root, Length::points(100.0))
        .expect("set root height");

    let child = tree.create_default_node();
    tree.set_display(child, Display::Block)
        .expect("set child display");
    tree.set_box_sizing(child, BoxSizing::ContentBox)
        .expect("set child box sizing");
    tree.set_linear_layout_gravity(child, LinearLayoutGravity::Left)
        .expect("set child layout gravity");
    tree.set_width(child, Length::points(28.0))
        .expect("set child width");
    tree.set_height(child, Length::points(14.0))
        .expect("set child height");
    tree.set_margin(child, StandaloneEdge::Left, Length::calc(4.0, 7.0))
        .expect("set child left margin");
    tree.set_margin(child, StandaloneEdge::Right, Length::calc(6.0, 3.0))
        .expect("set child right margin");
    tree.append_child(root, child).expect("append child");

    run_standalone_rust(tree, root, Constraints::definite(140.0, 100.0))
        .expect("vertical linear layout-gravity left with calc cross margins parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_vertical_linear_layout_gravity_right_with_percent_cross_margins()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_linear_orientation(root, LinearOrientation::Vertical)
        .expect("set root orientation");
    tree.set_align_items(root, AlignItems::FlexStart)
        .expect("set root align items");
    tree.set_width(root, Length::points(140.0))
        .expect("set root width");
    tree.set_height(root, Length::points(100.0))
        .expect("set root height");

    let child = tree.create_default_node();
    tree.set_display(child, Display::Block)
        .expect("set child display");
    tree.set_linear_layout_gravity(child, LinearLayoutGravity::Right)
        .expect("set child layout gravity");
    tree.set_width(child, Length::points(28.0))
        .expect("set child width");
    tree.set_height(child, Length::points(14.0))
        .expect("set child height");
    tree.set_margin(child, StandaloneEdge::Left, Length::percent(5.0))
        .expect("set child left margin");
    tree.set_margin(child, StandaloneEdge::Right, Length::percent(7.0))
        .expect("set child right margin");
    tree.append_child(root, child).expect("append child");

    run_standalone_rust(tree, root, Constraints::definite(140.0, 100.0))
        .expect("vertical linear layout-gravity right with percent cross margins parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_rtl_vertical_linear_layout_gravity_right_with_right_auto_cross_margin()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_direction(root, Direction::Rtl)
        .expect("set root direction");
    tree.set_linear_orientation(root, LinearOrientation::Vertical)
        .expect("set root orientation");
    tree.set_align_items(root, AlignItems::FlexStart)
        .expect("set root align items");
    tree.set_width(root, Length::points(140.0))
        .expect("set root width");
    tree.set_height(root, Length::points(100.0))
        .expect("set root height");

    let child = tree.create_default_node();
    tree.set_display(child, Display::Block)
        .expect("set child display");
    tree.set_box_sizing(child, BoxSizing::ContentBox)
        .expect("set child box sizing");
    tree.set_linear_layout_gravity(child, LinearLayoutGravity::Right)
        .expect("set child layout gravity");
    tree.set_width(child, Length::points(28.0))
        .expect("set child width");
    tree.set_height(child, Length::points(14.0))
        .expect("set child height");
    tree.set_margin(child, StandaloneEdge::Left, Length::calc(4.0, 5.0))
        .expect("set child left margin");
    tree.set_margin(child, StandaloneEdge::Right, Length::Auto)
        .expect("set child right auto margin");
    tree.append_child(root, child).expect("append child");

    run_standalone_rust(tree, root, Constraints::definite(140.0, 100.0))
        .expect("RTL vertical linear layout-gravity right with right auto cross margin parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_empty_container_baseline_matrix() {
    for (case_name, tree, root, constraints) in [
        horizontal_linear_empty_container_baseline_tree(),
        vertical_linear_empty_container_baseline_tree(),
    ] {
        run_standalone_rust(tree, root, constraints)
            .unwrap_or_else(|error| panic!("{case_name} parity failed: {error}"));
    }
}

#[test]
fn standalone_owned_tree_matches_cpp_for_horizontal_linear_largest_child_baseline() {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root linear orientation");
    tree.set_width(root, Length::points(100.0))
        .expect("set root width");
    tree.set_height(root, Length::points(40.0))
        .expect("set root height");

    let first = tree.create_default_measured_node(Size::new(10.0, 30.0));
    tree.set_display(first, Display::Block)
        .expect("set first display");
    tree.set_box_sizing(first, BoxSizing::ContentBox)
        .expect("set first box sizing");
    tree.set_baseline(first, Some(5.0))
        .expect("set first baseline");

    let second = tree.create_default_measured_node(Size::new(10.0, 20.0));
    tree.set_display(second, Display::Block)
        .expect("set second display");
    tree.set_box_sizing(second, BoxSizing::ContentBox)
        .expect("set second box sizing");
    tree.set_baseline(second, Some(15.0))
        .expect("set second baseline");

    tree.append_child(root, first).expect("append first");
    tree.append_child(root, second).expect("append second");

    run_standalone_rust(tree, root, Constraints::definite(100.0, 40.0))
        .expect("horizontal linear largest child baseline parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_vertical_linear_default_gravity_baseline() {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_width(root, Length::points(20.0))
        .expect("set root width");
    tree.set_height(root, Length::points(100.0))
        .expect("set root height");

    let first = tree.create_default_measured_node(Size::new(10.0, 20.0));
    tree.set_display(first, Display::Block)
        .expect("set first display");
    tree.set_box_sizing(first, BoxSizing::ContentBox)
        .expect("set first box sizing");
    tree.set_baseline(first, Some(5.0))
        .expect("set first baseline");

    let second = tree.create_default_measured_node(Size::new(10.0, 10.0));
    tree.set_display(second, Display::Block)
        .expect("set second display");
    tree.set_box_sizing(second, BoxSizing::ContentBox)
        .expect("set second box sizing");

    tree.append_child(root, first).expect("append first");
    tree.append_child(root, second).expect("append second");

    run_standalone_rust(tree, root, Constraints::definite(20.0, 100.0))
        .expect("vertical linear default gravity baseline parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_vertical_linear_center_gravity_baseline() {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_justify_content(root, JustifyContent::Center)
        .expect("set root justify content");
    tree.set_width(root, Length::points(20.0))
        .expect("set root width");
    tree.set_height(root, Length::points(100.0))
        .expect("set root height");

    let first = tree.create_default_measured_node(Size::new(10.0, 80.0));
    tree.set_display(first, Display::Block)
        .expect("set first display");
    tree.set_box_sizing(first, BoxSizing::ContentBox)
        .expect("set first box sizing");
    tree.set_baseline(first, Some(10.0))
        .expect("set first baseline");

    let second = tree.create_default_measured_node(Size::new(10.0, 70.0));
    tree.set_display(second, Display::Block)
        .expect("set second display");
    tree.set_box_sizing(second, BoxSizing::ContentBox)
        .expect("set second box sizing");

    tree.append_child(root, first).expect("append first");
    tree.append_child(root, second).expect("append second");

    run_standalone_rust(tree, root, Constraints::definite(20.0, 100.0))
        .expect("vertical linear center gravity baseline parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_vertical_linear_end_gravity_baseline() {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_gravity(root, LinearGravity::End)
        .expect("set root linear gravity");
    tree.set_width(root, Length::points(20.0))
        .expect("set root width");
    tree.set_height(root, Length::points(100.0))
        .expect("set root height");

    let first = tree.create_default_measured_node(Size::new(10.0, 20.0));
    tree.set_display(first, Display::Block)
        .expect("set first display");
    tree.set_box_sizing(first, BoxSizing::ContentBox)
        .expect("set first box sizing");
    tree.set_baseline(first, Some(5.0))
        .expect("set first baseline");

    let second = tree.create_default_measured_node(Size::new(10.0, 10.0));
    tree.set_display(second, Display::Block)
        .expect("set second display");
    tree.set_box_sizing(second, BoxSizing::ContentBox)
        .expect("set second box sizing");

    tree.append_child(root, first).expect("append first");
    tree.append_child(root, second).expect("append second");

    run_standalone_rust(tree, root, Constraints::definite(20.0, 100.0))
        .expect("vertical linear end gravity baseline parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_horizontal_linear_center_baseline_with_fixed_cross_margins()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root linear orientation");
    tree.set_align_items(root, AlignItems::Center)
        .expect("set root align items");
    tree.set_width(root, Length::points(120.0))
        .expect("set root width");
    tree.set_height(root, Length::points(80.0))
        .expect("set root height");

    let first = tree.create_default_measured_node(Size::new(20.0, 12.0));
    tree.set_display(first, Display::Block)
        .expect("set first display");
    tree.set_box_sizing(first, BoxSizing::ContentBox)
        .expect("set first box sizing");
    tree.set_baseline(first, Some(4.0))
        .expect("set first baseline");
    tree.set_margin(first, StandaloneEdge::Top, Length::points(3.0))
        .expect("set first top margin");
    tree.set_margin(first, StandaloneEdge::Bottom, Length::points(5.0))
        .expect("set first bottom margin");

    let second = tree.create_default_measured_node(Size::new(18.0, 18.0));
    tree.set_display(second, Display::Block)
        .expect("set second display");
    tree.set_box_sizing(second, BoxSizing::ContentBox)
        .expect("set second box sizing");
    tree.set_baseline(second, Some(9.0))
        .expect("set second baseline");
    tree.set_margin(second, StandaloneEdge::Top, Length::points(1.0))
        .expect("set second top margin");
    tree.set_margin(second, StandaloneEdge::Bottom, Length::points(2.0))
        .expect("set second bottom margin");

    tree.append_child(root, first).expect("append first");
    tree.append_child(root, second).expect("append second");

    run_standalone_rust(tree, root, Constraints::definite(120.0, 80.0))
        .expect("horizontal linear center baseline with fixed cross margins parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_horizontal_linear_end_baseline_with_percent_cross_margins()
{
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root linear orientation");
    tree.set_align_items(root, AlignItems::FlexEnd)
        .expect("set root align items");
    tree.set_width(root, Length::points(120.0))
        .expect("set root width");
    tree.set_height(root, Length::points(90.0))
        .expect("set root height");

    let child = tree.create_default_measured_node(Size::new(24.0, 16.0));
    tree.set_display(child, Display::Block)
        .expect("set child display");
    tree.set_box_sizing(child, BoxSizing::ContentBox)
        .expect("set child box sizing");
    tree.set_baseline(child, Some(7.0))
        .expect("set child baseline");
    tree.set_margin(child, StandaloneEdge::Top, Length::percent(10.0))
        .expect("set child top margin");
    tree.set_margin(child, StandaloneEdge::Bottom, Length::percent(5.0))
        .expect("set child bottom margin");
    tree.append_child(root, child).expect("append child");

    run_standalone_rust(tree, root, Constraints::definite(120.0, 90.0))
        .expect("horizontal linear end baseline with percent cross margins parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_horizontal_reverse_linear_center_baseline() {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::HorizontalReverse)
        .expect("set root linear orientation");
    tree.set_align_items(root, AlignItems::Center)
        .expect("set root align items");
    tree.set_width(root, Length::points(120.0))
        .expect("set root width");
    tree.set_height(root, Length::points(70.0))
        .expect("set root height");

    let first = tree.create_default_measured_node(Size::new(20.0, 14.0));
    tree.set_display(first, Display::Block)
        .expect("set first display");
    tree.set_box_sizing(first, BoxSizing::ContentBox)
        .expect("set first box sizing");
    tree.set_baseline(first, Some(6.0))
        .expect("set first baseline");

    let second = tree.create_default_measured_node(Size::new(18.0, 20.0));
    tree.set_display(second, Display::Block)
        .expect("set second display");
    tree.set_box_sizing(second, BoxSizing::ContentBox)
        .expect("set second box sizing");
    tree.set_baseline(second, Some(8.0))
        .expect("set second baseline");

    tree.append_child(root, first).expect("append first");
    tree.append_child(root, second).expect("append second");

    run_standalone_rust(tree, root, Constraints::definite(120.0, 70.0))
        .expect("horizontal-reverse linear center baseline parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_vertical_linear_space_between_baseline_uses_first_item_start()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Vertical)
        .expect("set root linear orientation");
    tree.set_linear_gravity(root, LinearGravity::SpaceBetween)
        .expect("set root linear gravity");
    tree.set_width(root, Length::points(60.0))
        .expect("set root width");
    tree.set_height(root, Length::points(120.0))
        .expect("set root height");

    let first = tree.create_default_measured_node(Size::new(20.0, 18.0));
    tree.set_display(first, Display::Block)
        .expect("set first display");
    tree.set_box_sizing(first, BoxSizing::ContentBox)
        .expect("set first box sizing");
    tree.set_baseline(first, Some(5.0))
        .expect("set first baseline");
    tree.set_margin(first, StandaloneEdge::Top, Length::points(4.0))
        .expect("set first top margin");
    tree.set_margin(first, StandaloneEdge::Bottom, Length::points(3.0))
        .expect("set first bottom margin");

    let second = tree.create_default_measured_node(Size::new(20.0, 16.0));
    tree.set_display(second, Display::Block)
        .expect("set second display");
    tree.set_box_sizing(second, BoxSizing::ContentBox)
        .expect("set second box sizing");
    tree.set_baseline(second, Some(12.0))
        .expect("set second baseline");

    tree.append_child(root, first).expect("append first");
    tree.append_child(root, second).expect("append second");

    run_standalone_rust(tree, root, Constraints::definite(60.0, 120.0))
        .expect("vertical linear space-between baseline parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_vertical_reverse_linear_center_baseline() {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::VerticalReverse)
        .expect("set root linear orientation");
    tree.set_linear_gravity(root, LinearGravity::Center)
        .expect("set root linear gravity");
    tree.set_width(root, Length::points(60.0))
        .expect("set root width");
    tree.set_height(root, Length::points(120.0))
        .expect("set root height");

    let first = tree.create_default_measured_node(Size::new(20.0, 18.0));
    tree.set_display(first, Display::Block)
        .expect("set first display");
    tree.set_box_sizing(first, BoxSizing::ContentBox)
        .expect("set first box sizing");
    tree.set_baseline(first, Some(6.0))
        .expect("set first baseline");

    let second = tree.create_default_measured_node(Size::new(20.0, 14.0));
    tree.set_display(second, Display::Block)
        .expect("set second display");
    tree.set_box_sizing(second, BoxSizing::ContentBox)
        .expect("set second box sizing");
    tree.set_baseline(second, Some(10.0))
        .expect("set second baseline");

    tree.append_child(root, first).expect("append first");
    tree.append_child(root, second).expect("append second");

    run_standalone_rust(tree, root, Constraints::definite(60.0, 120.0))
        .expect("vertical-reverse linear center baseline parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_vertical_linear_end_baseline_with_main_margins() {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Vertical)
        .expect("set root linear orientation");
    tree.set_linear_gravity(root, LinearGravity::End)
        .expect("set root linear gravity");
    tree.set_width(root, Length::points(60.0))
        .expect("set root width");
    tree.set_height(root, Length::points(120.0))
        .expect("set root height");

    let first = tree.create_default_measured_node(Size::new(20.0, 18.0));
    tree.set_display(first, Display::Block)
        .expect("set first display");
    tree.set_box_sizing(first, BoxSizing::ContentBox)
        .expect("set first box sizing");
    tree.set_baseline(first, Some(5.0))
        .expect("set first baseline");
    tree.set_margin(first, StandaloneEdge::Top, Length::points(4.0))
        .expect("set first top margin");
    tree.set_margin(first, StandaloneEdge::Bottom, Length::points(8.0))
        .expect("set first bottom margin");

    let second = tree.create_default_measured_node(Size::new(20.0, 12.0));
    tree.set_display(second, Display::Block)
        .expect("set second display");
    tree.set_box_sizing(second, BoxSizing::ContentBox)
        .expect("set second box sizing");
    tree.set_baseline(second, Some(9.0))
        .expect("set second baseline");

    tree.append_child(root, first).expect("append first");
    tree.append_child(root, second).expect("append second");

    run_standalone_rust(tree, root, Constraints::definite(60.0, 120.0))
        .expect("vertical linear end baseline with main margins parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_horizontal_linear_center_baseline_with_calc_cross_margins()
{
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root linear orientation");
    tree.set_align_items(root, AlignItems::Center)
        .expect("set root align items");
    tree.set_width(root, Length::points(140.0))
        .expect("set root width");
    tree.set_height(root, Length::points(90.0))
        .expect("set root height");

    let first = tree.create_default_measured_node(Size::new(22.0, 14.0));
    tree.set_display(first, Display::Block)
        .expect("set first display");
    tree.set_box_sizing(first, BoxSizing::ContentBox)
        .expect("set first box sizing");
    tree.set_baseline(first, Some(5.0))
        .expect("set first baseline");
    tree.set_margin(first, StandaloneEdge::Top, Length::calc(3.0, 10.0))
        .expect("set first top margin");
    tree.set_margin(first, StandaloneEdge::Bottom, Length::calc(4.0, 5.0))
        .expect("set first bottom margin");

    let second = tree.create_default_measured_node(Size::new(18.0, 20.0));
    tree.set_display(second, Display::Block)
        .expect("set second display");
    tree.set_box_sizing(second, BoxSizing::ContentBox)
        .expect("set second box sizing");
    tree.set_baseline(second, Some(8.0))
        .expect("set second baseline");
    tree.set_margin(second, StandaloneEdge::Top, Length::calc(1.0, 6.0))
        .expect("set second top margin");
    tree.set_margin(second, StandaloneEdge::Bottom, Length::calc(2.0, 4.0))
        .expect("set second bottom margin");

    tree.append_child(root, first).expect("append first");
    tree.append_child(root, second).expect("append second");

    run_standalone_rust(tree, root, Constraints::definite(140.0, 90.0))
        .expect("horizontal linear center baseline with calc cross margins parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_horizontal_linear_end_baseline_with_calc_cross_margins() {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root linear orientation");
    tree.set_align_items(root, AlignItems::FlexEnd)
        .expect("set root align items");
    tree.set_width(root, Length::points(140.0))
        .expect("set root width");
    tree.set_height(root, Length::points(90.0))
        .expect("set root height");

    let child = tree.create_default_measured_node(Size::new(24.0, 16.0));
    tree.set_display(child, Display::Block)
        .expect("set child display");
    tree.set_box_sizing(child, BoxSizing::ContentBox)
        .expect("set child box sizing");
    tree.set_baseline(child, Some(7.0))
        .expect("set child baseline");
    tree.set_margin(child, StandaloneEdge::Top, Length::calc(5.0, 8.0))
        .expect("set child top margin");
    tree.set_margin(child, StandaloneEdge::Bottom, Length::calc(3.0, 12.0))
        .expect("set child bottom margin");
    tree.append_child(root, child).expect("append child");

    run_standalone_rust(tree, root, Constraints::definite(140.0, 90.0))
        .expect("horizontal linear end baseline with calc cross margins parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_horizontal_linear_cross_gravity_center_baseline_with_calc_cross_margins()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root linear orientation");
    tree.set_align_items(root, AlignItems::FlexEnd)
        .expect("set root align items");
    tree.set_linear_cross_gravity(root, LinearCrossGravity::Center)
        .expect("set root cross gravity");
    tree.set_width(root, Length::points(140.0))
        .expect("set root width");
    tree.set_height(root, Length::points(90.0))
        .expect("set root height");

    let child = tree.create_default_measured_node(Size::new(26.0, 18.0));
    tree.set_display(child, Display::Block)
        .expect("set child display");
    tree.set_box_sizing(child, BoxSizing::ContentBox)
        .expect("set child box sizing");
    tree.set_baseline(child, Some(9.0))
        .expect("set child baseline");
    tree.set_margin(child, StandaloneEdge::Top, Length::calc(2.0, 15.0))
        .expect("set child top margin");
    tree.set_margin(child, StandaloneEdge::Bottom, Length::calc(4.0, 6.0))
        .expect("set child bottom margin");
    tree.append_child(root, child).expect("append child");

    run_standalone_rust(tree, root, Constraints::definite(140.0, 90.0))
        .expect("horizontal linear cross-gravity center baseline with calc cross margins parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_horizontal_linear_layout_gravity_end_baseline_with_calc_cross_margins()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root linear orientation");
    tree.set_align_items(root, AlignItems::Center)
        .expect("set root align items");
    tree.set_linear_cross_gravity(root, LinearCrossGravity::Center)
        .expect("set root cross gravity");
    tree.set_width(root, Length::points(140.0))
        .expect("set root width");
    tree.set_height(root, Length::points(90.0))
        .expect("set root height");

    let child = tree.create_default_measured_node(Size::new(28.0, 16.0));
    tree.set_display(child, Display::Block)
        .expect("set child display");
    tree.set_box_sizing(child, BoxSizing::ContentBox)
        .expect("set child box sizing");
    tree.set_linear_layout_gravity(child, LinearLayoutGravity::End)
        .expect("set child layout gravity");
    tree.set_baseline(child, Some(6.0))
        .expect("set child baseline");
    tree.set_margin(child, StandaloneEdge::Top, Length::calc(4.0, 7.0))
        .expect("set child top margin");
    tree.set_margin(child, StandaloneEdge::Bottom, Length::calc(5.0, 9.0))
        .expect("set child bottom margin");
    tree.append_child(root, child).expect("append child");

    run_standalone_rust(tree, root, Constraints::definite(140.0, 90.0))
        .expect("horizontal linear layout-gravity end baseline with calc cross margins parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_vertical_linear_center_baseline_with_calc_main_margins() {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Vertical)
        .expect("set root linear orientation");
    tree.set_linear_gravity(root, LinearGravity::Center)
        .expect("set root linear gravity");
    tree.set_width(root, Length::points(70.0))
        .expect("set root width");
    tree.set_height(root, Length::points(150.0))
        .expect("set root height");

    let first = tree.create_default_measured_node(Size::new(20.0, 18.0));
    tree.set_display(first, Display::Block)
        .expect("set first display");
    tree.set_box_sizing(first, BoxSizing::ContentBox)
        .expect("set first box sizing");
    tree.set_baseline(first, Some(5.0))
        .expect("set first baseline");
    tree.set_margin(first, StandaloneEdge::Top, Length::calc(4.0, 10.0))
        .expect("set first top margin");
    tree.set_margin(first, StandaloneEdge::Bottom, Length::calc(2.0, 5.0))
        .expect("set first bottom margin");

    let second = tree.create_default_measured_node(Size::new(20.0, 16.0));
    tree.set_display(second, Display::Block)
        .expect("set second display");
    tree.set_box_sizing(second, BoxSizing::ContentBox)
        .expect("set second box sizing");
    tree.set_baseline(second, Some(11.0))
        .expect("set second baseline");
    tree.append_child(root, first).expect("append first");
    tree.append_child(root, second).expect("append second");

    run_standalone_rust(tree, root, Constraints::definite(70.0, 150.0))
        .expect("vertical linear center baseline with calc main margins parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_vertical_linear_end_baseline_with_calc_main_margins() {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Vertical)
        .expect("set root linear orientation");
    tree.set_linear_gravity(root, LinearGravity::End)
        .expect("set root linear gravity");
    tree.set_width(root, Length::points(70.0))
        .expect("set root width");
    tree.set_height(root, Length::points(150.0))
        .expect("set root height");

    let first = tree.create_default_measured_node(Size::new(20.0, 18.0));
    tree.set_display(first, Display::Block)
        .expect("set first display");
    tree.set_box_sizing(first, BoxSizing::ContentBox)
        .expect("set first box sizing");
    tree.set_baseline(first, Some(6.0))
        .expect("set first baseline");
    tree.set_margin(first, StandaloneEdge::Top, Length::calc(3.0, 8.0))
        .expect("set first top margin");
    tree.set_margin(first, StandaloneEdge::Bottom, Length::calc(5.0, 6.0))
        .expect("set first bottom margin");

    let second = tree.create_default_measured_node(Size::new(20.0, 12.0));
    tree.set_display(second, Display::Block)
        .expect("set second display");
    tree.set_box_sizing(second, BoxSizing::ContentBox)
        .expect("set second box sizing");
    tree.set_baseline(second, Some(9.0))
        .expect("set second baseline");
    tree.append_child(root, first).expect("append first");
    tree.append_child(root, second).expect("append second");

    run_standalone_rust(tree, root, Constraints::definite(70.0, 150.0))
        .expect("vertical linear end baseline with calc main margins parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_horizontal_linear_layout_gravity_top_baseline_with_calc_cross_margins()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root orientation");
    tree.set_align_items(root, AlignItems::FlexEnd)
        .expect("set root align items");
    tree.set_width(root, Length::points(150.0))
        .expect("set root width");
    tree.set_height(root, Length::points(100.0))
        .expect("set root height");

    let child = tree.create_default_measured_node(Size::new(26.0, 18.0));
    tree.set_display(child, Display::Block)
        .expect("set child display");
    tree.set_box_sizing(child, BoxSizing::ContentBox)
        .expect("set child box sizing");
    tree.set_linear_layout_gravity(child, LinearLayoutGravity::Top)
        .expect("set child layout gravity");
    tree.set_baseline(child, Some(7.0))
        .expect("set child baseline");
    tree.set_margin(child, StandaloneEdge::Top, Length::calc(3.0, 8.0))
        .expect("set child top margin");
    tree.set_margin(child, StandaloneEdge::Bottom, Length::calc(4.0, 5.0))
        .expect("set child bottom margin");
    tree.append_child(root, child).expect("append child");

    run_standalone_rust(tree, root, Constraints::definite(150.0, 100.0))
        .expect("horizontal linear layout-gravity top baseline with calc cross margins parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_horizontal_linear_layout_gravity_bottom_baseline_with_percent_cross_margins()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root orientation");
    tree.set_align_items(root, AlignItems::FlexStart)
        .expect("set root align items");
    tree.set_width(root, Length::points(150.0))
        .expect("set root width");
    tree.set_height(root, Length::points(100.0))
        .expect("set root height");

    let child = tree.create_default_measured_node(Size::new(28.0, 16.0));
    tree.set_display(child, Display::Block)
        .expect("set child display");
    tree.set_linear_layout_gravity(child, LinearLayoutGravity::Bottom)
        .expect("set child layout gravity");
    tree.set_baseline(child, Some(6.0))
        .expect("set child baseline");
    tree.set_margin(child, StandaloneEdge::Top, Length::percent(8.0))
        .expect("set child top margin");
    tree.set_margin(child, StandaloneEdge::Bottom, Length::percent(4.0))
        .expect("set child bottom margin");
    tree.append_child(root, child).expect("append child");

    run_standalone_rust(tree, root, Constraints::definite(150.0, 100.0)).expect(
        "horizontal linear layout-gravity bottom baseline with percent cross margins parity",
    );
}

#[test]
fn standalone_owned_tree_matches_cpp_for_horizontal_linear_layout_gravity_center_vertical_baseline_with_calc_cross_margins()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root orientation");
    tree.set_align_items(root, AlignItems::FlexEnd)
        .expect("set root align items");
    tree.set_width(root, Length::points(150.0))
        .expect("set root width");
    tree.set_height(root, Length::points(100.0))
        .expect("set root height");

    let child = tree.create_default_measured_node(Size::new(30.0, 20.0));
    tree.set_display(child, Display::Block)
        .expect("set child display");
    tree.set_box_sizing(child, BoxSizing::ContentBox)
        .expect("set child box sizing");
    tree.set_linear_layout_gravity(child, LinearLayoutGravity::CenterVertical)
        .expect("set child layout gravity");
    tree.set_baseline(child, Some(9.0))
        .expect("set child baseline");
    tree.set_margin(child, StandaloneEdge::Top, Length::calc(5.0, 7.0))
        .expect("set child top margin");
    tree.set_margin(child, StandaloneEdge::Bottom, Length::calc(2.0, 6.0))
        .expect("set child bottom margin");
    tree.append_child(root, child).expect("append child");

    run_standalone_rust(tree, root, Constraints::definite(150.0, 100.0)).expect(
        "horizontal linear layout-gravity center-vertical baseline with calc cross margins parity",
    );
}

#[test]
fn standalone_owned_tree_matches_cpp_for_vertical_reverse_linear_end_baseline_with_calc_main_margins()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::VerticalReverse)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::End)
        .expect("set root linear gravity");
    tree.set_width(root, Length::points(80.0))
        .expect("set root width");
    tree.set_height(root, Length::points(160.0))
        .expect("set root height");

    let first = tree.create_default_measured_node(Size::new(24.0, 20.0));
    tree.set_display(first, Display::Block)
        .expect("set first display");
    tree.set_box_sizing(first, BoxSizing::ContentBox)
        .expect("set first box sizing");
    tree.set_baseline(first, Some(7.0))
        .expect("set first baseline");
    tree.set_margin(first, StandaloneEdge::Top, Length::calc(4.0, 9.0))
        .expect("set first top margin");
    tree.set_margin(first, StandaloneEdge::Bottom, Length::calc(2.0, 6.0))
        .expect("set first bottom margin");

    let second = tree.create_default_measured_node(Size::new(20.0, 16.0));
    tree.set_display(second, Display::Block)
        .expect("set second display");
    tree.set_box_sizing(second, BoxSizing::ContentBox)
        .expect("set second box sizing");
    tree.set_baseline(second, Some(10.0))
        .expect("set second baseline");

    tree.append_child(root, first).expect("append first");
    tree.append_child(root, second).expect("append second");

    run_standalone_rust(tree, root, Constraints::definite(80.0, 160.0))
        .expect("vertical-reverse linear end baseline with calc main margins parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_vertical_reverse_linear_top_gravity_baseline_with_percent_main_margins()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_linear_orientation(root, LinearOrientation::VerticalReverse)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::Top)
        .expect("set root linear gravity");
    tree.set_width(root, Length::points(80.0))
        .expect("set root width");
    tree.set_height(root, Length::points(160.0))
        .expect("set root height");

    let first = tree.create_default_measured_node(Size::new(24.0, 22.0));
    tree.set_display(first, Display::Block)
        .expect("set first display");
    tree.set_baseline(first, Some(8.0))
        .expect("set first baseline");
    tree.set_margin(first, StandaloneEdge::Top, Length::percent(5.0))
        .expect("set first top margin");
    tree.set_margin(first, StandaloneEdge::Bottom, Length::percent(3.0))
        .expect("set first bottom margin");

    let second = tree.create_default_measured_node(Size::new(20.0, 18.0));
    tree.set_display(second, Display::Block)
        .expect("set second display");
    tree.set_baseline(second, Some(11.0))
        .expect("set second baseline");

    tree.append_child(root, first).expect("append first");
    tree.append_child(root, second).expect("append second");

    run_standalone_rust(tree, root, Constraints::definite(80.0, 160.0))
        .expect("vertical-reverse linear top gravity baseline with percent main margins parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_vertical_reverse_linear_bottom_gravity_baseline_with_calc_main_margins()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::VerticalReverse)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::Bottom)
        .expect("set root linear gravity");
    tree.set_width(root, Length::points(80.0))
        .expect("set root width");
    tree.set_height(root, Length::points(160.0))
        .expect("set root height");

    let first = tree.create_default_measured_node(Size::new(24.0, 20.0));
    tree.set_display(first, Display::Block)
        .expect("set first display");
    tree.set_box_sizing(first, BoxSizing::ContentBox)
        .expect("set first box sizing");
    tree.set_baseline(first, Some(6.0))
        .expect("set first baseline");
    tree.set_margin(first, StandaloneEdge::Top, Length::calc(3.0, 8.0))
        .expect("set first top margin");
    tree.set_margin(first, StandaloneEdge::Bottom, Length::calc(5.0, 4.0))
        .expect("set first bottom margin");

    let second = tree.create_default_measured_node(Size::new(20.0, 14.0));
    tree.set_display(second, Display::Block)
        .expect("set second display");
    tree.set_box_sizing(second, BoxSizing::ContentBox)
        .expect("set second box sizing");
    tree.set_baseline(second, Some(9.0))
        .expect("set second baseline");

    tree.append_child(root, first).expect("append first");
    tree.append_child(root, second).expect("append second");

    run_standalone_rust(tree, root, Constraints::definite(80.0, 160.0))
        .expect("vertical-reverse linear bottom gravity baseline with calc main margins parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_horizontal_linear_align_self_center_baseline_precedes_cross_gravity_end()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root orientation");
    tree.set_align_items(root, AlignItems::FlexStart)
        .expect("set root align items");
    tree.set_linear_cross_gravity(root, LinearCrossGravity::End)
        .expect("set root cross gravity");
    tree.set_width(root, Length::points(150.0))
        .expect("set root width");
    tree.set_height(root, Length::points(100.0))
        .expect("set root height");

    let child = tree.create_default_measured_node(Size::new(30.0, 18.0));
    tree.set_display(child, Display::Block)
        .expect("set child display");
    tree.set_box_sizing(child, BoxSizing::ContentBox)
        .expect("set child box sizing");
    tree.set_align_self(child, Some(AlignItems::Center))
        .expect("set child align self");
    tree.set_baseline(child, Some(7.0))
        .expect("set child baseline");
    tree.set_margin(child, StandaloneEdge::Top, Length::calc(3.0, 8.0))
        .expect("set child top margin");
    tree.set_margin(child, StandaloneEdge::Bottom, Length::calc(5.0, 4.0))
        .expect("set child bottom margin");
    tree.append_child(root, child).expect("append child");

    run_standalone_rust(tree, root, Constraints::definite(150.0, 100.0))
        .expect("horizontal linear align-self center baseline precedence parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_horizontal_linear_align_self_stretch_baseline_precedes_cross_gravity_end()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root orientation");
    tree.set_align_items(root, AlignItems::FlexStart)
        .expect("set root align items");
    tree.set_linear_cross_gravity(root, LinearCrossGravity::End)
        .expect("set root cross gravity");
    tree.set_width(root, Length::points(150.0))
        .expect("set root width");
    tree.set_height(root, Length::points(100.0))
        .expect("set root height");

    let child = tree.create_default_measured_node(Size::new(28.0, 16.0));
    tree.set_display(child, Display::Block)
        .expect("set child display");
    tree.set_box_sizing(child, BoxSizing::ContentBox)
        .expect("set child box sizing");
    tree.set_align_self(child, Some(AlignItems::Stretch))
        .expect("set child align self");
    tree.set_baseline(child, Some(8.0))
        .expect("set child baseline");
    tree.set_margin(child, StandaloneEdge::Top, Length::points(4.0))
        .expect("set child top margin");
    tree.set_margin(child, StandaloneEdge::Bottom, Length::points(6.0))
        .expect("set child bottom margin");
    tree.append_child(root, child).expect("append child");

    run_standalone_rust(tree, root, Constraints::definite(150.0, 100.0))
        .expect("horizontal linear align-self stretch baseline precedence parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_horizontal_linear_layout_gravity_top_baseline_precedes_align_self_end_and_cross_gravity_center()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root orientation");
    tree.set_align_items(root, AlignItems::FlexStart)
        .expect("set root align items");
    tree.set_linear_cross_gravity(root, LinearCrossGravity::Center)
        .expect("set root cross gravity");
    tree.set_width(root, Length::points(150.0))
        .expect("set root width");
    tree.set_height(root, Length::points(100.0))
        .expect("set root height");

    let child = tree.create_default_measured_node(Size::new(32.0, 20.0));
    tree.set_display(child, Display::Block)
        .expect("set child display");
    tree.set_box_sizing(child, BoxSizing::ContentBox)
        .expect("set child box sizing");
    tree.set_align_self(child, Some(AlignItems::FlexEnd))
        .expect("set child align self");
    tree.set_linear_layout_gravity(child, LinearLayoutGravity::Top)
        .expect("set child layout gravity");
    tree.set_baseline(child, Some(9.0))
        .expect("set child baseline");
    tree.set_margin(child, StandaloneEdge::Top, Length::calc(4.0, 7.0))
        .expect("set child top margin");
    tree.set_margin(child, StandaloneEdge::Bottom, Length::calc(2.0, 6.0))
        .expect("set child bottom margin");
    tree.append_child(root, child).expect("append child");

    run_standalone_rust(tree, root, Constraints::definite(150.0, 100.0))
        .expect("horizontal linear layout-gravity top baseline precedence parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_horizontal_linear_layout_gravity_bottom_baseline_precedes_align_self_center_and_cross_gravity_start()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root orientation");
    tree.set_align_items(root, AlignItems::FlexStart)
        .expect("set root align items");
    tree.set_linear_cross_gravity(root, LinearCrossGravity::Start)
        .expect("set root cross gravity");
    tree.set_width(root, Length::points(150.0))
        .expect("set root width");
    tree.set_height(root, Length::points(100.0))
        .expect("set root height");

    let child = tree.create_default_measured_node(Size::new(30.0, 18.0));
    tree.set_display(child, Display::Block)
        .expect("set child display");
    tree.set_box_sizing(child, BoxSizing::ContentBox)
        .expect("set child box sizing");
    tree.set_align_self(child, Some(AlignItems::Center))
        .expect("set child align self");
    tree.set_linear_layout_gravity(child, LinearLayoutGravity::Bottom)
        .expect("set child layout gravity");
    tree.set_baseline(child, Some(6.0))
        .expect("set child baseline");
    tree.set_margin(child, StandaloneEdge::Top, Length::percent(6.0))
        .expect("set child top margin");
    tree.set_margin(child, StandaloneEdge::Bottom, Length::percent(4.0))
        .expect("set child bottom margin");
    tree.append_child(root, child).expect("append child");

    run_standalone_rust(tree, root, Constraints::definite(150.0, 100.0))
        .expect("horizontal linear layout-gravity bottom baseline precedence parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_vertical_linear_gravity_end_baseline_overrides_justify_content_center()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Vertical)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::End)
        .expect("set root gravity");
    tree.set_justify_content(root, JustifyContent::Center)
        .expect("set root justify content");
    tree.set_width(root, Length::points(80.0))
        .expect("set root width");
    tree.set_height(root, Length::points(160.0))
        .expect("set root height");

    let first = tree.create_default_measured_node(Size::new(24.0, 22.0));
    tree.set_display(first, Display::Block)
        .expect("set first display");
    tree.set_box_sizing(first, BoxSizing::ContentBox)
        .expect("set first box sizing");
    tree.set_baseline(first, Some(7.0))
        .expect("set first baseline");
    tree.set_margin(first, StandaloneEdge::Top, Length::calc(3.0, 8.0))
        .expect("set first top margin");
    tree.set_margin(first, StandaloneEdge::Bottom, Length::calc(5.0, 6.0))
        .expect("set first bottom margin");

    let second = tree.create_default_measured_node(Size::new(20.0, 16.0));
    tree.set_display(second, Display::Block)
        .expect("set second display");
    tree.set_box_sizing(second, BoxSizing::ContentBox)
        .expect("set second box sizing");
    tree.set_baseline(second, Some(10.0))
        .expect("set second baseline");

    tree.append_child(root, first).expect("append first");
    tree.append_child(root, second).expect("append second");

    run_standalone_rust(tree, root, Constraints::definite(80.0, 160.0))
        .expect("vertical linear gravity end overrides justify-content center baseline parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_vertical_linear_justify_content_flex_end_baseline_without_linear_gravity()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Vertical)
        .expect("set root orientation");
    tree.set_justify_content(root, JustifyContent::FlexEnd)
        .expect("set root justify content");
    tree.set_width(root, Length::points(80.0))
        .expect("set root width");
    tree.set_height(root, Length::points(160.0))
        .expect("set root height");

    let first = tree.create_default_measured_node(Size::new(24.0, 20.0));
    tree.set_display(first, Display::Block)
        .expect("set first display");
    tree.set_box_sizing(first, BoxSizing::ContentBox)
        .expect("set first box sizing");
    tree.set_baseline(first, Some(6.0))
        .expect("set first baseline");
    tree.set_margin(first, StandaloneEdge::Top, Length::percent(5.0))
        .expect("set first top margin");
    tree.set_margin(first, StandaloneEdge::Bottom, Length::percent(4.0))
        .expect("set first bottom margin");

    let second = tree.create_default_measured_node(Size::new(20.0, 14.0));
    tree.set_display(second, Display::Block)
        .expect("set second display");
    tree.set_box_sizing(second, BoxSizing::ContentBox)
        .expect("set second box sizing");
    tree.set_baseline(second, Some(9.0))
        .expect("set second baseline");

    tree.append_child(root, first).expect("append first");
    tree.append_child(root, second).expect("append second");

    run_standalone_rust(tree, root, Constraints::definite(80.0, 160.0))
        .expect("vertical linear justify-content flex-end baseline parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_horizontal_reverse_linear_end_baseline_with_percent_cross_margins()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::HorizontalReverse)
        .expect("set root orientation");
    tree.set_align_items(root, AlignItems::FlexEnd)
        .expect("set root align items");
    tree.set_width(root, Length::points(130.0))
        .expect("set root width");
    tree.set_height(root, Length::points(90.0))
        .expect("set root height");

    let child = tree.create_default_measured_node(Size::new(22.0, 18.0));
    tree.set_display(child, Display::Block)
        .expect("set child display");
    tree.set_box_sizing(child, BoxSizing::ContentBox)
        .expect("set child box sizing");
    tree.set_baseline(child, Some(7.0))
        .expect("set child baseline");
    tree.set_margin(child, StandaloneEdge::Top, Length::percent(6.0))
        .expect("set child top margin");
    tree.set_margin(child, StandaloneEdge::Bottom, Length::percent(4.0))
        .expect("set child bottom margin");
    tree.append_child(root, child).expect("append child");

    run_standalone_rust(tree, root, Constraints::definite(130.0, 90.0))
        .expect("horizontal-reverse linear end baseline with percent cross margins parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_horizontal_reverse_linear_layout_gravity_bottom_baseline_with_calc_cross_margins()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::HorizontalReverse)
        .expect("set root orientation");
    tree.set_align_items(root, AlignItems::FlexStart)
        .expect("set root align items");
    tree.set_width(root, Length::points(132.0))
        .expect("set root width");
    tree.set_height(root, Length::points(92.0))
        .expect("set root height");

    let child = tree.create_default_measured_node(Size::new(24.0, 16.0));
    tree.set_display(child, Display::Block)
        .expect("set child display");
    tree.set_box_sizing(child, BoxSizing::ContentBox)
        .expect("set child box sizing");
    tree.set_linear_layout_gravity(child, LinearLayoutGravity::Bottom)
        .expect("set child layout gravity");
    tree.set_baseline(child, Some(6.0))
        .expect("set child baseline");
    tree.set_margin(child, StandaloneEdge::Top, Length::calc(2.0, 5.0))
        .expect("set child top margin");
    tree.set_margin(child, StandaloneEdge::Bottom, Length::calc(3.0, 4.0))
        .expect("set child bottom margin");
    tree.append_child(root, child).expect("append child");

    run_standalone_rust(tree, root, Constraints::definite(132.0, 92.0))
        .expect("horizontal-reverse linear bottom layout-gravity baseline parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_horizontal_reverse_linear_align_self_center_baseline_precedes_cross_gravity_end()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::HorizontalReverse)
        .expect("set root orientation");
    tree.set_align_items(root, AlignItems::FlexStart)
        .expect("set root align items");
    tree.set_linear_cross_gravity(root, LinearCrossGravity::End)
        .expect("set root cross gravity");
    tree.set_width(root, Length::points(128.0))
        .expect("set root width");
    tree.set_height(root, Length::points(88.0))
        .expect("set root height");

    let child = tree.create_default_measured_node(Size::new(20.0, 18.0));
    tree.set_display(child, Display::Block)
        .expect("set child display");
    tree.set_box_sizing(child, BoxSizing::ContentBox)
        .expect("set child box sizing");
    tree.set_align_self(child, Some(AlignItems::Center))
        .expect("set child align-self");
    tree.set_baseline(child, Some(8.0))
        .expect("set child baseline");
    tree.set_margin(child, StandaloneEdge::Top, Length::calc(1.0, 4.0))
        .expect("set child top margin");
    tree.set_margin(child, StandaloneEdge::Bottom, Length::calc(2.0, 3.0))
        .expect("set child bottom margin");
    tree.append_child(root, child).expect("append child");

    run_standalone_rust(tree, root, Constraints::definite(128.0, 88.0))
        .expect("horizontal-reverse linear align-self center baseline precedence parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_vertical_reverse_linear_justify_content_flex_end_baseline_without_linear_gravity()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::VerticalReverse)
        .expect("set root orientation");
    tree.set_justify_content(root, JustifyContent::FlexEnd)
        .expect("set root justify content");
    tree.set_width(root, Length::points(84.0))
        .expect("set root width");
    tree.set_height(root, Length::points(166.0))
        .expect("set root height");

    let first = tree.create_default_measured_node(Size::new(24.0, 18.0));
    tree.set_display(first, Display::Block)
        .expect("set first display");
    tree.set_box_sizing(first, BoxSizing::ContentBox)
        .expect("set first box sizing");
    tree.set_baseline(first, Some(7.0))
        .expect("set first baseline");
    tree.set_margin(first, StandaloneEdge::Top, Length::percent(4.0))
        .expect("set first top margin");
    tree.set_margin(first, StandaloneEdge::Bottom, Length::percent(6.0))
        .expect("set first bottom margin");

    let second = tree.create_default_measured_node(Size::new(20.0, 14.0));
    tree.set_display(second, Display::Block)
        .expect("set second display");
    tree.set_box_sizing(second, BoxSizing::ContentBox)
        .expect("set second box sizing");
    tree.set_baseline(second, Some(10.0))
        .expect("set second baseline");

    tree.append_child(root, first).expect("append first");
    tree.append_child(root, second).expect("append second");

    run_standalone_rust(tree, root, Constraints::definite(84.0, 166.0))
        .expect("vertical-reverse linear justify-content flex-end baseline parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_vertical_reverse_linear_justify_content_center_baseline_with_calc_main_margins()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::VerticalReverse)
        .expect("set root orientation");
    tree.set_justify_content(root, JustifyContent::Center)
        .expect("set root justify content");
    tree.set_width(root, Length::points(86.0))
        .expect("set root width");
    tree.set_height(root, Length::points(170.0))
        .expect("set root height");

    let first = tree.create_default_measured_node(Size::new(25.0, 19.0));
    tree.set_display(first, Display::Block)
        .expect("set first display");
    tree.set_box_sizing(first, BoxSizing::ContentBox)
        .expect("set first box sizing");
    tree.set_baseline(first, Some(6.0))
        .expect("set first baseline");
    tree.set_margin(first, StandaloneEdge::Top, Length::calc(2.0, 4.0))
        .expect("set first top margin");
    tree.set_margin(first, StandaloneEdge::Bottom, Length::calc(3.0, 5.0))
        .expect("set first bottom margin");

    let second = tree.create_default_measured_node(Size::new(21.0, 13.0));
    tree.set_display(second, Display::Block)
        .expect("set second display");
    tree.set_box_sizing(second, BoxSizing::ContentBox)
        .expect("set second box sizing");
    tree.set_baseline(second, Some(9.0))
        .expect("set second baseline");

    tree.append_child(root, first).expect("append first");
    tree.append_child(root, second).expect("append second");

    run_standalone_rust(tree, root, Constraints::definite(86.0, 170.0))
        .expect("vertical-reverse linear justify-content center baseline parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_vertical_reverse_linear_justify_content_space_around_baseline_falls_back_to_start()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::VerticalReverse)
        .expect("set root orientation");
    tree.set_justify_content(root, JustifyContent::SpaceAround)
        .expect("set root justify content");
    tree.set_width(root, Length::points(82.0))
        .expect("set root width");
    tree.set_height(root, Length::points(164.0))
        .expect("set root height");

    let first = tree.create_default_measured_node(Size::new(23.0, 17.0));
    tree.set_display(first, Display::Block)
        .expect("set first display");
    tree.set_box_sizing(first, BoxSizing::ContentBox)
        .expect("set first box sizing");
    tree.set_baseline(first, Some(8.0))
        .expect("set first baseline");
    tree.set_margin(first, StandaloneEdge::Top, Length::percent(5.0))
        .expect("set first top margin");
    tree.set_margin(first, StandaloneEdge::Bottom, Length::percent(3.0))
        .expect("set first bottom margin");

    let second = tree.create_default_measured_node(Size::new(19.0, 15.0));
    tree.set_display(second, Display::Block)
        .expect("set second display");
    tree.set_box_sizing(second, BoxSizing::ContentBox)
        .expect("set second box sizing");
    tree.set_baseline(second, Some(11.0))
        .expect("set second baseline");

    tree.append_child(root, first).expect("append first");
    tree.append_child(root, second).expect("append second");

    run_standalone_rust(tree, root, Constraints::definite(82.0, 164.0))
        .expect("vertical-reverse linear justify-content space-around baseline fallback parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_horizontal_linear_align_items_end_baseline_with_top_auto_cross_margin()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root orientation");
    tree.set_align_items(root, AlignItems::FlexEnd)
        .expect("set root align items");
    tree.set_width(root, Length::points(126.0))
        .expect("set root width");
    tree.set_height(root, Length::points(92.0))
        .expect("set root height");

    let child = tree.create_default_measured_node(Size::new(26.0, 18.0));
    tree.set_display(child, Display::Block)
        .expect("set child display");
    tree.set_box_sizing(child, BoxSizing::ContentBox)
        .expect("set child box sizing");
    tree.set_baseline(child, Some(7.0))
        .expect("set child baseline");
    tree.set_margin(child, StandaloneEdge::Top, Length::Auto)
        .expect("set child top auto margin");
    tree.set_margin(child, StandaloneEdge::Bottom, Length::percent(5.0))
        .expect("set child bottom margin");
    tree.append_child(root, child).expect("append child");

    run_standalone_rust(tree, root, Constraints::definite(126.0, 92.0))
        .expect("horizontal linear align-items end baseline with top-auto margin parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_horizontal_linear_align_self_end_baseline_with_bottom_auto_cross_margin()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root orientation");
    tree.set_align_items(root, AlignItems::FlexStart)
        .expect("set root align items");
    tree.set_linear_cross_gravity(root, LinearCrossGravity::Center)
        .expect("set root cross gravity");
    tree.set_width(root, Length::points(128.0))
        .expect("set root width");
    tree.set_height(root, Length::points(94.0))
        .expect("set root height");

    let child = tree.create_default_measured_node(Size::new(24.0, 16.0));
    tree.set_display(child, Display::Block)
        .expect("set child display");
    tree.set_box_sizing(child, BoxSizing::ContentBox)
        .expect("set child box sizing");
    tree.set_align_self(child, Some(AlignItems::FlexEnd))
        .expect("set child align self");
    tree.set_baseline(child, Some(6.0))
        .expect("set child baseline");
    tree.set_margin(child, StandaloneEdge::Top, Length::calc(3.0, 5.0))
        .expect("set child top margin");
    tree.set_margin(child, StandaloneEdge::Bottom, Length::Auto)
        .expect("set child bottom auto margin");
    tree.append_child(root, child).expect("append child");

    run_standalone_rust(tree, root, Constraints::definite(128.0, 94.0))
        .expect("horizontal linear align-self end baseline with bottom-auto margin parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_horizontal_linear_layout_gravity_center_vertical_baseline_with_paired_auto_cross_margins()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root orientation");
    tree.set_align_items(root, AlignItems::FlexEnd)
        .expect("set root align items");
    tree.set_linear_cross_gravity(root, LinearCrossGravity::End)
        .expect("set root cross gravity");
    tree.set_width(root, Length::points(130.0))
        .expect("set root width");
    tree.set_height(root, Length::points(96.0))
        .expect("set root height");

    let child = tree.create_default_measured_node(Size::new(28.0, 14.0));
    tree.set_display(child, Display::Block)
        .expect("set child display");
    tree.set_box_sizing(child, BoxSizing::ContentBox)
        .expect("set child box sizing");
    tree.set_linear_layout_gravity(child, LinearLayoutGravity::CenterVertical)
        .expect("set child layout gravity");
    tree.set_baseline(child, Some(5.0))
        .expect("set child baseline");
    tree.set_margin(child, StandaloneEdge::Top, Length::Auto)
        .expect("set child top auto margin");
    tree.set_margin(child, StandaloneEdge::Bottom, Length::Auto)
        .expect("set child bottom auto margin");
    tree.append_child(root, child).expect("append child");

    run_standalone_rust(tree, root, Constraints::definite(130.0, 96.0)).expect(
        "horizontal linear layout-gravity center-vertical baseline with auto margins parity",
    );
}

#[test]
fn standalone_owned_tree_matches_cpp_for_rtl_horizontal_linear_layout_gravity_left_baseline_flips_to_after()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_direction(root, Direction::Rtl)
        .expect("set root direction");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root orientation");
    tree.set_align_items(root, AlignItems::FlexStart)
        .expect("set root align items");
    tree.set_width(root, Length::points(132.0))
        .expect("set root width");
    tree.set_height(root, Length::points(98.0))
        .expect("set root height");

    let child = tree.create_default_measured_node(Size::new(30.0, 18.0));
    tree.set_display(child, Display::Block)
        .expect("set child display");
    tree.set_box_sizing(child, BoxSizing::ContentBox)
        .expect("set child box sizing");
    tree.set_linear_layout_gravity(child, LinearLayoutGravity::Left)
        .expect("set child layout gravity");
    tree.set_baseline(child, Some(8.0))
        .expect("set child baseline");
    tree.set_margin(child, StandaloneEdge::Top, Length::calc(2.0, 4.0))
        .expect("set child top margin");
    tree.set_margin(child, StandaloneEdge::Bottom, Length::calc(3.0, 5.0))
        .expect("set child bottom margin");
    tree.append_child(root, child).expect("append child");

    run_standalone_rust(tree, root, Constraints::definite(132.0, 98.0))
        .expect("RTL horizontal linear left layout-gravity baseline flip parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_rtl_horizontal_linear_layout_gravity_right_baseline_flips_to_start()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_direction(root, Direction::Rtl)
        .expect("set root direction");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root orientation");
    tree.set_align_items(root, AlignItems::FlexEnd)
        .expect("set root align items");
    tree.set_width(root, Length::points(132.0))
        .expect("set root width");
    tree.set_height(root, Length::points(98.0))
        .expect("set root height");

    let child = tree.create_default_measured_node(Size::new(30.0, 18.0));
    tree.set_display(child, Display::Block)
        .expect("set child display");
    tree.set_box_sizing(child, BoxSizing::ContentBox)
        .expect("set child box sizing");
    tree.set_linear_layout_gravity(child, LinearLayoutGravity::Right)
        .expect("set child layout gravity");
    tree.set_baseline(child, Some(8.0))
        .expect("set child baseline");
    tree.set_margin(child, StandaloneEdge::Top, Length::percent(6.0))
        .expect("set child top margin");
    tree.set_margin(child, StandaloneEdge::Bottom, Length::percent(4.0))
        .expect("set child bottom margin");
    tree.append_child(root, child).expect("append child");

    run_standalone_rust(tree, root, Constraints::definite(132.0, 98.0))
        .expect("RTL horizontal linear right layout-gravity baseline flip parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_vertical_reverse_linear_justify_content_end_baseline_without_linear_gravity()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::VerticalReverse)
        .expect("set root orientation");
    tree.set_justify_content(root, JustifyContent::End)
        .expect("set root justify content");
    tree.set_width(root, Length::points(88.0))
        .expect("set root width");
    tree.set_height(root, Length::points(176.0))
        .expect("set root height");

    let first = tree.create_default_measured_node(Size::new(26.0, 20.0));
    tree.set_display(first, Display::Block)
        .expect("set first display");
    tree.set_box_sizing(first, BoxSizing::ContentBox)
        .expect("set first box sizing");
    tree.set_baseline(first, Some(7.0))
        .expect("set first baseline");
    tree.set_margin(first, StandaloneEdge::Top, Length::calc(3.0, 4.0))
        .expect("set first top margin");
    tree.set_margin(first, StandaloneEdge::Bottom, Length::calc(2.0, 6.0))
        .expect("set first bottom margin");

    let second = tree.create_default_measured_node(Size::new(20.0, 16.0));
    tree.set_display(second, Display::Block)
        .expect("set second display");
    tree.set_box_sizing(second, BoxSizing::ContentBox)
        .expect("set second box sizing");
    tree.set_baseline(second, Some(10.0))
        .expect("set second baseline");

    tree.append_child(root, first).expect("append first");
    tree.append_child(root, second).expect("append second");

    run_standalone_rust(tree, root, Constraints::definite(88.0, 176.0))
        .expect("vertical-reverse linear justify-content end baseline parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_horizontal_linear_center_baseline_with_root_padding_border_and_percent_cross_margins()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root orientation");
    tree.set_align_items(root, AlignItems::Center)
        .expect("set root align items");
    tree.set_width(root, Length::points(160.0))
        .expect("set root width");
    tree.set_height(root, Length::points(110.0))
        .expect("set root height");
    tree.set_padding(root, StandaloneEdge::Top, Length::points(5.0))
        .expect("set root top padding");
    tree.set_padding(root, StandaloneEdge::Bottom, Length::points(7.0))
        .expect("set root bottom padding");
    tree.set_border(root, StandaloneEdge::Top, 1.0)
        .expect("set root top border");
    tree.set_border(root, StandaloneEdge::Bottom, 2.0)
        .expect("set root bottom border");

    let first = tree.create_default_measured_node(Size::new(24.0, 18.0));
    tree.set_display(first, Display::Block)
        .expect("set first display");
    tree.set_box_sizing(first, BoxSizing::ContentBox)
        .expect("set first box sizing");
    tree.set_baseline(first, Some(7.0))
        .expect("set first baseline");
    tree.set_margin(first, StandaloneEdge::Top, Length::percent(6.0))
        .expect("set first top margin");
    tree.set_margin(first, StandaloneEdge::Bottom, Length::percent(4.0))
        .expect("set first bottom margin");

    let second = tree.create_default_measured_node(Size::new(22.0, 26.0));
    tree.set_display(second, Display::Block)
        .expect("set second display");
    tree.set_box_sizing(second, BoxSizing::ContentBox)
        .expect("set second box sizing");
    tree.set_baseline(second, Some(15.0))
        .expect("set second baseline");
    tree.set_margin(second, StandaloneEdge::Top, Length::points(3.0))
        .expect("set second top margin");
    tree.set_margin(second, StandaloneEdge::Bottom, Length::percent(5.0))
        .expect("set second bottom margin");

    tree.append_child(root, first).expect("append first");
    tree.append_child(root, second).expect("append second");

    run_standalone_rust(tree, root, Constraints::definite(160.0, 110.0)).expect(
        "horizontal linear center baseline with root padding/border and percent margins parity",
    );
}

#[test]
fn standalone_owned_tree_matches_cpp_for_horizontal_linear_layout_gravity_bottom_baseline_with_root_padding_border_and_calc_cross_margins()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root orientation");
    tree.set_align_items(root, AlignItems::FlexStart)
        .expect("set root align items");
    tree.set_width(root, Length::points(158.0))
        .expect("set root width");
    tree.set_height(root, Length::points(108.0))
        .expect("set root height");
    tree.set_padding(root, StandaloneEdge::Vertical, Length::points(6.0))
        .expect("set root vertical padding");
    tree.set_border(root, StandaloneEdge::Vertical, 2.0)
        .expect("set root vertical border");

    let child = tree.create_default_measured_node(Size::new(28.0, 20.0));
    tree.set_display(child, Display::Block)
        .expect("set child display");
    tree.set_box_sizing(child, BoxSizing::ContentBox)
        .expect("set child box sizing");
    tree.set_linear_layout_gravity(child, LinearLayoutGravity::Bottom)
        .expect("set child layout gravity");
    tree.set_baseline(child, Some(8.0))
        .expect("set child baseline");
    tree.set_margin(child, StandaloneEdge::Top, Length::calc(4.0, 6.0))
        .expect("set child top margin");
    tree.set_margin(child, StandaloneEdge::Bottom, Length::calc(2.0, 8.0))
        .expect("set child bottom margin");
    tree.append_child(root, child).expect("append child");

    run_standalone_rust(tree, root, Constraints::definite(158.0, 108.0))
        .expect("horizontal linear bottom layout-gravity baseline with root padding/border parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_rtl_horizontal_reverse_linear_layout_gravity_right_baseline_with_root_padding_border()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_direction(root, Direction::Rtl)
        .expect("set root direction");
    tree.set_linear_orientation(root, LinearOrientation::HorizontalReverse)
        .expect("set root orientation");
    tree.set_align_items(root, AlignItems::FlexStart)
        .expect("set root align items");
    tree.set_width(root, Length::points(150.0))
        .expect("set root width");
    tree.set_height(root, Length::points(104.0))
        .expect("set root height");
    tree.set_padding(root, StandaloneEdge::Top, Length::points(4.0))
        .expect("set root top padding");
    tree.set_padding(root, StandaloneEdge::Bottom, Length::points(8.0))
        .expect("set root bottom padding");
    tree.set_border(root, StandaloneEdge::Top, 1.0)
        .expect("set root top border");
    tree.set_border(root, StandaloneEdge::Bottom, 3.0)
        .expect("set root bottom border");

    let child = tree.create_default_measured_node(Size::new(30.0, 18.0));
    tree.set_display(child, Display::Block)
        .expect("set child display");
    tree.set_box_sizing(child, BoxSizing::ContentBox)
        .expect("set child box sizing");
    tree.set_linear_layout_gravity(child, LinearLayoutGravity::Right)
        .expect("set child layout gravity");
    tree.set_baseline(child, Some(9.0))
        .expect("set child baseline");
    tree.set_margin(child, StandaloneEdge::Top, Length::percent(5.0))
        .expect("set child top margin");
    tree.set_margin(child, StandaloneEdge::Bottom, Length::calc(3.0, 4.0))
        .expect("set child bottom margin");
    tree.append_child(root, child).expect("append child");

    run_standalone_rust(tree, root, Constraints::definite(150.0, 104.0)).expect(
        "RTL horizontal-reverse linear right layout-gravity baseline with root padding/border parity",
    );
}

#[test]
fn standalone_owned_tree_matches_cpp_for_vertical_linear_center_baseline_with_root_padding_border_and_percent_main_margins()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Vertical)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::Center)
        .expect("set root linear gravity");
    tree.set_width(root, Length::points(90.0))
        .expect("set root width");
    tree.set_height(root, Length::points(180.0))
        .expect("set root height");
    tree.set_padding(root, StandaloneEdge::Top, Length::points(7.0))
        .expect("set root top padding");
    tree.set_padding(root, StandaloneEdge::Bottom, Length::points(5.0))
        .expect("set root bottom padding");
    tree.set_border(root, StandaloneEdge::Top, 2.0)
        .expect("set root top border");
    tree.set_border(root, StandaloneEdge::Bottom, 1.0)
        .expect("set root bottom border");

    let first = tree.create_default_measured_node(Size::new(26.0, 24.0));
    tree.set_display(first, Display::Block)
        .expect("set first display");
    tree.set_box_sizing(first, BoxSizing::ContentBox)
        .expect("set first box sizing");
    tree.set_baseline(first, Some(8.0))
        .expect("set first baseline");
    tree.set_margin(first, StandaloneEdge::Top, Length::percent(4.0))
        .expect("set first top margin");
    tree.set_margin(first, StandaloneEdge::Bottom, Length::percent(6.0))
        .expect("set first bottom margin");

    let second = tree.create_default_measured_node(Size::new(22.0, 18.0));
    tree.set_display(second, Display::Block)
        .expect("set second display");
    tree.set_box_sizing(second, BoxSizing::ContentBox)
        .expect("set second box sizing");
    tree.set_baseline(second, Some(11.0))
        .expect("set second baseline");

    tree.append_child(root, first).expect("append first");
    tree.append_child(root, second).expect("append second");

    run_standalone_rust(tree, root, Constraints::definite(90.0, 180.0)).expect(
        "vertical linear center baseline with root padding/border and percent main margins parity",
    );
}

#[test]
fn standalone_owned_tree_matches_cpp_for_vertical_reverse_linear_bottom_gravity_baseline_with_root_padding_border_and_calc_main_margins()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::VerticalReverse)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::Bottom)
        .expect("set root linear gravity");
    tree.set_width(root, Length::points(92.0))
        .expect("set root width");
    tree.set_height(root, Length::points(182.0))
        .expect("set root height");
    tree.set_padding(root, StandaloneEdge::Vertical, Length::points(6.0))
        .expect("set root vertical padding");
    tree.set_border(root, StandaloneEdge::Vertical, 2.0)
        .expect("set root vertical border");

    let first = tree.create_default_measured_node(Size::new(28.0, 22.0));
    tree.set_display(first, Display::Block)
        .expect("set first display");
    tree.set_box_sizing(first, BoxSizing::ContentBox)
        .expect("set first box sizing");
    tree.set_baseline(first, Some(7.0))
        .expect("set first baseline");
    tree.set_margin(first, StandaloneEdge::Top, Length::calc(4.0, 5.0))
        .expect("set first top margin");
    tree.set_margin(first, StandaloneEdge::Bottom, Length::calc(3.0, 7.0))
        .expect("set first bottom margin");

    let second = tree.create_default_measured_node(Size::new(20.0, 16.0));
    tree.set_display(second, Display::Block)
        .expect("set second display");
    tree.set_box_sizing(second, BoxSizing::ContentBox)
        .expect("set second box sizing");
    tree.set_baseline(second, Some(10.0))
        .expect("set second baseline");

    tree.append_child(root, first).expect("append first");
    tree.append_child(root, second).expect("append second");

    run_standalone_rust(tree, root, Constraints::definite(92.0, 182.0))
        .expect("vertical-reverse linear bottom gravity baseline with root padding/border parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_horizontal_linear_largest_baseline_with_stretch_and_root_padding_border()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root orientation");
    tree.set_align_items(root, AlignItems::FlexStart)
        .expect("set root align items");
    tree.set_width(root, Length::points(170.0))
        .expect("set root width");
    tree.set_height(root, Length::points(112.0))
        .expect("set root height");
    tree.set_padding(root, StandaloneEdge::Vertical, Length::points(5.0))
        .expect("set root vertical padding");
    tree.set_border(root, StandaloneEdge::Vertical, 1.0)
        .expect("set root vertical border");

    let first = tree.create_default_measured_node(Size::new(26.0, 18.0));
    tree.set_display(first, Display::Block)
        .expect("set first display");
    tree.set_box_sizing(first, BoxSizing::ContentBox)
        .expect("set first box sizing");
    tree.set_linear_layout_gravity(first, LinearLayoutGravity::Stretch)
        .expect("set first layout gravity");
    tree.set_baseline(first, Some(6.0))
        .expect("set first baseline");
    tree.set_margin(first, StandaloneEdge::Top, Length::points(3.0))
        .expect("set first top margin");
    tree.set_margin(first, StandaloneEdge::Bottom, Length::points(4.0))
        .expect("set first bottom margin");

    let second = tree.create_default_measured_node(Size::new(24.0, 30.0));
    tree.set_display(second, Display::Block)
        .expect("set second display");
    tree.set_box_sizing(second, BoxSizing::ContentBox)
        .expect("set second box sizing");
    tree.set_baseline(second, Some(20.0))
        .expect("set second baseline");
    tree.set_margin(second, StandaloneEdge::Top, Length::calc(2.0, 3.0))
        .expect("set second top margin");
    tree.set_margin(second, StandaloneEdge::Bottom, Length::percent(4.0))
        .expect("set second bottom margin");

    tree.append_child(root, first).expect("append first");
    tree.append_child(root, second).expect("append second");

    run_standalone_rust(tree, root, Constraints::definite(170.0, 112.0))
        .expect("horizontal linear largest baseline with stretch and root padding/border parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_horizontal_linear_center_baseline_overflow_with_root_padding_border_and_percent_cross_margins()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root orientation");
    tree.set_align_items(root, AlignItems::Center)
        .expect("set root align items");
    tree.set_width(root, Length::points(150.0))
        .expect("set root width");
    tree.set_height(root, Length::points(44.0))
        .expect("set root height");
    tree.set_padding(root, StandaloneEdge::Top, Length::points(4.0))
        .expect("set root top padding");
    tree.set_padding(root, StandaloneEdge::Bottom, Length::points(6.0))
        .expect("set root bottom padding");
    tree.set_border(root, StandaloneEdge::Top, 1.0)
        .expect("set root top border");
    tree.set_border(root, StandaloneEdge::Bottom, 2.0)
        .expect("set root bottom border");

    let first = tree.create_default_measured_node(Size::new(26.0, 58.0));
    tree.set_display(first, Display::Block)
        .expect("set first display");
    tree.set_box_sizing(first, BoxSizing::ContentBox)
        .expect("set first box sizing");
    tree.set_baseline(first, Some(20.0))
        .expect("set first baseline");
    tree.set_margin(first, StandaloneEdge::Top, Length::percent(7.0))
        .expect("set first top margin");
    tree.set_margin(first, StandaloneEdge::Bottom, Length::percent(5.0))
        .expect("set first bottom margin");

    let second = tree.create_default_measured_node(Size::new(24.0, 20.0));
    tree.set_display(second, Display::Block)
        .expect("set second display");
    tree.set_box_sizing(second, BoxSizing::ContentBox)
        .expect("set second box sizing");
    tree.set_baseline(second, Some(9.0))
        .expect("set second baseline");

    tree.append_child(root, first).expect("append first");
    tree.append_child(root, second).expect("append second");

    run_standalone_rust(tree, root, Constraints::definite(150.0, 57.0))
        .expect("horizontal linear center baseline overflow with root padding/border parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_horizontal_linear_end_baseline_overflow_with_root_padding_border_and_calc_cross_margins()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root orientation");
    tree.set_align_items(root, AlignItems::FlexEnd)
        .expect("set root align items");
    tree.set_width(root, Length::points(148.0))
        .expect("set root width");
    tree.set_height(root, Length::points(40.0))
        .expect("set root height");
    tree.set_padding(root, StandaloneEdge::Vertical, Length::points(3.0))
        .expect("set root vertical padding");
    tree.set_border(root, StandaloneEdge::Vertical, 1.0)
        .expect("set root vertical border");

    let child = tree.create_default_measured_node(Size::new(30.0, 56.0));
    tree.set_display(child, Display::Block)
        .expect("set child display");
    tree.set_box_sizing(child, BoxSizing::ContentBox)
        .expect("set child box sizing");
    tree.set_baseline(child, Some(23.0))
        .expect("set child baseline");
    tree.set_margin(child, StandaloneEdge::Top, Length::calc(4.0, 5.0))
        .expect("set child top margin");
    tree.set_margin(child, StandaloneEdge::Bottom, Length::calc(3.0, 6.0))
        .expect("set child bottom margin");
    tree.append_child(root, child).expect("append child");

    run_standalone_rust(tree, root, Constraints::definite(148.0, 48.0))
        .expect("horizontal linear end baseline overflow with root padding/border parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_horizontal_reverse_linear_layout_gravity_bottom_baseline_overflow_with_percent_cross_margins()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::HorizontalReverse)
        .expect("set root orientation");
    tree.set_align_items(root, AlignItems::FlexStart)
        .expect("set root align items");
    tree.set_width(root, Length::points(136.0))
        .expect("set root width");
    tree.set_height(root, Length::points(46.0))
        .expect("set root height");

    let child = tree.create_default_measured_node(Size::new(28.0, 64.0));
    tree.set_display(child, Display::Block)
        .expect("set child display");
    tree.set_box_sizing(child, BoxSizing::ContentBox)
        .expect("set child box sizing");
    tree.set_linear_layout_gravity(child, LinearLayoutGravity::Bottom)
        .expect("set child layout gravity");
    tree.set_baseline(child, Some(24.0))
        .expect("set child baseline");
    tree.set_margin(child, StandaloneEdge::Top, Length::percent(6.0))
        .expect("set child top margin");
    tree.set_margin(child, StandaloneEdge::Bottom, Length::percent(5.0))
        .expect("set child bottom margin");
    tree.append_child(root, child).expect("append child");

    run_standalone_rust(tree, root, Constraints::definite(136.0, 46.0))
        .expect("horizontal-reverse linear bottom layout-gravity baseline overflow parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_rtl_horizontal_linear_layout_gravity_left_baseline_overflow_with_root_padding_border()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_direction(root, Direction::Rtl)
        .expect("set root direction");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root orientation");
    tree.set_align_items(root, AlignItems::FlexStart)
        .expect("set root align items");
    tree.set_width(root, Length::points(140.0))
        .expect("set root width");
    tree.set_height(root, Length::points(42.0))
        .expect("set root height");
    tree.set_padding(root, StandaloneEdge::Vertical, Length::points(4.0))
        .expect("set root vertical padding");
    tree.set_border(root, StandaloneEdge::Vertical, 1.0)
        .expect("set root vertical border");

    let child = tree.create_default_measured_node(Size::new(30.0, 60.0));
    tree.set_display(child, Display::Block)
        .expect("set child display");
    tree.set_box_sizing(child, BoxSizing::ContentBox)
        .expect("set child box sizing");
    tree.set_linear_layout_gravity(child, LinearLayoutGravity::Left)
        .expect("set child layout gravity");
    tree.set_baseline(child, Some(22.0))
        .expect("set child baseline");
    tree.set_margin(child, StandaloneEdge::Top, Length::calc(3.0, 5.0))
        .expect("set child top margin");
    tree.set_margin(child, StandaloneEdge::Bottom, Length::calc(2.0, 6.0))
        .expect("set child bottom margin");
    tree.append_child(root, child).expect("append child");

    run_standalone_rust(tree, root, Constraints::definite(140.0, 52.0))
        .expect("RTL horizontal linear left layout-gravity baseline overflow parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_vertical_linear_center_baseline_overflow_with_root_padding_border_and_percent_main_margins()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Vertical)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::Center)
        .expect("set root linear gravity");
    tree.set_width(root, Length::points(82.0))
        .expect("set root width");
    tree.set_height(root, Length::points(70.0))
        .expect("set root height");
    tree.set_padding(root, StandaloneEdge::Vertical, Length::points(4.0))
        .expect("set root vertical padding");
    tree.set_border(root, StandaloneEdge::Vertical, 1.0)
        .expect("set root vertical border");

    let first = tree.create_default_measured_node(Size::new(24.0, 54.0));
    tree.set_display(first, Display::Block)
        .expect("set first display");
    tree.set_box_sizing(first, BoxSizing::ContentBox)
        .expect("set first box sizing");
    tree.set_baseline(first, Some(18.0))
        .expect("set first baseline");
    tree.set_margin(first, StandaloneEdge::Top, Length::percent(8.0))
        .expect("set first top margin");
    tree.set_margin(first, StandaloneEdge::Bottom, Length::percent(6.0))
        .expect("set first bottom margin");

    let second = tree.create_default_measured_node(Size::new(22.0, 42.0));
    tree.set_display(second, Display::Block)
        .expect("set second display");
    tree.set_box_sizing(second, BoxSizing::ContentBox)
        .expect("set second box sizing");
    tree.set_baseline(second, Some(12.0))
        .expect("set second baseline");

    tree.append_child(root, first).expect("append first");
    tree.append_child(root, second).expect("append second");

    run_standalone_rust(tree, root, Constraints::definite(82.0, 80.0))
        .expect("vertical linear center baseline overflow with root padding/border parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_vertical_reverse_linear_end_baseline_overflow_with_root_padding_border_and_calc_main_margins()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::VerticalReverse)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::End)
        .expect("set root linear gravity");
    tree.set_width(root, Length::points(86.0))
        .expect("set root width");
    tree.set_height(root, Length::points(76.0))
        .expect("set root height");
    tree.set_padding(root, StandaloneEdge::Top, Length::points(5.0))
        .expect("set root top padding");
    tree.set_padding(root, StandaloneEdge::Bottom, Length::points(3.0))
        .expect("set root bottom padding");
    tree.set_border(root, StandaloneEdge::Vertical, 1.0)
        .expect("set root vertical border");

    let first = tree.create_default_measured_node(Size::new(26.0, 58.0));
    tree.set_display(first, Display::Block)
        .expect("set first display");
    tree.set_box_sizing(first, BoxSizing::ContentBox)
        .expect("set first box sizing");
    tree.set_baseline(first, Some(19.0))
        .expect("set first baseline");
    tree.set_margin(first, StandaloneEdge::Top, Length::calc(4.0, 5.0))
        .expect("set first top margin");
    tree.set_margin(first, StandaloneEdge::Bottom, Length::calc(3.0, 6.0))
        .expect("set first bottom margin");

    let second = tree.create_default_measured_node(Size::new(22.0, 46.0));
    tree.set_display(second, Display::Block)
        .expect("set second display");
    tree.set_box_sizing(second, BoxSizing::ContentBox)
        .expect("set second box sizing");
    tree.set_baseline(second, Some(13.0))
        .expect("set second baseline");

    tree.append_child(root, first).expect("append first");
    tree.append_child(root, second).expect("append second");

    run_standalone_rust(tree, root, Constraints::definite(86.0, 86.0))
        .expect("vertical-reverse linear end baseline overflow with root padding/border parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_horizontal_linear_ordered_baseline_skips_display_none_with_padding_border()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root orientation");
    tree.set_align_items(root, AlignItems::Center)
        .expect("set root align items");
    tree.set_width(root, Length::points(150.0))
        .expect("set root width");
    tree.set_height(root, Length::points(92.0))
        .expect("set root height");
    tree.set_padding(root, StandaloneEdge::Vertical, Length::points(4.0))
        .expect("set root vertical padding");
    tree.set_border(root, StandaloneEdge::Vertical, 1.0)
        .expect("set root vertical border");

    let hidden = tree.create_default_node();
    tree.set_display(hidden, Display::None)
        .expect("set hidden display");
    tree.set_width(hidden, Length::points(34.0))
        .expect("set hidden width");
    tree.set_height(hidden, Length::points(80.0))
        .expect("set hidden height");
    tree.set_order(hidden, -4).expect("set hidden order");

    let smaller = tree.create_default_measured_node(Size::new(22.0, 18.0));
    tree.set_display(smaller, Display::Block)
        .expect("set smaller display");
    tree.set_box_sizing(smaller, BoxSizing::ContentBox)
        .expect("set smaller box sizing");
    tree.set_baseline(smaller, Some(7.0))
        .expect("set smaller baseline");
    tree.set_order(smaller, 2).expect("set smaller order");

    let larger = tree.create_default_measured_node(Size::new(24.0, 24.0));
    tree.set_display(larger, Display::Block)
        .expect("set larger display");
    tree.set_box_sizing(larger, BoxSizing::ContentBox)
        .expect("set larger box sizing");
    tree.set_baseline(larger, Some(14.0))
        .expect("set larger baseline");
    tree.set_order(larger, 1).expect("set larger order");

    tree.append_child(root, hidden).expect("append hidden");
    tree.append_child(root, smaller).expect("append smaller");
    tree.append_child(root, larger).expect("append larger");

    run_standalone_rust(tree, root, Constraints::definite(150.0, 100.0))
        .expect("horizontal linear ordered baseline skips display-none with padding/border parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_horizontal_reverse_linear_ordered_baseline_skips_display_none_with_percent_cross_margins()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::HorizontalReverse)
        .expect("set root orientation");
    tree.set_align_items(root, AlignItems::FlexEnd)
        .expect("set root align items");
    tree.set_width(root, Length::points(140.0))
        .expect("set root width");
    tree.set_height(root, Length::points(96.0))
        .expect("set root height");

    let hidden = tree.create_default_node();
    tree.set_display(hidden, Display::None)
        .expect("set hidden display");
    tree.set_width(hidden, Length::points(30.0))
        .expect("set hidden width");
    tree.set_height(hidden, Length::points(88.0))
        .expect("set hidden height");
    tree.set_order(hidden, -6).expect("set hidden order");

    let first = tree.create_default_measured_node(Size::new(24.0, 18.0));
    tree.set_display(first, Display::Block)
        .expect("set first display");
    tree.set_box_sizing(first, BoxSizing::ContentBox)
        .expect("set first box sizing");
    tree.set_baseline(first, Some(8.0))
        .expect("set first baseline");
    tree.set_margin(first, StandaloneEdge::Top, Length::percent(6.0))
        .expect("set first top margin");
    tree.set_margin(first, StandaloneEdge::Bottom, Length::percent(4.0))
        .expect("set first bottom margin");
    tree.set_order(first, 3).expect("set first order");

    let second = tree.create_default_measured_node(Size::new(22.0, 22.0));
    tree.set_display(second, Display::Block)
        .expect("set second display");
    tree.set_box_sizing(second, BoxSizing::ContentBox)
        .expect("set second box sizing");
    tree.set_baseline(second, Some(11.0))
        .expect("set second baseline");
    tree.set_margin(second, StandaloneEdge::Top, Length::percent(3.0))
        .expect("set second top margin");
    tree.set_margin(second, StandaloneEdge::Bottom, Length::percent(5.0))
        .expect("set second bottom margin");
    tree.set_order(second, 1).expect("set second order");

    tree.append_child(root, first).expect("append first");
    tree.append_child(root, hidden).expect("append hidden");
    tree.append_child(root, second).expect("append second");

    run_standalone_rust(tree, root, Constraints::definite(140.0, 96.0)).expect(
        "horizontal-reverse linear ordered baseline skips display-none with percent margins parity",
    );
}

#[test]
fn standalone_owned_tree_matches_cpp_for_rtl_horizontal_linear_ordered_baseline_skips_display_none_with_physical_layout_gravity()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root orientation");
    tree.set_direction(root, Direction::Rtl)
        .expect("set root direction");
    tree.set_align_items(root, AlignItems::FlexStart)
        .expect("set root align items");
    tree.set_width(root, Length::points(150.0))
        .expect("set root width");
    tree.set_height(root, Length::points(90.0))
        .expect("set root height");

    let hidden = tree.create_default_node();
    tree.set_display(hidden, Display::None)
        .expect("set hidden display");
    tree.set_width(hidden, Length::points(28.0))
        .expect("set hidden width");
    tree.set_height(hidden, Length::points(86.0))
        .expect("set hidden height");
    tree.set_order(hidden, -2).expect("set hidden order");

    let physical = tree.create_default_measured_node(Size::new(24.0, 20.0));
    tree.set_display(physical, Display::Block)
        .expect("set physical display");
    tree.set_box_sizing(physical, BoxSizing::ContentBox)
        .expect("set physical box sizing");
    tree.set_linear_layout_gravity(physical, LinearLayoutGravity::Left)
        .expect("set physical layout gravity");
    tree.set_baseline(physical, Some(9.0))
        .expect("set physical baseline");
    tree.set_margin(physical, StandaloneEdge::Top, Length::calc(3.0, 5.0))
        .expect("set physical top margin");
    tree.set_margin(physical, StandaloneEdge::Bottom, Length::calc(4.0, 4.0))
        .expect("set physical bottom margin");
    tree.set_order(physical, 2).expect("set physical order");

    let default = tree.create_default_measured_node(Size::new(22.0, 18.0));
    tree.set_display(default, Display::Block)
        .expect("set default display");
    tree.set_box_sizing(default, BoxSizing::ContentBox)
        .expect("set default box sizing");
    tree.set_baseline(default, Some(6.0))
        .expect("set default baseline");
    tree.set_order(default, 1).expect("set default order");

    tree.append_child(root, physical).expect("append physical");
    tree.append_child(root, hidden).expect("append hidden");
    tree.append_child(root, default).expect("append default");

    run_standalone_rust(tree, root, Constraints::definite(150.0, 90.0)).expect(
        "RTL horizontal linear ordered baseline skips display-none with physical layout-gravity parity",
    );
}

#[test]
fn standalone_owned_tree_matches_cpp_for_vertical_linear_ordered_first_baseline_skips_display_none()
{
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Vertical)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::End)
        .expect("set root gravity");
    tree.set_width(root, Length::points(82.0))
        .expect("set root width");
    tree.set_height(root, Length::points(150.0))
        .expect("set root height");

    let hidden = tree.create_default_node();
    tree.set_display(hidden, Display::None)
        .expect("set hidden display");
    tree.set_width(hidden, Length::points(32.0))
        .expect("set hidden width");
    tree.set_height(hidden, Length::points(100.0))
        .expect("set hidden height");
    tree.set_order(hidden, -5).expect("set hidden order");

    let ordered_first = tree.create_default_measured_node(Size::new(24.0, 20.0));
    tree.set_display(ordered_first, Display::Block)
        .expect("set ordered first display");
    tree.set_box_sizing(ordered_first, BoxSizing::ContentBox)
        .expect("set ordered first box sizing");
    tree.set_baseline(ordered_first, Some(7.0))
        .expect("set ordered first baseline");
    tree.set_margin(ordered_first, StandaloneEdge::Top, Length::points(4.0))
        .expect("set ordered first top margin");
    tree.set_margin(ordered_first, StandaloneEdge::Bottom, Length::points(6.0))
        .expect("set ordered first bottom margin");
    tree.set_order(ordered_first, -1)
        .expect("set ordered first order");

    let appended_first = tree.create_default_measured_node(Size::new(24.0, 18.0));
    tree.set_display(appended_first, Display::Block)
        .expect("set appended first display");
    tree.set_box_sizing(appended_first, BoxSizing::ContentBox)
        .expect("set appended first box sizing");
    tree.set_baseline(appended_first, Some(13.0))
        .expect("set appended first baseline");
    tree.set_order(appended_first, 3)
        .expect("set appended first order");

    tree.append_child(root, appended_first)
        .expect("append appended first");
    tree.append_child(root, hidden).expect("append hidden");
    tree.append_child(root, ordered_first)
        .expect("append ordered first");

    run_standalone_rust(tree, root, Constraints::definite(82.0, 150.0))
        .expect("vertical linear ordered first baseline skips display-none parity");
}

#[test]
fn standalone_owned_tree_matches_cpp_for_vertical_reverse_linear_ordered_first_baseline_skips_display_none_with_calc_margins()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::VerticalReverse)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::Center)
        .expect("set root gravity");
    tree.set_width(root, Length::points(86.0))
        .expect("set root width");
    tree.set_height(root, Length::points(156.0))
        .expect("set root height");

    let hidden = tree.create_default_node();
    tree.set_display(hidden, Display::None)
        .expect("set hidden display");
    tree.set_width(hidden, Length::points(36.0))
        .expect("set hidden width");
    tree.set_height(hidden, Length::points(104.0))
        .expect("set hidden height");
    tree.set_order(hidden, -8).expect("set hidden order");

    let ordered_first = tree.create_default_measured_node(Size::new(26.0, 22.0));
    tree.set_display(ordered_first, Display::Block)
        .expect("set ordered first display");
    tree.set_box_sizing(ordered_first, BoxSizing::ContentBox)
        .expect("set ordered first box sizing");
    tree.set_baseline(ordered_first, Some(8.0))
        .expect("set ordered first baseline");
    tree.set_margin(ordered_first, StandaloneEdge::Top, Length::calc(3.0, 6.0))
        .expect("set ordered first top margin");
    tree.set_margin(
        ordered_first,
        StandaloneEdge::Bottom,
        Length::calc(4.0, 5.0),
    )
    .expect("set ordered first bottom margin");
    tree.set_order(ordered_first, -2)
        .expect("set ordered first order");

    let later = tree.create_default_measured_node(Size::new(22.0, 18.0));
    tree.set_display(later, Display::Block)
        .expect("set later display");
    tree.set_box_sizing(later, BoxSizing::ContentBox)
        .expect("set later box sizing");
    tree.set_baseline(later, Some(12.0))
        .expect("set later baseline");
    tree.set_order(later, 4).expect("set later order");

    tree.append_child(root, later).expect("append later");
    tree.append_child(root, ordered_first)
        .expect("append ordered first");
    tree.append_child(root, hidden).expect("append hidden");

    run_standalone_rust(tree, root, Constraints::definite(86.0, 156.0)).expect(
        "vertical-reverse linear ordered first baseline skips display-none with calc margins parity",
    );
}

#[test]
fn standalone_owned_tree_matches_cpp_for_vertical_linear_justify_flex_end_ordered_first_baseline_skips_display_none_with_padding_border()
 {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Vertical)
        .expect("set root orientation");
    tree.set_justify_content(root, JustifyContent::FlexEnd)
        .expect("set root justify content");
    tree.set_width(root, Length::points(88.0))
        .expect("set root width");
    tree.set_height(root, Length::points(160.0))
        .expect("set root height");
    tree.set_padding(root, StandaloneEdge::Vertical, Length::points(5.0))
        .expect("set root vertical padding");
    tree.set_border(root, StandaloneEdge::Vertical, 1.0)
        .expect("set root vertical border");

    let hidden = tree.create_default_node();
    tree.set_display(hidden, Display::None)
        .expect("set hidden display");
    tree.set_width(hidden, Length::points(40.0))
        .expect("set hidden width");
    tree.set_height(hidden, Length::points(112.0))
        .expect("set hidden height");
    tree.set_order(hidden, -10).expect("set hidden order");

    let ordered_first = tree.create_default_measured_node(Size::new(26.0, 24.0));
    tree.set_display(ordered_first, Display::Block)
        .expect("set ordered first display");
    tree.set_box_sizing(ordered_first, BoxSizing::ContentBox)
        .expect("set ordered first box sizing");
    tree.set_baseline(ordered_first, Some(9.0))
        .expect("set ordered first baseline");
    tree.set_margin(ordered_first, StandaloneEdge::Top, Length::percent(4.0))
        .expect("set ordered first top margin");
    tree.set_margin(ordered_first, StandaloneEdge::Bottom, Length::percent(3.0))
        .expect("set ordered first bottom margin");
    tree.set_order(ordered_first, -1)
        .expect("set ordered first order");

    let later = tree.create_default_measured_node(Size::new(22.0, 16.0));
    tree.set_display(later, Display::Block)
        .expect("set later display");
    tree.set_box_sizing(later, BoxSizing::ContentBox)
        .expect("set later box sizing");
    tree.set_baseline(later, Some(12.0))
        .expect("set later baseline");
    tree.set_order(later, 3).expect("set later order");

    tree.append_child(root, later).expect("append later");
    tree.append_child(root, hidden).expect("append hidden");
    tree.append_child(root, ordered_first)
        .expect("append ordered first");

    run_standalone_rust(tree, root, Constraints::definite(88.0, 160.0)).expect(
        "vertical linear justify flex-end ordered first baseline skips display-none with padding/border parity",
    );
}

#[test]
fn standalone_owned_tree_matches_cpp_for_linear_overflowing_cross_axis_auto_margins() {
    let (tree, root, constraints) = linear_overflowing_cross_axis_auto_margins_tree();

    run_standalone_rust(tree, root, constraints)
        .expect("linear overflowing cross-axis auto-margin parity");
}

fn callback_measure(constraints: Constraints) -> Size {
    let width = constraints.width.bounded_size().unwrap_or(20.0) - 3.0;
    let height = constraints.height.bounded_size().unwrap_or(12.0) - 1.0;
    Size::new(width, height)
}

fn cross_axis_bounded_measure(constraints: Constraints) -> Size {
    Size::new(constraints.width.bounded_size().unwrap_or(150.0), 10.0)
}

#[derive(Clone, Copy)]
enum OutOfFlowNaturalSize {
    Subtree(Size),
    Measured(Size),
}

fn out_of_flow_intrinsic_sizing_tree(
    case_name: &'static str,
    position_type: PositionType,
    attach_under_nested_parent: bool,
    width: Length,
    height: Length,
    natural_size: OutOfFlowNaturalSize,
) -> (&'static str, StandaloneTree, NodeId, Constraints) {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    set_block_content_box(&mut tree, root);
    tree.set_width(root, Length::points(200.0))
        .expect("set root width");
    tree.set_height(root, Length::points(100.0))
        .expect("set root height");

    let parent = if attach_under_nested_parent {
        let nested = tree.create_default_node();
        set_block_content_box(&mut tree, nested);
        tree.set_width(nested, Length::points(20.0))
            .expect("set nested width");
        tree.set_height(nested, Length::points(20.0))
            .expect("set nested height");
        tree.append_child(root, nested)
            .expect("append nested child");
        nested
    } else {
        root
    };

    let out_of_flow = match natural_size {
        OutOfFlowNaturalSize::Measured(size) => tree.create_default_measured_node(size),
        OutOfFlowNaturalSize::Subtree(_) => tree.create_default_node(),
    };
    set_block_content_box(&mut tree, out_of_flow);
    tree.set_position_type(out_of_flow, position_type)
        .expect("set out-of-flow position");
    tree.set_width(out_of_flow, width)
        .expect("set out-of-flow width");
    tree.set_height(out_of_flow, height)
        .expect("set out-of-flow height");
    tree.set_position(out_of_flow, StandaloneEdge::Left, Length::points(7.0))
        .expect("set out-of-flow left");
    tree.set_position(out_of_flow, StandaloneEdge::Top, Length::points(9.0))
        .expect("set out-of-flow top");
    tree.append_child(parent, out_of_flow)
        .expect("append out-of-flow child");

    if let OutOfFlowNaturalSize::Subtree(size) = natural_size {
        let grandchild = tree.create_default_node();
        set_block_content_box(&mut tree, grandchild);
        tree.set_width(grandchild, Length::points(size.width))
            .expect("set grandchild width");
        tree.set_height(grandchild, Length::points(size.height))
            .expect("set grandchild height");
        tree.append_child(out_of_flow, grandchild)
            .expect("append grandchild");
    }

    (case_name, tree, root, Constraints::definite(200.0, 100.0))
}

fn set_block_content_box(tree: &mut StandaloneTree, node: NodeId) {
    tree.set_display(node, Display::Block)
        .expect("set block display");
    tree.set_box_sizing(node, BoxSizing::ContentBox)
        .expect("set content-box sizing");
}

fn absolute_linear_gravity_alignment_tree() -> (&'static str, StandaloneTree, NodeId, Constraints) {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::Center)
        .expect("set root gravity");
    tree.set_width(root, Length::points(100.0))
        .expect("set root width");
    tree.set_height(root, Length::points(40.0))
        .expect("set root height");

    let absolute = tree.create_default_node();
    tree.set_display(absolute, Display::Flex)
        .expect("set absolute display");
    tree.set_box_sizing(absolute, BoxSizing::ContentBox)
        .expect("set absolute box sizing");
    tree.set_position_type(absolute, PositionType::Absolute)
        .expect("set absolute position");
    tree.set_width(absolute, Length::points(20.0))
        .expect("set absolute width");
    tree.set_height(absolute, Length::points(10.0))
        .expect("set absolute height");
    tree.set_linear_layout_gravity(absolute, LinearLayoutGravity::End)
        .expect("set absolute layout gravity");
    tree.append_child(root, absolute)
        .expect("append absolute child");

    (
        "absolute linear child uses linear gravity and layout-gravity",
        tree,
        root,
        Constraints::definite(100.0, 40.0),
    )
}

fn absolute_rtl_horizontal_linear_front_alignment_tree()
-> (&'static str, StandaloneTree, NodeId, Constraints) {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_box_sizing(root, BoxSizing::ContentBox)
        .expect("set root box sizing");
    tree.set_direction(root, Direction::Rtl)
        .expect("set root direction");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root orientation");
    tree.set_linear_gravity(root, LinearGravity::Right)
        .expect("set root gravity");
    tree.set_width(root, Length::points(100.0))
        .expect("set root width");
    tree.set_height(root, Length::points(40.0))
        .expect("set root height");

    let absolute = tree.create_default_node();
    tree.set_display(absolute, Display::Flex)
        .expect("set absolute display");
    tree.set_box_sizing(absolute, BoxSizing::ContentBox)
        .expect("set absolute box sizing");
    tree.set_position_type(absolute, PositionType::Absolute)
        .expect("set absolute position");
    tree.set_width(absolute, Length::points(20.0))
        .expect("set absolute width");
    tree.set_height(absolute, Length::points(10.0))
        .expect("set absolute height");
    tree.set_linear_layout_gravity(absolute, LinearLayoutGravity::End)
        .expect("set absolute layout gravity");
    tree.append_child(root, absolute)
        .expect("append absolute child");

    (
        "absolute RTL horizontal linear child uses RTL main front",
        tree,
        root,
        Constraints::definite(100.0, 40.0),
    )
}

fn set_tight_constraint_padding_border(tree: &mut StandaloneTree, node: NodeId) {
    tree.set_padding(node, StandaloneEdge::Left, Length::points(10.0))
        .expect("set left padding");
    tree.set_padding(node, StandaloneEdge::Right, Length::points(15.0))
        .expect("set right padding");
    tree.set_padding(node, StandaloneEdge::Top, Length::points(8.0))
        .expect("set top padding");
    tree.set_padding(node, StandaloneEdge::Bottom, Length::points(9.0))
        .expect("set bottom padding");
    tree.set_border(node, StandaloneEdge::Left, 2.0)
        .expect("set left border");
    tree.set_border(node, StandaloneEdge::Right, 3.0)
        .expect("set right border");
    tree.set_border(node, StandaloneEdge::Top, 1.0)
        .expect("set top border");
    tree.set_border(node, StandaloneEdge::Bottom, 4.0)
        .expect("set bottom border");
}

fn linear_display_none_and_ordered_stack_tree() -> (StandaloneTree, NodeId, Constraints) {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_linear_orientation(root, LinearOrientation::Vertical)
        .expect("set root linear orientation");
    tree.set_width(root, Length::points(100.0))
        .expect("set root width");
    tree.set_align_items(root, AlignItems::FlexStart)
        .expect("set root align items");

    let later = tree.create_default_node();
    tree.set_display(later, Display::Block)
        .expect("set later display");
    tree.set_height(later, Length::points(10.0))
        .expect("set later height");
    tree.set_order(later, 1).expect("set later order");

    let hidden = tree.create_default_node();
    tree.set_display(hidden, Display::None)
        .expect("set hidden display");
    tree.set_width(hidden, Length::points(100.0))
        .expect("set hidden width");
    tree.set_height(hidden, Length::points(50.0))
        .expect("set hidden height");
    tree.set_order(hidden, -2).expect("set hidden order");

    let earlier = tree.create_default_node();
    tree.set_display(earlier, Display::Block)
        .expect("set earlier display");
    tree.set_height(earlier, Length::points(20.0))
        .expect("set earlier height");
    tree.set_order(earlier, -1).expect("set earlier order");

    tree.append_child(root, later).expect("append later child");
    tree.append_child(root, hidden)
        .expect("append hidden child");
    tree.append_child(root, earlier)
        .expect("append earlier child");

    (
        tree,
        root,
        Constraints::new(
            SideConstraint::definite(100.0),
            SideConstraint::indefinite(),
        ),
    )
}

fn linear_at_most_main_axis_sizing_tree() -> (StandaloneTree, NodeId, Constraints) {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root linear orientation");
    tree.set_height(root, Length::points(20.0))
        .expect("set root height");
    tree.set_align_items(root, AlignItems::FlexStart)
        .expect("set root align items");

    let first = tree.create_default_node();
    tree.set_display(first, Display::Block)
        .expect("set first display");
    tree.set_width(first, Length::points(80.0))
        .expect("set first width");
    tree.set_height(first, Length::Auto)
        .expect("set first height");

    let weighted = tree.create_default_node();
    tree.set_display(weighted, Display::Block)
        .expect("set weighted display");
    tree.set_linear_weight(weighted, 1.0)
        .expect("set weighted weight");
    tree.set_height(weighted, Length::Auto)
        .expect("set weighted height");

    let second = tree.create_default_node();
    tree.set_display(second, Display::Block)
        .expect("set second display");
    tree.set_width(second, Length::points(70.0))
        .expect("set second width");
    tree.set_height(second, Length::Auto)
        .expect("set second height");

    tree.append_child(root, first).expect("append first child");
    tree.append_child(root, weighted)
        .expect("append weighted child");
    tree.append_child(root, second)
        .expect("append second child");

    (
        tree,
        root,
        Constraints::new(
            SideConstraint::at_most(100.0),
            SideConstraint::definite(20.0),
        ),
    )
}

fn linear_auto_cross_axis_parent_constraint_tree() -> (StandaloneTree, NodeId, Constraints) {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root linear orientation");
    tree.set_width(root, Length::points(100.0))
        .expect("set root width");
    tree.set_height(root, Length::Auto)
        .expect("set root auto height");

    let child = tree.create_default_node();
    tree.set_display(child, Display::Block)
        .expect("set child display");
    tree.set_width(child, Length::points(10.0))
        .expect("set child width");
    tree.set_height(child, Length::Auto)
        .expect("set child auto height");

    tree.append_child(root, child).expect("append child");

    (tree, root, Constraints::definite(100.0, 80.0))
}

fn linear_at_most_cross_axis_stretch_suppression_tree() -> (StandaloneTree, NodeId, Constraints) {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_direction(root, Direction::Rtl)
        .expect("set root direction");
    tree.set_linear_orientation(root, LinearOrientation::Vertical)
        .expect("set root linear orientation");
    tree.set_min_width(root, Length::points(20.0))
        .expect("set root min width");

    let auto_child = tree.create_default_node();
    tree.set_display(auto_child, Display::Block)
        .expect("set auto child display");
    tree.set_width(auto_child, Length::Auto)
        .expect("set auto child width");
    tree.set_height(auto_child, Length::percent(40.0))
        .expect("set auto child height");
    tree.set_min_width(auto_child, Length::points(12.0))
        .expect("set auto child min width");

    let wider_sibling = tree.create_default_node();
    tree.set_display(wider_sibling, Display::Block)
        .expect("set wider sibling display");
    tree.set_width(wider_sibling, Length::points(14.0))
        .expect("set wider sibling width");
    tree.set_height(wider_sibling, Length::points(1.0))
        .expect("set wider sibling height");

    tree.append_child(root, auto_child)
        .expect("append auto child");
    tree.append_child(root, wider_sibling)
        .expect("append wider sibling");

    (
        tree,
        root,
        Constraints::new(SideConstraint::at_most(100.0), SideConstraint::indefinite()),
    )
}

fn linear_container_min_width_max_height_clamp_tree()
-> (&'static str, StandaloneTree, NodeId, Constraints) {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root linear orientation");
    tree.set_min_width(root, Length::points(40.0))
        .expect("set root min width");
    tree.set_max_height(root, Length::points(25.0))
        .expect("set root max height");

    let child = fixed_standalone_block(&mut tree, 20.0, 30.0);
    tree.append_child(root, child).expect("append child");

    (
        "linear container min-width/max-height clamp",
        tree,
        root,
        Constraints::indefinite(),
    )
}

fn linear_container_max_width_min_height_clamp_tree()
-> (&'static str, StandaloneTree, NodeId, Constraints) {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_max_width(root, Length::points(60.0))
        .expect("set root max width");
    tree.set_min_height(root, Length::points(40.0))
        .expect("set root min height");

    let child = fixed_standalone_block(&mut tree, 100.0, 10.0);
    tree.append_child(root, child).expect("append child");

    (
        "linear container max-width/min-height clamp",
        tree,
        root,
        Constraints::indefinite(),
    )
}

fn linear_container_padding_border_tight_constraint_clamp_tree()
-> (&'static str, StandaloneTree, NodeId, Constraints) {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root linear orientation");
    set_tight_constraint_padding_border(&mut tree, root);

    (
        "linear container padding/border tight constraint clamp",
        tree,
        root,
        Constraints::definite(8.0, 7.0),
    )
}

fn fixed_standalone_block(tree: &mut StandaloneTree, width: f32, height: f32) -> NodeId {
    let node = tree.create_default_node();
    tree.set_display(node, Display::Block)
        .expect("set block display");
    tree.set_width(node, Length::points(width))
        .expect("set block width");
    tree.set_height(node, Length::points(height))
        .expect("set block height");
    node
}

fn linear_measured_cross_axis_constraint_tree() -> (StandaloneTree, NodeId, Constraints) {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_linear_orientation(root, LinearOrientation::Vertical)
        .expect("set root linear orientation");
    tree.set_align_items(root, AlignItems::Center)
        .expect("set root align items");

    let measured = tree.create_default_node();
    tree.set_display(measured, Display::Block)
        .expect("set measured display");
    tree.set_measure_func(measured, Some(cross_axis_bounded_measure))
        .expect("set measured callback");

    tree.append_child(root, measured)
        .expect("append measured child");

    (
        tree,
        root,
        Constraints::new(SideConstraint::at_most(100.0), SideConstraint::indefinite()),
    )
}

fn linear_percent_cross_size_final_remeasure_tree() -> (StandaloneTree, NodeId, Constraints) {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_linear_orientation(root, LinearOrientation::Vertical)
        .expect("set root linear orientation");
    tree.set_width(root, Length::points(100.0))
        .expect("set root width");
    tree.set_height(root, Length::points(40.0))
        .expect("set root height");

    let child = tree.create_default_node();
    tree.set_display(child, Display::Block)
        .expect("set child display");
    tree.set_width(child, Length::percent(50.0))
        .expect("set child percent width");
    tree.set_height(child, Length::points(10.0))
        .expect("set child height");
    tree.append_child(root, child).expect("append child");

    (tree, root, Constraints::definite(100.0, 40.0))
}

fn linear_weight_and_gravity_tree() -> (StandaloneTree, NodeId, Constraints) {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root linear orientation");
    tree.set_linear_gravity(root, LinearGravity::Right)
        .expect("set root linear gravity");
    tree.set_linear_weight_sum(root, 4.0)
        .expect("set root weight sum");
    tree.set_align_items(root, AlignItems::FlexStart)
        .expect("set root align items");
    tree.set_width(root, Length::points(120.0))
        .expect("set root width");
    tree.set_height(root, Length::points(40.0))
        .expect("set root height");

    let fixed = tree.create_default_node();
    tree.set_display(fixed, Display::Block)
        .expect("set fixed display");
    tree.set_width(fixed, Length::points(10.0))
        .expect("set fixed width");
    tree.set_height(fixed, Length::points(8.0))
        .expect("set fixed height");

    let capped = tree.create_default_node();
    tree.set_display(capped, Display::Block)
        .expect("set capped display");
    tree.set_linear_weight(capped, 1.0)
        .expect("set capped weight");
    tree.set_max_width(capped, Length::points(25.0))
        .expect("set capped max width");
    tree.set_height(capped, Length::points(10.0))
        .expect("set capped height");
    tree.set_linear_layout_gravity(capped, LinearLayoutGravity::Stretch)
        .expect("set capped layout gravity");

    let flexible = tree.create_default_node();
    tree.set_display(flexible, Display::Block)
        .expect("set flexible display");
    tree.set_linear_weight(flexible, 1.0)
        .expect("set flexible weight");
    tree.set_min_width(flexible, Length::points(20.0))
        .expect("set flexible min width");
    tree.set_height(flexible, Length::points(12.0))
        .expect("set flexible height");
    tree.set_linear_layout_gravity(flexible, LinearLayoutGravity::End)
        .expect("set flexible layout gravity");

    tree.append_child(root, fixed).expect("append fixed");
    tree.append_child(root, capped).expect("append capped");
    tree.append_child(root, flexible).expect("append flexible");

    (tree, root, Constraints::definite(120.0, 40.0))
}

fn linear_weight_max_width_freeze_tree() -> (&'static str, StandaloneTree, NodeId, Constraints) {
    let mut tree = StandaloneTree::new();
    let root = weighted_linear_freeze_root(
        &mut tree,
        LinearOrientation::Horizontal,
        Size::new(100.0, 20.0),
    );
    let capped = weighted_linear_child(&mut tree, true);
    tree.set_max_width(capped, Length::points(30.0))
        .expect("set capped max width");
    let flexible = weighted_linear_child(&mut tree, true);
    tree.append_child(root, capped).expect("append capped");
    tree.append_child(root, flexible).expect("append flexible");

    (
        "linear weight point max-width freeze",
        tree,
        root,
        Constraints::definite(100.0, 20.0),
    )
}

fn linear_weight_percent_max_width_freeze_tree()
-> (&'static str, StandaloneTree, NodeId, Constraints) {
    let mut tree = StandaloneTree::new();
    let root = weighted_linear_freeze_root(
        &mut tree,
        LinearOrientation::Horizontal,
        Size::new(100.0, 20.0),
    );
    let capped = weighted_linear_child(&mut tree, true);
    tree.set_max_width(capped, Length::percent(30.0))
        .expect("set capped percent max width");
    let flexible = weighted_linear_child(&mut tree, true);
    tree.append_child(root, capped).expect("append capped");
    tree.append_child(root, flexible).expect("append flexible");

    (
        "linear weight percent max-width freeze",
        tree,
        root,
        Constraints::definite(100.0, 20.0),
    )
}

fn linear_weight_min_width_freeze_tree() -> (&'static str, StandaloneTree, NodeId, Constraints) {
    let mut tree = StandaloneTree::new();
    let root = weighted_linear_freeze_root(
        &mut tree,
        LinearOrientation::Horizontal,
        Size::new(100.0, 20.0),
    );
    let floor = weighted_linear_child(&mut tree, true);
    tree.set_min_width(floor, Length::points(70.0))
        .expect("set floor min width");
    let flexible = weighted_linear_child(&mut tree, true);
    tree.append_child(root, floor).expect("append floor");
    tree.append_child(root, flexible).expect("append flexible");

    (
        "linear weight point min-width freeze",
        tree,
        root,
        Constraints::definite(100.0, 20.0),
    )
}

fn linear_weight_percent_min_width_freeze_tree()
-> (&'static str, StandaloneTree, NodeId, Constraints) {
    let mut tree = StandaloneTree::new();
    let root = weighted_linear_freeze_root(
        &mut tree,
        LinearOrientation::Horizontal,
        Size::new(100.0, 20.0),
    );
    let floor = weighted_linear_child(&mut tree, true);
    tree.set_min_width(floor, Length::percent(70.0))
        .expect("set floor percent min width");
    let flexible = weighted_linear_child(&mut tree, true);
    tree.append_child(root, floor).expect("append floor");
    tree.append_child(root, flexible).expect("append flexible");

    (
        "linear weight percent min-width freeze",
        tree,
        root,
        Constraints::definite(100.0, 20.0),
    )
}

fn linear_weight_max_height_freeze_tree() -> (&'static str, StandaloneTree, NodeId, Constraints) {
    let mut tree = StandaloneTree::new();
    let root = weighted_linear_freeze_root(
        &mut tree,
        LinearOrientation::Vertical,
        Size::new(20.0, 100.0),
    );
    let capped = weighted_linear_child(&mut tree, false);
    tree.set_max_height(capped, Length::points(30.0))
        .expect("set capped max height");
    let flexible = weighted_linear_child(&mut tree, false);
    tree.append_child(root, capped).expect("append capped");
    tree.append_child(root, flexible).expect("append flexible");

    (
        "linear weight point max-height freeze",
        tree,
        root,
        Constraints::definite(20.0, 100.0),
    )
}

fn linear_weight_percent_max_height_freeze_tree()
-> (&'static str, StandaloneTree, NodeId, Constraints) {
    let mut tree = StandaloneTree::new();
    let root = weighted_linear_freeze_root(
        &mut tree,
        LinearOrientation::Vertical,
        Size::new(20.0, 100.0),
    );
    let capped = weighted_linear_child(&mut tree, false);
    tree.set_max_height(capped, Length::percent(30.0))
        .expect("set capped percent max height");
    let flexible = weighted_linear_child(&mut tree, false);
    tree.append_child(root, capped).expect("append capped");
    tree.append_child(root, flexible).expect("append flexible");

    (
        "linear weight percent max-height freeze",
        tree,
        root,
        Constraints::definite(20.0, 100.0),
    )
}

fn linear_weight_min_height_freeze_tree() -> (&'static str, StandaloneTree, NodeId, Constraints) {
    let mut tree = StandaloneTree::new();
    let root = weighted_linear_freeze_root(
        &mut tree,
        LinearOrientation::Vertical,
        Size::new(20.0, 100.0),
    );
    let floor = weighted_linear_child(&mut tree, false);
    tree.set_min_height(floor, Length::points(70.0))
        .expect("set floor min height");
    let flexible = weighted_linear_child(&mut tree, false);
    tree.append_child(root, floor).expect("append floor");
    tree.append_child(root, flexible).expect("append flexible");

    (
        "linear weight point min-height freeze",
        tree,
        root,
        Constraints::definite(20.0, 100.0),
    )
}

fn linear_weight_percent_min_height_freeze_tree()
-> (&'static str, StandaloneTree, NodeId, Constraints) {
    let mut tree = StandaloneTree::new();
    let root = weighted_linear_freeze_root(
        &mut tree,
        LinearOrientation::Vertical,
        Size::new(20.0, 100.0),
    );
    let floor = weighted_linear_child(&mut tree, false);
    tree.set_min_height(floor, Length::percent(70.0))
        .expect("set floor percent min height");
    let flexible = weighted_linear_child(&mut tree, false);
    tree.append_child(root, floor).expect("append floor");
    tree.append_child(root, flexible).expect("append flexible");

    (
        "linear weight percent min-height freeze",
        tree,
        root,
        Constraints::definite(20.0, 100.0),
    )
}

fn weighted_linear_freeze_root(
    tree: &mut StandaloneTree,
    orientation: LinearOrientation,
    size: Size,
) -> NodeId {
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_linear_orientation(root, orientation)
        .expect("set root linear orientation");
    tree.set_width(root, Length::points(size.width))
        .expect("set root width");
    tree.set_height(root, Length::points(size.height))
        .expect("set root height");
    tree.set_align_items(root, AlignItems::FlexStart)
        .expect("set root align items");
    root
}

fn weighted_linear_child(tree: &mut StandaloneTree, is_horizontal: bool) -> NodeId {
    let child = tree.create_default_node();
    tree.set_display(child, Display::Block)
        .expect("set weighted child display");
    tree.set_linear_weight(child, 1.0)
        .expect("set weighted child weight");
    if is_horizontal {
        tree.set_height(child, Length::points(10.0))
            .expect("set weighted child height");
    } else {
        tree.set_width(child, Length::points(10.0))
            .expect("set weighted child width");
    }
    child
}

fn linear_weight_sum_unallocated_space_tree() -> (&'static str, StandaloneTree, NodeId, Constraints)
{
    let mut tree = StandaloneTree::new();
    let root = weighted_linear_freeze_root(
        &mut tree,
        LinearOrientation::Horizontal,
        Size::new(100.0, 20.0),
    );
    tree.set_linear_weight_sum(root, 4.0)
        .expect("set root weight sum");
    let first = weighted_linear_child(&mut tree, true);
    let second = weighted_linear_child(&mut tree, true);
    tree.append_child(root, first).expect("append first");
    tree.append_child(root, second).expect("append second");

    (
        "linear weight-sum leaves unallocated space",
        tree,
        root,
        Constraints::definite(100.0, 20.0),
    )
}

fn linear_total_weight_below_one_unallocated_space_tree()
-> (&'static str, StandaloneTree, NodeId, Constraints) {
    let mut tree = StandaloneTree::new();
    let root = weighted_linear_freeze_root(
        &mut tree,
        LinearOrientation::Horizontal,
        Size::new(100.0, 20.0),
    );
    let child = weighted_linear_child(&mut tree, true);
    tree.set_linear_weight(child, 0.5)
        .expect("set partial child weight");
    tree.append_child(root, child).expect("append child");

    (
        "linear total weight below one leaves unallocated space",
        tree,
        root,
        Constraints::definite(100.0, 20.0),
    )
}

fn linear_weight_sub_epsilon_min_violation_tree()
-> (&'static str, StandaloneTree, NodeId, Constraints) {
    let mut tree = StandaloneTree::new();
    let root = weighted_linear_freeze_root(
        &mut tree,
        LinearOrientation::Horizontal,
        Size::new(100.0, 20.0),
    );
    for _ in 0..2 {
        let child = weighted_linear_child(&mut tree, true);
        tree.set_min_width(child, Length::points(50.00006))
            .expect("set tiny min-width violation");
        tree.append_child(root, child).expect("append child");
    }

    (
        "linear weight sub-epsilon min-width violations",
        tree,
        root,
        Constraints::definite(100.0, 20.0),
    )
}

fn vertical_linear_gravity_mapping_tree(
    gravity: LinearGravity,
) -> (String, StandaloneTree, NodeId, Constraints) {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_linear_gravity(root, gravity)
        .expect("set root linear gravity");
    tree.set_width(root, Length::points(30.0))
        .expect("set root width");
    tree.set_height(root, Length::points(100.0))
        .expect("set root height");

    for height in [10.0, 20.0] {
        let child = fixed_linear_gravity_child(&mut tree, 10.0, height);
        tree.append_child(root, child)
            .expect("append vertical gravity child");
    }

    (
        format!("vertical linear gravity {gravity:?} mapping"),
        tree,
        root,
        Constraints::definite(30.0, 100.0),
    )
}

fn horizontal_linear_gravity_overrides_justify_content_tree()
-> (String, StandaloneTree, NodeId, Constraints) {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root linear orientation");
    tree.set_linear_gravity(root, LinearGravity::Right)
        .expect("set root linear gravity");
    tree.set_justify_content(root, JustifyContent::FlexStart)
        .expect("set root justify content");
    tree.set_width(root, Length::points(100.0))
        .expect("set root width");
    tree.set_height(root, Length::points(20.0))
        .expect("set root height");

    for width in [10.0, 20.0] {
        let child = fixed_linear_gravity_child(&mut tree, width, 10.0);
        tree.append_child(root, child)
            .expect("append horizontal gravity child");
    }

    (
        "horizontal linear gravity overrides justify-content".to_owned(),
        tree,
        root,
        Constraints::definite(100.0, 20.0),
    )
}

fn rtl_horizontal_linear_gravity_tree(
    gravity: LinearGravity,
) -> (String, StandaloneTree, NodeId, Constraints) {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_direction(root, Direction::Rtl)
        .expect("set root direction");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root linear orientation");
    tree.set_linear_gravity(root, gravity)
        .expect("set root linear gravity");
    tree.set_width(root, Length::points(100.0))
        .expect("set root width");
    tree.set_height(root, Length::points(20.0))
        .expect("set root height");

    for width in [10.0, 20.0] {
        let child = fixed_linear_gravity_child(&mut tree, width, 10.0);
        tree.append_child(root, child)
            .expect("append RTL horizontal gravity child");
    }

    (
        format!("RTL horizontal linear gravity {gravity:?} physical front"),
        tree,
        root,
        Constraints::definite(100.0, 20.0),
    )
}

fn fixed_linear_gravity_child(tree: &mut StandaloneTree, width: f32, height: f32) -> NodeId {
    let child = tree.create_default_node();
    tree.set_display(child, Display::Block)
        .expect("set gravity child display");
    tree.set_width(child, Length::points(width))
        .expect("set gravity child width");
    tree.set_height(child, Length::points(height))
        .expect("set gravity child height");
    child
}

fn linear_layout_gravity_variants() -> [LinearLayoutGravity; 13] {
    [
        LinearLayoutGravity::None,
        LinearLayoutGravity::Top,
        LinearLayoutGravity::Left,
        LinearLayoutGravity::Start,
        LinearLayoutGravity::Right,
        LinearLayoutGravity::Bottom,
        LinearLayoutGravity::End,
        LinearLayoutGravity::CenterHorizontal,
        LinearLayoutGravity::CenterVertical,
        LinearLayoutGravity::Center,
        LinearLayoutGravity::FillHorizontal,
        LinearLayoutGravity::FillVertical,
        LinearLayoutGravity::Stretch,
    ]
}

fn linear_layout_gravity_end_overrides_stretch_tree()
-> (String, StandaloneTree, NodeId, Constraints) {
    let mut tree = StandaloneTree::new();
    let root = linear_layout_gravity_root(&mut tree, LinearOrientation::Vertical, Direction::Ltr);
    let child = linear_layout_gravity_child(&mut tree, LinearLayoutGravity::End, 20.0, 10.0);
    tree.append_child(root, child).expect("append child");

    (
        "linear layout-gravity End overrides container stretch".to_owned(),
        tree,
        root,
        Constraints::definite(100.0, 100.0),
    )
}

fn linear_layout_gravity_stretch_overrides_explicit_cross_size_tree()
-> (String, StandaloneTree, NodeId, Constraints) {
    let mut tree = StandaloneTree::new();
    let root = linear_layout_gravity_root(&mut tree, LinearOrientation::Vertical, Direction::Ltr);
    let child = linear_layout_gravity_child(&mut tree, LinearLayoutGravity::Stretch, 20.0, 10.0);
    tree.append_child(root, child).expect("append child");

    (
        "linear layout-gravity Stretch overrides explicit cross size".to_owned(),
        tree,
        root,
        Constraints::definite(100.0, 100.0),
    )
}

fn linear_layout_gravity_stretch_overrides_weighted_cross_size_tree()
-> (String, StandaloneTree, NodeId, Constraints) {
    let mut tree = StandaloneTree::new();
    let root = linear_layout_gravity_root(&mut tree, LinearOrientation::Vertical, Direction::Ltr);
    let child = tree.create_default_node();
    tree.set_display(child, Display::Block)
        .expect("set weighted child display");
    tree.set_width(child, Length::points(20.0))
        .expect("set weighted child width");
    tree.set_linear_weight(child, 1.0)
        .expect("set weighted child weight");
    tree.set_linear_layout_gravity(child, LinearLayoutGravity::Stretch)
        .expect("set weighted child layout gravity");
    tree.append_child(root, child).expect("append child");

    (
        "linear layout-gravity Stretch overrides weighted cross size".to_owned(),
        tree,
        root,
        Constraints::definite(100.0, 100.0),
    )
}

fn vertical_linear_layout_gravity_mapping_tree(
    gravity: LinearLayoutGravity,
) -> (String, StandaloneTree, NodeId, Constraints) {
    let mut tree = StandaloneTree::new();
    let root = linear_layout_gravity_root(&mut tree, LinearOrientation::Vertical, Direction::Ltr);
    let child = linear_layout_gravity_child(&mut tree, gravity, 20.0, 10.0);
    tree.append_child(root, child).expect("append child");

    (
        format!("vertical linear layout-gravity {gravity:?} mapping"),
        tree,
        root,
        Constraints::definite(100.0, 100.0),
    )
}

fn horizontal_linear_layout_gravity_mapping_tree(
    gravity: LinearLayoutGravity,
) -> (String, StandaloneTree, NodeId, Constraints) {
    let mut tree = StandaloneTree::new();
    let root = linear_layout_gravity_root(&mut tree, LinearOrientation::Horizontal, Direction::Ltr);
    let child = linear_layout_gravity_child(&mut tree, gravity, 20.0, 10.0);
    tree.append_child(root, child).expect("append child");

    (
        format!("horizontal linear layout-gravity {gravity:?} mapping"),
        tree,
        root,
        Constraints::definite(100.0, 100.0),
    )
}

fn rtl_vertical_linear_layout_gravity_tree(
    gravity: LinearLayoutGravity,
) -> (String, StandaloneTree, NodeId, Constraints) {
    let mut tree = StandaloneTree::new();
    let root = linear_layout_gravity_root(&mut tree, LinearOrientation::Vertical, Direction::Rtl);
    let child = linear_layout_gravity_child(&mut tree, gravity, 20.0, 10.0);
    tree.append_child(root, child).expect("append child");

    (
        format!("RTL vertical linear layout-gravity {gravity:?} physical side"),
        tree,
        root,
        Constraints::definite(100.0, 100.0),
    )
}

fn linear_layout_gravity_root(
    tree: &mut StandaloneTree,
    orientation: LinearOrientation,
    direction: Direction,
) -> NodeId {
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_direction(root, direction)
        .expect("set root direction");
    tree.set_linear_orientation(root, orientation)
        .expect("set root linear orientation");
    tree.set_width(root, Length::points(100.0))
        .expect("set root width");
    tree.set_height(root, Length::points(100.0))
        .expect("set root height");
    root
}

fn linear_layout_gravity_child(
    tree: &mut StandaloneTree,
    gravity: LinearLayoutGravity,
    width: f32,
    height: f32,
) -> NodeId {
    let child = fixed_linear_gravity_child(tree, width, height);
    tree.set_linear_layout_gravity(child, gravity)
        .expect("set child layout gravity");
    child
}

fn linear_cross_gravity_variants() -> [LinearCrossGravity; 5] {
    [
        LinearCrossGravity::None,
        LinearCrossGravity::Start,
        LinearCrossGravity::End,
        LinearCrossGravity::Center,
        LinearCrossGravity::Stretch,
    ]
}

fn vertical_linear_cross_gravity_mapping_tree(
    cross_gravity: LinearCrossGravity,
) -> (String, StandaloneTree, NodeId, Constraints) {
    let mut tree = StandaloneTree::new();
    let root = linear_cross_gravity_root(
        &mut tree,
        LinearOrientation::Vertical,
        cross_gravity,
        AlignItems::FlexStart,
    );
    let child = fixed_linear_gravity_child(&mut tree, 20.0, 10.0);
    tree.append_child(root, child).expect("append child");

    (
        format!("vertical linear cross-gravity {cross_gravity:?} mapping"),
        tree,
        root,
        Constraints::definite(100.0, 100.0),
    )
}

fn horizontal_linear_cross_gravity_mapping_tree(
    cross_gravity: LinearCrossGravity,
) -> (String, StandaloneTree, NodeId, Constraints) {
    let mut tree = StandaloneTree::new();
    let root = linear_cross_gravity_root(
        &mut tree,
        LinearOrientation::Horizontal,
        cross_gravity,
        AlignItems::FlexStart,
    );
    let child = fixed_linear_gravity_child(&mut tree, 20.0, 10.0);
    tree.append_child(root, child).expect("append child");

    (
        format!("horizontal linear cross-gravity {cross_gravity:?} mapping"),
        tree,
        root,
        Constraints::definite(100.0, 100.0),
    )
}

fn linear_cross_gravity_root(
    tree: &mut StandaloneTree,
    orientation: LinearOrientation,
    cross_gravity: LinearCrossGravity,
    align_items: AlignItems,
) -> NodeId {
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_linear_orientation(root, orientation)
        .expect("set root linear orientation");
    tree.set_align_items(root, align_items)
        .expect("set root align items");
    tree.set_linear_cross_gravity(root, cross_gravity)
        .expect("set root linear cross gravity");
    tree.set_width(root, Length::points(100.0))
        .expect("set root width");
    tree.set_height(root, Length::points(100.0))
        .expect("set root height");
    root
}

fn linear_cross_gravity_auto_margins_and_baseline_tree() -> (StandaloneTree, NodeId, Constraints) {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root linear orientation");
    tree.set_linear_cross_gravity(root, LinearCrossGravity::End)
        .expect("set root linear cross gravity");
    tree.set_align_items(root, AlignItems::FlexStart)
        .expect("set root align items");
    tree.set_width(root, Length::points(100.0))
        .expect("set root width");
    tree.set_height(root, Length::points(100.0))
        .expect("set root height");

    let paired_auto_margin = tree.create_default_measured_node(Size::new(20.0, 10.0));
    tree.set_display(paired_auto_margin, Display::Block)
        .expect("set paired auto-margin display");
    tree.set_baseline(paired_auto_margin, Some(4.0))
        .expect("set paired auto-margin baseline");
    tree.set_margin(paired_auto_margin, StandaloneEdge::Top, Length::Auto)
        .expect("set paired auto-margin top");
    tree.set_margin(paired_auto_margin, StandaloneEdge::Bottom, Length::Auto)
        .expect("set paired auto-margin bottom");

    let start_auto_margin = tree.create_default_measured_node(Size::new(16.0, 12.0));
    tree.set_display(start_auto_margin, Display::Block)
        .expect("set start auto-margin display");
    tree.set_baseline(start_auto_margin, Some(5.0))
        .expect("set start auto-margin baseline");
    tree.set_margin(start_auto_margin, StandaloneEdge::Top, Length::Auto)
        .expect("set start auto-margin top");

    tree.append_child(root, paired_auto_margin)
        .expect("append paired auto-margin child");
    tree.append_child(root, start_auto_margin)
        .expect("append start auto-margin child");

    (tree, root, Constraints::definite(100.0, 100.0))
}

fn horizontal_linear_empty_container_baseline_tree()
-> (&'static str, StandaloneTree, NodeId, Constraints) {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_linear_orientation(root, LinearOrientation::Horizontal)
        .expect("set root linear orientation");
    tree.set_width(root, Length::points(20.0))
        .expect("set root width");
    tree.set_height(root, Length::points(10.0))
        .expect("set root height");

    (
        "horizontal linear empty container baseline",
        tree,
        root,
        Constraints::definite(20.0, 10.0),
    )
}

fn vertical_linear_empty_container_baseline_tree()
-> (&'static str, StandaloneTree, NodeId, Constraints) {
    let mut tree = StandaloneTree::new();
    let root = tree.create_default_node();
    tree.set_display(root, Display::Linear)
        .expect("set root display");
    tree.set_width(root, Length::points(20.0))
        .expect("set root width");
    tree.set_height(root, Length::points(10.0))
        .expect("set root height");

    (
        "vertical linear empty container baseline",
        tree,
        root,
        Constraints::definite(20.0, 10.0),
    )
}

fn linear_overflowing_cross_axis_auto_margins_tree() -> (StandaloneTree, NodeId, Constraints) {
    let mut tree = StandaloneTree::new();
    let root = linear_cross_gravity_root(
        &mut tree,
        LinearOrientation::Horizontal,
        LinearCrossGravity::None,
        AlignItems::FlexStart,
    );

    let child = tree.create_default_node();
    tree.set_display(child, Display::Block)
        .expect("set child display");
    tree.set_width(child, Length::points(20.0))
        .expect("set child width");
    tree.set_height(child, Length::points(140.0))
        .expect("set child height");
    tree.set_margin(child, StandaloneEdge::Top, Length::Auto)
        .expect("set child top auto margin");
    tree.set_margin(child, StandaloneEdge::Bottom, Length::Auto)
        .expect("set child bottom auto margin");
    tree.append_child(root, child).expect("append child");

    (tree, root, Constraints::definite(100.0, 100.0))
}
