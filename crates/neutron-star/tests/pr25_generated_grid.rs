//! Pure-Rust generated Grid matrices migrated from PupilTong/lynx#25.
//!
//! PR #25 compared these generated trees with Lynx C++. This target keeps
//! the six matrices but executes neutron-star only, as requested. Each case
//! validates finite, non-negative geometry and the matrix cardinality so a
//! dimension cannot silently disappear.

mod pr25_support;
mod support;

use pr25_support::*;

fn assert_valid(tree: &SimpleTree, root: usize, expected_children: usize) {
    let root_layout = tree.nodes[root].layout;
    assert!(root_layout.size.width.is_finite() && root_layout.size.width >= 0.0);
    assert!(root_layout.size.height.is_finite() && root_layout.size.height >= 0.0);
    assert_eq!(tree.nodes[root].children.len(), expected_children);
    for &child in &tree.nodes[root].children {
        let layout = tree.nodes[child].layout;
        assert!(layout.offset.x.is_finite() && layout.offset.y.is_finite());
        assert!(layout.size.width.is_finite() && layout.size.width >= 0.0);
        assert!(layout.size.height.is_finite() && layout.size.height >= 0.0);
    }
}

fn fixed_child(tree: &mut SimpleTree, style: Style) -> usize {
    tree.push(SimpleNode::new(Style {
        width: Length::points(12.0),
        height: Length::points(8.0),
        ..style
    }))
}

#[test]
fn generated_grid_item_alignment_matrix_runs_in_rust() {
    let mut cases = 0;
    for direction in [Direction::Ltr, Direction::Rtl] {
        for justify_self in [
            JustifyItems::Auto,
            JustifyItems::Stretch,
            JustifyItems::Start,
            JustifyItems::Center,
            JustifyItems::End,
        ] {
            for align_self in [
                None,
                Some(AlignItems::Stretch),
                Some(AlignItems::Start),
                Some(AlignItems::Center),
                Some(AlignItems::End),
                Some(AlignItems::FlexStart),
                Some(AlignItems::FlexEnd),
                Some(AlignItems::Baseline),
            ] {
                let mut tree = SimpleTree::default();
                let root = tree.push(SimpleNode::new(Style {
                    display: Display::Grid,
                    direction,
                    width: Length::points(60.0),
                    height: Length::points(40.0),
                    grid_template_columns: vec![Length::points(60.0)],
                    grid_template_rows: vec![Length::points(40.0)],
                    ..Style::default()
                }));
                let child = fixed_child(
                    &mut tree,
                    Style {
                        align_self,
                        justify_self,
                        ..Style::default()
                    },
                );
                tree.append_child(root, child);
                run_rust_layout(&mut tree, root, Constraints::definite(60.0, 40.0));
                assert_valid(&tree, root, 1);
                cases += 1;
            }
        }
    }
    assert_eq!(cases, 80);
}

#[test]
fn generated_grid_auto_margin_alignment_matrix_runs_in_rust() {
    let margins = [
        Rect::all(Length::ZERO),
        Rect::new(Length::Auto, Length::ZERO, Length::ZERO, Length::ZERO),
        Rect::new(Length::ZERO, Length::Auto, Length::ZERO, Length::ZERO),
        Rect::new(Length::Auto, Length::Auto, Length::Auto, Length::Auto),
    ];
    let mut cases = 0;
    for direction in [Direction::Ltr, Direction::Rtl] {
        for margin in margins {
            for justify_self in [JustifyItems::Start, JustifyItems::Center, JustifyItems::End] {
                let mut tree = SimpleTree::default();
                let root = tree.push(SimpleNode::new(Style {
                    display: Display::Grid,
                    direction,
                    width: Length::points(64.0),
                    height: Length::points(36.0),
                    grid_template_columns: vec![Length::points(64.0)],
                    grid_template_rows: vec![Length::points(36.0)],
                    ..Style::default()
                }));
                let child = fixed_child(
                    &mut tree,
                    Style {
                        margin,
                        justify_self,
                        align_self: Some(AlignItems::Center),
                        ..Style::default()
                    },
                );
                tree.append_child(root, child);
                run_rust_layout(&mut tree, root, Constraints::definite(64.0, 36.0));
                assert_valid(&tree, root, 1);
                cases += 1;
            }
        }
    }
    assert_eq!(cases, 24);
}

#[test]
fn generated_grid_track_sizing_matrix_runs_in_rust() {
    let tracks = [
        Length::points(18.0),
        Length::Auto,
        Length::MinContent,
        Length::MaxContent,
        Length::fr(1.0),
        Length::fit_content(Some(BaseLength::fixed(28.0))),
        Length::fit_content(Some(BaseLength::fixed_and_percent(4.0, 40.0))),
    ];
    let constraints = [
        Constraints::indefinite(),
        Constraints::definite(100.0, 40.0),
        Constraints::new(
            SideConstraint::at_most(100.0),
            SideConstraint::at_most(40.0),
        ),
    ];
    let mut cases = 0;
    for track in tracks {
        for constraint in constraints {
            let mut tree = SimpleTree::default();
            let root = tree.push(SimpleNode::new(Style {
                display: Display::Grid,
                grid_template_columns: vec![track, Length::points(12.0)],
                grid_template_rows: vec![Length::Auto],
                column_gap: Length::points(3.0),
                ..Style::default()
            }));
            let measured = tree.push(SimpleNode::with_measured_size(
                Style {
                    justify_self: JustifyItems::Start,
                    align_self: Some(AlignItems::Start),
                    ..Style::default()
                },
                Size::new(42.0, 16.0),
            ));
            let marker = fixed_child(
                &mut tree,
                Style {
                    grid_column_start: Some(2),
                    ..Style::default()
                },
            );
            tree.append_child(root, measured);
            tree.append_child(root, marker);
            run_rust_layout(&mut tree, root, constraint);
            assert_valid(&tree, root, 2);
            cases += 1;
        }
    }
    assert_eq!(cases, 21);
}

#[test]
fn generated_grid_content_alignment_matrix_runs_in_rust() {
    let values = [
        AlignContent::Start,
        AlignContent::End,
        AlignContent::Center,
        AlignContent::Stretch,
        AlignContent::SpaceBetween,
        AlignContent::SpaceAround,
        AlignContent::SpaceEvenly,
    ];
    let mut cases = 0;
    for justify_content in values {
        for align_content in values {
            let mut tree = SimpleTree::default();
            let root = tree.push(SimpleNode::new(Style {
                display: Display::Grid,
                width: Length::points(100.0),
                height: Length::points(80.0),
                grid_template_columns: vec![Length::points(20.0), Length::points(30.0)],
                grid_template_rows: vec![Length::points(15.0), Length::points(20.0)],
                column_gap: Length::points(5.0),
                row_gap: Length::points(4.0),
                justify_content,
                align_content,
                ..Style::default()
            }));
            for _ in 0..4 {
                let child = fixed_child(&mut tree, Style::default());
                tree.append_child(root, child);
            }
            run_rust_layout(&mut tree, root, Constraints::definite(100.0, 80.0));
            assert_valid(&tree, root, 4);
            cases += 1;
        }
    }
    assert_eq!(cases, 49);
}

#[test]
fn generated_grid_auto_flow_placement_matrix_runs_in_rust() {
    let mut cases = 0;
    for grid_auto_flow in [
        GridAutoFlow::Row,
        GridAutoFlow::Column,
        GridAutoFlow::Dense,
        GridAutoFlow::RowDense,
        GridAutoFlow::ColumnDense,
    ] {
        for locked in [false, true] {
            let mut tree = SimpleTree::default();
            let root = tree.push(SimpleNode::new(Style {
                display: Display::Grid,
                width: Length::points(72.0),
                height: Length::points(54.0),
                grid_auto_flow,
                grid_template_columns: vec![Length::points(20.0); 3],
                grid_template_rows: vec![Length::points(14.0); 3],
                column_gap: Length::points(3.0),
                row_gap: Length::points(2.0),
                ..Style::default()
            }));
            for index in 0..5 {
                let child = fixed_child(
                    &mut tree,
                    Style {
                        grid_column_start: (locked && index == 0).then_some(2),
                        grid_row_start: (locked && index == 0).then_some(2),
                        grid_column_span: if index == 1 { 2 } else { 1 },
                        ..Style::default()
                    },
                );
                tree.append_child(root, child);
            }
            run_rust_layout(&mut tree, root, Constraints::definite(72.0, 54.0));
            assert_valid(&tree, root, 5);
            cases += 1;
        }
    }
    assert_eq!(cases, 10);
}

#[test]
fn generated_grid_out_of_flow_area_matrix_runs_in_rust() {
    let mut cases = 0;
    for direction in [Direction::Ltr, Direction::Rtl] {
        for position in [PositionType::Absolute, PositionType::Fixed] {
            for partial_lines in [false, true] {
                for paired_insets in [false, true] {
                    let mut tree = SimpleTree::default();
                    let root = tree.push(SimpleNode::new(Style {
                        display: Display::Grid,
                        direction,
                        width: Length::points(90.0),
                        height: Length::points(50.0),
                        grid_template_columns: vec![Length::points(30.0), Length::points(40.0)],
                        grid_template_rows: vec![Length::points(20.0), Length::points(20.0)],
                        column_gap: Length::points(4.0),
                        row_gap: Length::points(3.0),
                        ..Style::default()
                    }));
                    let child = fixed_child(
                        &mut tree,
                        Style {
                            position,
                            grid_column_start: Some(2),
                            grid_column_end: (!partial_lines).then_some(3),
                            grid_row_start: Some(1),
                            grid_row_end: (!partial_lines).then_some(2),
                            left: if paired_insets {
                                Length::points(2.0)
                            } else {
                                Length::Auto
                            },
                            right: if paired_insets {
                                Length::points(3.0)
                            } else {
                                Length::Auto
                            },
                            top: Length::points(1.0),
                            ..Style::default()
                        },
                    );
                    tree.append_child(root, child);
                    run_rust_layout(&mut tree, root, Constraints::definite(90.0, 50.0));
                    assert_valid(&tree, root, 1);
                    cases += 1;
                }
            }
        }
    }
    assert_eq!(cases, 16);
}
