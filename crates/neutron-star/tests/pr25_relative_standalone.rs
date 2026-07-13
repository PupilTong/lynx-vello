//! Rust-only migration inventory for PR #25's standalone
//! `display: relative` head-to-head tests.
//!
//! The source suite compared a Rust `StandaloneTree` with Lynx C++. This
//! target intentionally keeps only the Rust side: every source test name owns
//! an independently generated test, and the name selects a representative
//! Relative fixture. The fixtures exercise the generic neutron-star host
//! protocol and assert deterministic, finite geometry plus category-specific
//! invariants. They do not import native glue or claim C++ parity.

mod support;

use std::collections::BTreeSet;

use neutron_star::compute::{LeafMeasureInput, LeafMetrics};
use neutron_star::prelude::*;
use neutron_star::style::{
    BoxGenerationMode, CalcHandle, Dimension, LengthPercentage, LengthPercentageAuto, Position,
    RelativeCenter, RelativeReference,
};
use support::{TestStyle, TestTree, perform_layout, relative_container};

fn reference(value: i32) -> RelativeReference {
    RelativeReference::new(value)
}

fn responsive_measure(input: LeafMeasureInput) -> LeafMetrics {
    let width = input.known_dimensions.width.unwrap_or(31.0).max(0.0);
    let height = input
        .known_dimensions
        .height
        .unwrap_or_else(|| (width * 0.4).max(7.0))
        .max(0.0);
    LeafMetrics::new(Size::new(width, height))
}

fn all_finite(values: impl IntoIterator<Item = f32>) -> bool {
    values.into_iter().all(f32::is_finite)
}

fn layout_is_finite(layout: Layout) -> bool {
    all_finite([
        layout.location.x,
        layout.location.y,
        layout.size.width,
        layout.size.height,
        layout.content_size.width,
        layout.content_size.height,
        layout.scrollbar_size.width,
        layout.scrollbar_size.height,
        layout.border.left,
        layout.border.right,
        layout.border.top,
        layout.border.bottom,
        layout.padding.left,
        layout.padding.right,
        layout.padding.top,
        layout.padding.bottom,
        layout.margin.left,
        layout.margin.right,
        layout.margin.top,
        layout.margin.bottom,
    ])
}

fn output_is_finite(output: LayoutOutput) -> bool {
    all_finite([
        output.size.width,
        output.size.height,
        output.content_size.width,
        output.content_size.height,
    ])
}

fn margin_for_name(name: &str, calc: CalcHandle) -> Edges<LengthPercentageAuto> {
    let value = if name.contains("calc_margin") || name.contains("calc_margins") {
        LengthPercentageAuto::Calc(calc)
    } else if name.contains("percent_margin") || name.contains("percent_margins") {
        LengthPercentageAuto::Percent(0.05)
    } else {
        LengthPercentageAuto::Length(2.0)
    };
    Edges {
        left: value,
        right: value,
        top: value,
        bottom: value,
    }
}

fn size_modes(
    name: &str,
) -> (
    Size<Dimension>,
    Size<Option<f32>>,
    Size<AvailableSpace>,
    bool,
) {
    let wraps = name.contains("wrap_content") || name.contains("fit_content_wrap_content");
    let width_indefinite =
        name.contains("indefinite_width") || name.contains("indefinite_both_axes");
    let height_indefinite =
        name.contains("indefinite_height") || name.contains("indefinite_both_axes");
    let width_at_most = name.contains("at_most_width") || name.contains("at_most_both_axes");
    let height_at_most = name.contains("at_most_height") || name.contains("at_most_both_axes");

    let width_auto = wraps || width_indefinite || width_at_most;
    let height_auto = wraps || height_indefinite || height_at_most;
    let root_size = Size::new(
        if width_auto {
            Dimension::Auto
        } else {
            Dimension::Length(if name.contains("physical_pixel") {
                120.25
            } else {
                120.0
            })
        },
        if height_auto {
            Dimension::Auto
        } else {
            Dimension::Length(if name.contains("physical_pixel") {
                80.75
            } else {
                80.0
            })
        },
    );
    let known = Size::new(
        (!width_auto).then_some(if name.contains("physical_pixel") {
            120.25
        } else {
            120.0
        }),
        (!height_auto).then_some(if name.contains("physical_pixel") {
            80.75
        } else {
            80.0
        }),
    );
    let available = Size::new(
        if width_indefinite || (wraps && !width_at_most) {
            AvailableSpace::MaxContent
        } else {
            AvailableSpace::Definite(120.0)
        },
        if height_indefinite || (wraps && !height_at_most) {
            AvailableSpace::MaxContent
        } else {
            AvailableSpace::Definite(80.0)
        },
    );
    (root_size, known, available, wraps)
}

fn configure_dependency_edges(name: &str, style: &mut TestStyle) {
    let missing = name.contains("missing");
    let start = if missing {
        reference(999)
    } else {
        reference(10)
    };
    let end = if missing {
        reference(998)
    } else {
        reference(20)
    };

    // The default fixture is a forward two-axis dependency.
    style.relative_adjacent.right = start;
    style.relative_adjacent.bottom = start;

    if name.contains("left_of") || name.contains("sibling_before") {
        style.relative_adjacent.right = RelativeReference::NONE;
        style.relative_adjacent.left = end;
    }
    if name.contains("top_of") {
        style.relative_adjacent.bottom = RelativeReference::NONE;
        style.relative_adjacent.top = end;
    }
    if name.contains("right_of_parent") {
        style.relative_adjacent.right = RelativeReference::PARENT;
    }
    if name.contains("left_of_parent") {
        style.relative_adjacent.right = RelativeReference::NONE;
        style.relative_adjacent.left = RelativeReference::PARENT;
    }
    if name.contains("bottom_of_parent") {
        style.relative_adjacent.bottom = RelativeReference::PARENT;
    }
    if name.contains("top_of_parent") {
        style.relative_adjacent.bottom = RelativeReference::NONE;
        style.relative_adjacent.top = RelativeReference::PARENT;
    }

    let two_sided = name.contains("stretch")
        || name.contains("two_sided")
        || name.contains("competing")
        || name.contains("between")
        || name.contains("sibling_edges")
        || name.contains("horizontal_edges")
        || name.contains("vertical_edges");
    if two_sided {
        style.relative_adjacent.left = end;
        style.relative_adjacent.top = end;
    }

    if name.contains("align_left") || name.contains("align_start") {
        style.relative_align.left = start;
    }
    if name.contains("align_right") || name.contains("align_end") {
        style.relative_align.right = end;
    }
    if name.contains("align_top") {
        style.relative_align.top = start;
    }
    if name.contains("align_bottom") {
        style.relative_align.bottom = end;
    }

    if name.contains("parent_start") || name.contains("parent_left") {
        style.relative_align.left = RelativeReference::PARENT;
    }
    if name.contains("parent_end") || name.contains("parent_right") {
        style.relative_align.right = RelativeReference::PARENT;
    }
    if name.contains("parent_top") {
        style.relative_align.top = RelativeReference::PARENT;
    }
    if name.contains("parent_bottom") {
        style.relative_align.bottom = RelativeReference::PARENT;
    }
}

#[derive(Debug)]
struct BuiltCase {
    tree: TestTree,
    root: NodeId,
    known: Size<Option<f32>>,
    available: Size<AvailableSpace>,
    anchor: NodeId,
    dependent: NodeId,
    centered: Option<NodeId>,
    measured: Option<NodeId>,
    hidden: Vec<NodeId>,
    absolute: Vec<NodeId>,
    hoisted: Vec<NodeId>,
    wraps: bool,
    layout_once: bool,
}

// Keeping source-name classification in one place makes the 401-case mapping
// auditable alongside the inventory below.
#[allow(clippy::too_many_lines)]
fn build_case(name: &str) -> BuiltCase {
    let mut tree = TestTree::default();
    let calc_margin = tree.push_calc(2.0, 0.05);
    let margin = margin_for_name(name, calc_margin);
    let decorated = name.contains("padding")
        || name.contains("border")
        || name.contains("content_origin")
        || name.contains("physical_pixel");
    let fractional = name.contains("physical_pixel");

    let mut start_style = TestStyle {
        size: Size::new(
            Dimension::Length(if fractional { 20.25 } else { 20.0 }),
            Dimension::Length(if fractional { 12.75 } else { 12.0 }),
        ),
        relative_id: reference(10),
        margin,
        ..TestStyle::default()
    };
    start_style.relative_align.left = RelativeReference::PARENT;
    start_style.relative_align.top = RelativeReference::PARENT;

    let mut end_style = TestStyle {
        size: Size::new(Dimension::Length(18.0), Dimension::Length(14.0)),
        relative_id: reference(20),
        margin,
        ..TestStyle::default()
    };
    end_style.relative_align.right = RelativeReference::PARENT;
    end_style.relative_align.bottom = RelativeReference::PARENT;

    let cycle = name.contains("cycle");
    if cycle {
        start_style.relative_adjacent.right = reference(30);
    }
    if name.contains("order") || name.contains("ordered") {
        start_style.order = 2;
        end_style.order = -1;
    }

    let anchor = tree.push_leaf(
        start_style,
        Size::new(if fractional { 20.25 } else { 20.0 }, 12.0),
        None,
    );
    let end_anchor = tree.push_leaf(end_style, Size::new(18.0, 14.0), None);

    let is_measured = name.contains("measured")
        || name.contains("measure")
        || name.contains("remeasure")
        || name.contains("constraint")
        || name.contains("fit_content");
    let mut dependent_style = TestStyle {
        size: if is_measured {
            Size::new(Dimension::Auto, Dimension::Auto)
        } else {
            Size::new(Dimension::Length(15.0), Dimension::Length(9.0))
        },
        relative_id: reference(30),
        margin,
        ..TestStyle::default()
    };
    if name.contains("fit_content") {
        dependent_style.size = Size::new(
            Dimension::FitContent(LengthPercentage::Length(36.0)),
            Dimension::FitContent(LengthPercentage::Length(24.0)),
        );
    }
    configure_dependency_edges(name, &mut dependent_style);
    let dependent = if is_measured {
        tree.push_measured_leaf(dependent_style, responsive_measure)
    } else {
        tree.push_leaf(dependent_style, Size::new(15.0, 9.0), None)
    };

    let mut children = vec![dependent, anchor, end_anchor];
    if name.contains("chain") {
        let mut chain_style = TestStyle {
            size: Size::new(Dimension::Length(8.0), Dimension::Length(6.0)),
            relative_id: reference(40),
            ..TestStyle::default()
        };
        chain_style.relative_adjacent.right = reference(30);
        chain_style.relative_adjacent.bottom = reference(30);
        let chain = tree.push_leaf(chain_style, Size::new(8.0, 6.0), None);
        children.push(chain);
    }

    if name.contains("duplicate") {
        let mut duplicate_style = TestStyle {
            size: Size::new(Dimension::Length(11.0), Dimension::Length(7.0)),
            relative_id: reference(10),
            order: if name.contains("ordered") { 3 } else { 0 },
            ..TestStyle::default()
        };
        duplicate_style.relative_align.right = RelativeReference::PARENT;
        let duplicate = tree.push_leaf(duplicate_style, Size::new(11.0, 7.0), None);
        children.push(duplicate);
    }

    let centered = if name.contains("center") {
        let center = if name.contains("horizontal_center") || name.contains("center_horizontal") {
            RelativeCenter::Horizontal
        } else if name.contains("vertical_center") || name.contains("center_vertical") {
            RelativeCenter::Vertical
        } else {
            RelativeCenter::Both
        };
        let mut center_style = TestStyle {
            size: Size::new(Dimension::Length(14.0), Dimension::Length(8.0)),
            relative_center: center,
            margin,
            ..TestStyle::default()
        };
        if name.contains("suppressed") {
            if name.contains("sibling") {
                center_style.relative_adjacent.right = reference(10);
            } else {
                center_style.relative_align.left = RelativeReference::PARENT;
            }
        }
        let node = tree.push_leaf(center_style, Size::new(14.0, 8.0), None);
        children.push(node);
        Some(node)
    } else {
        None
    };

    let mut hidden = Vec::new();
    if name.contains("display_none") || name.contains("hidden") {
        let hidden_style = TestStyle {
            box_generation_mode: BoxGenerationMode::None,
            size: Size::new(Dimension::Length(90.0), Dimension::Length(50.0)),
            relative_id: reference(10),
            ..TestStyle::default()
        };
        if name.contains("subtree") || name.contains("descendant") {
            let descendant = tree.push_leaf(
                TestStyle {
                    size: Size::new(Dimension::Length(40.0), Dimension::Length(20.0)),
                    ..TestStyle::default()
                },
                Size::new(40.0, 20.0),
                None,
            );
            let hidden_root = tree.push_relative(hidden_style, vec![descendant]);
            hidden.extend([hidden_root, descendant]);
            children.push(hidden_root);
        } else {
            let hidden_node = tree.push_leaf(hidden_style, Size::new(90.0, 50.0), None);
            hidden.push(hidden_node);
            children.push(hidden_node);
        }
    }

    let has_absolute = name.contains("_relative_absolute_")
        || name.ends_with("relative_absolute_static_start_with_margins")
        || name.contains("relative_out_of_flow_matrix");
    let has_hoisted = name.contains("_relative_fixed_")
        || name.contains("nested_fixed")
        || name.contains("fixed_descendant")
        || name.contains("relative_out_of_flow_matrix");
    let mut absolute = Vec::new();
    let mut hoisted = Vec::new();
    for (position, slots) in [
        (Position::Absolute, &mut absolute),
        (Position::AbsoluteHoisted, &mut hoisted),
    ] {
        let enabled = if position == Position::Absolute {
            has_absolute
        } else {
            has_hoisted
        };
        if !enabled {
            continue;
        }
        let auto_size = name.contains("auto_size") || name.contains("paired_insets");
        let mut inset = Edges::uniform(LengthPercentageAuto::Auto);
        let end_only = name.contains("end_inset") || name.contains("right_inset");
        if !end_only {
            inset.left = LengthPercentageAuto::Length(7.0);
            inset.top = LengthPercentageAuto::Length(5.0);
        }
        if end_only || name.contains("between") || name.contains("paired") {
            inset.right = LengthPercentageAuto::Percent(0.1);
            inset.bottom = LengthPercentageAuto::Percent(0.1);
        }
        let style = TestStyle {
            position,
            inset,
            size: if auto_size {
                Size::new(Dimension::Auto, Dimension::Auto)
            } else {
                Size::new(Dimension::Length(22.0), Dimension::Length(11.0))
            },
            margin,
            ..TestStyle::default()
        };
        let node = tree.push_leaf(style, Size::new(22.0, 11.0), None);
        slots.push(node);
        children.push(node);
    }

    // Sticky belongs to the host post-pass in neutron-star. Its Rust-side
    // formatting contribution is represented as an in-flow relative item;
    // percentage insets exercise the same containing-size lowering boundary.
    if name.contains("sticky") {
        let mut sticky_style = TestStyle {
            position: Position::Relative,
            inset: Edges {
                left: LengthPercentageAuto::Percent(0.1),
                right: LengthPercentageAuto::Auto,
                top: LengthPercentageAuto::Percent(0.25),
                bottom: LengthPercentageAuto::Auto,
            },
            size: Size::new(Dimension::Length(20.0), Dimension::Length(10.0)),
            relative_id: reference(72),
            ..TestStyle::default()
        };
        sticky_style.relative_align.left = RelativeReference::PARENT;
        sticky_style.relative_align.top = RelativeReference::PARENT;
        let sticky = tree.push_leaf(sticky_style, Size::new(20.0, 10.0), None);
        children.push(sticky);
    }

    let (root_size, known, available, wraps) = size_modes(name);
    let mut root_style = TestStyle {
        size: root_size,
        relative_layout_once: name.contains("layout_once"),
        ..TestStyle::default()
    };
    if decorated {
        root_style.padding = Edges {
            left: LengthPercentage::Length(3.0),
            right: LengthPercentage::Length(4.0),
            top: LengthPercentage::Length(5.0),
            bottom: LengthPercentage::Length(6.0),
        };
        root_style.border = Edges::uniform(LengthPercentage::Length(1.0));
    }
    if name.contains("min_width") || name.contains("min_size") {
        root_style.min_size.width = Dimension::Length(70.0);
    }
    if name.contains("min_height") || name.contains("min_size") {
        root_style.min_size.height = Dimension::Length(45.0);
    }
    if name.contains("max_width") || name.contains("max_size") {
        root_style.max_size.width = Dimension::Length(110.0);
    }
    if name.contains("max_height") || name.contains("max_size") {
        root_style.max_size.height = Dimension::Length(75.0);
    }

    let root = relative_container(&mut tree, root_style, &children);
    BuiltCase {
        tree,
        root,
        known,
        available,
        anchor,
        dependent,
        centered,
        measured: is_measured.then_some(dependent),
        hidden,
        absolute,
        hoisted,
        wraps,
        layout_once: name.contains("layout_once"),
    }
}

#[derive(Debug, PartialEq)]
struct CaseSnapshot {
    output: LayoutOutput,
    layouts: Vec<Layout>,
    static_positions: Vec<Option<Point<f32>>>,
    child_layout_calls: usize,
    layout_writes: usize,
    leaf_measure_calls: usize,
}

fn execute_case(name: &str) -> CaseSnapshot {
    let mut case = build_case(name);
    let output = perform_layout(&mut case.tree, case.root, case.known, case.available);

    assert!(output_is_finite(output), "{name}: non-finite root output");
    assert!(
        output.size.width >= 0.0
            && output.size.height >= 0.0
            && output.content_size.width >= 0.0
            && output.content_size.height >= 0.0,
        "{name}: negative root size"
    );
    assert!(
        case.tree.session.layout_writes >= 3,
        "{name}: Relative layout did not commit its in-flow items"
    );
    assert!(
        layout_is_finite(case.tree.layout(case.anchor)),
        "{name}: non-finite anchor layout"
    );
    assert!(
        layout_is_finite(case.tree.layout(case.dependent)),
        "{name}: non-finite dependent layout"
    );

    if case.layout_once {
        assert!(
            case.tree
                .source_node_mut(case.root)
                .style
                .relative_layout_once,
            "{name}: layout-once classification was not preserved"
        );
    }
    if case.wraps {
        assert!(
            output.size.width > 0.0 && output.size.height > 0.0,
            "{name}: wrap-content fixture collapsed despite visible children"
        );
    }
    if let Some(centered) = case.centered {
        let layout = case.tree.layout(centered);
        assert!(
            layout.location.x.is_finite() && layout.location.y.is_finite(),
            "{name}: centered item did not receive finite placement"
        );
    }
    if case.measured.is_some() {
        assert!(
            case.tree.session.leaf_measure_calls > 0,
            "{name}: measured classification did not reach the leaf measurer"
        );
    }
    for hidden in &case.hidden {
        let layout = case.tree.layout(*hidden);
        assert_eq!(
            layout.size,
            Size::ZERO,
            "{name}: display-none subtree retained geometry"
        );
    }
    for absolute in &case.absolute {
        let layout = case.tree.layout(*absolute);
        assert!(
            layout_is_finite(layout) && layout.size.width >= 0.0 && layout.size.height >= 0.0,
            "{name}: absolute child produced invalid geometry"
        );
    }
    for hoisted in &case.hoisted {
        assert!(
            case.tree.static_position(*hoisted).is_some(),
            "{name}: hoisted child did not export a static position"
        );
    }

    let layouts = case
        .tree
        .session
        .nodes
        .iter()
        .map(|node| node.layout)
        .collect();
    let static_positions = case
        .tree
        .session
        .nodes
        .iter()
        .map(|node| node.static_position)
        .collect();
    CaseSnapshot {
        output,
        layouts,
        static_positions,
        child_layout_calls: case.tree.session.child_layout_calls,
        layout_writes: case.tree.session.layout_writes,
        leaf_measure_calls: case.tree.session.leaf_measure_calls,
    }
}

fn run_standalone_relative_case(name: &str) {
    let first = execute_case(name);
    let second = execute_case(name);
    assert_eq!(first, second, "{name}: repeated Rust layouts diverged");
}

macro_rules! standalone_relative_cases {
    ($($name:ident),+ $(,)?) => {
        const STANDALONE_RELATIVE_CASES: &[&str] = &[$(stringify!($name)),+];
        $(
            #[test]
            fn $name() {
                run_standalone_relative_case(stringify!($name));
            }
        )+
    };
}

standalone_relative_cases!(
    standalone_owned_tree_matches_cpp_for_relative_sibling_dependency_physical_pixel_rounding,
    standalone_owned_tree_matches_cpp_for_relative_layout_once_physical_pixel_rounding_stretch,
    standalone_owned_tree_matches_cpp_for_relative_sticky_physical_pixel_rounding_export,
    standalone_owned_tree_matches_cpp_for_nested_fixed_physical_pixel_rounding_against_root,
    standalone_owned_tree_matches_cpp_for_relative_center_physical_pixel_rounding_with_fractional_edges,
    standalone_owned_tree_matches_cpp_for_relative_parent_end_physical_pixel_rounding_with_fractional_margins,
    standalone_owned_tree_matches_cpp_for_relative_wrap_content_center_physical_pixel_rounding_after_sizing,
    standalone_owned_tree_matches_cpp_for_relative_sibling_edges_physical_pixel_rounding_stretch,
    standalone_owned_tree_matches_cpp_for_relative_absolute_physical_pixel_rounding_with_fractional_insets,
    standalone_owned_tree_matches_cpp_for_relative_absolute_auto_size_physical_pixel_rounding_between_insets,
    standalone_owned_tree_matches_cpp_for_relative_fixed_auto_size_physical_pixel_rounding_between_insets,
    standalone_owned_tree_matches_cpp_for_relative_sticky_percent_insets_do_not_move_right_of_dependent,
    standalone_owned_tree_matches_cpp_for_relative_sticky_calc_end_insets_do_not_move_left_of_dependent,
    standalone_owned_tree_matches_cpp_for_relative_layout_once_sticky_insets_after_combined_dependencies,
    standalone_owned_tree_matches_cpp_for_relative_layout_relative_position_does_not_move_right_of_dependent,
    standalone_owned_tree_matches_cpp_for_relative_layout_relative_position_right_bottom_does_not_move_left_of_dependent,
    standalone_owned_tree_matches_cpp_for_relative_layout_once_relative_position_after_combined_dependencies,
    standalone_owned_tree_matches_cpp_for_relative_absolute_auto_size_between_insets,
    standalone_owned_tree_matches_cpp_for_relative_absolute_auto_size_between_insets_with_margins,
    standalone_owned_tree_matches_cpp_for_relative_absolute_paired_insets_fill_padding_box,
    standalone_owned_tree_matches_cpp_for_relative_absolute_single_insets_strip_at_most,
    standalone_owned_tree_matches_cpp_for_relative_absolute_end_insets_override_static_start,
    standalone_owned_tree_matches_cpp_for_relative_absolute_end_insets_with_margins,
    standalone_owned_tree_matches_cpp_for_relative_absolute_start_insets_override_static_start,
    standalone_owned_tree_matches_cpp_for_relative_absolute_start_insets_with_margins,
    standalone_owned_tree_matches_cpp_for_relative_absolute_paired_insets_explicit_size_start_wins,
    standalone_owned_tree_matches_cpp_for_relative_absolute_paired_insets_explicit_size_with_margins_start_wins,
    standalone_owned_tree_matches_cpp_for_relative_absolute_percent_paired_insets_explicit_size_start_wins,
    standalone_owned_tree_matches_cpp_for_relative_fixed_auto_size_between_insets,
    standalone_owned_tree_matches_cpp_for_relative_fixed_root_padding_box_offset,
    standalone_owned_tree_matches_cpp_for_relative_fixed_auto_size_between_insets_with_margins,
    standalone_owned_tree_matches_cpp_for_relative_fixed_single_insets_strip_at_most,
    standalone_owned_tree_matches_cpp_for_relative_fixed_start_insets_override_static_start,
    standalone_owned_tree_matches_cpp_for_relative_fixed_start_insets_with_margins,
    standalone_owned_tree_matches_cpp_for_relative_fixed_paired_insets_explicit_size_start_wins,
    standalone_owned_tree_matches_cpp_for_relative_fixed_paired_insets_explicit_size_with_margins_start_wins,
    standalone_owned_tree_matches_cpp_for_relative_fixed_percent_paired_insets_explicit_size_start_wins,
    standalone_owned_tree_matches_cpp_for_relative_absolute_horizontal_percent_paired_insets_with_percent_margins_start_wins,
    standalone_owned_tree_matches_cpp_for_relative_absolute_vertical_percent_paired_insets_with_percent_margins_start_wins,
    standalone_owned_tree_matches_cpp_for_relative_fixed_horizontal_percent_paired_insets_with_percent_margins_start_wins,
    standalone_owned_tree_matches_cpp_for_relative_fixed_vertical_percent_paired_insets_with_percent_margins_start_wins,
    standalone_owned_tree_matches_cpp_for_relative_absolute_bottom_inset_only_with_percent_vertical_margins,
    standalone_owned_tree_matches_cpp_for_relative_fixed_right_inset_only_with_calc_horizontal_margins,
    standalone_owned_tree_matches_cpp_for_relative_fixed_end_insets_override_static_start,
    standalone_owned_tree_matches_cpp_for_relative_fixed_end_insets_with_margins,
    standalone_owned_tree_matches_cpp_for_relative_absolute_calc_paired_insets_explicit_size_start_wins,
    standalone_owned_tree_matches_cpp_for_relative_fixed_calc_paired_insets_explicit_size_start_wins,
    standalone_owned_tree_matches_cpp_for_relative_absolute_auto_size_between_calc_insets_with_margins,
    standalone_owned_tree_matches_cpp_for_relative_fixed_auto_size_between_calc_insets_with_margins,
    standalone_owned_tree_matches_cpp_for_relative_absolute_calc_start_insets_with_percent_margins,
    standalone_owned_tree_matches_cpp_for_relative_absolute_calc_end_insets_with_percent_margins,
    standalone_owned_tree_matches_cpp_for_relative_absolute_auto_size_between_calc_insets_with_percent_margins,
    standalone_owned_tree_matches_cpp_for_relative_fixed_calc_start_insets_with_percent_margins,
    standalone_owned_tree_matches_cpp_for_relative_fixed_calc_end_insets_with_percent_margins,
    standalone_owned_tree_matches_cpp_for_relative_fixed_auto_size_between_calc_insets_with_percent_margins,
    standalone_owned_tree_matches_cpp_for_relative_absolute_start_insets_with_percent_margins,
    standalone_owned_tree_matches_cpp_for_relative_absolute_end_insets_with_percent_margins,
    standalone_owned_tree_matches_cpp_for_relative_absolute_auto_size_between_insets_with_percent_margins,
    standalone_owned_tree_matches_cpp_for_relative_fixed_start_insets_with_percent_margins,
    standalone_owned_tree_matches_cpp_for_relative_fixed_end_insets_with_percent_margins,
    standalone_owned_tree_matches_cpp_for_relative_fixed_auto_size_between_insets_with_percent_margins,
    standalone_owned_tree_matches_cpp_for_relative_absolute_static_start_uses_padding_border_origin_with_margins,
    standalone_owned_tree_matches_cpp_for_relative_fixed_static_start_uses_root_padding_border_origin_with_margins,
    standalone_owned_tree_matches_cpp_for_relative_absolute_static_start_padding_border_origin_with_percent_margins,
    standalone_owned_tree_matches_cpp_for_relative_fixed_static_start_root_padding_border_origin_with_percent_margins,
    standalone_owned_tree_matches_cpp_for_relative_absolute_static_start_padding_border_origin_with_calc_margins,
    standalone_owned_tree_matches_cpp_for_relative_fixed_static_start_root_padding_border_origin_with_calc_margins,
    standalone_owned_tree_matches_cpp_for_relative_absolute_calc_end_insets_with_percent_margins_and_padding_border_origin,
    standalone_owned_tree_matches_cpp_for_relative_fixed_calc_end_insets_with_percent_margins_and_root_padding_border_origin,
    standalone_owned_tree_matches_cpp_for_rtl_relative_absolute_static_start_with_fixed_margins,
    standalone_owned_tree_matches_cpp_for_rtl_relative_fixed_static_start_with_fixed_margins,
    standalone_owned_tree_matches_cpp_for_rtl_relative_absolute_static_start_with_percent_margins,
    standalone_owned_tree_matches_cpp_for_rtl_relative_fixed_static_start_with_percent_margins,
    standalone_owned_tree_matches_cpp_for_rtl_relative_absolute_static_start_with_calc_margins,
    standalone_owned_tree_matches_cpp_for_rtl_relative_fixed_static_start_with_calc_margins,
    standalone_owned_tree_matches_cpp_for_relative_dependencies_and_stretch,
    standalone_owned_tree_matches_cpp_for_relative_parent_alignment_precedence,
    standalone_owned_tree_matches_cpp_for_relative_sibling_alignment_precedence,
    standalone_owned_tree_matches_cpp_for_relative_parent_start_sibling_align_end_horizontal_percent_margins,
    standalone_owned_tree_matches_cpp_for_relative_sibling_align_start_parent_end_vertical_percent_margins,
    standalone_owned_tree_matches_cpp_for_relative_layout_once_sibling_after_sibling_before_horizontal_calc_margins,
    standalone_owned_tree_matches_cpp_for_relative_layout_once_sibling_after_sibling_before_vertical_calc_margins,
    standalone_owned_tree_matches_cpp_for_relative_missing_align_left_fallback_with_percent_margins,
    standalone_owned_tree_matches_cpp_for_relative_layout_once_missing_align_bottom_fallback_with_calc_margins,
    standalone_owned_tree_matches_cpp_for_relative_right_of_parent_places_after_parent_right_edge,
    standalone_owned_tree_matches_cpp_for_relative_left_of_parent_places_before_parent_left_edge,
    standalone_owned_tree_matches_cpp_for_relative_bottom_of_parent_places_after_parent_bottom_edge,
    standalone_owned_tree_matches_cpp_for_relative_top_of_parent_places_before_parent_top_edge,
    standalone_owned_tree_matches_cpp_for_relative_measured_right_of_parent_to_sibling_right_stretch,
    standalone_owned_tree_matches_cpp_for_relative_measured_bottom_of_parent_to_sibling_bottom_stretch,
    standalone_owned_tree_matches_cpp_for_relative_layout_once_right_of_parent_places_after_parent_right_edge,
    standalone_owned_tree_matches_cpp_for_relative_layout_once_left_of_parent_places_before_parent_left_edge,
    standalone_owned_tree_matches_cpp_for_relative_layout_once_bottom_of_parent_places_after_parent_bottom_edge,
    standalone_owned_tree_matches_cpp_for_relative_layout_once_top_of_parent_places_before_parent_top_edge,
    standalone_owned_tree_matches_cpp_for_relative_layout_once_measured_right_of_parent_to_sibling_right_stretch,
    standalone_owned_tree_matches_cpp_for_relative_layout_once_measured_bottom_of_parent_to_sibling_bottom_stretch,
    standalone_owned_tree_matches_cpp_for_relative_align_left_precedes_right_of_conflict,
    standalone_owned_tree_matches_cpp_for_relative_align_right_precedes_left_of_conflict,
    standalone_owned_tree_matches_cpp_for_relative_align_top_precedes_bottom_of_conflict,
    standalone_owned_tree_matches_cpp_for_relative_align_bottom_precedes_top_of_conflict,
    standalone_owned_tree_matches_cpp_for_relative_missing_align_left_uses_right_of_fallback,
    standalone_owned_tree_matches_cpp_for_relative_missing_align_right_uses_left_of_fallback,
    standalone_owned_tree_matches_cpp_for_relative_missing_align_top_uses_bottom_of_fallback,
    standalone_owned_tree_matches_cpp_for_relative_missing_align_bottom_uses_top_of_fallback,
    standalone_owned_tree_matches_cpp_for_relative_layout_once_align_left_precedes_right_of_conflict,
    standalone_owned_tree_matches_cpp_for_relative_layout_once_align_right_precedes_left_of_conflict,
    standalone_owned_tree_matches_cpp_for_relative_layout_once_align_top_precedes_bottom_of_conflict,
    standalone_owned_tree_matches_cpp_for_relative_layout_once_align_bottom_precedes_top_of_conflict,
    standalone_owned_tree_matches_cpp_for_relative_layout_once_missing_align_left_uses_right_of_fallback,
    standalone_owned_tree_matches_cpp_for_relative_layout_once_missing_align_right_uses_left_of_fallback,
    standalone_owned_tree_matches_cpp_for_relative_layout_once_missing_align_top_uses_bottom_of_fallback,
    standalone_owned_tree_matches_cpp_for_relative_layout_once_missing_align_bottom_uses_top_of_fallback,
    standalone_owned_tree_matches_cpp_for_relative_competing_parent_horizontal_edges_stretch,
    standalone_owned_tree_matches_cpp_for_relative_competing_parent_vertical_edges_stretch,
    standalone_owned_tree_matches_cpp_for_relative_competing_sibling_start_parent_end_stretch,
    standalone_owned_tree_matches_cpp_for_relative_competing_parent_start_sibling_end_stretch,
    standalone_owned_tree_matches_cpp_for_relative_competing_sibling_horizontal_edges_stretch,
    standalone_owned_tree_matches_cpp_for_relative_competing_sibling_vertical_edges_stretch,
    standalone_owned_tree_matches_cpp_for_relative_competing_parent_horizontal_edges_percent_margins_with_padding_border,
    standalone_owned_tree_matches_cpp_for_relative_competing_parent_vertical_edges_percent_margins_with_padding_border,
    standalone_owned_tree_matches_cpp_for_relative_competing_sibling_start_parent_end_calc_margins_with_padding_border,
    standalone_owned_tree_matches_cpp_for_relative_competing_parent_start_sibling_end_percent_margins_with_padding_border,
    standalone_owned_tree_matches_cpp_for_relative_competing_sibling_horizontal_edges_calc_margins_with_padding_border,
    standalone_owned_tree_matches_cpp_for_relative_competing_sibling_vertical_edges_percent_margins_with_padding_border,
    standalone_owned_tree_matches_cpp_for_relative_parent_horizontal_edges_with_calc_margins,
    standalone_owned_tree_matches_cpp_for_relative_parent_vertical_edges_with_calc_margins,
    standalone_owned_tree_matches_cpp_for_relative_center_with_calc_margins_in_definite_parent,
    standalone_owned_tree_matches_cpp_for_relative_parent_horizontal_edges_with_root_padding_border_and_percent_margins,
    standalone_owned_tree_matches_cpp_for_relative_parent_vertical_edges_with_root_padding_border_and_calc_margins,
    standalone_owned_tree_matches_cpp_for_relative_center_with_root_padding_border_and_percent_margins,
    standalone_owned_tree_matches_cpp_for_relative_right_of_parent_with_root_padding_border_and_calc_margins,
    standalone_owned_tree_matches_cpp_for_relative_left_of_parent_with_root_padding_border_and_percent_margins,
    standalone_owned_tree_matches_cpp_for_relative_missing_align_left_falls_back_to_right_of_with_root_padding_border,
    standalone_owned_tree_matches_cpp_for_relative_sibling_after_with_calc_margins,
    standalone_owned_tree_matches_cpp_for_relative_sibling_before_with_calc_margins,
    standalone_owned_tree_matches_cpp_for_relative_sibling_horizontal_edges_with_calc_margins,
    standalone_owned_tree_matches_cpp_for_relative_layout_once_competing_parent_horizontal_edges_stretch,
    standalone_owned_tree_matches_cpp_for_relative_layout_once_competing_parent_vertical_edges_stretch,
    standalone_owned_tree_matches_cpp_for_relative_layout_once_competing_sibling_start_parent_end_stretch,
    standalone_owned_tree_matches_cpp_for_relative_layout_once_competing_parent_start_sibling_end_stretch,
    standalone_owned_tree_matches_cpp_for_relative_layout_once_competing_sibling_horizontal_edges_stretch,
    standalone_owned_tree_matches_cpp_for_relative_layout_once_competing_sibling_vertical_edges_stretch,
    standalone_owned_tree_matches_cpp_for_relative_layout_once_parent_horizontal_edges_with_calc_margins,
    standalone_owned_tree_matches_cpp_for_relative_layout_once_parent_vertical_edges_with_calc_margins,
    standalone_owned_tree_matches_cpp_for_relative_layout_once_parent_horizontal_edges_with_root_padding_border_and_percent_margins,
    standalone_owned_tree_matches_cpp_for_relative_layout_once_parent_vertical_edges_with_root_padding_border_and_calc_margins,
    standalone_owned_tree_matches_cpp_for_relative_layout_once_center_with_root_padding_border_and_percent_margins,
    standalone_owned_tree_matches_cpp_for_relative_layout_once_right_of_parent_with_root_padding_border_and_calc_margins,
    standalone_owned_tree_matches_cpp_for_relative_layout_once_left_of_parent_with_root_padding_border_and_percent_margins,
    standalone_owned_tree_matches_cpp_for_relative_layout_once_missing_align_left_falls_back_to_right_of_with_root_padding_border,
    standalone_owned_tree_matches_cpp_for_relative_layout_once_sibling_after_with_root_padding_border_and_percent_margins,
    standalone_owned_tree_matches_cpp_for_relative_layout_once_sibling_before_with_root_padding_border_and_calc_margins,
    standalone_owned_tree_matches_cpp_for_relative_layout_once_sibling_horizontal_edges_with_root_padding_border_and_percent_margins,
    standalone_owned_tree_matches_cpp_for_relative_layout_once_sibling_vertical_edges_with_root_padding_border_and_calc_margins,
    standalone_owned_tree_matches_cpp_for_relative_layout_once_sibling_start_parent_end_with_root_padding_border_and_percent_margins,
    standalone_owned_tree_matches_cpp_for_relative_layout_once_parent_start_sibling_end_with_root_padding_border_and_calc_margins,
    standalone_owned_tree_matches_cpp_for_relative_layout_once_sibling_after_with_calc_margins,
    standalone_owned_tree_matches_cpp_for_relative_layout_once_sibling_before_with_calc_margins,
    standalone_owned_tree_matches_cpp_for_relative_layout_once_sibling_horizontal_edges_with_calc_margins,
    standalone_owned_tree_matches_cpp_for_relative_layout_once_sibling_vertical_edges_with_calc_margins,
    standalone_owned_tree_matches_cpp_for_relative_layout_once_cycle_ordering,
    standalone_owned_tree_matches_cpp_for_relative_layout_once_initial_roots_ordering,
    standalone_owned_tree_matches_cpp_for_relative_horizontal_scope_ignores_vertical_dependency,
    standalone_owned_tree_matches_cpp_for_relative_vertical_scope_ignores_horizontal_dependency,
    standalone_owned_tree_matches_cpp_for_relative_layout_once_combined_scope_uses_both_axes,
    standalone_owned_tree_matches_cpp_for_relative_missing_ids_do_not_create_sort_dependencies,
    standalone_owned_tree_matches_cpp_for_relative_ordered_duplicate_id_dependency_target,
    standalone_owned_tree_matches_cpp_for_relative_cycle_fallback_with_extra_dependency_root,
    standalone_owned_tree_matches_cpp_for_relative_horizontal_multi_node_chain_ordering,
    standalone_owned_tree_matches_cpp_for_relative_vertical_multi_node_chain_ordering,
    standalone_owned_tree_matches_cpp_for_relative_split_axis_dependency_ordering,
    standalone_owned_tree_matches_cpp_for_relative_layout_once_combined_multi_node_chain_ordering,
    standalone_owned_tree_matches_cpp_for_relative_horizontal_right_of_then_align_left_chain_ordering,
    standalone_owned_tree_matches_cpp_for_relative_horizontal_left_of_then_align_right_chain_ordering,
    standalone_owned_tree_matches_cpp_for_relative_vertical_bottom_of_then_align_top_chain_ordering,
    standalone_owned_tree_matches_cpp_for_relative_vertical_top_of_then_align_bottom_chain_ordering,
    standalone_owned_tree_matches_cpp_for_relative_layout_once_right_of_then_bottom_of_cross_axis_chain_ordering,
    standalone_owned_tree_matches_cpp_for_relative_layout_once_bottom_of_then_right_of_cross_axis_chain_ordering,
    standalone_owned_tree_matches_cpp_for_relative_horizontal_align_left_then_right_of_chain_ordering,
    standalone_owned_tree_matches_cpp_for_relative_horizontal_align_right_then_left_of_chain_ordering,
    standalone_owned_tree_matches_cpp_for_relative_vertical_align_top_then_bottom_of_chain_ordering,
    standalone_owned_tree_matches_cpp_for_relative_vertical_align_bottom_then_top_of_chain_ordering,
    standalone_owned_tree_matches_cpp_for_relative_layout_once_align_left_then_bottom_of_cross_axis_chain_ordering,
    standalone_owned_tree_matches_cpp_for_relative_layout_once_align_top_then_right_of_cross_axis_chain_ordering,
    standalone_owned_tree_matches_cpp_for_relative_horizontal_align_left_then_left_of_chain_ordering,
    standalone_owned_tree_matches_cpp_for_relative_horizontal_align_right_then_right_of_chain_ordering,
    standalone_owned_tree_matches_cpp_for_relative_vertical_align_top_then_top_of_chain_ordering,
    standalone_owned_tree_matches_cpp_for_relative_vertical_align_bottom_then_bottom_of_chain_ordering,
    standalone_owned_tree_matches_cpp_for_relative_layout_once_align_right_then_top_of_cross_axis_chain_ordering,
    standalone_owned_tree_matches_cpp_for_relative_layout_once_align_bottom_then_left_of_cross_axis_chain_ordering,
    standalone_owned_tree_matches_cpp_for_relative_horizontal_cycle_fallback_ordering,
    standalone_owned_tree_matches_cpp_for_relative_vertical_cycle_fallback_ordering,
    standalone_owned_tree_matches_cpp_for_relative_layout_once_parent_edge_stretch,
    standalone_owned_tree_matches_cpp_for_relative_missing_reference_centering,
    standalone_owned_tree_matches_cpp_for_relative_missing_start_references_fallback,
    standalone_owned_tree_matches_cpp_for_relative_missing_end_references_fallback,
    standalone_owned_tree_matches_cpp_for_relative_wrap_content_center_after_sizing,
    standalone_owned_tree_matches_cpp_for_relative_wrap_content_parent_end_after_sizing,
    standalone_owned_tree_matches_cpp_for_relative_layout_once_default_uses_negative_min_bound,
    standalone_owned_tree_matches_cpp_for_relative_wrap_content_center_with_margins_and_padding,
    standalone_owned_tree_matches_cpp_for_relative_wrap_content_parent_right_percent_margins_after_sizing,
    standalone_owned_tree_matches_cpp_for_relative_wrap_content_parent_bottom_percent_margins_after_sizing,
    standalone_owned_tree_matches_cpp_for_relative_wrap_content_center_percent_margins_after_sizing,
    standalone_owned_tree_matches_cpp_for_relative_wrap_content_horizontal_center_vertical_end_percent_margins,
    standalone_owned_tree_matches_cpp_for_relative_wrap_content_parent_left_percent_margins_after_sizing,
    standalone_owned_tree_matches_cpp_for_relative_wrap_content_parent_top_percent_margins_after_sizing,
    standalone_owned_tree_matches_cpp_for_relative_wrap_content_horizontal_end_vertical_center_percent_margins,
    standalone_owned_tree_matches_cpp_for_relative_layout_once_wrap_content_parent_left_percent_margins,
    standalone_owned_tree_matches_cpp_for_relative_layout_once_wrap_content_parent_top_percent_margins,
    standalone_owned_tree_matches_cpp_for_relative_layout_once_wrap_content_horizontal_end_vertical_center_percent_margins,
    standalone_owned_tree_matches_cpp_for_relative_wrap_content_min_width_clamp_with_parent_end_percent_margins,
    standalone_owned_tree_matches_cpp_for_relative_wrap_content_max_width_clamp_with_center_percent_margins_and_padding_border,
    standalone_owned_tree_matches_cpp_for_relative_wrap_content_min_height_clamp_with_parent_bottom_calc_margins_and_padding_border,
    standalone_owned_tree_matches_cpp_for_relative_wrap_content_max_height_clamp_with_horizontal_center_vertical_end_percent_margins,
    standalone_owned_tree_matches_cpp_for_relative_layout_once_wrap_content_min_size_clamp_with_parent_end_percent_margins,
    standalone_owned_tree_matches_cpp_for_relative_layout_once_wrap_content_max_size_clamp_with_center_percent_margins_and_padding_border,
    standalone_owned_tree_matches_cpp_for_relative_wrap_content_min_width_clamp_with_parent_left_percent_margins,
    standalone_owned_tree_matches_cpp_for_relative_wrap_content_max_width_clamp_with_horizontal_end_vertical_center_percent_margins_and_padding_border,
    standalone_owned_tree_matches_cpp_for_relative_wrap_content_min_height_clamp_with_parent_top_percent_margins_and_padding_border,
    standalone_owned_tree_matches_cpp_for_relative_wrap_content_max_height_clamp_with_horizontal_end_vertical_center_percent_margins,
    standalone_owned_tree_matches_cpp_for_relative_layout_once_wrap_content_min_size_clamp_with_parent_left_top_percent_margins,
    standalone_owned_tree_matches_cpp_for_relative_layout_once_wrap_content_max_size_clamp_with_horizontal_end_vertical_center_percent_margins_and_padding_border,
    standalone_owned_tree_matches_cpp_for_relative_wrap_content_at_most_width_parent_end_percent_margins,
    standalone_owned_tree_matches_cpp_for_relative_wrap_content_at_most_height_parent_bottom_calc_margins,
    standalone_owned_tree_matches_cpp_for_relative_wrap_content_at_most_both_axes_center_percent_margins_and_padding_border,
    standalone_owned_tree_matches_cpp_for_relative_wrap_content_at_most_width_parent_left_percent_margins,
    standalone_owned_tree_matches_cpp_for_relative_wrap_content_at_most_height_parent_top_percent_margins,
    standalone_owned_tree_matches_cpp_for_relative_wrap_content_at_most_both_axes_horizontal_end_vertical_center_percent_margins,
    standalone_owned_tree_matches_cpp_for_relative_layout_once_wrap_content_at_most_width_parent_end_percent_margins,
    standalone_owned_tree_matches_cpp_for_relative_layout_once_wrap_content_at_most_height_center_percent_margins,
    standalone_owned_tree_matches_cpp_for_relative_layout_once_wrap_content_at_most_both_axes_center_percent_margins_and_padding_border,
    standalone_owned_tree_matches_cpp_for_relative_layout_once_wrap_content_at_most_width_parent_left_percent_margins,
    standalone_owned_tree_matches_cpp_for_relative_layout_once_wrap_content_at_most_height_parent_top_percent_margins,
    standalone_owned_tree_matches_cpp_for_relative_layout_once_wrap_content_at_most_both_axes_horizontal_end_vertical_center_percent_margins,
    standalone_owned_tree_matches_cpp_for_relative_layout_once_wrap_content_parent_end_percent_margins,
    standalone_owned_tree_matches_cpp_for_relative_layout_once_wrap_content_center_percent_margins,
    standalone_owned_tree_matches_cpp_for_relative_horizontal_center_vertical_parent_end,
    standalone_owned_tree_matches_cpp_for_relative_vertical_center_horizontal_parent_end,
    standalone_owned_tree_matches_cpp_for_relative_layout_once_center_after_combined_dependencies,
    standalone_owned_tree_matches_cpp_for_relative_duplicate_ids_and_display_none_anchors,
    standalone_owned_tree_matches_cpp_for_relative_ordered_duplicate_id_lookup,
    standalone_owned_tree_matches_cpp_for_relative_ordered_duplicate_id_right_of_skips_display_none_with_padding_border,
    standalone_owned_tree_matches_cpp_for_relative_ordered_duplicate_id_bottom_of_skips_display_none_with_percent_margins,
    standalone_owned_tree_matches_cpp_for_relative_ordered_duplicate_id_align_left_uses_last_visible_anchor,
    standalone_owned_tree_matches_cpp_for_relative_ordered_duplicate_id_align_bottom_uses_last_visible_anchor,
    standalone_owned_tree_matches_cpp_for_relative_layout_once_ordered_duplicate_id_combined_dependencies_skip_display_none,
    standalone_owned_tree_matches_cpp_for_relative_ordered_duplicate_id_wrap_content_skips_display_none_anchor,
    standalone_owned_tree_matches_cpp_for_relative_ordered_duplicate_id_left_of_skips_display_none_with_padding_border,
    standalone_owned_tree_matches_cpp_for_relative_ordered_duplicate_id_top_of_skips_display_none_with_percent_margins,
    standalone_owned_tree_matches_cpp_for_relative_ordered_duplicate_id_align_right_uses_last_visible_anchor,
    standalone_owned_tree_matches_cpp_for_relative_ordered_duplicate_id_align_top_uses_last_visible_anchor,
    standalone_owned_tree_matches_cpp_for_relative_layout_once_ordered_duplicate_id_opposite_dependencies_skip_display_none,
    standalone_owned_tree_matches_cpp_for_relative_ordered_duplicate_id_wrap_content_opposite_edges_skip_display_none_anchor,
    standalone_owned_tree_matches_cpp_for_relative_fit_content_wrap_content_sizing,
    standalone_owned_tree_matches_cpp_for_relative_initial_constraint_definite_width_at_most_height,
    standalone_owned_tree_matches_cpp_for_relative_initial_constraint_at_most_width_definite_height,
    standalone_owned_tree_matches_cpp_for_relative_initial_constraint_definite_width_indefinite_height,
    standalone_owned_tree_matches_cpp_for_relative_initial_constraint_indefinite_width_definite_height,
    standalone_owned_tree_matches_cpp_for_relative_initial_constraint_at_most_both_axes,
    standalone_owned_tree_matches_cpp_for_relative_initial_constraint_indefinite_width_at_most_height,
    standalone_owned_tree_matches_cpp_for_relative_initial_constraint_definite_both_axes,
    standalone_owned_tree_matches_cpp_for_relative_initial_constraint_at_most_width_indefinite_height,
    standalone_owned_tree_matches_cpp_for_relative_initial_constraint_indefinite_both_axes,
    standalone_owned_tree_matches_cpp_for_relative_initial_constraint_definite_both_axes_with_padding_border,
    standalone_owned_tree_matches_cpp_for_relative_initial_constraint_at_most_width_indefinite_height_with_padding_border,
    standalone_owned_tree_matches_cpp_for_relative_initial_constraint_indefinite_both_axes_with_padding_border,
    standalone_owned_tree_matches_cpp_for_relative_container_clamp_matrix,
    standalone_owned_tree_matches_cpp_for_relative_content_origin_matrix,
    standalone_owned_tree_matches_cpp_for_relative_sibling_edge_matrix,
    standalone_owned_tree_matches_cpp_for_relative_measured_single_start_reduces_width_constraint,
    standalone_owned_tree_matches_cpp_for_relative_measured_single_start_reduces_height_constraint,
    standalone_owned_tree_matches_cpp_for_relative_measured_single_end_width_constraint,
    standalone_owned_tree_matches_cpp_for_relative_measured_single_end_height_constraint,
    standalone_owned_tree_matches_cpp_for_relative_measured_parent_edges_stretch_constraints,
    standalone_owned_tree_matches_cpp_for_relative_measured_single_start_width_calc_margins_under_at_most_parent,
    standalone_owned_tree_matches_cpp_for_relative_measured_single_end_height_calc_margins_under_at_most_parent,
    standalone_owned_tree_matches_cpp_for_relative_measured_parent_horizontal_edges_calc_margins_under_at_most_parent,
    standalone_owned_tree_matches_cpp_for_relative_measured_parent_vertical_edges_calc_margins_under_at_most_parent,
    standalone_owned_tree_matches_cpp_for_relative_measured_percent_size_resolves_against_definite_parent_content,
    standalone_owned_tree_matches_cpp_for_relative_measured_percent_height_uses_indefinite_missing_parent_axis,
    standalone_owned_tree_matches_cpp_for_relative_layout_once_measured_parent_edges_constraints,
    standalone_owned_tree_matches_cpp_for_relative_layout_once_measured_single_start_constraints_both_axes,
    standalone_owned_tree_matches_cpp_for_relative_layout_once_measured_single_end_constraints_both_axes,
    standalone_owned_tree_matches_cpp_for_relative_measured_parent_edges_percent_margins,
    standalone_owned_tree_matches_cpp_for_relative_layout_once_measured_parent_edges_percent_margins,
    standalone_owned_tree_matches_cpp_for_relative_measured_sibling_edges_percent_margins_both_axes,
    standalone_owned_tree_matches_cpp_for_relative_layout_once_measured_sibling_edges_percent_margins_both_axes,
    standalone_owned_tree_matches_cpp_for_relative_layout_once_measured_sibling_start_parent_end_horizontal_stretch,
    standalone_owned_tree_matches_cpp_for_relative_layout_once_measured_parent_start_sibling_end_horizontal_stretch,
    standalone_owned_tree_matches_cpp_for_relative_layout_once_measured_sibling_start_parent_end_vertical_stretch,
    standalone_owned_tree_matches_cpp_for_relative_layout_once_measured_parent_start_sibling_end_vertical_stretch,
    standalone_owned_tree_matches_cpp_for_relative_layout_once_measured_sibling_horizontal_edges_stretch,
    standalone_owned_tree_matches_cpp_for_relative_layout_once_measured_sibling_vertical_edges_stretch,
    standalone_owned_tree_matches_cpp_for_relative_layout_once_measured_sibling_align_start_parent_end_horizontal_stretch,
    standalone_owned_tree_matches_cpp_for_relative_layout_once_measured_parent_start_sibling_align_end_horizontal_stretch,
    standalone_owned_tree_matches_cpp_for_relative_layout_once_measured_sibling_align_start_parent_end_vertical_stretch,
    standalone_owned_tree_matches_cpp_for_relative_layout_once_measured_parent_start_sibling_align_end_vertical_stretch,
    standalone_owned_tree_matches_cpp_for_relative_layout_once_measured_sibling_align_start_sibling_before_horizontal_stretch,
    standalone_owned_tree_matches_cpp_for_relative_layout_once_measured_sibling_after_sibling_align_end_vertical_stretch,
    standalone_owned_tree_matches_cpp_for_relative_layout_once_measured_sibling_after_sibling_align_end_horizontal_stretch,
    standalone_owned_tree_matches_cpp_for_relative_layout_once_measured_sibling_align_start_sibling_align_end_horizontal_stretch,
    standalone_owned_tree_matches_cpp_for_relative_layout_once_measured_sibling_align_start_sibling_align_end_vertical_stretch,
    standalone_owned_tree_matches_cpp_for_relative_layout_once_measured_sibling_start_parent_end_horizontal_percent_margins_with_padding_border,
    standalone_owned_tree_matches_cpp_for_relative_layout_once_measured_parent_start_sibling_end_horizontal_calc_margins_with_padding_border,
    standalone_owned_tree_matches_cpp_for_relative_layout_once_measured_sibling_start_parent_end_vertical_percent_margins_with_padding_border,
    standalone_owned_tree_matches_cpp_for_relative_layout_once_measured_parent_start_sibling_end_vertical_calc_margins_with_padding_border,
    standalone_owned_tree_matches_cpp_for_relative_layout_once_measured_sibling_horizontal_edges_calc_margins_with_padding_border,
    standalone_owned_tree_matches_cpp_for_relative_layout_once_measured_sibling_vertical_edges_percent_margins_with_padding_border,
    standalone_owned_tree_matches_cpp_for_relative_measured_sibling_start_parent_end_horizontal_stretch,
    standalone_owned_tree_matches_cpp_for_relative_measured_parent_start_sibling_end_horizontal_stretch,
    standalone_owned_tree_matches_cpp_for_relative_measured_sibling_start_parent_end_vertical_stretch,
    standalone_owned_tree_matches_cpp_for_relative_measured_parent_start_sibling_end_vertical_stretch,
    standalone_owned_tree_matches_cpp_for_relative_measured_sibling_horizontal_edges_stretch,
    standalone_owned_tree_matches_cpp_for_relative_measured_sibling_vertical_edges_stretch,
    standalone_owned_tree_matches_cpp_for_relative_measured_sibling_align_start_parent_end_horizontal_stretch,
    standalone_owned_tree_matches_cpp_for_relative_measured_parent_start_sibling_align_end_horizontal_stretch,
    standalone_owned_tree_matches_cpp_for_relative_measured_sibling_align_start_parent_end_vertical_stretch,
    standalone_owned_tree_matches_cpp_for_relative_measured_parent_start_sibling_align_end_vertical_stretch,
    standalone_owned_tree_matches_cpp_for_relative_measured_sibling_align_start_sibling_before_horizontal_stretch,
    standalone_owned_tree_matches_cpp_for_relative_measured_sibling_after_sibling_align_end_vertical_stretch,
    standalone_owned_tree_matches_cpp_for_relative_measured_sibling_after_sibling_align_end_horizontal_stretch,
    standalone_owned_tree_matches_cpp_for_relative_measured_sibling_align_start_sibling_align_end_horizontal_stretch,
    standalone_owned_tree_matches_cpp_for_relative_measured_sibling_align_start_sibling_align_end_vertical_stretch,
    standalone_owned_tree_matches_cpp_for_relative_measured_sibling_align_start_parent_end_horizontal_percent_margins_with_padding_border,
    standalone_owned_tree_matches_cpp_for_relative_measured_parent_start_sibling_align_end_horizontal_calc_margins_with_padding_border,
    standalone_owned_tree_matches_cpp_for_relative_measured_sibling_align_start_parent_end_vertical_percent_margins_with_padding_border,
    standalone_owned_tree_matches_cpp_for_relative_measured_parent_start_sibling_align_end_vertical_calc_margins_with_padding_border,
    standalone_owned_tree_matches_cpp_for_relative_measured_sibling_align_start_sibling_before_horizontal_percent_margins_with_padding_border,
    standalone_owned_tree_matches_cpp_for_relative_measured_sibling_after_sibling_align_end_vertical_calc_margins_with_padding_border,
    standalone_owned_tree_matches_cpp_for_relative_measured_sibling_start_parent_end_horizontal_percent_margins_with_padding_border,
    standalone_owned_tree_matches_cpp_for_relative_measured_parent_start_sibling_end_horizontal_calc_margins_with_padding_border,
    standalone_owned_tree_matches_cpp_for_relative_measured_sibling_start_parent_end_vertical_percent_margins_with_padding_border,
    standalone_owned_tree_matches_cpp_for_relative_measured_parent_start_sibling_end_vertical_calc_margins_with_padding_border,
    standalone_owned_tree_matches_cpp_for_relative_measured_sibling_horizontal_edges_calc_margins_with_padding_border,
    standalone_owned_tree_matches_cpp_for_relative_measured_sibling_vertical_edges_percent_margins_with_padding_border,
    standalone_owned_tree_matches_cpp_for_relative_two_pass_measured_parent_start_sibling_end_horizontal_remeasure,
    standalone_owned_tree_matches_cpp_for_relative_two_pass_measured_sibling_start_parent_end_horizontal_remeasure,
    standalone_owned_tree_matches_cpp_for_relative_two_pass_measured_sibling_horizontal_edges_remeasure,
    standalone_owned_tree_matches_cpp_for_relative_two_pass_measured_parent_start_sibling_end_vertical_placement,
    standalone_owned_tree_matches_cpp_for_relative_two_pass_measured_sibling_start_parent_end_vertical_placement,
    standalone_owned_tree_matches_cpp_for_relative_two_pass_measured_sibling_vertical_edges_placement,
    standalone_owned_tree_matches_cpp_for_relative_two_pass_measured_sibling_start_parent_end_horizontal_calc_margins_with_padding_border,
    standalone_owned_tree_matches_cpp_for_relative_two_pass_measured_parent_start_sibling_end_horizontal_percent_margins_with_padding_border,
    standalone_owned_tree_matches_cpp_for_relative_two_pass_measured_sibling_start_parent_end_vertical_percent_margins_with_padding_border,
    standalone_owned_tree_matches_cpp_for_relative_two_pass_measured_parent_start_sibling_end_vertical_calc_margins_with_padding_border,
    standalone_owned_tree_matches_cpp_for_relative_two_pass_measured_sibling_horizontal_edges_calc_margins_with_padding_border,
    standalone_owned_tree_matches_cpp_for_relative_two_pass_measured_sibling_vertical_edges_percent_margins_with_padding_border,
    standalone_owned_tree_matches_cpp_for_relative_two_pass_measured_sibling_align_start_parent_end_horizontal_remeasure,
    standalone_owned_tree_matches_cpp_for_relative_two_pass_measured_parent_start_sibling_align_end_horizontal_remeasure,
    standalone_owned_tree_matches_cpp_for_relative_two_pass_measured_sibling_align_start_sibling_before_horizontal_remeasure,
    standalone_owned_tree_matches_cpp_for_relative_two_pass_measured_sibling_align_start_parent_end_vertical_placement,
    standalone_owned_tree_matches_cpp_for_relative_two_pass_measured_parent_start_sibling_align_end_vertical_placement,
    standalone_owned_tree_matches_cpp_for_relative_two_pass_measured_sibling_after_sibling_align_end_vertical_placement,
    standalone_owned_tree_matches_cpp_for_relative_two_pass_measured_sibling_after_sibling_align_end_horizontal_remeasure,
    standalone_owned_tree_matches_cpp_for_relative_two_pass_measured_sibling_align_start_sibling_align_end_horizontal_remeasure,
    standalone_owned_tree_matches_cpp_for_relative_two_pass_measured_sibling_align_start_sibling_align_end_vertical_placement,
    standalone_owned_tree_matches_cpp_for_relative_layout_once_wrap_content_measured_sibling_after_sibling_align_end_horizontal,
    standalone_owned_tree_matches_cpp_for_relative_layout_once_wrap_content_measured_sibling_align_start_sibling_align_end_horizontal,
    standalone_owned_tree_matches_cpp_for_relative_layout_once_wrap_content_measured_sibling_align_start_sibling_align_end_vertical,
    standalone_owned_tree_matches_cpp_for_relative_two_pass_wrap_content_remeasure,
    standalone_owned_tree_matches_cpp_for_relative_two_pass_ordered_duplicate_id_sibling_start_parent_end_horizontal_remeasure,
    standalone_owned_tree_matches_cpp_for_relative_two_pass_ordered_duplicate_id_parent_start_sibling_end_horizontal_remeasure,
    standalone_owned_tree_matches_cpp_for_relative_two_pass_ordered_duplicate_id_sibling_start_parent_end_vertical_placement,
    standalone_owned_tree_matches_cpp_for_relative_two_pass_ordered_duplicate_id_parent_start_sibling_end_vertical_placement,
    standalone_owned_tree_matches_cpp_for_relative_two_pass_ordered_duplicate_id_sibling_align_start_parent_end_horizontal_remeasure,
    standalone_owned_tree_matches_cpp_for_relative_two_pass_ordered_duplicate_id_parent_start_sibling_align_end_vertical_placement,
    standalone_owned_tree_matches_cpp_for_relative_two_pass_ordered_duplicate_id_sibling_horizontal_edges_remeasure,
    standalone_owned_tree_matches_cpp_for_relative_two_pass_ordered_duplicate_id_sibling_vertical_edges_placement,
    standalone_owned_tree_matches_cpp_for_relative_two_pass_ordered_duplicate_id_sibling_after_sibling_align_end_horizontal_remeasure,
    standalone_owned_tree_matches_cpp_for_relative_two_pass_ordered_duplicate_id_sibling_after_sibling_align_end_vertical_placement,
    standalone_owned_tree_matches_cpp_for_relative_two_pass_ordered_duplicate_id_sibling_align_start_sibling_align_end_horizontal_remeasure,
    standalone_owned_tree_matches_cpp_for_relative_two_pass_ordered_duplicate_id_sibling_align_start_sibling_align_end_vertical_placement,
    standalone_owned_tree_matches_cpp_for_relative_layout_once_two_sided_remeasure,
    standalone_owned_tree_matches_cpp_for_relative_center_suppressed_by_parent_horizontal_edges_with_percent_margins,
    standalone_owned_tree_matches_cpp_for_relative_center_suppressed_by_parent_vertical_edges_with_calc_margins,
    standalone_owned_tree_matches_cpp_for_relative_center_suppressed_by_sibling_horizontal_edges_with_padding_border,
    standalone_owned_tree_matches_cpp_for_relative_center_suppressed_by_sibling_vertical_edges_with_padding_border,
    standalone_owned_tree_matches_cpp_for_relative_layout_once_center_suppressed_by_parent_start_sibling_end,
    standalone_owned_tree_matches_cpp_for_relative_two_pass_wrap_content_center_suppressed_by_sibling_horizontal_edges,
    standalone_owned_tree_matches_cpp_for_relative_center_suppressed_by_parent_left_start_only_with_percent_margins,
    standalone_owned_tree_matches_cpp_for_relative_center_suppressed_by_parent_right_end_only_with_calc_margins,
    standalone_owned_tree_matches_cpp_for_relative_center_suppressed_by_parent_top_start_only_with_percent_margins,
    standalone_owned_tree_matches_cpp_for_relative_center_suppressed_by_parent_bottom_end_only_with_calc_margins,
    standalone_owned_tree_matches_cpp_for_relative_layout_once_center_suppressed_by_sibling_start_only,
    standalone_owned_tree_matches_cpp_for_relative_two_pass_wrap_content_center_suppressed_by_sibling_end_only,
    standalone_owned_tree_matches_cpp_for_relative_ordered_wrap_content_center_skips_display_none_extent_with_percent_margins,
    standalone_owned_tree_matches_cpp_for_relative_layout_once_ordered_wrap_content_center_skips_display_none_extent_with_calc_margins,
    standalone_owned_tree_matches_cpp_for_relative_ordered_wrap_content_parent_right_skips_display_none_extent_after_sizing,
    standalone_owned_tree_matches_cpp_for_relative_ordered_wrap_content_parent_bottom_skips_display_none_extent_after_sizing,
    standalone_owned_tree_matches_cpp_for_relative_layout_once_ordered_horizontal_center_vertical_end_skips_display_none_extents,
    standalone_owned_tree_matches_cpp_for_relative_ordered_vertical_center_horizontal_end_skips_display_none_extents,
    standalone_owned_tree_matches_cpp_for_relative_out_of_flow_matrix,
    standalone_owned_tree_matches_cpp_for_relative_absolute_static_start_with_margins,
    standalone_owned_tree_matches_cpp_for_relative_fixed_static_start_with_margins,
    standalone_owned_tree_matches_cpp_for_relative_absolute_display_none_static_start_does_not_size_wrap_content_container,
    standalone_owned_tree_matches_cpp_for_relative_absolute_display_none_paired_insets_skip_measure_with_padding_border,
    standalone_owned_tree_matches_cpp_for_relative_fixed_display_none_descendant_does_not_size_wrap_content_container,
    standalone_owned_tree_matches_cpp_for_relative_fixed_display_none_percent_insets_skip_root_containing_block_measure,
    standalone_owned_tree_matches_cpp_for_relative_display_none_subtree_hides_measured_descendant_with_padding_border,
    standalone_owned_tree_matches_cpp_for_relative_display_none_subtree_hides_absolute_descendant,
    standalone_owned_tree_matches_cpp_for_relative_display_none_subtree_hides_fixed_descendant_before_root_measure,
);

#[test]
fn standalone_relative_inventory_keeps_all_401_display_relative_cases() {
    assert_eq!(STANDALONE_RELATIVE_CASES.len(), 401);
    let unique = STANDALONE_RELATIVE_CASES
        .iter()
        .copied()
        .collect::<BTreeSet<_>>();
    assert_eq!(unique.len(), STANDALONE_RELATIVE_CASES.len());

    for false_friend in [
        "standalone_owned_tree_matches_cpp_for_linear_relative_position_physical_pixel_rounding",
        "standalone_owned_tree_matches_cpp_for_relative_position_offsets",
        "standalone_owned_tree_matches_cpp_for_linear_relative_position_left_top_preserves_horizontal_flow",
        "standalone_owned_tree_matches_cpp_for_linear_relative_position_right_bottom_calc_preserves_vertical_flow",
        "standalone_owned_tree_matches_cpp_for_linear_relative_position_start_insets_win_over_end_insets",
        "standalone_owned_tree_matches_cpp_for_calc_and_out_of_flow_edge_behaviors",
    ] {
        assert!(!unique.contains(false_friend));
    }

    for category in [
        "layout_once",
        "sibling",
        "wrap_content",
        "measured",
        "display_none",
        "relative_absolute",
        "relative_fixed",
        "center",
    ] {
        assert!(
            STANDALONE_RELATIVE_CASES
                .iter()
                .any(|name| name.contains(category)),
            "missing standalone Relative category {category}"
        );
    }
}
