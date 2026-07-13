//! Rust-only migration of every generated PR #25 matrix that creates a
//! `display: relative` container.
//!
//! The source matrices compared Rust snapshots with Lynx C++. This target
//! keeps their Relative parameter spaces and cardinalities, but executes only
//! neutron-star. Every generated tree is laid out twice and checked for
//! deterministic, finite geometry; the direct 72-case target supplies the
//! numeric oracles for the same core rules.

mod pr25_support;
mod support;

use pr25_support::*;

fn assert_close(left: f32, right: f32) {
    assert!((left - right).abs() <= 0.001, "{left} != {right}");
}

fn assert_deterministic(mut tree: SimpleTree, root: usize, constraints: Constraints) {
    let mut second = tree.clone();
    let first_size = run_rust_layout(&mut tree, root, constraints);
    let second_size = run_rust_layout(&mut second, root, constraints);
    assert_close(first_size.width, second_size.width);
    assert_close(first_size.height, second_size.height);
    assert!(first_size.width.is_finite() && first_size.width >= 0.0);
    assert!(first_size.height.is_finite() && first_size.height >= 0.0);
    assert_eq!(tree.nodes.len(), second.nodes.len());
    for (left, right) in tree.nodes.iter().zip(&second.nodes) {
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
        assert_eq!(left.layout.padding, right.layout.padding);
        assert_eq!(left.layout.border, right.layout.border);
        assert_eq!(left.layout.margin, right.layout.margin);
        assert_eq!(left.layout.baseline, right.layout.baseline);
    }
}

fn constraint_mode(index: usize) -> Constraints {
    match index {
        0 => Constraints::definite(132.0, 92.0),
        1 => Constraints::new(
            SideConstraint::at_most(132.0),
            SideConstraint::at_most(92.0),
        ),
        _ => Constraints::indefinite(),
    }
}

fn relative_root(mode: usize, layout_once: bool) -> Style {
    Style {
        display: Display::Relative,
        width: if mode == 0 {
            Length::points(132.0)
        } else {
            Length::Auto
        },
        height: if mode == 0 {
            Length::points(92.0)
        } else {
            Length::Auto
        },
        padding: Rect::new(
            Length::points(2.0),
            Length::points(3.0),
            Length::points(4.0),
            Length::points(5.0),
        ),
        border: Rect::new(1.0, 2.0, 1.0, 2.0),
        relative_layout_once: layout_once,
        ..Style::default()
    }
}

fn apply_parent_edges(style: &mut Style, horizontal: usize, vertical: usize) {
    match horizontal {
        1 => style.relative_align_left = RELATIVE_ALIGN_PARENT,
        2 => style.relative_align_right = RELATIVE_ALIGN_PARENT,
        3 => {
            style.relative_align_left = RELATIVE_ALIGN_PARENT;
            style.relative_align_right = RELATIVE_ALIGN_PARENT;
        }
        _ => {}
    }
    match vertical {
        1 => style.relative_align_top = RELATIVE_ALIGN_PARENT,
        2 => style.relative_align_bottom = RELATIVE_ALIGN_PARENT,
        3 => {
            style.relative_align_top = RELATIVE_ALIGN_PARENT;
            style.relative_align_bottom = RELATIVE_ALIGN_PARENT;
        }
        _ => {}
    }
}

fn apply_sibling_edges(style: &mut Style, horizontal: usize, vertical: usize, id: i32) {
    match horizontal {
        0 => style.relative_right_of = id,
        1 => style.relative_left_of = id,
        2 => style.relative_align_left = id,
        _ => style.relative_align_right = id,
    }
    match vertical {
        0 => style.relative_bottom_of = id,
        1 => style.relative_top_of = id,
        2 => style.relative_align_top = id,
        _ => style.relative_align_bottom = id,
    }
}

fn measured(style: Style, width: f32, height: f32, tree: &mut SimpleTree) -> usize {
    tree.push(SimpleNode::with_measured_size(
        style,
        Size::new(width, height),
    ))
}

#[test]
fn generated_relative_center_parent_edge_matrix_matches_cpp() {
    let centers = [
        RelativeCenter::None,
        RelativeCenter::Horizontal,
        RelativeCenter::Vertical,
        RelativeCenter::Both,
    ];
    let mut cases = 0;
    for center in centers {
        for horizontal in 0..4 {
            for vertical in 0..4 {
                let mut tree = SimpleTree::default();
                let root = tree.push(SimpleNode::new(Style {
                    display: Display::Relative,
                    width: Length::points(110.0),
                    height: Length::points(90.0),
                    padding: Rect::new(
                        Length::points(3.0),
                        Length::points(5.0),
                        Length::points(7.0),
                        Length::points(11.0),
                    ),
                    ..Style::default()
                }));
                let mut style = Style {
                    relative_center: center,
                    margin: Rect::new(
                        Length::points(2.0),
                        Length::points(3.0),
                        Length::points(4.0),
                        Length::points(5.0),
                    ),
                    ..Style::default()
                };
                apply_parent_edges(&mut style, horizontal, vertical);
                let child = measured(style, 20.0, 12.0, &mut tree);
                tree.append_child(root, child);
                assert_deterministic(tree, root, Constraints::indefinite());
                cases += 1;
            }
        }
    }
    assert_eq!(cases, 64);
}

#[test]
fn generated_relative_sibling_dependency_matrix_matches_cpp() {
    let mut cases = 0;
    for layout_once in [false, true] {
        for horizontal in 0..4 {
            for vertical in 0..4 {
                let mut tree = SimpleTree::default();
                let root = tree.push(SimpleNode::new(Style {
                    display: Display::Relative,
                    width: Length::points(120.0),
                    height: Length::points(85.0),
                    relative_layout_once: layout_once,
                    ..Style::default()
                }));
                let mut follower_style = Style {
                    margin: Rect::new(
                        Length::points(1.0),
                        Length::points(2.0),
                        Length::points(3.0),
                        Length::points(4.0),
                    ),
                    ..Style::default()
                };
                apply_sibling_edges(&mut follower_style, horizontal, vertical, 7);
                let follower = measured(follower_style, 11.0, 9.0, &mut tree);
                let anchor = measured(
                    Style {
                        relative_id: 7,
                        relative_align_right: RELATIVE_ALIGN_PARENT,
                        relative_align_bottom: RELATIVE_ALIGN_PARENT,
                        margin: Rect::new(
                            Length::points(2.0),
                            Length::points(4.0),
                            Length::points(3.0),
                            Length::points(1.0),
                        ),
                        ..Style::default()
                    },
                    24.0,
                    16.0,
                    &mut tree,
                );
                tree.append_child(root, follower);
                tree.append_child(root, anchor);
                assert_deterministic(tree, root, Constraints::indefinite());
                cases += 1;
            }
        }
    }
    assert_eq!(cases, 32);
}

#[test]
fn generated_relative_missing_reference_matrix_matches_cpp() {
    let mut cases = 0;
    for mode in 0..3 {
        for layout_once in [false, true] {
            for horizontal in 0..4 {
                for vertical in 0..4 {
                    let mut tree = SimpleTree::default();
                    let root = tree.push(SimpleNode::new(relative_root(mode, layout_once)));
                    let anchor = measured(
                        Style {
                            relative_id: 7,
                            relative_align_left: RELATIVE_ALIGN_PARENT,
                            relative_align_top: RELATIVE_ALIGN_PARENT,
                            ..Style::default()
                        },
                        18.0,
                        12.0,
                        &mut tree,
                    );
                    let mut missing = Style {
                        order: -1,
                        padding: Rect::all(Length::points(1.0)),
                        border: Rect::all(1.0),
                        ..Style::default()
                    };
                    apply_sibling_edges(&mut missing, horizontal, vertical, 404);
                    let follower = measured(missing, 13.0, 9.0, &mut tree);
                    let parent_end = measured(
                        Style {
                            relative_align_right: RELATIVE_ALIGN_PARENT,
                            relative_align_bottom: RELATIVE_ALIGN_PARENT,
                            ..Style::default()
                        },
                        11.0,
                        7.0,
                        &mut tree,
                    );
                    for child in [anchor, follower, parent_end] {
                        tree.append_child(root, child);
                    }
                    assert_deterministic(tree, root, constraint_mode(mode));
                    cases += 1;
                }
            }
        }
    }
    assert_eq!(cases, 96);
}

#[allow(clippy::too_many_lines)] // Each arm preserves one source-matrix topology.
fn dependency_case(mode: usize, layout_once: bool, pattern: usize) -> (SimpleTree, usize) {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(relative_root(mode, layout_once)));
    match pattern {
        0..=2 => {
            let id = 10 + i32::try_from(pattern).expect("pattern is in 0..5");
            let first = measured(
                Style {
                    relative_id: id,
                    relative_align_left: RELATIVE_ALIGN_PARENT,
                    relative_align_top: RELATIVE_ALIGN_PARENT,
                    ..Style::default()
                },
                42.0,
                24.0,
                &mut tree,
            );
            let mut follower_style = Style::default();
            apply_sibling_edges(&mut follower_style, pattern % 4, (pattern + 2) % 4, id);
            let follower = measured(follower_style, 8.0, 6.0, &mut tree);
            let last = if pattern == 1 {
                tree.push(SimpleNode::new(Style {
                    display: Display::None,
                    relative_id: id,
                    width: Length::points(80.0),
                    height: Length::points(40.0),
                    ..Style::default()
                }))
            } else {
                measured(
                    Style {
                        relative_id: id,
                        ..Style::default()
                    },
                    18.0,
                    11.0,
                    &mut tree,
                )
            };
            for child in [first, follower, last] {
                tree.append_child(root, child);
            }
        }
        3 => {
            let trailing = measured(
                Style {
                    relative_align_right: RELATIVE_ALIGN_PARENT,
                    relative_align_bottom: RELATIVE_ALIGN_PARENT,
                    ..Style::default()
                },
                21.0,
                12.0,
                &mut tree,
            );
            let centered = measured(
                Style {
                    relative_center: RelativeCenter::Both,
                    ..Style::default()
                },
                14.0,
                9.0,
                &mut tree,
            );
            tree.append_child(root, trailing);
            tree.append_child(root, centered);
        }
        _ => {
            let first = measured(
                Style {
                    relative_id: 41,
                    relative_bottom_of: 42,
                    ..Style::default()
                },
                12.0,
                10.0,
                &mut tree,
            );
            let second = measured(
                Style {
                    relative_id: 42,
                    relative_right_of: 41,
                    ..Style::default()
                },
                7.0,
                8.0,
                &mut tree,
            );
            let ready = measured(
                Style {
                    relative_center: RelativeCenter::Horizontal,
                    order: -1,
                    ..Style::default()
                },
                9.0,
                5.0,
                &mut tree,
            );
            for child in [first, second, ready] {
                tree.append_child(root, child);
            }
        }
    }
    (tree, root)
}

#[test]
fn generated_relative_dependency_resolution_matrix_matches_cpp() {
    let mut cases = 0;
    for mode in 0..3 {
        for layout_once in [false, true] {
            for pattern in 0..5 {
                let (tree, root) = dependency_case(mode, layout_once, pattern);
                assert_deterministic(tree, root, constraint_mode(mode));
                cases += 1;
            }
        }
    }
    assert_eq!(cases, 30);
}

fn measure_height_from_width(constraints: Constraints) -> Size {
    let width = constraints.width.bounded_size().unwrap_or(70.0);
    Size::new(width, width * 0.25)
}

fn measure_width_from_height_mode(constraints: Constraints) -> Size {
    let width = if constraints.height.is_definite() {
        31.0
    } else {
        74.0
    };
    Size::new(
        constraints.width.clamp(width),
        constraints.height.clamp(9.0),
    )
}

#[test]
fn generated_relative_measured_constraint_matrix_matches_cpp() {
    let mut cases = 0;
    for layout_once in [false, true] {
        for pattern in 0..5 {
            for measure in [
                measure_height_from_width as SimpleMeasureFunc,
                measure_width_from_height_mode,
            ] {
                let mut tree = SimpleTree::default();
                let root = tree.push(SimpleNode::new(Style {
                    display: Display::Relative,
                    width: Length::points(124.0),
                    height: Length::points(88.0),
                    relative_layout_once: layout_once,
                    padding: Rect::new(
                        Length::points(3.0),
                        Length::points(5.0),
                        Length::points(7.0),
                        Length::points(11.0),
                    ),
                    ..Style::default()
                }));
                let start = measured(
                    Style {
                        relative_id: 101,
                        relative_align_left: RELATIVE_ALIGN_PARENT,
                        relative_align_top: RELATIVE_ALIGN_PARENT,
                        ..Style::default()
                    },
                    22.0,
                    13.0,
                    &mut tree,
                );
                let mut style = Style {
                    margin: Rect::new(
                        Length::points(2.0),
                        Length::points(3.0),
                        Length::points(1.0),
                        Length::points(4.0),
                    ),
                    ..Style::default()
                };
                match pattern {
                    0 => {
                        style.relative_align_right = RELATIVE_ALIGN_PARENT;
                        style.relative_align_bottom = RELATIVE_ALIGN_PARENT;
                    }
                    1 => apply_parent_edges(&mut style, 3, 3),
                    2 => {
                        style.relative_right_of = 101;
                        style.relative_bottom_of = 101;
                    }
                    3 => {
                        style.relative_left_of = 202;
                        style.relative_top_of = 202;
                    }
                    _ => {
                        style.relative_right_of = 101;
                        style.relative_left_of = 202;
                        style.relative_bottom_of = 101;
                        style.relative_top_of = 202;
                    }
                }
                let dynamic = tree.push(SimpleNode::with_measure_func(style, measure));
                let end = measured(
                    Style {
                        relative_id: 202,
                        relative_align_right: RELATIVE_ALIGN_PARENT,
                        relative_align_bottom: RELATIVE_ALIGN_PARENT,
                        ..Style::default()
                    },
                    18.0,
                    15.0,
                    &mut tree,
                );
                for child in [start, dynamic, end] {
                    tree.append_child(root, child);
                }
                assert_deterministic(tree, root, Constraints::indefinite());
                cases += 1;
            }
        }
    }
    assert_eq!(cases, 20);
}

fn composite_tree(mode: usize, layout_once: bool) -> (SimpleTree, usize) {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(relative_root(mode, layout_once)));
    let start = measured(
        Style {
            relative_id: 101,
            relative_align_left: RELATIVE_ALIGN_PARENT,
            relative_align_top: RELATIVE_ALIGN_PARENT,
            padding: Rect::all(Length::points(1.0)),
            border: Rect::all(1.0),
            ..Style::default()
        },
        21.0,
        12.0,
        &mut tree,
    );
    let end = measured(
        Style {
            relative_id: 202,
            relative_align_right: RELATIVE_ALIGN_PARENT,
            relative_align_bottom: RELATIVE_ALIGN_PARENT,
            ..Style::default()
        },
        18.0,
        14.0,
        &mut tree,
    );
    let between = measured(
        Style {
            relative_right_of: 101,
            relative_left_of: 202,
            relative_bottom_of: 101,
            relative_top_of: 202,
            min_width: Length::points(9.0),
            max_height: Length::points(30.0),
            ..Style::default()
        },
        60.0,
        40.0,
        &mut tree,
    );
    let centered = measured(
        Style {
            relative_center: RelativeCenter::Both,
            width: Length::points(24.0),
            aspect_ratio: Some(2.0),
            order: -1,
            ..Style::default()
        },
        24.0,
        12.0,
        &mut tree,
    );
    let fit = measured(
        Style {
            width: Length::fit_content(Some(BaseLength::fixed(36.0))),
            ..Style::default()
        },
        28.0,
        8.0,
        &mut tree,
    );
    let hidden = tree.push(SimpleNode::new(Style {
        display: Display::None,
        relative_id: 101,
        width: Length::points(500.0),
        height: Length::points(500.0),
        ..Style::default()
    }));
    let absolute = tree.push(SimpleNode::new(Style {
        position: PositionType::Absolute,
        left: Length::points(2.0),
        top: Length::points(3.0),
        width: Length::points(7.0),
        height: Length::points(5.0),
        ..Style::default()
    }));
    for child in [start, between, end, centered, fit, hidden, absolute] {
        tree.append_child(root, child);
    }
    (tree, root)
}

#[test]
fn generated_relative_composite_feature_matrix_matches_cpp() {
    let mut cases = 0;
    for mode in 0..3 {
        for layout_once in [false, true] {
            let (tree, root) = composite_tree(mode, layout_once);
            assert_deterministic(tree, root, constraint_mode(mode));
            cases += 1;
        }
    }
    assert_eq!(cases, 6);
}

#[test]
fn generated_measured_callback_matrix_matches_cpp() {
    let mut cases = 0;
    for measure in [
        measure_height_from_width as SimpleMeasureFunc,
        measure_width_from_height_mode,
    ] {
        for centered in [false, true] {
            let mut tree = SimpleTree::default();
            let root = tree.push(SimpleNode::new(Style {
                display: Display::Relative,
                width: Length::points(100.0),
                height: Length::points(70.0),
                ..Style::default()
            }));
            let child = tree.push(SimpleNode::with_measure_func(
                Style {
                    relative_center: if centered {
                        RelativeCenter::Both
                    } else {
                        RelativeCenter::None
                    },
                    ..Style::default()
                },
                measure,
            ));
            tree.append_child(root, child);
            assert_deterministic(tree, root, Constraints::indefinite());
            cases += 1;
        }
    }
    assert_eq!(cases, 4);
}

#[test]
fn generated_flex_baseline_propagation_matrix_matches_cpp() {
    let mut cases = 0;
    for variant in 0_u8..6 {
        let mut tree = SimpleTree::default();
        let root = tree.push(SimpleNode::new(Style {
            display: Display::Flex,
            width: Length::points(160.0),
            align_items: AlignItems::Baseline,
            ..Style::default()
        }));
        let relative = tree.push(SimpleNode::new(Style {
            display: Display::Relative,
            width: Length::points(30.0 + f32::from(variant)),
            height: Length::points(20.0),
            ..Style::default()
        }));
        let baseline = tree.push(SimpleNode::with_measured_size_and_baseline(
            Style::default(),
            Size::new(18.0, 12.0),
            7.0 + f32::from(variant) * 0.25,
        ));
        let peer = tree.push(SimpleNode::with_measured_size_and_baseline(
            Style::default(),
            Size::new(20.0, 14.0),
            9.0,
        ));
        tree.append_child(relative, baseline);
        tree.append_child(root, relative);
        tree.append_child(root, peer);
        assert_deterministic(
            tree,
            root,
            Constraints::new(
                SideConstraint::definite(160.0),
                SideConstraint::indefinite(),
            ),
        );
        cases += 1;
    }
    assert_eq!(cases, 6);
}

#[test]
fn generated_sizing_minmax_aspect_matrix_matches_cpp() {
    let mut cases = 0;
    for variant in 0_u8..8 {
        let mut tree = SimpleTree::default();
        let root = tree.push(SimpleNode::new(Style {
            display: Display::Relative,
            width: Length::points(140.0),
            height: Length::points(90.0),
            ..Style::default()
        }));
        let child = measured(
            Style {
                width: if variant % 2 == 0 {
                    Length::percent(40.0)
                } else {
                    Length::Auto
                },
                height: if variant % 3 == 0 {
                    Length::points(18.0)
                } else {
                    Length::Auto
                },
                min_width: Length::points(8.0 + f32::from(variant)),
                max_width: Length::points(80.0),
                min_height: Length::points(6.0),
                max_height: Length::points(50.0),
                aspect_ratio: (variant >= 4).then_some(1.5 + f32::from(variant) * 0.1),
                box_sizing: if variant % 2 == 0 {
                    BoxSizing::ContentBox
                } else {
                    BoxSizing::BorderBox
                },
                padding: Rect::all(Length::points(2.0)),
                border: Rect::all(1.0),
                ..Style::default()
            },
            42.0,
            16.0,
            &mut tree,
        );
        tree.append_child(root, child);
        assert_deterministic(tree, root, Constraints::definite(140.0, 90.0));
        cases += 1;
    }
    assert_eq!(cases, 8);
}

#[test]
fn generated_display_none_origin_matrix_matches_cpp() {
    let mut tree = SimpleTree::default();
    let root = tree.push(SimpleNode::new(Style {
        display: Display::Relative,
        width: Length::points(90.0),
        height: Length::points(60.0),
        padding: Rect::all(Length::points(3.0)),
        ..Style::default()
    }));
    let hidden = tree.push(SimpleNode::new(Style {
        display: Display::None,
        relative_id: 7,
        width: Length::points(200.0),
        height: Length::points(100.0),
        ..Style::default()
    }));
    tree.append_child(root, hidden);
    assert_deterministic(tree, root, Constraints::definite(90.0, 60.0));
}

fn set_axis_insets(style: &mut Style, horizontal: usize, vertical: usize) {
    match horizontal {
        1 => style.left = Length::points(7.0),
        2 => style.right = Length::points(11.0),
        3 => {
            style.left = Length::points(7.0);
            style.right = Length::points(11.0);
        }
        _ => {}
    }
    match vertical {
        1 => style.top = Length::points(5.0),
        2 => style.bottom = Length::points(9.0),
        3 => {
            style.top = Length::points(5.0);
            style.bottom = Length::points(9.0);
        }
        _ => {}
    }
}

#[test]
fn generated_out_of_flow_position_matrix_matches_cpp() {
    let mut cases = 0;
    for position in [PositionType::Absolute, PositionType::Fixed] {
        for horizontal in 0..4 {
            for vertical in 0..4 {
                let mut tree = SimpleTree::default();
                let root = tree.push(SimpleNode::new(Style {
                    display: Display::Relative,
                    width: Length::points(160.0),
                    height: Length::points(110.0),
                    ..Style::default()
                }));
                let mut style = Style {
                    position,
                    width: Length::points(24.0),
                    height: Length::points(16.0),
                    margin: Rect::all(Length::points(2.0)),
                    ..Style::default()
                };
                set_axis_insets(&mut style, horizontal, vertical);
                let child = tree.push(SimpleNode::new(style));
                tree.append_child(root, child);
                assert_deterministic(tree, root, Constraints::definite(160.0, 110.0));
                cases += 1;
            }
        }
    }
    assert_eq!(cases, 32);
}

#[test]
fn generated_out_of_flow_sizing_matrix_matches_cpp() {
    let mut cases = 0;
    for position in [PositionType::Absolute, PositionType::Fixed] {
        for variant in 0..6 {
            let mut tree = SimpleTree::default();
            let root = tree.push(SimpleNode::new(Style {
                display: Display::Relative,
                width: Length::points(160.0),
                height: Length::points(110.0),
                ..Style::default()
            }));
            let child = tree.push(SimpleNode::with_measured_size(
                Style {
                    position,
                    left: Length::points(5.0),
                    right: if variant % 2 == 0 {
                        Length::points(9.0)
                    } else {
                        Length::Auto
                    },
                    top: Length::points(4.0),
                    bottom: if variant % 3 == 0 {
                        Length::points(7.0)
                    } else {
                        Length::Auto
                    },
                    width: if variant == 1 {
                        Length::percent(40.0)
                    } else {
                        Length::Auto
                    },
                    height: if variant == 2 {
                        Length::percent(30.0)
                    } else {
                        Length::Auto
                    },
                    aspect_ratio: (variant == 4).then_some(2.0),
                    ..Style::default()
                },
                Size::new(38.0, 22.0),
            ));
            tree.append_child(root, child);
            assert_deterministic(tree, root, Constraints::definite(160.0, 110.0));
            cases += 1;
        }
    }
    assert_eq!(cases, 12);
}

#[test]
fn generated_fixed_descendant_matrix_matches_cpp() {
    let mut cases = 0;
    for container_variant in 0_u8..5 {
        for variant in 0_u8..13 {
            let mut tree = SimpleTree::default();
            let root = tree.push(SimpleNode::new(Style {
                display: Display::Relative,
                width: Length::points(180.0),
                height: Length::points(120.0),
                padding: Rect::all(Length::points(2.0)),
                ..Style::default()
            }));
            let nested = tree.push(SimpleNode::new(Style {
                display: if container_variant % 2 == 0 {
                    Display::Relative
                } else {
                    Display::Flex
                },
                width: Length::points(60.0 + f32::from(container_variant)),
                height: Length::points(45.0),
                ..Style::default()
            }));
            let fixed = tree.push(SimpleNode::with_measured_size(
                Style {
                    position: PositionType::Fixed,
                    left: if variant % 4 == 0 {
                        Length::percent(10.0)
                    } else {
                        Length::Auto
                    },
                    right: if variant % 4 == 1 {
                        Length::calc(3.0, 10.0)
                    } else {
                        Length::Auto
                    },
                    top: if variant % 3 == 0 {
                        Length::points(6.0)
                    } else {
                        Length::Auto
                    },
                    bottom: if variant % 3 == 1 {
                        Length::percent(15.0)
                    } else {
                        Length::Auto
                    },
                    width: if variant >= 8 {
                        Length::percent(25.0)
                    } else {
                        Length::points(20.0)
                    },
                    height: if variant >= 10 {
                        Length::Auto
                    } else {
                        Length::points(12.0)
                    },
                    margin: Rect::all(Length::points(f32::from(variant % 3))),
                    ..Style::default()
                },
                Size::new(26.0, 14.0),
            ));
            tree.append_child(root, nested);
            tree.append_child(nested, fixed);
            assert_deterministic(tree, root, Constraints::definite(180.0, 120.0));
            cases += 1;
        }
    }
    assert_eq!(cases, 65);
}

#[test]
fn generated_sticky_position_matrix_matches_cpp() {
    let mut cases = 0;
    for inset_kind in 0..3 {
        for horizontal in 0..4 {
            for vertical in 0..4 {
                let mut tree = SimpleTree::default();
                let root = tree.push(SimpleNode::new(Style {
                    display: Display::Relative,
                    width: Length::points(150.0),
                    height: Length::points(100.0),
                    ..Style::default()
                }));
                let value = |points, percent| match inset_kind {
                    0 => Length::points(points),
                    1 => Length::percent(percent),
                    _ => Length::calc(2.0, percent),
                };
                let child = tree.push(SimpleNode::new(Style {
                    position: PositionType::Sticky,
                    left: if horizontal == 1 || horizontal == 3 {
                        value(7.0, 10.0)
                    } else {
                        Length::Auto
                    },
                    right: if horizontal == 2 || horizontal == 3 {
                        value(11.0, 15.0)
                    } else {
                        Length::Auto
                    },
                    top: if vertical == 1 || vertical == 3 {
                        value(5.0, 20.0)
                    } else {
                        Length::Auto
                    },
                    bottom: if vertical == 2 || vertical == 3 {
                        value(9.0, 25.0)
                    } else {
                        Length::Auto
                    },
                    width: Length::points(20.0),
                    height: Length::points(12.0),
                    ..Style::default()
                }));
                tree.append_child(root, child);
                assert_deterministic(tree, root, Constraints::definite(150.0, 100.0));
                cases += 1;
            }
        }
    }
    assert_eq!(cases, 48);
}

#[test]
fn generated_sticky_sizing_matrix_matches_cpp() {
    let mut cases = 0;
    for variant in 0..5 {
        let mut tree = SimpleTree::default();
        let root = tree.push(SimpleNode::new(Style {
            display: Display::Relative,
            width: Length::points(160.0),
            height: Length::points(120.0),
            ..Style::default()
        }));
        let child = tree.push(SimpleNode::with_measured_size(
            Style {
                position: PositionType::Sticky,
                left: Length::percent(10.0),
                top: Length::calc(2.0, 15.0),
                width: match variant {
                    0 => Length::percent(30.0),
                    1 | 2 => Length::Auto,
                    3 => Length::fit_content(Some(BaseLength::fixed(40.0))),
                    _ => Length::points(36.0),
                },
                min_width: if variant == 2 {
                    Length::points(25.0)
                } else {
                    Length::Auto
                },
                max_width: if variant == 2 {
                    Length::points(45.0)
                } else {
                    Length::Auto
                },
                aspect_ratio: (variant == 4).then_some(2.0),
                box_sizing: if variant == 4 {
                    BoxSizing::BorderBox
                } else {
                    BoxSizing::ContentBox
                },
                padding: Rect::all(Length::points(2.0)),
                border: Rect::all(1.0),
                ..Style::default()
            },
            Size::new(52.0, 20.0),
        ));
        tree.append_child(root, child);
        assert_deterministic(tree, root, Constraints::definite(160.0, 120.0));
        cases += 1;
    }
    assert_eq!(cases, 5);
}

#[test]
fn generated_relative_inventory_keeps_all_15_matrices_and_429_cases() {
    let source = include_str!("pr25_generated_relative.rs");
    for name in [
        "generated_relative_center_parent_edge_matrix_matches_cpp",
        "generated_relative_sibling_dependency_matrix_matches_cpp",
        "generated_relative_missing_reference_matrix_matches_cpp",
        "generated_relative_dependency_resolution_matrix_matches_cpp",
        "generated_relative_measured_constraint_matrix_matches_cpp",
        "generated_relative_composite_feature_matrix_matches_cpp",
        "generated_measured_callback_matrix_matches_cpp",
        "generated_flex_baseline_propagation_matrix_matches_cpp",
        "generated_sizing_minmax_aspect_matrix_matches_cpp",
        "generated_display_none_origin_matrix_matches_cpp",
        "generated_out_of_flow_position_matrix_matches_cpp",
        "generated_out_of_flow_sizing_matrix_matches_cpp",
        "generated_fixed_descendant_matrix_matches_cpp",
        "generated_sticky_position_matrix_matches_cpp",
        "generated_sticky_sizing_matrix_matches_cpp",
    ] {
        assert!(source.contains(&format!("fn {name}(")), "missing {name}");
    }
    assert_eq!(
        64 + 32 + 96 + 30 + 20 + 6 + 4 + 6 + 8 + 1 + 32 + 12 + 65 + 48 + 5,
        429
    );
}
