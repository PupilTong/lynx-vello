//! css-anchor-position-1 `anchor-size()` (§5.1, §5.1.1) over the shared mock
//! host: what an absolutely positioned box's containing block makes of the
//! function in each algorithm's absolute pass, and what every other box — an
//! in-flow item — makes of it.
//!
//! The mock host answers every query by name (`TestTree::anchors`); which
//! element is the target is the real host's business and is covered in
//! `dom`'s `tests/anchor_size.rs`.

#![allow(clippy::float_cmp)]

mod support;

use hughie::prelude::*;
use hughie::style::{
    AlignFlags, Inset, LengthPercentage, Margin, MaxSize, PositionProperty, StyleSize,
};
use style_traits::values::specified::AllowedNumericType;
use stylo::Atom;
use stylo::typed_om::NumericBaseType;
use stylo::values::DashedIdent;
use stylo::values::computed::Length;
use stylo::values::computed::length_percentage::{CalcNode, CalcPercentageLeaf, ComputedLeaf};
use stylo::values::generics::calc::GenericAnchorFunctionFallback;
use stylo::values::generics::length::{AnchorSizeKeyword, GenericAnchorSizeFunction};
use stylo::values::generics::position::TreeScoped;
use stylo::values::generics::{NonNegative, Optional};
use support::*;

#[allow(
    clippy::unnecessary_box_returns,
    reason = "every stylo value holding the function holds it boxed"
)]
fn function<V>(
    name: &str,
    size: AnchorSizeKeyword,
    fallback: Option<V>,
) -> Box<GenericAnchorSizeFunction<V>> {
    Box::new(GenericAnchorSizeFunction {
        target_element: TreeScoped::with_default_level(DashedIdent(Atom::from(name))),
        size,
        fallback: fallback.map_or(Optional::None, Optional::Some),
    })
}

fn size_anchor(name: &str, size: AnchorSizeKeyword, fallback: Option<f32>) -> StyleSize {
    StyleSize::AnchorSizeFunction(function(name, size, fallback.map(size_px)))
}

fn max_anchor(name: &str, size: AnchorSizeKeyword, fallback: Option<f32>) -> MaxSize {
    MaxSize::AnchorSizeFunction(function(name, size, fallback.map(max_px)))
}

fn margin_anchor(name: &str, size: AnchorSizeKeyword, fallback: Option<f32>) -> Margin {
    Margin::AnchorSizeFunction(function(name, size, fallback.map(margin_px)))
}

fn inset_anchor(name: &str, size: AnchorSizeKeyword, fallback: Option<f32>) -> Inset {
    Inset::AnchorSizeFunction(function(name, size, fallback.map(inset_px)))
}

/// `calc(<percent>% - anchor-size(<name> height[, <fallback>px]))`, clamped
/// the way a non-negative property clamps it.
fn calc_minus_anchor(percent: f32, name: &str, fallback: Option<f32>) -> LengthPercentage {
    calc_minus_anchor_clamped(percent, name, fallback, AllowedNumericType::NonNegative)
}

fn calc_minus_anchor_clamped(
    percent: f32,
    name: &str,
    fallback: Option<f32>,
    clamping: AllowedNumericType,
) -> LengthPercentage {
    let anchor = CalcNode::AnchorSize(function(
        name,
        AnchorSizeKeyword::Height,
        fallback.map(|px| {
            Box::new(GenericAnchorFunctionFallback::new(
                false,
                CalcNode::Leaf(ComputedLeaf::Length(Length::new(px))),
            ))
        }),
    ));
    LengthPercentage::new_calc(
        CalcNode::Sum(
            vec![
                CalcNode::Leaf(ComputedLeaf::Percentage(CalcPercentageLeaf::new(
                    percent / 100.0,
                    Optional::Some(NumericBaseType::Length),
                ))),
                CalcNode::Negate(Box::new(anchor)),
            ]
            .into(),
        ),
        clamping,
    )
}

fn absolute(style: TestStyle) -> TestStyle {
    TestStyle {
        position: PositionProperty::Absolute,
        ..style
    }
}

fn tree_with_anchor() -> TestTree {
    let mut tree = TestTree::default();
    tree.anchors = vec![("--a", Size::new(40.0, 30.0))];
    tree
}

fn container_style() -> TestStyle {
    TestStyle {
        size: Size::new(size_px(300.0), size_px(200.0)),
        ..TestStyle::default()
    }
}

#[test]
fn keywords_map_onto_physical_axes() {
    use hughie::compute::anchor_size_axis;
    use hughie::style::PhysicalAxis::{Horizontal, Vertical};
    for property in [Horizontal, Vertical] {
        assert_eq!(
            anchor_size_axis(AnchorSizeKeyword::None, property),
            property
        );
        for keyword in [
            AnchorSizeKeyword::Width,
            AnchorSizeKeyword::Inline,
            AnchorSizeKeyword::SelfInline,
        ] {
            assert_eq!(anchor_size_axis(keyword, property), Horizontal);
        }
        for keyword in [
            AnchorSizeKeyword::Height,
            AnchorSizeKeyword::Block,
            AnchorSizeKeyword::SelfBlock,
        ] {
            assert_eq!(anchor_size_axis(keyword, property), Vertical);
        }
    }
}

#[test]
fn a_flex_container_resolves_its_absolute_childs_sizes_insets_and_margins() {
    let mut tree = tree_with_anchor();
    let sized = tree.push_leaf(
        absolute(TestStyle {
            size: Size::new(
                size_anchor("--a", AnchorSizeKeyword::Width, None),
                size_anchor("--a", AnchorSizeKeyword::None, Some(10.0)),
            ),
            ..TestStyle::default()
        }),
        Size::ZERO,
        None,
    );
    let inset = tree.push_leaf(
        absolute(TestStyle {
            inset: Edges {
                left: inset_anchor("--a", AnchorSizeKeyword::None, None),
                right: inset_auto(),
                top: inset_anchor("--a", AnchorSizeKeyword::Width, None),
                bottom: inset_auto(),
            },
            ..fixed_leaf_style(1.0, 1.0)
        }),
        Size::ZERO,
        None,
    );
    let margin = tree.push_leaf(
        absolute(TestStyle {
            margin: Edges {
                left: margin_anchor("--a", AnchorSizeKeyword::None, None),
                right: margin_px(0.0),
                top: margin_anchor("--a", AnchorSizeKeyword::None, None),
                bottom: margin_anchor("--missing", AnchorSizeKeyword::None, Some(4.0)),
            },
            ..fixed_leaf_style(1.0, 1.0)
        }),
        Size::ZERO,
        None,
    );
    let root = flex_container(&mut tree, container_style(), &[sized, inset, margin]);
    definite_layout(&tree, root, 300.0, 200.0);

    assert_size(tree.layout(sized).size, Size::new(40.0, 30.0));
    assert_point(tree.layout(inset).location, Point::new(40.0, 40.0));
    let margin_layout = tree.layout(margin);
    assert_point(margin_layout.location, Point::new(40.0, 30.0));
    assert_eq!(
        (
            margin_layout.margin.left,
            margin_layout.margin.top,
            margin_layout.margin.bottom
        ),
        (40.0, 30.0, 4.0)
    );
}

#[test]
fn without_a_target_the_fallback_or_the_initial_value_applies() {
    let mut tree = TestTree::default();
    let fallback = tree.push_leaf(
        absolute(TestStyle {
            size: Size::new(
                size_anchor("--a", AnchorSizeKeyword::Width, Some(12.0)),
                size_anchor("", AnchorSizeKeyword::Height, Some(8.0)),
            ),
            ..TestStyle::default()
        }),
        Size::ZERO,
        None,
    );
    // No fallback: `auto`, the box's content size.
    let initial = tree.push_leaf(
        absolute(TestStyle {
            size: Size::new(
                size_anchor("--a", AnchorSizeKeyword::Width, None),
                size_px(2.0),
            ),
            max_size: Size::new(
                max_anchor("--a", AnchorSizeKeyword::Width, None),
                max_none(),
            ),
            margin: Edges::uniform(margin_anchor("--a", AnchorSizeKeyword::Width, None)),
            inset: Edges {
                left: inset_anchor("--a", AnchorSizeKeyword::Width, None),
                right: inset_px(5.0),
                top: inset_px(0.0),
                bottom: inset_auto(),
            },
            ..TestStyle::default()
        }),
        Size::new(7.0, 9.0),
        None,
    );
    let root = flex_container(&mut tree, container_style(), &[fallback, initial]);
    definite_layout(&tree, root, 300.0, 200.0);

    assert_size(tree.layout(fallback).size, Size::new(12.0, 8.0));
    let initial = tree.layout(initial);
    assert_size(initial.size, Size::new(7.0, 2.0));
    // `left: auto` against `right: 5px`, with every margin `0`.
    assert_point(initial.location, Point::new(300.0 - 5.0 - 7.0, 0.0));
}

#[test]
fn a_calc_with_an_anchor_resolves_against_the_percentage_basis() {
    let mut tree = TestTree::default();
    tree.anchors = vec![("--t", Size::new(0.0, 60.0))];
    let resolved = tree.push_leaf(
        absolute(TestStyle {
            size: Size::new(
                size_px(1.0),
                StyleSize::AnchorContainingCalcFunction(NonNegative(calc_minus_anchor(
                    100.0, "--t", None,
                ))),
            ),
            ..TestStyle::default()
        }),
        Size::ZERO,
        None,
    );
    let fallback = tree.push_leaf(
        absolute(TestStyle {
            size: Size::new(
                size_px(1.0),
                StyleSize::AnchorContainingCalcFunction(NonNegative(calc_minus_anchor(
                    50.0,
                    "--missing",
                    Some(20.0),
                ))),
            ),
            ..TestStyle::default()
        }),
        Size::ZERO,
        None,
    );
    let invalid = tree.push_leaf(
        absolute(TestStyle {
            size: Size::new(
                size_px(1.0),
                StyleSize::AnchorContainingCalcFunction(NonNegative(calc_minus_anchor(
                    50.0,
                    "--missing",
                    None,
                ))),
            ),
            max_size: Size::new(
                max_none(),
                MaxSize::AnchorContainingCalcFunction(NonNegative(calc_minus_anchor(
                    100.0, "--t", None,
                ))),
            ),
            inset: Edges {
                left: inset_px(0.0),
                right: inset_auto(),
                top: Inset::AnchorContainingCalcFunction(calc_minus_anchor_clamped(
                    10.0,
                    "--t",
                    Some(0.0),
                    AllowedNumericType::All,
                )),
                bottom: inset_auto(),
            },
            margin: Edges {
                left: Margin::AnchorContainingCalcFunction(calc_minus_anchor_clamped(
                    50.0,
                    "--missing",
                    None,
                    AllowedNumericType::All,
                )),
                ..Edges::uniform(margin_px(0.0))
            },
            ..TestStyle::default()
        }),
        Size::new(3.0, 500.0),
        None,
    );
    let root = flex_container(&mut tree, container_style(), &[resolved, fallback, invalid]);
    definite_layout(&tree, root, 300.0, 200.0);

    assert_size(tree.layout(resolved).size, Size::new(1.0, 140.0));
    assert_size(tree.layout(fallback).size, Size::new(1.0, 80.0));
    let invalid = tree.layout(invalid);
    // `height: auto` from content (500), capped by `max-height: 100% - 60px`;
    // `top: 10% - 60px` clamps at nothing — insets may be negative.
    assert_size(invalid.size, Size::new(1.0, 140.0));
    assert_point(invalid.location, Point::new(0.0, -40.0));
}

#[test]
fn content_sized_axes_with_anchored_limits_are_measured_and_clamped() {
    let mut tree = tree_with_anchor();
    let min_width = tree.push_intrinsic_leaf(
        absolute(TestStyle {
            min_size: Size::new(
                size_anchor("--a", AnchorSizeKeyword::Width, None),
                size_auto(),
            ),
            ..TestStyle::default()
        }),
        Size::new(10.0, 5.0),
        Size::new(20.0, 5.0),
    );
    let max_width = tree.push_intrinsic_leaf(
        absolute(TestStyle {
            max_size: Size::new(
                max_anchor("--a", AnchorSizeKeyword::Height, None),
                max_none(),
            ),
            margin: Edges {
                left: margin_anchor("--a", AnchorSizeKeyword::None, None),
                ..Edges::uniform(margin_px(0.0))
            },
            ..TestStyle::default()
        }),
        Size::new(10.0, 5.0),
        Size::new(100.0, 5.0),
    );
    let min_height = tree.push_intrinsic_leaf(
        absolute(TestStyle {
            min_size: Size::new(
                size_auto(),
                size_anchor("--a", AnchorSizeKeyword::None, None),
            ),
            ..TestStyle::default()
        }),
        Size::new(10.0, 5.0),
        Size::new(20.0, 5.0),
    );
    let max_height_fixed_width = tree.push_intrinsic_leaf(
        absolute(TestStyle {
            size: Size::new(size_px(15.0), size_auto()),
            max_size: Size::new(
                max_none(),
                max_anchor("--a", AnchorSizeKeyword::Height, None),
            ),
            ..TestStyle::default()
        }),
        Size::new(10.0, 50.0),
        Size::new(20.0, 50.0),
    );
    let root = flex_container(
        &mut tree,
        container_style(),
        &[min_width, max_width, min_height, max_height_fixed_width],
    );
    tree.enable_cache();
    definite_layout(&tree, root, 300.0, 200.0);

    assert_size(tree.layout(min_width).size, Size::new(40.0, 5.0));
    let max_width_layout = tree.layout(max_width);
    assert_size(max_width_layout.size, Size::new(30.0, 5.0));
    assert_eq!(max_width_layout.margin.left, 40.0);
    assert_size(tree.layout(min_height).size, Size::new(20.0, 30.0));
    assert_size(
        tree.layout(max_height_fixed_width).size,
        Size::new(15.0, 30.0),
    );

    // A size measured from the box's own content moves with that content, so
    // the committed input is not independent of it on that axis.
    let committed = tree.committed_input(min_width).expect("committed");
    assert_eq!(committed.goal.independence(), Some(Size::new(false, true)));
}

#[test]
fn an_aspect_ratio_carries_an_anchored_size_across() {
    let mut tree = tree_with_anchor();
    let child = tree.push_leaf(
        absolute(TestStyle {
            size: Size::new(
                size_anchor("--a", AnchorSizeKeyword::Width, None),
                size_auto(),
            ),
            aspect_ratio: ratio(2.0),
            ..TestStyle::default()
        }),
        Size::ZERO,
        None,
    );
    let root = flex_container(&mut tree, container_style(), &[child]);
    definite_layout(&tree, root, 300.0, 200.0);
    assert_size(tree.layout(child).size, Size::new(40.0, 20.0));
}

#[test]
fn every_algorithms_absolute_pass_resolves() {
    fn child(tree: &mut TestTree) -> TestId {
        tree.push_leaf(
            absolute(TestStyle {
                size: Size::new(
                    size_anchor("--a", AnchorSizeKeyword::Width, None),
                    size_anchor("--a", AnchorSizeKeyword::Height, None),
                ),
                inset: Edges {
                    left: inset_auto(),
                    right: inset_auto(),
                    top: inset_anchor("--a", AnchorSizeKeyword::Height, None),
                    bottom: inset_auto(),
                },
                ..TestStyle::default()
            }),
            Size::ZERO,
            None,
        )
    }
    let mut tree = tree_with_anchor();
    let in_linear = child(&mut tree);
    let linear = linear_container(&mut tree, container_style(), &[in_linear]);
    let in_grid = child(&mut tree);
    let grid = tree.push_grid(container_style(), vec![in_grid]);
    let in_lanes = child(&mut tree);
    let lanes = tree.push_grid_lanes(
        TestStyle {
            template_columns: track_list(vec![track_px(100.0)]),
            ..container_style()
        },
        vec![in_lanes],
    );
    let in_relative = child(&mut tree);
    let relative = relative_container(&mut tree, container_style(), &[in_relative]);
    for (container, child) in [
        (linear, in_linear),
        (grid, in_grid),
        (lanes, in_lanes),
        (relative, in_relative),
    ] {
        definite_layout(&tree, container, 300.0, 200.0);
        let layout = tree.layout(child);
        assert_size(layout.size, Size::new(40.0, 30.0));
        assert_eq!(layout.location.y, 30.0, "{container}");
    }
}

#[test]
fn a_changed_anchor_is_a_changed_input_not_a_stale_cache_hit() {
    let mut tree = tree_with_anchor();
    let child = tree.push_leaf(
        absolute(TestStyle {
            size: Size::new(
                size_anchor("--a", AnchorSizeKeyword::Width, None),
                size_px(1.0),
            ),
            ..TestStyle::default()
        }),
        Size::ZERO,
        None,
    );
    let root = flex_container(&mut tree, container_style(), &[child]);
    tree.enable_cache();
    definite_layout(&tree, root, 300.0, 200.0);
    assert_size(tree.layout(child).size, Size::new(40.0, 1.0));

    // What a moved anchor does to a real host: the containing block's cache
    // clears on the way up from the anchor, never the query box's own.
    tree.anchors = vec![("--a", Size::new(70.0, 30.0))];
    tree.clear_layout_cache(root);
    definite_layout(&tree, root, 300.0, 200.0);
    assert_size(tree.layout(child).size, Size::new(70.0, 1.0));
}

// Anchors exist in the tests below, but an in-flow box is not one §5.1.1
// resolves for.

#[test]
fn flex_items_take_the_fallback_or_the_initial_value() {
    let mut tree = tree_with_anchor();
    let flex_item = tree.push_leaf(
        TestStyle {
            size: Size::new(
                size_anchor("--a", AnchorSizeKeyword::Width, Some(11.0)),
                size_anchor("--a", AnchorSizeKeyword::Height, None),
            ),
            min_size: Size::new(
                size_anchor("--a", AnchorSizeKeyword::Width, None),
                size_auto(),
            ),
            max_size: Size::new(
                max_anchor("--a", AnchorSizeKeyword::Width, Some(50.0)),
                max_none(),
            ),
            flex_shrink: nn(0.0),
            ..TestStyle::default()
        },
        Size::new(3.0, 9.0),
        None,
    );
    let content_basis = tree.push_leaf(
        TestStyle {
            size: Size::new(
                size_anchor("--a", AnchorSizeKeyword::Width, None),
                size_px(1.0),
            ),
            flex_basis: basis_content(),
            flex_shrink: nn(0.0),
            ..TestStyle::default()
        },
        Size::new(6.0, 1.0),
        None,
    );
    let flex = flex_container(&mut tree, container_style(), &[flex_item, content_basis]);
    definite_layout(&tree, flex, 300.0, 200.0);
    assert_eq!(tree.layout(flex_item).size.width, 11.0);
    assert_eq!(tree.layout(content_basis).size.width, 6.0);
}

#[test]
fn a_linear_items_relative_nudge_takes_the_fallback() {
    let mut tree = tree_with_anchor();
    let nudged = tree.push_leaf(
        TestStyle {
            inset: Edges {
                left: inset_anchor("--a", AnchorSizeKeyword::Width, Some(3.0)),
                right: inset_auto(),
                top: inset_anchor("--a", AnchorSizeKeyword::Height, None),
                bottom: inset_auto(),
            },
            margin: Edges {
                left: margin_anchor("--a", AnchorSizeKeyword::Width, None),
                ..Edges::uniform(margin_px(0.0))
            },
            ..fixed_leaf_style(10.0, 10.0)
        },
        Size::new(10.0, 10.0),
        None,
    );
    let linear = linear_container(
        &mut tree,
        TestStyle {
            size: Size::new(size_auto(), size_px(200.0)),
            ..TestStyle::default()
        },
        &[nudged],
    );
    perform_layout(
        &tree,
        linear,
        Size::NONE,
        Size::new(AvailableSpace::MaxContent, AvailableSpace::Definite(200.0)),
    );
    assert_point(tree.layout(nudged).location, Point::new(3.0, 0.0));
}

#[test]
fn grid_and_relative_items_take_the_fallback_or_the_initial_value() {
    let mut tree = tree_with_anchor();
    let grid_item = tree.push_leaf(
        TestStyle {
            size: Size::new(
                size_anchor("--a", AnchorSizeKeyword::Width, Some(12.0)),
                size_px(1.0),
            ),
            min_size: Size::new(
                size_anchor("--a", AnchorSizeKeyword::Width, None),
                size_auto(),
            ),
            ..TestStyle::default()
        },
        Size::new(3.0, 1.0),
        None,
    );
    let grid = tree.push_grid(
        TestStyle {
            template_columns: track_list(vec![track_auto()]),
            justify_items: justify_items(AlignFlags::START),
            ..container_style()
        },
        vec![grid_item],
    );
    definite_layout(&tree, grid, 300.0, 200.0);
    assert_eq!(tree.layout(grid_item).size.width, 12.0);

    let relative_item = tree.push_leaf(
        TestStyle {
            size: Size::new(
                size_anchor("--a", AnchorSizeKeyword::Width, None),
                size_px(1.0),
            ),
            ..TestStyle::default()
        },
        Size::new(5.0, 1.0),
        None,
    );
    let relative = relative_container(&mut tree, container_style(), &[relative_item]);
    definite_layout(&tree, relative, 300.0, 200.0);
    assert_eq!(tree.layout(relative_item).size.width, 5.0);
}

/// The width an absolutely positioned wrapping flex box with a 150px and a
/// 250px item takes in a 300px-wide containing block, under `style`.
fn keyword_box_width(anchors: Vec<(&'static str, Size<f32>)>, style: TestStyle) -> f32 {
    let mut tree = TestTree::default();
    tree.anchors = anchors;
    let items = [150.0, 250.0].map(|width| {
        tree.push_leaf(
            TestStyle {
                flex_shrink: nn(0.0),
                ..fixed_leaf_style(width, 10.0)
            },
            Size::new(width, 10.0),
            None,
        )
    });
    let target = tree.push_flex(
        absolute(TestStyle {
            flex_wrap: stylo::computed_values::flex_wrap::T::WRAP,
            ..style
        }),
        items.to_vec(),
    );
    let root = flex_container(&mut tree, container_style(), &[target]);
    definite_layout(&tree, root, 300.0, 200.0);
    tree.layout(target).size.width
}

/// An anchored limit or margin on an axis sized by an intrinsic keyword
/// changes only the limit: the keyword still picks the size, exactly as it
/// does with the same limit written in pixels. (The items' min-content width
/// is 250 and their max-content width 400; the wrapping box's own run sizes
/// `fit-content` in 260px at its widest line, 250.)
#[test]
fn an_anchored_axis_keeps_its_intrinsic_sizing_keyword() {
    let anchor = |width| vec![("--a", Size::new(width, 30.0))];
    let cases = [
        (
            "width: min-content; max-width: 300px",
            250.0,
            anchor(300.0),
            TestStyle {
                size: Size::new(size_min_content(), size_auto()),
                max_size: Size::new(
                    max_anchor("--a", AnchorSizeKeyword::Width, None),
                    max_none(),
                ),
                ..TestStyle::default()
            },
            TestStyle {
                size: Size::new(size_min_content(), size_auto()),
                max_size: Size::new(max_px(300.0), max_none()),
                ..TestStyle::default()
            },
        ),
        (
            "width: max-content; min-width: 40px",
            400.0,
            anchor(40.0),
            TestStyle {
                size: Size::new(size_max_content(), size_auto()),
                min_size: Size::new(
                    size_anchor("--a", AnchorSizeKeyword::Width, None),
                    size_auto(),
                ),
                ..TestStyle::default()
            },
            TestStyle {
                size: Size::new(size_max_content(), size_auto()),
                min_size: Size::new(size_px(40.0), size_auto()),
                ..TestStyle::default()
            },
        ),
        (
            "width: fit-content; margin-left: 40px",
            250.0,
            anchor(40.0),
            TestStyle {
                size: Size::new(StyleSize::FitContent, size_auto()),
                margin: Edges {
                    left: margin_anchor("--a", AnchorSizeKeyword::Width, None),
                    ..Edges::uniform(margin_px(0.0))
                },
                ..TestStyle::default()
            },
            TestStyle {
                size: Size::new(StyleSize::FitContent, size_auto()),
                margin: Edges {
                    left: margin_px(40.0),
                    ..Edges::uniform(margin_px(0.0))
                },
                ..TestStyle::default()
            },
        ),
    ];
    for (case, expected, anchors, anchored, plain) in cases {
        assert_eq!(keyword_box_width(Vec::new(), plain), expected, "{case}");
        assert_eq!(keyword_box_width(anchors, anchored), expected, "{case}");
    }
}
