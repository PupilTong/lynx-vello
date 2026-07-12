//! Additional Flexbox coverage translated from PupilTong/lynx#25.
//!
//! The main Flexbox conformance matrix lives in `flexbox.rs`. This target
//! keeps the smaller cross-cutting cases that exercise the host protocol,
//! positioned Flex children, aspect-ratio transfer, and solver invariants
//! which are easy to lose when the algorithm is reorganized.

mod support;

use neutron_star::compute::{
    FnLeafMeasurer, LeafMetrics, compute_flexbox_layout, compute_grid_layout, compute_leaf_layout,
    hide_subtree,
};
use neutron_star::prelude::*;
use neutron_star::style::{
    AlignItems, BoxGenerationMode, BoxSizing, Dimension, Direction, FlexDirection, FlexWrap,
    JustifyContent, LengthPercentage, LengthPercentageAuto, Position, Visibility,
};
use support::*;

fn absolute_leaf(tree: &mut TestTree, width: f32, height: f32) -> NodeId {
    let mut style = fixed_leaf_style(width, height);
    style.position = Position::Absolute;
    tree.push_leaf(style, Size::new(width, height), None)
}

#[test]
fn flex_item_derives_cross_size_from_main_size_and_aspect_ratio() {
    let mut tree = TestTree::default();
    let mut child_style = fixed_leaf_style(40.0, 0.0);
    child_style.size.height = Dimension::Auto;
    child_style.aspect_ratio = Some(2.0);
    let child = tree.push_leaf(child_style, Size::ZERO, None);
    let root = flex_container(
        &mut tree,
        TestStyle {
            align_items: Some(AlignItems::FlexStart),
            ..TestStyle::default()
        },
        &[child],
    );

    let output = perform_layout(
        &mut tree,
        root,
        Size::new(Some(100.0), None),
        Size::new(AvailableSpace::Definite(100.0), AvailableSpace::MaxContent),
    );

    assert_size(output.size, Size::new(100.0, 20.0));
    assert_size(tree.layout(child).size, Size::new(40.0, 20.0));
}

#[test]
fn flex_percentage_padding_resolves_against_definite_containing_block_width() {
    let mut tree = TestTree::default();
    let child = fixed_leaf(&mut tree, 10.0, 10.0);
    let inner = flex_container(
        &mut tree,
        TestStyle {
            box_sizing: BoxSizing::BorderBox,
            size: Size::new(Dimension::Length(100.0), Dimension::Auto),
            padding: Edges::uniform(LengthPercentage::percent(0.1)),
            align_items: Some(AlignItems::FlexStart),
            ..TestStyle::default()
        },
        &[child],
    );
    let root = flex_container(
        &mut tree,
        TestStyle {
            align_items: Some(AlignItems::FlexStart),
            ..TestStyle::default()
        },
        &[inner],
    );

    let output = perform_layout(
        &mut tree,
        root,
        Size::new(Some(100.0), None),
        Size::new(AvailableSpace::Definite(100.0), AvailableSpace::MaxContent),
    );

    assert_close(tree.layout(inner).padding.left, 10.0);
    assert_close(tree.layout(inner).padding.top, 10.0);
    assert_size(tree.layout(inner).size, Size::new(100.0, 30.0));
    assert_point(tree.layout(child).location, Point::new(10.0, 10.0));
    assert_size(output.size, Size::new(100.0, 30.0));
}

#[test]
fn vertical_percentage_padding_and_margin_use_width_percent_base() {
    let mut tree = TestTree::default();
    let mut child_style = fixed_leaf_style(10.0, 5.0);
    child_style.margin.top = LengthPercentageAuto::Percent(0.05);
    child_style.margin.bottom = LengthPercentageAuto::Percent(0.02);
    let child = tree.push_leaf(child_style, Size::new(10.0, 5.0), None);
    let root = flex_container(
        &mut tree,
        TestStyle {
            size: Size::new(Dimension::Length(120.0), Dimension::Auto),
            padding: Edges::uniform(LengthPercentage::percent(0.1)),
            align_items: Some(AlignItems::FlexStart),
            ..TestStyle::default()
        },
        &[child],
    );

    let output = perform_layout(
        &mut tree,
        root,
        Size::NONE,
        Size::new(AvailableSpace::Definite(120.0), AvailableSpace::MaxContent),
    );

    assert_close(tree.layout(child).margin.top, 6.0);
    assert_close(tree.layout(child).margin.bottom, 2.4);
    assert_size(tree.layout(child).size, Size::new(10.0, 5.0));
    assert_point(tree.layout(child).location, Point::new(12.0, 18.0));
    // Keep fractional CSS-pixel geometry. PR #25's source assertion rounded
    // this to an integer layout unit, which is not part of CSS Flexbox.
    assert_size(output.size, Size::new(144.0, 37.4));
}

#[test]
fn collapsed_flex_item_preserves_its_first_round_baseline_line_cross_size() {
    let mut tree = TestTree::default();
    let mut collapsed_style = fixed_leaf_style(10.0, 40.0);
    collapsed_style.visibility = Visibility::Collapse;
    let collapsed = tree.push_leaf(collapsed_style, Size::new(10.0, 40.0), Some(5.0));
    let visible = tree.push_leaf(
        fixed_leaf_style(10.0, 40.0),
        Size::new(10.0, 40.0),
        Some(35.0),
    );
    let root = flex_container(
        &mut tree,
        TestStyle {
            align_items: Some(AlignItems::Baseline),
            ..TestStyle::default()
        },
        &[collapsed, visible],
    );

    let output = perform_layout(
        &mut tree,
        root,
        Size::new(Some(100.0), None),
        Size::new(AvailableSpace::Definite(100.0), AvailableSpace::MaxContent),
    );

    // First round: max(before-baseline)=35 and max(after-baseline)=35,
    // therefore the line (and the collapsed item's strut) is 70px tall.
    // The collapsed item's own 40px outer cross size is not the strut size.
    assert_size(output.size, Size::new(100.0, 70.0));
    assert_size(tree.layout(collapsed).size, Size::ZERO);
    assert_size(tree.layout(visible).size, Size::new(10.0, 40.0));
}

#[test]
fn absolute_position_can_use_right_and_bottom_insets_in_a_flex_container() {
    let mut tree = TestTree::default();
    let absolute = absolute_leaf(&mut tree, 20.0, 10.0);
    tree.source_node_mut(absolute).style.inset = Edges {
        left: LengthPercentageAuto::Auto,
        right: LengthPercentageAuto::Length(5.0),
        top: LengthPercentageAuto::Auto,
        bottom: LengthPercentageAuto::Length(7.0),
    };
    let in_flow = fixed_leaf(&mut tree, 15.0, 10.0);
    let root = flex_container(&mut tree, TestStyle::default(), &[absolute, in_flow]);

    definite_layout(&mut tree, root, 100.0, 80.0);

    assert_point(tree.layout(absolute).location, Point::new(75.0, 63.0));
    assert_point(tree.layout(in_flow).location, Point::ZERO);
}

#[test]
fn absolute_flex_child_without_insets_uses_container_alignment() {
    let mut tree = TestTree::default();
    let absolute = absolute_leaf(&mut tree, 20.0, 10.0);
    let root = flex_container(
        &mut tree,
        TestStyle {
            justify_content: Some(JustifyContent::Center),
            align_items: Some(AlignItems::FlexEnd),
            ..TestStyle::default()
        },
        &[absolute],
    );

    definite_layout(&mut tree, root, 100.0, 40.0);

    assert_point(tree.layout(absolute).location, Point::new(40.0, 30.0));
}

#[test]
fn absolute_flex_child_center_alignment_keeps_negative_free_space() {
    let mut tree = TestTree::default();
    let absolute = absolute_leaf(&mut tree, 140.0, 10.0);
    let root = flex_container(
        &mut tree,
        TestStyle {
            justify_content: Some(JustifyContent::Center),
            align_items: Some(AlignItems::FlexStart),
            ..TestStyle::default()
        },
        &[absolute],
    );

    definite_layout(&mut tree, root, 100.0, 40.0);

    assert_point(tree.layout(absolute).location, Point::new(-20.0, 0.0));
}

#[test]
fn absolute_flex_child_wrap_reverse_reverses_cross_axis_static_alignment() {
    let mut tree = TestTree::default();
    let absolute = absolute_leaf(&mut tree, 20.0, 10.0);
    let root = flex_container(
        &mut tree,
        TestStyle {
            flex_wrap: FlexWrap::WrapReverse,
            align_items: Some(AlignItems::FlexEnd),
            ..TestStyle::default()
        },
        &[absolute],
    );

    definite_layout(&mut tree, root, 100.0, 40.0);

    assert_point(tree.layout(absolute).location, Point::ZERO);
}

#[test]
fn absolute_rtl_flex_child_without_insets_uses_physical_fronts() {
    let cases = [
        (FlexDirection::Row, FlexWrap::NoWrap, 80.0),
        (FlexDirection::Column, FlexWrap::NoWrap, 80.0),
        (FlexDirection::Column, FlexWrap::WrapReverse, 0.0),
    ];

    for (flex_direction, flex_wrap, expected_x) in cases {
        let mut tree = TestTree::default();
        let absolute = absolute_leaf(&mut tree, 20.0, 10.0);
        let root = flex_container(
            &mut tree,
            TestStyle {
                direction: Direction::Rtl,
                flex_direction,
                flex_wrap,
                align_items: Some(AlignItems::FlexStart),
                ..TestStyle::default()
            },
            &[absolute],
        );

        definite_layout(&mut tree, root, 100.0, 40.0);

        assert_point(tree.layout(absolute).location, Point::new(expected_x, 0.0));
    }
}

#[test]
fn flex_relative_child_percent_offsets_use_container_size() {
    let mut tree = TestTree::default();
    let mut relative_style = fixed_leaf_style(20.0, 10.0);
    relative_style.inset = Edges {
        left: LengthPercentageAuto::Percent(0.10),
        right: LengthPercentageAuto::Auto,
        top: LengthPercentageAuto::Percent(0.25),
        bottom: LengthPercentageAuto::Auto,
    };
    let relative = tree.push_leaf(relative_style, Size::new(20.0, 10.0), None);
    let following = fixed_leaf(&mut tree, 10.0, 10.0);
    let root = flex_container(
        &mut tree,
        TestStyle {
            align_items: Some(AlignItems::FlexStart),
            ..TestStyle::default()
        },
        &[relative, following],
    );

    definite_layout(&mut tree, root, 100.0, 40.0);

    assert_point(tree.layout(relative).location, Point::new(10.0, 10.0));
    // Relative positioning is visual only: the following item keeps the
    // unshifted flow position.
    assert_point(tree.layout(following).location, Point::new(20.0, 0.0));
}

fn lower_host_sticky_insets(
    insets: Edges<LengthPercentageAuto>,
    containing_size: Size<f32>,
) -> Edges<Option<f32>> {
    fn lower(value: LengthPercentageAuto, basis: f32) -> Option<f32> {
        match value {
            LengthPercentageAuto::Length(value) => Some(value),
            LengthPercentageAuto::Percent(fraction) => Some(fraction * basis),
            LengthPercentageAuto::Calc(_) => {
                panic!("the additional sticky fixture does not use calc()")
            }
            LengthPercentageAuto::Auto => None,
        }
    }

    Edges {
        left: lower(insets.left, containing_size.width),
        right: lower(insets.right, containing_size.width),
        top: lower(insets.top, containing_size.height),
        bottom: lower(insets.bottom, containing_size.height),
    }
}

fn assert_sticky_host_boundary(
    authored_insets: Edges<LengthPercentageAuto>,
    expected_lowered: Edges<Option<f32>>,
) {
    let mut tree = TestTree::default();
    let sticky = fixed_leaf(&mut tree, 20.0, 10.0);
    let following = fixed_leaf(&mut tree, 10.0, 10.0);
    let root = flex_container(
        &mut tree,
        TestStyle {
            align_items: Some(AlignItems::FlexStart),
            ..TestStyle::default()
        },
        &[sticky, following],
    );

    // `Position::Sticky` is deliberately a host post-pass in the current
    // protocol. The host therefore supplies an ordinary in-flow box to
    // neutron-star, retains the authored insets, and resolves them against
    // the Flex containing block for its sticky-position pass.
    definite_layout(&mut tree, root, 100.0, 40.0);
    assert_point(tree.layout(sticky).location, Point::ZERO);
    assert_point(tree.layout(following).location, Point::new(20.0, 0.0));
    assert_eq!(
        lower_host_sticky_insets(authored_insets, Size::new(100.0, 40.0)),
        expected_lowered
    );
}

#[test]
fn flex_sticky_start_percent_insets_lower_at_the_host_boundary() {
    assert_sticky_host_boundary(
        Edges {
            left: LengthPercentageAuto::Percent(0.10),
            right: LengthPercentageAuto::Auto,
            top: LengthPercentageAuto::Percent(0.25),
            bottom: LengthPercentageAuto::Auto,
        },
        Edges {
            left: Some(10.0),
            right: None,
            top: Some(10.0),
            bottom: None,
        },
    );
}

#[test]
fn flex_sticky_end_percent_insets_lower_at_the_host_boundary() {
    assert_sticky_host_boundary(
        Edges {
            left: LengthPercentageAuto::Auto,
            right: LengthPercentageAuto::Percent(0.20),
            top: LengthPercentageAuto::Auto,
            bottom: LengthPercentageAuto::Percent(0.50),
        },
        Edges {
            left: None,
            right: Some(20.0),
            top: None,
            bottom: Some(20.0),
        },
    );
}

#[derive(Debug, Clone, Copy)]
struct HostGridPlacement {
    column_start: i32,
    column_end: i32,
    row_start: i32,
    row_end: i32,
    column_span: u16,
    row_span: u16,
}

fn flex_geometry_with_host_grid_metadata(metadata: HostGridPlacement) -> [Layout; 2] {
    let mut tree = TestTree::default();
    let placed_like_grid_item = fixed_leaf(&mut tree, 10.0, 8.0);
    let following = fixed_leaf(&mut tree, 20.0, 8.0);
    let root = flex_container(
        &mut tree,
        TestStyle {
            align_items: Some(AlignItems::FlexStart),
            ..TestStyle::default()
        },
        &[placed_like_grid_item, following],
    );

    // Grid placement is host metadata and is intentionally absent from
    // `FlexItemStyle`; keeping it outside TestStyle makes the irrelevance a
    // protocol property rather than an algorithm branch.
    let metadata_checksum = i64::from(metadata.column_start)
        + i64::from(metadata.column_end)
        + i64::from(metadata.row_start)
        + i64::from(metadata.row_end)
        + i64::from(metadata.column_span)
        + i64::from(metadata.row_span);
    assert_ne!(metadata_checksum, 0);

    definite_layout(&mut tree, root, 100.0, 20.0);
    [tree.layout(placed_like_grid_item), tree.layout(following)]
}

#[test]
fn host_grid_placement_metadata_does_not_affect_flex_items() {
    let ordinary = flex_geometry_with_host_grid_metadata(HostGridPlacement {
        column_start: 1,
        column_end: 2,
        row_start: 1,
        row_end: 2,
        column_span: 1,
        row_span: 1,
    });
    let extreme = flex_geometry_with_host_grid_metadata(HostGridPlacement {
        column_start: 99,
        column_end: 100,
        row_start: -99,
        row_end: -98,
        column_span: 3,
        row_span: 4,
    });

    assert_eq!(ordinary, extreme);
    assert_point(extreme[0].location, Point::ZERO);
    assert_point(extreme[1].location, Point::new(10.0, 0.0));
}

#[test]
fn immutable_source_style_view_survives_mutable_session_recursion() {
    let mut tree = TestTree::default();
    let child = fixed_leaf(&mut tree, 11.0, 7.0);
    let root = flex_container(
        &mut tree,
        TestStyle {
            flex_direction: FlexDirection::Row,
            align_items: Some(AlignItems::FlexStart),
            ..TestStyle::default()
        },
        &[child],
    );

    let root_style = &tree.source.nodes[usize::from(root)].style;
    let output = tree.session.compute_child_layout(
        &tree.source,
        root,
        LayoutInput::perform_layout(
            Size::new(Some(60.0), Some(20.0)),
            Size::new(Some(60.0), Some(20.0)),
            Size::new(
                AvailableSpace::Definite(60.0),
                AvailableSpace::Definite(20.0),
            ),
        ),
    );

    // Using the borrowed style after recursive mutation of the independent
    // session is the compile-time property this regression protects.
    assert_eq!(root_style.flex_direction, FlexDirection::Row);
    assert_size(output.size, Size::new(60.0, 20.0));
    assert_size(tree.layout(child).size, Size::new(11.0, 7.0));
    assert!(tree.session.layout_writes > 0);
}

#[test]
fn external_host_measurement_baseline_and_writeback_survive_split_storage() {
    let mut tree = TestTree::default();
    let child = tree.push_leaf(TestStyle::default(), Size::new(10.2, 5.2), Some(4.0));
    let root = flex_container(
        &mut tree,
        TestStyle {
            size: Size::new(Dimension::Length(80.0), Dimension::Length(40.0)),
            padding: Edges::uniform(LengthPercentage::length(2.0)),
            align_items: Some(AlignItems::FlexStart),
            ..TestStyle::default()
        },
        &[child],
    );

    let output = perform_layout(
        &mut tree,
        root,
        Size::NONE,
        Size::new(
            AvailableSpace::Definite(100.0),
            AvailableSpace::Definite(80.0),
        ),
    );

    assert_size(output.size, Size::new(84.0, 44.0));
    assert_point(tree.layout(child).location, Point::new(2.0, 2.0));
    assert_size(tree.layout(child).size, Size::new(10.2, 5.2));
    assert_eq!(tree.session_node(child).output.first_baselines.y, Some(4.0));
    assert!(tree.session.layout_writes > 0);
}

#[derive(Debug)]
struct WriteOnlySession {
    layouts: Vec<Layout>,
    layout_writes: usize,
}

impl LayoutState for WriteOnlySession {
    fn set_unrounded_layout(&mut self, node: NodeId, layout: &Layout) {
        self.layouts[usize::from(node)] = *layout;
        self.layout_writes += 1;
    }

    fn set_static_position(&mut self, _child: NodeId, _static_position: Point<f32>) {}
}

impl CacheState for WriteOnlySession {
    fn cache_get(&self, _node: NodeId, _input: LayoutInput) -> Option<LayoutOutput> {
        None
    }

    fn cache_store(&mut self, _node: NodeId, _input: LayoutInput, _output: LayoutOutput) {}

    fn cache_clear(&mut self, _node: NodeId) {}
}

impl LayoutSession<TestSource> for WriteOnlySession {
    fn compute_child_layout(
        &mut self,
        source: &TestSource,
        child: NodeId,
        input: LayoutInput,
    ) -> LayoutOutput {
        let node = &source.nodes[usize::from(child)];
        if node.style.box_generation_mode == BoxGenerationMode::None {
            hide_subtree(source, self, child);
            return LayoutOutput::HIDDEN;
        }

        match node.display {
            TestDisplay::Flex => compute_flexbox_layout(source, self, child, input),
            TestDisplay::Grid => compute_grid_layout(source, self, child, input),
            TestDisplay::Leaf => {
                let mut measurer = FnLeafMeasurer::new(|_| LeafMetrics::new(Size::<f32>::ZERO));
                compute_leaf_layout(
                    input,
                    &node.style,
                    |calc, basis| source.resolve_calc(calc, basis),
                    &mut measurer,
                )
            }
        }
    }
}

#[test]
fn flex_layout_runs_with_a_minimal_write_only_session() {
    let mut tree = TestTree::default();
    let child = fixed_leaf(&mut tree, 11.0, 7.0);
    let root = flex_container(
        &mut tree,
        TestStyle {
            align_items: Some(AlignItems::FlexStart),
            ..TestStyle::default()
        },
        &[child],
    );
    let mut session = WriteOnlySession {
        layouts: vec![Layout::default(); tree.source.nodes.len()],
        layout_writes: 0,
    };

    let output = session.compute_child_layout(
        &tree.source,
        root,
        LayoutInput::perform_layout(
            Size::new(Some(60.0), Some(20.0)),
            Size::new(Some(60.0), Some(20.0)),
            Size::new(
                AvailableSpace::Definite(60.0),
                AvailableSpace::Definite(20.0),
            ),
        ),
    );

    assert_size(output.size, Size::new(60.0, 20.0));
    assert_size(
        session.layouts[usize::from(child)].size,
        Size::new(11.0, 7.0),
    );
    assert!(session.layout_writes > 0);
}

#[test]
fn flex_factor_selection_uses_hypothetical_sizes_not_flex_base_sum() {
    let mut tree = TestTree::default();
    let mut capped_style = fixed_leaf_style(120.0, 10.0);
    capped_style.flex_grow = 1.0;
    capped_style.max_size.width = Dimension::Length(20.0);
    let capped = tree.push_leaf(capped_style, Size::new(120.0, 10.0), None);
    let mut flexible_style = fixed_leaf_style(10.0, 10.0);
    flexible_style.flex_grow = 1.0;
    flexible_style.flex_shrink = 0.0;
    let flexible = tree.push_leaf(flexible_style, Size::new(10.0, 10.0), None);
    let root = flex_container(
        &mut tree,
        TestStyle {
            align_items: Some(AlignItems::FlexStart),
            ..TestStyle::default()
        },
        &[capped, flexible],
    );

    definite_layout(&mut tree, root, 100.0, 10.0);

    // Flex bases overflow (130px), but hypothetical sizes total only 30px,
    // so §9.7 selects the grow factor. The capped item freezes at 20px and
    // the other item receives the remaining 80px.
    assert_close(tree.layout(capped).size.width, 20.0);
    assert_close(tree.layout(flexible).size.width, 80.0);
}

#[test]
fn initial_free_space_uses_frozen_targets_outer_margins_and_gap() {
    let mut tree = TestTree::default();
    let mut capped_style = fixed_leaf_style(40.0, 10.0);
    capped_style.flex_grow = 1.0;
    capped_style.max_size.width = Dimension::Length(20.0);
    capped_style.margin.left = LengthPercentageAuto::Length(5.0);
    capped_style.margin.right = LengthPercentageAuto::Length(5.0);
    let capped = tree.push_leaf(capped_style, Size::new(40.0, 10.0), None);
    let mut flexible_style = fixed_leaf_style(10.0, 10.0);
    flexible_style.flex_grow = 1.0;
    let flexible = tree.push_leaf(flexible_style, Size::new(10.0, 10.0), None);
    let root = flex_container(
        &mut tree,
        TestStyle {
            gap: Size::new(LengthPercentage::Length(10.0), LengthPercentage::ZERO),
            align_items: Some(AlignItems::FlexStart),
            ..TestStyle::default()
        },
        &[capped, flexible],
    );

    definite_layout(&mut tree, root, 100.0, 10.0);

    // 100 - capped outer target (5 + 20 + 5) - base 10 - gap 10 = 50.
    assert_close(tree.layout(capped).size.width, 20.0);
    assert_close(tree.layout(flexible).size.width, 60.0);
    assert_close(tree.layout(capped).location.x, 5.0);
    assert_close(tree.layout(flexible).location.x, 40.0);
}
