//! Pure-Rust migration of every generated PR #25 matrix case whose source
//! tree contains `display: linear`.
//!
//! PR #25 compared each generated Rust tree with Lynx C++. This port keeps the
//! exact Rust-side parameter matrices, deterministic RNG, source tree
//! builders, regression IDs, and Linear case selection. The C++ runner is
//! deliberately absent. Every selected tree is instead laid out twice from
//! the same pristine source and checked for identical, finite full geometry.

mod pr25_generated_linear_support;
mod pr25_support;
mod support;

use pr25_generated_linear_support::*;
use pr25_support::*;

const LAYOUT_DIRECTIONS: [Direction; 2] = [Direction::Ltr, Direction::Rtl];
const DEFAULT_DETERMINISTIC_SUPPORTED_TREE_CASES: usize = 32_768;

fn assert_close(left: f32, right: f32) {
    assert!((left - right).abs() <= 0.001, "{left} != {right}");
}

fn assert_layout_result_deterministic(left: LayoutResult, right: LayoutResult) {
    for (a, b) in [
        (left.offset.x, right.offset.x),
        (left.offset.y, right.offset.y),
        (left.size.width, right.size.width),
        (left.size.height, right.size.height),
        (left.padding.left, right.padding.left),
        (left.padding.right, right.padding.right),
        (left.padding.top, right.padding.top),
        (left.padding.bottom, right.padding.bottom),
        (left.border.left, right.border.left),
        (left.border.right, right.border.right),
        (left.border.top, right.border.top),
        (left.border.bottom, right.border.bottom),
        (left.margin.left, right.margin.left),
        (left.margin.right, right.margin.right),
        (left.margin.top, right.margin.top),
        (left.margin.bottom, right.margin.bottom),
        (left.sticky_pos.left, right.sticky_pos.left),
        (left.sticky_pos.right, right.sticky_pos.right),
        (left.sticky_pos.top, right.sticky_pos.top),
        (left.sticky_pos.bottom, right.sticky_pos.bottom),
    ] {
        assert!(a.is_finite(), "non-finite generated layout value: {a}");
        assert_close(a, b);
    }
    assert!(left.size.width >= 0.0 && left.size.height >= 0.0);
    match (left.baseline, right.baseline) {
        (Some(a), Some(b)) => {
            assert!(a.is_finite());
            assert_close(a, b);
        }
        (None, None) => {}
        values => panic!("baseline changed between identical layouts: {values:?}"),
    }
}

fn assert_deterministic(tree: SimpleTree, root: usize, constraints: Constraints) {
    let mut first = tree.clone();
    let mut second = tree;
    let first_size =
        LayoutEngine::new().layout_with_owner_constraints(&mut first, root, constraints);
    let second_size =
        LayoutEngine::new().layout_with_owner_constraints(&mut second, root, constraints);
    assert_close(first_size.width, second_size.width);
    assert_close(first_size.height, second_size.height);
    assert!(first_size.width.is_finite() && first_size.width >= 0.0);
    assert!(first_size.height.is_finite() && first_size.height >= 0.0);
    assert_eq!(first.nodes.len(), second.nodes.len());
    for (left, right) in first.nodes.iter().zip(&second.nodes) {
        assert_layout_result_deterministic(left.layout, right.layout);
    }
}

fn expected_sticky_inset(length: Length, percent_base: f32) -> f32 {
    match length {
        Length::Auto => STICKY_AUTO_INSET,
        Length::Points(value) => value,
        Length::Percent(value) => percent_base * (value / 100.0),
        Length::Calc { fixed, percent } => fixed + percent_base * (percent / 100.0),
        value => panic!("unexpected generated sticky inset: {value:?}"),
    }
}

fn assert_generated_sticky_semantics(tree: SimpleTree, root: usize, constraints: Constraints) {
    let sticky = tree.nodes[root].children[0];
    let authored = tree.nodes[sticky].style.clone();

    let mut actual = tree.clone();
    LayoutEngine::new().layout_with_owner_constraints(&mut actual, root, constraints);

    // A Sticky item has the same normal-flow geometry as the corresponding
    // item with no visual insets. This reference tree catches accidental
    // reinterpretation of Sticky as shifted `position: relative`.
    let mut normal_flow = tree.clone();
    let normal_style = &mut normal_flow.nodes[sticky].style;
    normal_style.position = PositionType::Relative;
    normal_style.left = Length::Auto;
    normal_style.right = Length::Auto;
    normal_style.top = Length::Auto;
    normal_style.bottom = Length::Auto;
    LayoutEngine::new().layout_with_owner_constraints(&mut normal_flow, root, constraints);

    for (sticky_node, normal_node) in actual.nodes.iter().zip(&normal_flow.nodes) {
        for (sticky_value, normal_value) in [
            (sticky_node.layout.offset.x, normal_node.layout.offset.x),
            (sticky_node.layout.offset.y, normal_node.layout.offset.y),
            (sticky_node.layout.size.width, normal_node.layout.size.width),
            (
                sticky_node.layout.size.height,
                normal_node.layout.size.height,
            ),
        ] {
            assert_close(sticky_value, normal_value);
        }
    }

    // Every source Sticky-matrix root has an explicit content-box size. Use
    // that independent authored oracle rather than deriving the basis from
    // the facade's exported root geometry.
    let inline_basis = match tree.nodes[root].style.width {
        Length::Points(value) => value,
        value => panic!("unexpected generated sticky root width: {value:?}"),
    };
    let block_basis = match tree.nodes[root].style.height {
        Length::Points(value) => value,
        value => panic!("unexpected generated sticky root height: {value:?}"),
    };
    let sticky_pos = actual.nodes[sticky].layout.sticky_pos;
    assert_close(
        sticky_pos.left,
        expected_sticky_inset(authored.left, inline_basis),
    );
    assert_close(
        sticky_pos.right,
        expected_sticky_inset(authored.right, inline_basis),
    );
    assert_close(
        sticky_pos.top,
        expected_sticky_inset(authored.top, block_basis),
    );
    assert_close(
        sticky_pos.bottom,
        expected_sticky_inset(authored.bottom, block_basis),
    );

    assert_deterministic(tree, root, constraints);
}

#[test]
fn generated_measured_callback_matrix_matches_cpp() {
    let variants = [
        MeasuredVariant::Plain,
        MeasuredVariant::Baseline,
        MeasuredVariant::MinMax,
        MeasuredVariant::AspectBorderBox,
    ];
    let mut executions = 0;
    for container in GENERATED_CONTAINERS {
        for variant in variants {
            let (tree, root) = measured_callback_tree(container, variant);
            if tree_contains_linear(&tree) {
                assert_deterministic(tree, root, Constraints::definite(142.0, 104.0));
                executions += 1;
            }
        }
    }
    assert_eq!(executions, 8);
}

#[test]
fn generated_flex_baseline_propagation_matrix_matches_cpp() {
    let constraint_modes = [
        BaselineConstraintMode::DefiniteRoot,
        BaselineConstraintMode::AtMostOwner,
        BaselineConstraintMode::IndefiniteOwner,
    ];
    let triggers = [
        BaselineTrigger::ContainerAlignItems,
        BaselineTrigger::ChildAlignSelf,
    ];
    let sources = [
        BaselineSource::MeasuredLeaf,
        BaselineSource::NestedFlex,
        BaselineSource::NestedFlexColumn,
        BaselineSource::NestedFlexColumnReverse,
        BaselineSource::NestedLinear,
        BaselineSource::NestedLinearVertical,
        BaselineSource::NestedLinearVerticalReverse,
        BaselineSource::NestedGridFallback,
        BaselineSource::NestedRelativeFallback,
    ];
    let mut executions = 0;
    for constraint_mode in constraint_modes {
        for trigger in triggers {
            for source in sources {
                let (tree, root, constraints) =
                    flex_baseline_propagation_tree(constraint_mode, trigger, source);
                if tree_contains_linear(&tree) {
                    assert_deterministic(tree, root, constraints);
                    executions += 1;
                }
            }
        }
    }
    assert_eq!(executions, 18);
}

#[test]
fn generated_sizing_minmax_aspect_matrix_matches_cpp() {
    let variants = [
        SizingVariant::PercentCalcRoot,
        SizingVariant::FitContentRoot,
        SizingVariant::FitContentSubtree,
        SizingVariant::PercentMinMaxRoot,
        SizingVariant::BorderBoxPercentMinMaxRoot,
        SizingVariant::ContentBoxAspectRoot,
        SizingVariant::BorderBoxAspectRoot,
        SizingVariant::IntrinsicMeasuredChild,
    ];
    let mut executions = 0;
    for container in GENERATED_CONTAINERS {
        for variant in variants {
            let (tree, root) = sizing_minmax_aspect_tree(container, variant);
            if tree_contains_linear(&tree) {
                assert_deterministic(tree, root, Constraints::definite(160.0, 120.0));
                executions += 1;
            }
        }
    }
    assert_eq!(executions, 16);
}

#[test]
fn generated_linear_orientation_justify_direction_matrix_matches_cpp() {
    let orientations = all_linear_orientations();
    let justify_content_values = [
        JustifyContent::FlexStart,
        JustifyContent::Center,
        JustifyContent::FlexEnd,
        JustifyContent::SpaceBetween,
        JustifyContent::SpaceAround,
        JustifyContent::SpaceEvenly,
        JustifyContent::Stretch,
    ];
    let mut executions = 0;
    for orientation in orientations {
        for direction in LAYOUT_DIRECTIONS {
            for justify_content in justify_content_values {
                let (tree, root) = linear_orientation_tree(orientation, direction, justify_content);
                assert_deterministic(tree, root, Constraints::definite(120.0, 90.0));
                executions += 1;
            }
        }
    }
    assert_eq!(executions, 112);
}

#[test]
fn generated_linear_gravity_orientation_direction_matrix_matches_cpp() {
    let values = [
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
    ];
    let mut executions = 0;
    for orientation in all_linear_orientations() {
        for direction in LAYOUT_DIRECTIONS {
            for gravity in values {
                let (tree, root) = linear_gravity_tree(orientation, direction, gravity);
                assert_deterministic(tree, root, Constraints::definite(120.0, 90.0));
                executions += 1;
            }
        }
    }
    assert_eq!(executions, 176);
}

#[test]
fn generated_linear_layout_gravity_orientation_direction_matrix_matches_cpp() {
    let values = [
        LinearLayoutGravity::None,
        LinearLayoutGravity::Top,
        LinearLayoutGravity::Bottom,
        LinearLayoutGravity::Left,
        LinearLayoutGravity::Right,
        LinearLayoutGravity::CenterVertical,
        LinearLayoutGravity::CenterHorizontal,
        LinearLayoutGravity::FillVertical,
        LinearLayoutGravity::FillHorizontal,
        LinearLayoutGravity::Center,
        LinearLayoutGravity::Stretch,
        LinearLayoutGravity::Start,
        LinearLayoutGravity::End,
    ];
    let mut executions = 0;
    for orientation in all_linear_orientations() {
        for direction in LAYOUT_DIRECTIONS {
            for gravity in values {
                let (tree, root) = linear_layout_gravity_tree(orientation, direction, gravity);
                assert_deterministic(tree, root, Constraints::definite(120.0, 90.0));
                executions += 1;
            }
        }
    }
    assert_eq!(executions, 208);
}

#[test]
fn generated_linear_cross_gravity_orientation_direction_matrix_matches_cpp() {
    let values = [
        LinearCrossGravity::None,
        LinearCrossGravity::Start,
        LinearCrossGravity::End,
        LinearCrossGravity::Center,
        LinearCrossGravity::Stretch,
    ];
    let mut executions = 0;
    for orientation in all_linear_orientations() {
        for direction in LAYOUT_DIRECTIONS {
            for gravity in values {
                let (tree, root) = linear_cross_gravity_tree(orientation, direction, gravity);
                assert_deterministic(tree, root, Constraints::definite(120.0, 90.0));
                executions += 1;
            }
        }
    }
    assert_eq!(executions, 80);
}

#[test]
fn generated_linear_css_alignment_matrix_matches_cpp() {
    let align_items_values = [
        AlignItems::Stretch,
        AlignItems::FlexStart,
        AlignItems::Start,
        AlignItems::Center,
        AlignItems::FlexEnd,
        AlignItems::End,
        AlignItems::Baseline,
    ];
    let align_self_values = [
        None,
        Some(AlignItems::Stretch),
        Some(AlignItems::FlexStart),
        Some(AlignItems::Center),
        Some(AlignItems::FlexEnd),
        Some(AlignItems::Baseline),
    ];
    let mut executions = 0;
    for orientation in all_linear_orientations() {
        for direction in LAYOUT_DIRECTIONS {
            for align_items in align_items_values {
                for align_self in align_self_values {
                    let (tree, root) =
                        linear_css_alignment_tree(orientation, direction, align_items, align_self);
                    assert_deterministic(tree, root, Constraints::definite(120.0, 90.0));
                    executions += 1;
                }
            }
        }
    }
    assert_eq!(executions, 672);
}

#[test]
fn generated_linear_start_end_alias_matrix_matches_cpp() {
    let mut executions = 0;
    for orientation in all_linear_orientations() {
        for direction in LAYOUT_DIRECTIONS {
            for justify_content in [JustifyContent::Start, JustifyContent::End] {
                let (tree, root) = linear_orientation_tree(orientation, direction, justify_content);
                assert_deterministic(tree, root, Constraints::definite(120.0, 90.0));
                executions += 1;
            }
        }
    }
    assert_eq!(executions, 32);
}

#[test]
fn generated_linear_weight_gravity_constraint_matrix_matches_cpp() {
    let orientations = [
        LinearOrientation::Horizontal,
        LinearOrientation::HorizontalReverse,
        LinearOrientation::Vertical,
        LinearOrientation::VerticalReverse,
    ];
    let modes = [
        LinearConstraintMode::DefiniteRoot,
        LinearConstraintMode::AtMostOwner,
        LinearConstraintMode::IndefiniteOwner,
    ];
    let patterns = [
        LinearEdgePattern::WeightedMinMax,
        LinearEdgePattern::WeightSumMainGravity,
        LinearEdgePattern::LayoutGravityOverride,
        LinearEdgePattern::CrossAutoMarginBaseline,
    ];
    let mut executions = 0;
    for orientation in orientations {
        for direction in LAYOUT_DIRECTIONS {
            for mode in modes {
                for pattern in patterns {
                    let (tree, root, constraints) =
                        linear_edge_case_tree(orientation, direction, mode, pattern);
                    assert_deterministic(tree, root, constraints);
                    executions += 1;
                }
            }
        }
    }
    assert_eq!(executions, 96);
}

#[test]
fn generated_linear_composite_feature_matrix_matches_cpp() {
    let orientations = [
        LinearOrientation::Horizontal,
        LinearOrientation::HorizontalReverse,
        LinearOrientation::Vertical,
        LinearOrientation::VerticalReverse,
    ];
    let modes = [
        LinearConstraintMode::DefiniteRoot,
        LinearConstraintMode::AtMostOwner,
        LinearConstraintMode::IndefiniteOwner,
    ];
    let mut executions = 0;
    for orientation in orientations {
        for direction in LAYOUT_DIRECTIONS {
            for mode in modes {
                let (tree, root, constraints) =
                    linear_composite_feature_tree(orientation, direction, mode);
                assert_deterministic(tree, root, constraints);
                executions += 1;
            }
        }
    }
    assert_eq!(executions, 24);
}

#[test]
fn generated_display_none_origin_matrix_matches_cpp() {
    let mut executions = 0;
    for container in GENERATED_CONTAINERS {
        let (tree, root) = display_none_origin_tree(container);
        if tree_contains_linear(&tree) {
            assert_deterministic(tree, root, Constraints::definite(128.0, 88.0));
            executions += 1;
        }
    }
    assert_eq!(executions, 2);
}

#[test]
fn generated_out_of_flow_position_matrix_matches_cpp() {
    let positions = [PositionType::Absolute, PositionType::Fixed];
    let insets = [
        OutOfFlowInset::None,
        OutOfFlowInset::Start,
        OutOfFlowInset::End,
        OutOfFlowInset::Both,
    ];
    let mut executions = 0;
    for container in GENERATED_CONTAINERS {
        for position in positions {
            for horizontal in insets {
                for vertical in insets {
                    let (tree, root) =
                        out_of_flow_position_tree(container, position, horizontal, vertical);
                    if tree_contains_linear(&tree) {
                        assert_deterministic(tree, root, Constraints::definite(160.0, 120.0));
                        executions += 1;
                    }
                }
            }
        }
    }
    assert_eq!(executions, 64);
}

#[test]
fn generated_out_of_flow_sizing_matrix_matches_cpp() {
    let positions = [PositionType::Absolute, PositionType::Fixed];
    let variants = [
        OutOfFlowSizingVariant::PercentCalc,
        OutOfFlowSizingVariant::FillAvailable,
        OutOfFlowSizingVariant::OversizedFillAvailableMeasured,
        OutOfFlowSizingVariant::MinMaxMeasuredClamp,
        OutOfFlowSizingVariant::FitContentMeasured,
        OutOfFlowSizingVariant::AspectBorderBoxMeasured,
    ];
    let mut executions = 0;
    for container in GENERATED_CONTAINERS {
        for position in positions {
            for variant in variants {
                let (tree, root) = out_of_flow_sizing_tree(container, position, variant);
                if tree_contains_linear(&tree) {
                    assert_deterministic(tree, root, Constraints::definite(160.0, 120.0));
                    executions += 1;
                }
            }
        }
    }
    assert_eq!(executions, 24);
}

#[test]
fn generated_fixed_descendant_matrix_matches_cpp() {
    let roots = [
        GeneratedContainer::Block,
        GeneratedContainer::FlexRow,
        GeneratedContainer::FlexColumnRtl,
        GeneratedContainer::LinearColumnRtl,
        GeneratedContainer::LinearRow,
        GeneratedContainer::Relative,
        GeneratedContainer::Grid,
    ];
    let nested = [
        GeneratedContainer::Block,
        GeneratedContainer::FlexColumnRtl,
        GeneratedContainer::FlexRow,
        GeneratedContainer::LinearRow,
        GeneratedContainer::LinearColumnRtl,
        GeneratedContainer::Relative,
        GeneratedContainer::Grid,
    ];
    let variants = [
        FixedDescendantVariant::PercentStart,
        FixedDescendantVariant::CalcEnd,
        FixedDescendantVariant::FillAvailable,
        FixedDescendantVariant::MeasuredAspect,
        FixedDescendantVariant::FitContentSubtree,
    ];
    let mut executions = 0;
    for root_container in roots {
        for nested_container in nested {
            for variant in variants {
                let (tree, root) = fixed_descendant_tree(root_container, nested_container, variant);
                if tree_contains_linear(&tree) {
                    assert_deterministic(tree, root, Constraints::definite(180.0, 130.0));
                    executions += 1;
                }
            }
        }
    }
    assert_eq!(executions, 120);
}

#[test]
fn generated_sticky_position_matrix_matches_cpp() {
    let lengths = [
        StickyInsetLength::Points,
        StickyInsetLength::Percent,
        StickyInsetLength::Calc,
    ];
    let insets = [
        OutOfFlowInset::None,
        OutOfFlowInset::Start,
        OutOfFlowInset::End,
        OutOfFlowInset::Both,
    ];
    let mut executions = 0;
    for container in GENERATED_CONTAINERS {
        for length in lengths {
            for horizontal in insets {
                for vertical in insets {
                    let (tree, root) =
                        sticky_position_tree(container, length, horizontal, vertical);
                    if tree_contains_linear(&tree) {
                        assert_generated_sticky_semantics(
                            tree,
                            root,
                            Constraints::definite(160.0, 120.0),
                        );
                        executions += 1;
                    }
                }
            }
        }
    }
    assert_eq!(executions, 96);
}

#[test]
fn generated_sticky_sizing_matrix_matches_cpp() {
    let variants = [
        StickySizingVariant::PercentCalc,
        StickySizingVariant::AutoMeasured,
        StickySizingVariant::MinMaxMeasuredClamp,
        StickySizingVariant::FitContentMeasured,
        StickySizingVariant::AspectBorderBoxMeasured,
    ];
    let mut executions = 0;
    for container in GENERATED_CONTAINERS {
        for variant in variants {
            let (tree, root) = sticky_sizing_tree(container, variant);
            if tree_contains_linear(&tree) {
                assert_generated_sticky_semantics(tree, root, Constraints::definite(160.0, 120.0));
                executions += 1;
            }
        }
    }
    assert_eq!(executions, 10);
}

#[test]
fn generated_deterministic_supported_tree_fuzz_matches_cpp() {
    let mut rng = DeterministicRng::new(0x5A17_1A64);
    let mut executions = 0;
    for case_index in 0..DEFAULT_DETERMINISTIC_SUPPORTED_TREE_CASES {
        let (tree, root, constraints) = deterministic_supported_tree(&mut rng, case_index);
        if tree_contains_linear(&tree) {
            assert_deterministic(tree, root, constraints);
            executions += 1;
        }
    }
    assert_eq!(executions, 27_794);
}

#[allow(clippy::unreadable_literal)] // IDs are copied byte-for-byte from PR #25.
const DETERMINISTIC_HIGH_CASES: &[usize] = &[
    25, 26, 95, 172, 175, 215, 481, 992, 1012, 1234, 2167, 2299, 2425, 2523, 2704, 2740, 2791,
    3109, 3814, 4187, 5572, 6723, 6754, 7009, 7662, 7834, 8359, 8638, 9259, 9591, 9907, 10035,
    10733, 12823, 13868, 14304, 14505, 15500, 16328, 18982, 19719, 19993, 22474, 23012, 23362,
    25535, 27453, 27673, 27731, 29021, 29221, 29902, 31230, 34113, 41175, 42293, 42544, 44450,
    45883, 47159, 51367, 54850, 55744, 56293, 64120, 64135, 68032, 68538, 68701, 69145, 71254,
    76766, 79192, 83434, 85507, 86849, 86992, 87239, 88209, 89938, 91679, 96812, 99274, 105004,
    105770, 106204, 109786, 110407, 114658, 117329, 117836, 121948, 127981, 134513, 139357, 139979,
    146179, 149574, 160141, 161737, 161817, 164190, 164482, 165472, 166953, 176185, 176542, 176761,
    178066, 178583, 179252, 184937, 186434, 190825, 191781, 197620, 197653, 202380, 203219, 207391,
    210793, 218134, 226104, 226687, 237668, 242282, 243040, 244918, 251182, 259483, 269542, 278605,
    282829, 283687, 283842, 285802, 289600, 291152, 292360, 299934, 299965, 302041, 302185, 307159,
    308572, 309457, 310564, 316984, 318982, 319761, 320341, 320509, 324307, 328591, 331564, 331954,
    333262, 337984, 339274, 340393, 349150, 351670, 352168, 352507, 353716, 355459, 356476, 357577,
    358597, 359128, 370004, 372628, 379001, 379945, 380056, 383395, 385732, 389959, 390103, 392284,
    394393, 396763, 406621, 407422, 411883, 411918, 413818, 422704, 428455, 429025, 433231, 434269,
    435562, 437389, 441040, 441260, 441274, 443098, 453535, 455530, 456847, 458437, 466714, 467131,
    467404, 467710, 468928, 470710, 476011, 479230, 480139, 482731, 483016, 483940, 486302, 486361,
    488938, 495574, 496681, 497539, 500887, 501562, 504658, 516046, 517072, 520441, 523900, 524797,
    532018, 536077, 536206, 537223, 539452, 540076, 540469, 540769, 541387, 545509, 549595, 549826,
    559621, 559681, 562162, 562909, 563128, 566536, 566881, 569228, 570262, 573658, 573712, 575104,
    577384, 579595, 583909, 588640, 590308, 591160, 595567, 597256, 598741, 602614, 603613, 610390,
    611671, 612157, 621826, 622414, 622600, 627868, 630196, 631288, 631777, 633370, 634267, 637210,
    637504, 642361, 646950, 652294, 653143, 655681, 655924, 656965, 659557, 661243, 663121, 663199,
    667462, 668800, 673411, 674587, 675739, 679180, 679381, 680551, 687208, 690619, 691855, 692140,
    692560, 693904, 694987, 698668, 700933, 711010, 712609, 712636, 715732, 721072, 724985, 726547,
    726901, 734488, 738802, 739687, 742114, 742318, 746173, 747619, 748012, 751531, 761683, 762991,
    763882, 763942, 764425, 764680, 772306, 776896,
];

fn run_selected_cases(case_indices: &[usize]) -> usize {
    assert!(!case_indices.is_empty());
    let mut sorted = case_indices.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    let mut rng = DeterministicRng::new(0x5A17_1A64);
    let mut next = 0;
    let mut executions = 0;
    let max = *sorted.last().expect("case list is not empty");
    for case_index in 0..=max {
        let (tree, root, constraints) = deterministic_supported_tree(&mut rng, case_index);
        if sorted.get(next).copied() != Some(case_index) {
            continue;
        }
        next += 1;
        if tree_contains_linear(&tree) {
            assert_deterministic(tree, root, constraints);
            executions += 1;
        }
    }
    assert_eq!(next, sorted.len());
    executions
}

/// Runs every explicitly named source regression ID. These lists are semantic
/// inventories in PR #25, so they stay intact even when one of their generated
/// trees contains no literal Linear node.
fn run_exact_cases(case_indices: &[usize]) -> usize {
    assert!(!case_indices.is_empty());
    let mut sorted = case_indices.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    let mut rng = DeterministicRng::new(0x5A17_1A64);
    let mut next = 0;
    let max = *sorted.last().expect("case list is not empty");
    for case_index in 0..=max {
        let (tree, root, constraints) = deterministic_supported_tree(&mut rng, case_index);
        if sorted.get(next).copied() != Some(case_index) {
            continue;
        }
        next += 1;
        assert_deterministic(tree, root, constraints);
    }
    assert_eq!(next, sorted.len());
    next
}

#[test]
fn generated_deterministic_high_case_regressions_match_cpp() {
    assert_eq!(DETERMINISTIC_HIGH_CASES.len(), 330);
    assert_eq!(run_selected_cases(DETERMINISTIC_HIGH_CASES), 257);
}

#[test]
fn generated_deterministic_flex_basis_cache_regressions_match_cpp() {
    assert_eq!(run_exact_cases(&[19, 46, 544, 787, 1006]), 5);
}

#[test]
fn generated_deterministic_percentage_rounding_regressions_match_cpp() {
    assert_eq!(run_exact_cases(&[102]), 1);
}

#[test]
fn generated_deterministic_reverse_flex_bound_rounding_regressions_match_cpp() {
    assert_eq!(run_exact_cases(&[2011]), 1);
}

#[test]
fn generated_deterministic_linear_final_cross_regressions_match_cpp() {
    assert_eq!(run_exact_cases(&[5, 9, 39, 48, 63, 83, 223, 989, 2661]), 9);
}

#[test]
fn generated_deterministic_padding_border_clamp_regressions_match_cpp() {
    assert_eq!(run_exact_cases(&[308]), 1);
}
