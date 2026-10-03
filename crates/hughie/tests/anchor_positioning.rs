//! css-anchor-position-1 in the absolute pass, over the shared mock host:
//! `anchor()` (§3.2), `position-area` (§3.1, §4.1), `anchor-center` (§4.2),
//! the self-alignment of absolutely positioned boxes it builds on
//! (css-position-3 §4.3, css-align-3 §4.4.1.2), position fallback (§6.5,
//! §6.2) and the outcome the host keeps (§3.3, §6.6).
//!
//! The mock host answers anchors by name with fixed rectangles
//! (`TestTree::anchor_rects`, in the containing block's padding-box
//! coordinates), gives every box the same default anchor
//! (`TestTree::default_anchor`) and hands out hand-built option styles
//! (`TestTree::position_options`); which element is a target, and how
//! `@position-try` cascades, is the real host's business. Where a test ports
//! a WPT case it names the file and uses its numbers.

#![allow(clippy::float_cmp)]

mod support;

use hughie::geometry::Rect;
use hughie::prelude::*;
use hughie::style::{
    AlignFlags, Inset, LengthPercentage, PositionArea, PositionProperty, PositionTryOrder,
    SelfAlignment, StyleSize, direction,
};
use hughie::tree::AnchorOutcome;
use style_traits::values::specified::AllowedNumericType;
use stylo::Atom;
use stylo::typed_om::NumericBaseType;
use stylo::values::DashedIdent;
use stylo::values::computed::length_percentage::{CalcNode, CalcPercentageLeaf, ComputedLeaf};
use stylo::values::computed::{Length, Percentage, PositionAreaKeyword as K};
use stylo::values::generics::Optional;
use stylo::values::generics::calc::GenericAnchorFunctionFallback;
use stylo::values::generics::length::{AnchorSizeKeyword, GenericAnchorSizeFunction};
use stylo::values::generics::position::{
    AnchorSideKeyword as Side, GenericAnchorFunction, GenericAnchorSide, TreeScoped,
};
use support::*;

// ---------------------------------------------------------------------------
// Value builders.

fn scoped(name: &str) -> TreeScoped<DashedIdent> {
    TreeScoped::with_default_level(DashedIdent(Atom::from(name)))
}

/// `anchor(<name> <side>[, <fallback>px])`; an empty name is the default
/// anchor.
fn anchor(name: &str, side: Side, fallback: Option<f32>) -> Inset {
    Inset::AnchorFunction(Box::new(GenericAnchorFunction {
        target_element: scoped(name),
        side: GenericAnchorSide::Keyword(side),
        fallback: fallback.map_or(Optional::None, |px| Optional::Some(inset_px(px))),
    }))
}

/// `anchor(<name> <percent>%)`.
fn anchor_percent(name: &str, percent: f32) -> Inset {
    Inset::AnchorFunction(Box::new(GenericAnchorFunction {
        target_element: scoped(name),
        side: GenericAnchorSide::Percentage(Percentage(percent / 100.0)),
        fallback: Optional::None,
    }))
}

/// `calc(anchor(<name> <side>) + <px>px)`, or with a percentage side.
fn anchor_calc(name: &str, side: GenericAnchorSide<Box<CalcNode>>, plus: f32) -> Inset {
    let anchor = CalcNode::Anchor(Box::new(GenericAnchorFunction {
        target_element: scoped(name),
        side,
        fallback: Optional::None,
    }));
    Inset::AnchorContainingCalcFunction(LengthPercentage::new_calc(
        CalcNode::Sum(
            vec![
                anchor,
                CalcNode::Leaf(ComputedLeaf::Length(Length::new(plus))),
            ]
            .into(),
        ),
        AllowedNumericType::All,
    ))
}

/// `calc(anchor(<name> <side>, <fallback>px) + 0px)`.
fn anchor_calc_with_fallback(name: &str, side: Side, fallback: f32) -> Inset {
    let anchor = CalcNode::Anchor(Box::new(GenericAnchorFunction {
        target_element: scoped(name),
        side: GenericAnchorSide::Keyword(side),
        fallback: Optional::Some(Box::new(GenericAnchorFunctionFallback::new(
            false,
            CalcNode::Leaf(ComputedLeaf::Length(Length::new(fallback))),
        ))),
    }));
    Inset::AnchorContainingCalcFunction(LengthPercentage::new_calc(
        CalcNode::Sum(
            vec![
                anchor,
                CalcNode::Leaf(ComputedLeaf::Length(Length::new(0.0))),
            ]
            .into(),
        ),
        AllowedNumericType::All,
    ))
}

fn area(first: K, second: K) -> PositionArea {
    PositionArea { first, second }
}

fn align(flags: AlignFlags) -> SelfAlignment {
    SelfAlignment(flags)
}

fn rect(x: f32, y: f32, width: f32, height: f32) -> Rect<f32> {
    Rect::new(Point::new(x, y), Size::new(width, height))
}

fn edges<T>(top: T, right: T, bottom: T, left: T) -> Edges<T> {
    Edges {
        left,
        right,
        top,
        bottom,
    }
}

fn inset(top: f32, right: f32, bottom: f32, left: f32) -> Edges<Inset> {
    edges(
        inset_px(top),
        inset_px(right),
        inset_px(bottom),
        inset_px(left),
    )
}

// ---------------------------------------------------------------------------
// Harness: one containing block (a flex container, which establishes it)
// with one absolutely positioned leaf.

fn absolute(width: f32, height: f32) -> TestStyle {
    TestStyle {
        position: PositionProperty::Absolute,
        size: Size::new(size_px(width), size_px(height)),
        ..TestStyle::default()
    }
}

fn container(width: f32, height: f32) -> TestStyle {
    TestStyle {
        size: Size::new(size_px(width), size_px(height)),
        ..TestStyle::default()
    }
}

/// Lays out `target` (a leaf with `content` as its min- and max-content
/// size) in a `cb`-sized containing block and returns its box.
fn place_in(tree: &mut TestTree, cb: TestStyle, target: TestStyle, content: Size<f32>) -> TestId {
    let target = tree.push_leaf(target, content, None);
    let width = match cb.size.width {
        StyleSize::LengthPercentage(ref lp) => lp.0.to_length().map_or(0.0, Length::px),
        _ => 0.0,
    };
    let height = match cb.size.height {
        StyleSize::LengthPercentage(ref lp) => lp.0.to_length().map_or(0.0, Length::px),
        _ => 0.0,
    };
    let root = tree.push_flex(cb, vec![target]);
    definite_layout(tree, root, width, height);
    target
}

fn place(tree: &mut TestTree, cb: Size<f32>, target: TestStyle) -> Layout {
    let id = place_in(
        tree,
        container(cb.width, cb.height),
        target,
        Size::new(10.0, 10.0),
    );
    tree.layout(id)
}

fn assert_box(layout: &Layout, expected: (f32, f32, f32, f32), case: &str) {
    let actual = (
        layout.location.x,
        layout.location.y,
        layout.size.width,
        layout.size.height,
    );
    let close = |a: f32, b: f32| (a - b).abs() <= 0.01;
    assert!(
        close(actual.0, expected.0)
            && close(actual.1, expected.1)
            && close(actual.2, expected.2)
            && close(actual.3, expected.3),
        "{case}: expected (x, y, w, h) = {expected:?}, got {actual:?}"
    );
}

fn with_default_anchor(name: &'static str, anchor_rect: Rect<f32>) -> TestTree {
    let mut tree = TestTree::default();
    tree.anchor_rects = vec![(name, anchor_rect)];
    tree.default_anchor = Some(name);
    tree
}

fn outcome(tree: &TestTree, id: TestId) -> AnchorOutcome {
    *tree
        .anchor_outcomes(id)
        .last()
        .expect("an anchor-positioned box reports an outcome")
}

// ---------------------------------------------------------------------------
// §3.1 position-area and §4.1 area-specific default alignment.

/// WPT `position-area-align-justify.html`: a 300px containing block, a 100px
/// anchor at (100, 100), a 10px box with `inset: 10px 15px 20px 25px`.
#[test]
fn position_area_aligns_toward_the_anchor_wpt_align_justify() {
    let cases = [
        ("span-all", area(K::SpanAll, K::None), 145.0, 145.0),
        ("top left", area(K::Top, K::Left), 75.0, 70.0),
        ("top center", area(K::Top, K::Center), 150.0, 70.0),
        ("top right", area(K::Top, K::Right), 225.0, 70.0),
        ("center left", area(K::Center, K::Left), 75.0, 140.0),
        ("center center", area(K::Center, K::Center), 150.0, 140.0),
        ("center right", area(K::Center, K::Right), 225.0, 140.0),
        ("bottom left", area(K::Bottom, K::Left), 75.0, 210.0),
        ("bottom center", area(K::Bottom, K::Center), 150.0, 210.0),
        ("bottom right", area(K::Bottom, K::Right), 225.0, 210.0),
        ("top span-left", area(K::Top, K::SpanLeft), 175.0, 70.0),
        ("top span-right", area(K::Top, K::SpanRight), 125.0, 70.0),
        ("span-top left", area(K::SpanTop, K::Left), 75.0, 170.0),
        (
            "span-bottom left",
            area(K::SpanBottom, K::Left),
            75.0,
            110.0,
        ),
    ];
    for (case, position_area, x, y) in cases {
        let mut tree = with_default_anchor("--anchor", rect(100.0, 100.0, 100.0, 100.0));
        let layout = place(
            &mut tree,
            Size::new(300.0, 300.0),
            TestStyle {
                inset: inset(10.0, 15.0, 20.0, 25.0),
                position_area,
                ..absolute(10.0, 10.0)
            },
        );
        assert_box(&layout, (x, y, 10.0, 10.0), case);
    }
}

/// WPT `position-area-basic.html`: `align-self`/`justify-self: stretch`
/// fill the selected region of a 400px containing block around a
/// 150×75 anchor at (100, 150); logical keywords resolve through
/// `horizontal-tb ltr`.
#[test]
fn position_area_regions_wpt_basic() {
    let cases = [
        (
            "span-all",
            area(K::SpanAll, K::None),
            (0.0, 0.0, 400.0, 400.0),
        ),
        ("top left", area(K::Top, K::Left), (0.0, 0.0, 100.0, 150.0)),
        (
            "top center",
            area(K::Top, K::Center),
            (100.0, 0.0, 150.0, 150.0),
        ),
        (
            "center center",
            area(K::Center, K::Center),
            (100.0, 150.0, 150.0, 75.0),
        ),
        (
            "bottom right",
            area(K::Bottom, K::Right),
            (250.0, 225.0, 150.0, 175.0),
        ),
        (
            "start end",
            area(K::Start, K::End),
            (250.0, 0.0, 150.0, 150.0),
        ),
        (
            "end start",
            area(K::End, K::Start),
            (0.0, 225.0, 100.0, 175.0),
        ),
        (
            "center start",
            area(K::Center, K::Start),
            (0.0, 150.0, 100.0, 75.0),
        ),
        (
            "self-end self-end",
            area(K::SelfEnd, K::SelfEnd),
            (250.0, 225.0, 150.0, 175.0),
        ),
        (
            "y-start x-start",
            area(K::YStart, K::XStart),
            (0.0, 0.0, 100.0, 150.0),
        ),
        (
            "block-end inline-start",
            area(K::BlockEnd, K::InlineStart),
            (0.0, 225.0, 100.0, 175.0),
        ),
        (
            "top (span-all)",
            area(K::Top, K::None),
            (0.0, 0.0, 400.0, 150.0),
        ),
        (
            "left (span-all)",
            area(K::Left, K::None),
            (0.0, 0.0, 100.0, 400.0),
        ),
        (
            "span-bottom span-right",
            area(K::SpanBottom, K::SpanRight),
            (100.0, 150.0, 300.0, 250.0),
        ),
    ];
    for (case, position_area, expected) in cases {
        let mut tree = with_default_anchor("--anchor", rect(100.0, 150.0, 150.0, 75.0));
        let layout = place(
            &mut tree,
            Size::new(400.0, 400.0),
            TestStyle {
                position: PositionProperty::Absolute,
                align_self: align(AlignFlags::STRETCH),
                justify_self: align(AlignFlags::STRETCH),
                position_area,
                ..TestStyle::default()
            },
        );
        assert_box(&layout, expected, case);
    }
}

#[test]
fn position_area_logical_keywords_follow_the_containing_block_direction() {
    // `start end` in an rtl containing block: inline-end is the left.
    let mut tree = with_default_anchor("--anchor", rect(100.0, 150.0, 150.0, 75.0));
    let rtl = TestStyle {
        direction: direction::T::Rtl,
        ..container(400.0, 400.0)
    };
    let id = place_in(
        &mut tree,
        rtl,
        TestStyle {
            position: PositionProperty::Absolute,
            direction: direction::T::Rtl,
            align_self: align(AlignFlags::STRETCH),
            justify_self: align(AlignFlags::STRETCH),
            position_area: area(K::Start, K::End),
            ..TestStyle::default()
        },
        Size::ZERO,
    );
    assert_box(&tree.layout(id), (0.0, 0.0, 100.0, 150.0), "rtl start end");
}

/// §3.1.1: lines 1 and 4 extend to an anchor outside the containing block,
/// so a track on that side can be empty.
#[test]
fn position_area_grid_extends_to_an_anchor_outside_the_containing_block() {
    let stretch = |position_area| TestStyle {
        position: PositionProperty::Absolute,
        align_self: align(AlignFlags::STRETCH),
        justify_self: align(AlignFlags::STRETCH),
        position_area,
        ..TestStyle::default()
    };
    let anchor_rect = rect(-50.0, 20.0, 40.0, 40.0);
    let cases = [
        ("left", area(K::Left, K::None), (-50.0, 0.0, 0.0, 300.0)),
        ("center", area(K::Center, K::Top), (-50.0, 0.0, 40.0, 20.0)),
        ("right", area(K::Right, K::None), (-10.0, 0.0, 310.0, 300.0)),
        (
            "span-all",
            area(K::SpanAll, K::None),
            (-50.0, 0.0, 350.0, 300.0),
        ),
    ];
    for (case, position_area, expected) in cases {
        let mut tree = with_default_anchor("--a", anchor_rect);
        let layout = place(&mut tree, Size::new(300.0, 300.0), stretch(position_area));
        assert_box(&layout, expected, case);
    }
}

#[test]
fn position_area_without_a_default_anchor_has_no_effect() {
    let mut tree = TestTree::default();
    tree.anchor_rects = vec![("--a", rect(100.0, 100.0, 50.0, 50.0))];
    let target = TestStyle {
        inset: edges(inset_px(5.0), inset_auto(), inset_auto(), inset_px(7.0)),
        position_area: area(K::Bottom, K::Right),
        ..absolute(10.0, 10.0)
    };
    let id = place_in(
        &mut tree,
        container(300.0, 300.0),
        target,
        Size::new(10.0, 10.0),
    );
    assert_box(
        &tree.layout(id),
        (7.0, 5.0, 10.0, 10.0),
        "no default anchor",
    );
    let reported = outcome(&tree, id);
    assert!(reported.references_default_anchor && !reported.default_anchor_resolved);
    assert_eq!(reported.compensates, Size::new(false, false));
}

/// WPT `position-area-inset-one-side-auto.tentative.html`: with one `auto`
/// inset on an axis, §4.1 aligns unsafely toward the non-`auto` one.
#[test]
fn one_auto_inset_aligns_unsafely_toward_the_other_wpt() {
    let mut tree = with_default_anchor("--a", rect(100.0, 100.0, 100.0, 100.0));
    let layout = place(
        &mut tree,
        Size::new(200.0, 200.0),
        TestStyle {
            inset: edges(inset_pct(1.0), inset_auto(), inset_auto(), inset_px(100.0)),
            position_area: area(K::Top, K::Left),
            ..absolute(50.0, 50.0)
        },
    );
    assert_box(
        &layout,
        (100.0, 100.0, 50.0, 50.0),
        "top left, one-sided insets",
    );

    // The mirrored single inset aligns toward the end edges.
    let mut tree = with_default_anchor("--a", rect(100.0, 100.0, 100.0, 100.0));
    let layout = place(
        &mut tree,
        Size::new(200.0, 200.0),
        TestStyle {
            inset: edges(inset_auto(), inset_px(10.0), inset_px(20.0), inset_auto()),
            position_area: area(K::Bottom, K::Right),
            ..absolute(50.0, 50.0)
        },
    );
    // Region: x [200, 200], y [200, 200]; right 10 and bottom 20 move the
    // end edges to 190 and 180, and the IMCB is corrected to zero there.
    assert_box(
        &layout,
        (140.0, 130.0, 50.0, 50.0),
        "bottom right, one-sided insets",
    );
}

/// WPT `auto-margins-position-area.html` against its reference: with
/// `inset: 0; margin: auto 0 0 auto`, `position-area` (and `anchor-center`
/// on its own axis) makes `auto` margins 0.
#[test]
fn position_area_and_anchor_center_zero_auto_margins_wpt() {
    let anchor_rect = rect(100.0, 100.0, 152.0, 77.0);
    let cases = [
        ("none", PositionArea::none(), None, None, (378.0, 378.0)),
        (
            "span-all",
            area(K::SpanAll, K::None),
            None,
            None,
            (165.0, 127.5),
        ),
        (
            "top center",
            area(K::Top, K::Center),
            None,
            None,
            (165.0, 78.0),
        ),
        (
            "left center",
            area(K::Left, K::Center),
            None,
            None,
            (78.0, 127.5),
        ),
        (
            "bottom right",
            area(K::Bottom, K::Right),
            None,
            None,
            (252.0, 177.0),
        ),
        (
            "span-right",
            area(K::SpanRight, K::None),
            None,
            None,
            (100.0, 127.5),
        ),
        (
            "span-bottom",
            area(K::SpanBottom, K::None),
            None,
            None,
            (165.0, 100.0),
        ),
        ("right", area(K::Right, K::None), None, None, (252.0, 127.5)),
        (
            "bottom",
            area(K::Bottom, K::None),
            None,
            None,
            (165.0, 177.0),
        ),
        (
            "align-self: anchor-center",
            PositionArea::none(),
            None,
            Some(AlignFlags::ANCHOR_CENTER),
            (378.0, 127.5),
        ),
        (
            "justify-self: anchor-center",
            PositionArea::none(),
            Some(AlignFlags::ANCHOR_CENTER),
            None,
            (165.0, 378.0),
        ),
    ];
    for (case, position_area, justify, align_flags, (x, y)) in cases {
        let mut tree = with_default_anchor("--anchor", anchor_rect);
        let layout = place(
            &mut tree,
            Size::new(400.0, 400.0),
            TestStyle {
                inset: inset(0.0, 0.0, 0.0, 0.0),
                margin: edges(margin_auto(), margin_px(0.0), margin_px(0.0), margin_auto()),
                position_area,
                justify_self: justify.map_or(SelfAlignment::auto(), align),
                align_self: align_flags.map_or(SelfAlignment::auto(), align),
                ..absolute(22.0, 22.0)
            },
        );
        assert_box(&layout, (x, y, 22.0, 22.0), case);
    }
}

/// Percentages in the box's sizes resolve against the region (§3.1's note:
/// "max-height: 100% will be relative to the position-area").
#[test]
fn percentages_resolve_against_the_position_area() {
    let mut tree = with_default_anchor("--a", rect(100.0, 100.0, 100.0, 100.0));
    let layout = place(
        &mut tree,
        Size::new(400.0, 300.0),
        TestStyle {
            position: PositionProperty::Absolute,
            size: Size::new(size_pct(0.5), size_pct(0.5)),
            position_area: area(K::Bottom, K::Right),
            ..TestStyle::default()
        },
    );
    // Region x [200, 400], y [200, 300]; §4.1 aligns toward the anchor.
    assert_box(&layout, (200.0, 200.0, 100.0, 50.0), "50% of the region");
}

/// A `position-area` region is laid out against even when an explicit
/// alignment replaces §4.1's default.
#[test]
fn explicit_alignment_overrides_the_area_default() {
    let mut tree = with_default_anchor("--a", rect(100.0, 100.0, 100.0, 100.0));
    let layout = place(
        &mut tree,
        Size::new(300.0, 300.0),
        TestStyle {
            position_area: area(K::Top, K::Left),
            justify_self: align(AlignFlags::START),
            align_self: align(AlignFlags::CENTER),
            ..absolute(10.0, 10.0)
        },
    );
    assert_box(
        &layout,
        (0.0, 45.0, 10.0, 10.0),
        "start / center in top left",
    );
}

/// css-position-4's scrollable containing block replaces the local one for a
/// box with a default anchor.
#[test]
fn a_scrollable_containing_block_extends_the_position_area_grid() {
    let mut tree = with_default_anchor("--a", rect(100.0, 100.0, 50.0, 50.0));
    tree.scrollable_containing_block = Some(Size::new(600.0, 250.0));
    let layout = place(
        &mut tree,
        Size::new(300.0, 300.0),
        TestStyle {
            position: PositionProperty::Absolute,
            align_self: align(AlignFlags::STRETCH),
            justify_self: align(AlignFlags::STRETCH),
            position_area: area(K::Right, K::None),
            ..TestStyle::default()
        },
    );
    // Never smaller than the padding box: 600 × 300.
    assert_box(&layout, (150.0, 0.0, 450.0, 300.0), "right of the anchor");
}

// ---------------------------------------------------------------------------
// §3.2 anchor().

/// WPT `anchor-inside-outside.html`: a 50px anchor at (150, 250) in a 400px
/// containing block, 10px boxes.
#[test]
fn inside_and_outside_pick_the_side_relative_to_the_inset_wpt() {
    let cases = [
        (
            "left: anchor(inside)",
            PhysicalEdge::Left,
            Side::Inside,
            (150.0, 0.0),
        ),
        (
            "left: anchor(outside)",
            PhysicalEdge::Left,
            Side::Outside,
            (200.0, 0.0),
        ),
        (
            "right: anchor(inside)",
            PhysicalEdge::Right,
            Side::Inside,
            (190.0, 0.0),
        ),
        (
            "right: anchor(outside)",
            PhysicalEdge::Right,
            Side::Outside,
            (140.0, 0.0),
        ),
        (
            "top: anchor(inside)",
            PhysicalEdge::Top,
            Side::Inside,
            (0.0, 250.0),
        ),
        (
            "top: anchor(outside)",
            PhysicalEdge::Top,
            Side::Outside,
            (0.0, 300.0),
        ),
        (
            "bottom: anchor(inside)",
            PhysicalEdge::Bottom,
            Side::Inside,
            (0.0, 290.0),
        ),
        (
            "bottom: anchor(outside)",
            PhysicalEdge::Bottom,
            Side::Outside,
            (0.0, 240.0),
        ),
    ];
    for (case, edge, side, (x, y)) in cases {
        let mut tree = with_default_anchor("--a", rect(150.0, 250.0, 50.0, 50.0));
        let mut style = TestStyle {
            inset: edges(inset_auto(), inset_auto(), inset_auto(), inset_auto()),
            ..absolute(10.0, 10.0)
        };
        *edge.of(&mut style.inset) = anchor("", side, None);
        let layout = place(&mut tree, Size::new(400.0, 400.0), style);
        assert_box(&layout, (x, y, 10.0, 10.0), case);
    }
}

#[derive(Clone, Copy)]
enum PhysicalEdge {
    Left,
    Right,
    Top,
    Bottom,
}

impl PhysicalEdge {
    fn of<T>(self, edges: &mut Edges<T>) -> &mut T {
        match self {
            Self::Left => &mut edges.left,
            Self::Right => &mut edges.right,
            Self::Top => &mut edges.top,
            Self::Bottom => &mut edges.bottom,
        }
    }
}

/// WPT `anchor-position-001.html`: four insets on two named anchors size the
/// box between them.
#[test]
fn insets_on_two_named_anchors_size_the_box_wpt_001() {
    let mut tree = TestTree::default();
    tree.anchor_rects = vec![
        ("--a1", rect(100.0, 0.0, 100.0, 100.0)),
        ("--a2", rect(500.0, 200.0, 100.0, 100.0)),
    ];
    let layout = place(
        &mut tree,
        Size::new(800.0, 300.0),
        TestStyle {
            position: PositionProperty::Absolute,
            inset: edges(
                anchor("--a1", Side::Bottom, None),
                anchor("--a2", Side::Left, None),
                anchor("--a2", Side::Top, None),
                anchor("--a1", Side::Right, None),
            ),
            ..TestStyle::default()
        },
    );
    assert_box(&layout, (200.0, 100.0, 300.0, 100.0), "between the anchors");
}

#[test]
fn start_end_self_and_percentages_resolve_by_direction() {
    let anchor_rect = rect(100.0, 40.0, 80.0, 20.0);
    // (case, containing block rtl, box rtl, side, expected x)
    let cases = [
        (
            "start, ltr",
            false,
            false,
            AnchorEdge::Keyword(Side::Start),
            100.0,
        ),
        (
            "end, ltr",
            false,
            false,
            AnchorEdge::Keyword(Side::End),
            180.0,
        ),
        (
            "start, rtl cb",
            true,
            true,
            AnchorEdge::Keyword(Side::Start),
            180.0,
        ),
        (
            "end, rtl cb",
            true,
            true,
            AnchorEdge::Keyword(Side::End),
            100.0,
        ),
        (
            "self-start, rtl box in ltr cb",
            false,
            true,
            AnchorEdge::Keyword(Side::SelfStart),
            180.0,
        ),
        (
            "start, rtl box in ltr cb",
            false,
            true,
            AnchorEdge::Keyword(Side::Start),
            100.0,
        ),
        (
            "self-end, ltr",
            false,
            false,
            AnchorEdge::Keyword(Side::SelfEnd),
            180.0,
        ),
        (
            "center",
            false,
            false,
            AnchorEdge::Keyword(Side::Center),
            140.0,
        ),
        ("25%", false, false, AnchorEdge::Percent(25.0), 120.0),
        ("25%, rtl cb", true, true, AnchorEdge::Percent(25.0), 160.0),
    ];
    for (case, cb_rtl, box_rtl, side, x) in cases {
        let mut tree = TestTree::default();
        tree.anchor_rects = vec![("--a", anchor_rect)];
        let rtl = |flag| {
            if flag {
                direction::T::Rtl
            } else {
                direction::T::Ltr
            }
        };
        let left = match side {
            AnchorEdge::Keyword(keyword) => anchor("--a", keyword, None),
            AnchorEdge::Percent(percent) => anchor_percent("--a", percent),
        };
        let id = place_in(
            &mut tree,
            TestStyle {
                direction: rtl(cb_rtl),
                ..container(300.0, 100.0)
            },
            TestStyle {
                direction: rtl(box_rtl),
                inset: edges(inset_px(0.0), inset_auto(), inset_auto(), left),
                ..absolute(10.0, 10.0)
            },
            Size::new(10.0, 10.0),
        );
        assert_box(&tree.layout(id), (x, 0.0, 10.0, 10.0), case);
    }
}

#[derive(Clone, Copy)]
enum AnchorEdge {
    Keyword(Side),
    Percent(f32),
}

/// §3.2.1: a physical keyword resolves only in an inset on its own axis;
/// otherwise the fallback, and without one the inset is `auto`.
#[test]
fn physical_sides_resolve_only_on_the_matching_axis() {
    let mut tree = TestTree::default();
    tree.anchor_rects = vec![("--a", rect(100.0, 40.0, 80.0, 20.0))];
    let layout = place(
        &mut tree,
        Size::new(300.0, 100.0),
        TestStyle {
            inset: edges(
                anchor("--a", Side::Left, Some(7.0)),
                inset_auto(),
                inset_auto(),
                anchor("--a", Side::Top, None),
            ),
            ..absolute(10.0, 10.0)
        },
    );
    // top takes its fallback; left becomes `auto`, so the static position.
    assert_box(&layout, (0.0, 7.0, 10.0, 10.0), "mismatched axes");
}

#[test]
fn a_missing_anchor_takes_the_fallback_else_the_initial_value() {
    let mut tree = TestTree::default();
    let layout = place(
        &mut tree,
        Size::new(300.0, 100.0),
        TestStyle {
            inset: edges(
                anchor("--missing", Side::Bottom, Some(12.0)),
                inset_auto(),
                inset_auto(),
                anchor("--missing", Side::Right, None),
            ),
            ..absolute(10.0, 10.0)
        },
    );
    assert_box(&layout, (0.0, 12.0, 10.0, 10.0), "fallback and initial");
}

#[test]
fn anchor_resolves_inside_math_functions() {
    let mut tree = TestTree::default();
    tree.anchor_rects = vec![("--a", rect(100.0, 40.0, 80.0, 20.0))];
    let quarter = CalcNode::Leaf(ComputedLeaf::Percentage(CalcPercentageLeaf::new(
        0.25,
        Optional::Some(NumericBaseType::Percent),
    )));
    let layout = place(
        &mut tree,
        Size::new(300.0, 100.0),
        TestStyle {
            inset: edges(
                anchor_calc("--a", GenericAnchorSide::Keyword(Side::Bottom), 5.0),
                inset_auto(),
                inset_auto(),
                anchor_calc("--a", GenericAnchorSide::Percentage(Box::new(quarter)), 1.0),
            ),
            ..absolute(10.0, 10.0)
        },
    );
    assert_box(&layout, (121.0, 65.0, 10.0, 10.0), "calc(anchor() + px)");

    // An unresolvable one inside calc() takes its fallback there.
    let layout = place(
        &mut tree,
        Size::new(300.0, 100.0),
        TestStyle {
            inset: edges(
                anchor_calc_with_fallback("--missing", Side::Top, 9.0),
                inset_auto(),
                inset_auto(),
                inset_auto(),
            ),
            ..absolute(10.0, 10.0)
        },
    );
    assert_box(&layout, (0.0, 9.0, 10.0, 10.0), "calc fallback");
}

/// An `anchor()` on a box that is not absolutely positioned is
/// unresolvable: a relatively positioned box takes the fallback.
#[test]
fn anchor_on_an_in_flow_box_takes_its_fallback() {
    let mut tree = TestTree::default();
    tree.anchor_rects = vec![("--a", rect(100.0, 40.0, 80.0, 20.0))];
    let item = tree.push_leaf(
        TestStyle {
            inset: edges(
                inset_auto(),
                inset_auto(),
                inset_auto(),
                anchor("--a", Side::Right, Some(4.0)),
            ),
            ..fixed_leaf_style(10.0, 10.0)
        },
        Size::new(10.0, 10.0),
        None,
    );
    let root = tree.push_flex(container(300.0, 100.0), vec![item]);
    definite_layout(&tree, root, 300.0, 100.0);
    assert_box(
        &tree.layout(item),
        (4.0, 0.0, 10.0, 10.0),
        "relative offset",
    );
}

#[test]
fn anchor_size_without_a_name_reads_the_default_anchor() {
    let mut tree = with_default_anchor("--a", rect(10.0, 10.0, 70.0, 30.0));
    let function = Box::new(GenericAnchorSizeFunction {
        target_element: scoped(""),
        size: AnchorSizeKeyword::None,
        fallback: Optional::None,
    });
    let layout = place(
        &mut tree,
        Size::new(300.0, 100.0),
        TestStyle {
            position: PositionProperty::Absolute,
            inset: edges(inset_px(0.0), inset_auto(), inset_auto(), inset_px(0.0)),
            size: Size::new(StyleSize::AnchorSizeFunction(function), size_px(10.0)),
            ..TestStyle::default()
        },
    );
    assert_box(&layout, (0.0, 0.0, 70.0, 10.0), "anchor-size(width)");
}

// ---------------------------------------------------------------------------
// §4.2 anchor-center.

/// (case, insets, max-width, expected width, expected x)
type CenterCase = (&'static str, Edges<Inset>, Option<f32>, f32, f32);

/// WPT `anchor-center-htb-htb.html`: a 100px containing block, a 50px anchor
/// at (40, 40), a box whose content wraps to whatever width it gets, with
/// `justify-self: anchor-center` and various insets.
#[test]
fn anchor_center_sizes_in_the_imcb_and_shifts_to_stay_inside_wpt() {
    let cases: [CenterCase; 10] = [
        (
            "no insets",
            edges(inset_auto(), inset_auto(), inset_auto(), inset_auto()),
            None,
            100.0,
            0.0,
        ),
        (
            "max-width 60",
            edges(inset_auto(), inset_auto(), inset_auto(), inset_auto()),
            Some(60.0),
            60.0,
            35.0,
        ),
        (
            "left 20",
            edges(inset_auto(), inset_auto(), inset_auto(), inset_px(20.0)),
            None,
            80.0,
            20.0,
        ),
        (
            "right 20",
            edges(inset_auto(), inset_px(20.0), inset_auto(), inset_auto()),
            None,
            80.0,
            0.0,
        ),
        (
            "right -20",
            edges(inset_auto(), inset_px(-20.0), inset_auto(), inset_auto()),
            None,
            120.0,
            0.0,
        ),
        (
            "max-width 100, right -20",
            edges(inset_auto(), inset_px(-20.0), inset_auto(), inset_auto()),
            Some(100.0),
            100.0,
            15.0,
        ),
        (
            "right -50",
            edges(inset_auto(), inset_px(-50.0), inset_auto(), inset_auto()),
            None,
            150.0,
            0.0,
        ),
        (
            "left 10 right 20",
            edges(inset_auto(), inset_px(20.0), inset_auto(), inset_px(10.0)),
            None,
            70.0,
            10.0,
        ),
        (
            "left 10 right -20",
            edges(inset_auto(), inset_px(-20.0), inset_auto(), inset_px(10.0)),
            None,
            110.0,
            10.0,
        ),
        (
            "left -10 right -50",
            edges(inset_auto(), inset_px(-50.0), inset_auto(), inset_px(-10.0)),
            None,
            160.0,
            -10.0,
        ),
    ];
    for (case, mut insets, max_width, width, x) in cases {
        let mut tree = with_default_anchor("--anchor", rect(40.0, 40.0, 50.0, 50.0));
        insets.top = anchor("", Side::Bottom, None);
        let target = TestStyle {
            position: PositionProperty::Absolute,
            justify_self: align(AlignFlags::ANCHOR_CENTER),
            inset: insets,
            size: Size::new(size_auto(), size_px(20.0)),
            max_size: Size::new(max_width.map_or(max_none(), max_px), max_none()),
            ..TestStyle::default()
        };
        let target = tree.push_measured_leaf(target, wrapping_text);
        let root = tree.push_flex(container(100.0, 100.0), vec![target]);
        definite_layout(&tree, root, 100.0, 100.0);
        assert_box(&tree.layout(target), (x, 90.0, width, 20.0), case);
    }
}

/// Text that wraps: as wide as it may be, between one word (10px) and one
/// line (1000px).
fn wrapping_text(input: LeafMeasureInput) -> LeafMetrics {
    let width = input
        .known_dimensions
        .width
        .unwrap_or(match input.available_space.width {
            AvailableSpace::MinContent => 10.0,
            AvailableSpace::MaxContent => 1000.0,
            AvailableSpace::Definite(width) => width.clamp(10.0, 1000.0),
        });
    LeafMetrics::new(Size::new(
        width,
        input.known_dimensions.height.unwrap_or(20.0),
    ))
}

/// WPT `anchor-center-no-default.html`: without a default anchor
/// `anchor-center` is `center` and leaves `auto` insets alone.
#[test]
fn anchor_center_without_a_default_anchor_is_center() {
    let mut tree = TestTree::default();
    let layout = place(
        &mut tree,
        Size::new(200.0, 100.0),
        TestStyle {
            inset: edges(inset_px(0.0), inset_px(0.0), inset_auto(), inset_px(0.0)),
            justify_self: align(AlignFlags::ANCHOR_CENTER),
            align_self: align(AlignFlags::ANCHOR_CENTER),
            ..absolute(40.0, 20.0)
        },
    );
    // Horizontal: both insets, centered. Vertical: one `auto` inset, so the
    // top inset places it and alignment has no effect.
    assert_box(&layout, (80.0, 0.0, 40.0, 20.0), "center");
}

#[test]
fn anchor_center_near_an_edge_shifts_within_the_containing_block() {
    // Anchor center at x = 15: centered, a 40px box would start at -5.
    let mut tree = with_default_anchor("--a", rect(5.0, 0.0, 20.0, 20.0));
    let layout = place(
        &mut tree,
        Size::new(200.0, 100.0),
        TestStyle {
            justify_self: align(AlignFlags::ANCHOR_CENTER),
            inset: edges(inset_px(0.0), inset_auto(), inset_auto(), inset_auto()),
            ..absolute(40.0, 20.0)
        },
    );
    assert_box(&layout, (0.0, 0.0, 40.0, 20.0), "safe anchor-center");

    let mut tree = with_default_anchor("--a", rect(5.0, 0.0, 20.0, 20.0));
    let layout = place(
        &mut tree,
        Size::new(200.0, 100.0),
        TestStyle {
            justify_self: align(AlignFlags::ANCHOR_CENTER | AlignFlags::UNSAFE),
            inset: edges(inset_px(0.0), inset_auto(), inset_auto(), inset_auto()),
            ..absolute(40.0, 20.0)
        },
    );
    assert_box(&layout, (-5.0, 0.0, 40.0, 20.0), "unsafe anchor-center");
}

// ---------------------------------------------------------------------------
// css-position-3 §4.3 self-alignment of absolutely positioned boxes.

fn aligned(justify: AlignFlags, align_flags: AlignFlags, insets: Edges<Inset>) -> TestStyle {
    TestStyle {
        inset: insets,
        justify_self: align(justify),
        align_self: align(align_flags),
        ..absolute(40.0, 20.0)
    }
}

#[test]
fn self_alignment_places_the_margin_box_in_the_imcb() {
    let all = || inset(0.0, 0.0, 0.0, 0.0);
    let cases = [
        (
            "start / start",
            AlignFlags::START,
            AlignFlags::START,
            all(),
            (0.0, 0.0),
        ),
        (
            "center / center",
            AlignFlags::CENTER,
            AlignFlags::CENTER,
            all(),
            (80.0, 40.0),
        ),
        (
            "end / end",
            AlignFlags::END,
            AlignFlags::END,
            all(),
            (160.0, 80.0),
        ),
        (
            "right / flex-end",
            AlignFlags::RIGHT,
            AlignFlags::FLEX_END,
            all(),
            (160.0, 80.0),
        ),
        (
            "self-end / last baseline",
            AlignFlags::SELF_END,
            AlignFlags::LAST_BASELINE,
            all(),
            (160.0, 80.0),
        ),
        (
            "normal / auto",
            AlignFlags::NORMAL,
            AlignFlags::AUTO,
            all(),
            (0.0, 0.0),
        ),
        (
            "center in an inset rectangle",
            AlignFlags::CENTER,
            AlignFlags::END,
            inset(10.0, 30.0, 20.0, 50.0),
            (90.0, 60.0),
        ),
        (
            "one auto inset: no effect",
            AlignFlags::END,
            AlignFlags::CENTER,
            edges(inset_px(5.0), inset_auto(), inset_auto(), inset_px(7.0)),
            (7.0, 5.0),
        ),
    ];
    for (case, justify, align_flags, insets, (x, y)) in cases {
        let mut tree = TestTree::default();
        let layout = place(
            &mut tree,
            Size::new(200.0, 100.0),
            aligned(justify, align_flags, insets),
        );
        assert_box(&layout, (x, y, 40.0, 20.0), case);
    }
}

#[test]
fn non_stretch_alignment_fits_auto_sizes_to_content() {
    let mut tree = TestTree::default();
    let id = place_in(
        &mut tree,
        container(200.0, 100.0),
        TestStyle {
            position: PositionProperty::Absolute,
            inset: inset(0.0, 0.0, 0.0, 0.0),
            justify_self: align(AlignFlags::CENTER),
            ..TestStyle::default()
        },
        Size::new(30.0, 10.0),
    );
    // Horizontal: fit-content, centered; vertical: `normal`, stretched.
    assert_box(
        &tree.layout(id),
        (85.0, 0.0, 30.0, 100.0),
        "center / normal",
    );

    let mut tree = TestTree::default();
    let id = place_in(
        &mut tree,
        container(200.0, 100.0),
        TestStyle {
            position: PositionProperty::Absolute,
            inset: inset(0.0, 0.0, 0.0, 0.0),
            justify_self: align(AlignFlags::STRETCH),
            align_self: align(AlignFlags::END),
            ..TestStyle::default()
        },
        Size::new(30.0, 10.0),
    );
    assert_box(&tree.layout(id), (0.0, 90.0, 200.0, 10.0), "stretch / end");
}

#[test]
fn auto_margins_take_precedence_over_alignment() {
    let mut tree = TestTree::default();
    let layout = place(
        &mut tree,
        Size::new(200.0, 100.0),
        TestStyle {
            margin: edges(
                margin_px(0.0),
                margin_px(0.0),
                margin_px(0.0),
                margin_auto(),
            ),
            ..aligned(
                AlignFlags::START,
                AlignFlags::START,
                inset(0.0, 0.0, 0.0, 0.0),
            )
        },
    );
    assert_box(&layout, (160.0, 0.0, 40.0, 20.0), "margin-left: auto");
}

/// css-align-3 §4.4.1.2: default and `safe` alignment keep an overflowing
/// box inside the overflow limit rect; `unsafe` honors the alignment.
#[test]
fn overflowing_alignment_is_safe_by_default() {
    let cases = [
        // IMCB [0, 200]; a 250px box does not fit the limit rect either, so
        // it start-aligns in it.
        (
            "default center, too big",
            AlignFlags::CENTER,
            inset(0.0, 0.0, 0.0, 0.0),
            250.0,
            0.0,
        ),
        (
            "unsafe center, too big",
            AlignFlags::CENTER | AlignFlags::UNSAFE,
            inset(0.0, 0.0, 0.0, 0.0),
            250.0,
            -25.0,
        ),
        (
            "safe center, too big",
            AlignFlags::CENTER | AlignFlags::SAFE,
            inset(0.0, 0.0, 0.0, 0.0),
            250.0,
            0.0,
        ),
        // IMCB [150, 200]; an 80px box covers it and stays in [0, 200].
        (
            "default start, overflowing",
            AlignFlags::START,
            inset(0.0, 0.0, 0.0, 150.0),
            80.0,
            120.0,
        ),
        (
            "unsafe start, overflowing",
            AlignFlags::START | AlignFlags::UNSAFE,
            inset(0.0, 0.0, 0.0, 150.0),
            80.0,
            150.0,
        ),
        (
            "default end, overflowing",
            AlignFlags::END,
            inset(0.0, 0.0, 0.0, 150.0),
            80.0,
            120.0,
        ),
    ];
    for (case, justify, insets, width, x) in cases {
        let mut tree = TestTree::default();
        let layout = place(
            &mut tree,
            Size::new(200.0, 100.0),
            TestStyle {
                inset: insets,
                justify_self: align(justify),
                ..absolute(width, 20.0)
            },
        );
        assert_box(&layout, (x, 0.0, width, 20.0), case);
    }
}

#[test]
fn start_and_end_follow_the_containing_block_direction() {
    let rtl_cb = TestStyle {
        direction: direction::T::Rtl,
        ..container(200.0, 100.0)
    };
    let cases = [
        ("start", AlignFlags::START, direction::T::Rtl, 160.0),
        ("end", AlignFlags::END, direction::T::Rtl, 0.0),
        ("left", AlignFlags::LEFT, direction::T::Rtl, 0.0),
        (
            "self-start, ltr box",
            AlignFlags::SELF_START,
            direction::T::Ltr,
            0.0,
        ),
    ];
    for (case, justify, box_direction, x) in cases {
        let mut tree = TestTree::default();
        let id = place_in(
            &mut tree,
            rtl_cb.clone(),
            TestStyle {
                direction: box_direction,
                ..aligned(justify, AlignFlags::START, inset(0.0, 0.0, 0.0, 0.0))
            },
            Size::new(40.0, 20.0),
        );
        assert_box(&tree.layout(id), (x, 0.0, 40.0, 20.0), case);
    }
}

/// Lynx's `linear` and `relative` containers place an absolutely positioned
/// child by its insets alone; `justify-self`/`align-self` do not move it.
#[test]
fn lynx_containers_ignore_authored_self_alignment() {
    for relative in [false, true] {
        let mut tree = TestTree::default();
        let child = tree.push_leaf(
            aligned(
                AlignFlags::CENTER,
                AlignFlags::END,
                inset(0.0, 0.0, 0.0, 0.0),
            ),
            Size::new(40.0, 20.0),
            None,
        );
        let root = if relative {
            tree.push_relative(container(200.0, 100.0), vec![child])
        } else {
            tree.push_linear(container(200.0, 100.0), vec![child])
        };
        definite_layout(&tree, root, 200.0, 100.0);
        assert_box(&tree.layout(child), (0.0, 0.0, 40.0, 20.0), "insets only");
    }
}

// ---------------------------------------------------------------------------
// §6.5 position fallback.

/// WPT `position-try-001.html`: a 40×15 box with 5px margins, placed right of
/// `--a1`; the options are left of it, below, above, and right with another
/// size.
fn try_001(tree: &mut TestTree, cb_width: f32, anchor_rect: Rect<f32>) -> TestId {
    tree.anchor_rects = vec![("--a1", anchor_rect)];
    let base = TestStyle {
        margin: edges(
            margin_px(5.0),
            margin_px(5.0),
            margin_px(5.0),
            margin_px(5.0),
        ),
        inset: edges(
            anchor("--a1", Side::Top, None),
            inset_auto(),
            inset_auto(),
            anchor("--a1", Side::Right, None),
        ),
        ..absolute(40.0, 15.0)
    };
    let option = |top, right, bottom, left| TestStyle {
        inset: edges(top, right, bottom, left),
        ..base.clone()
    };
    let options = vec![
        option(
            anchor("--a1", Side::Top, None),
            anchor("--a1", Side::Left, None),
            inset_auto(),
            inset_auto(),
        ),
        option(
            anchor("--a1", Side::Bottom, None),
            inset_auto(),
            inset_auto(),
            anchor("--a1", Side::Left, None),
        ),
        option(
            inset_auto(),
            inset_auto(),
            anchor("--a1", Side::Top, None),
            anchor("--a1", Side::Left, None),
        ),
        TestStyle {
            size: Size::new(size_px(35.0), size_px(40.0)),
            ..option(
                anchor("--a1", Side::Top, None),
                inset_auto(),
                inset_auto(),
                anchor("--a1", Side::Right, None),
            )
        },
    ];
    let target = tree.push_leaf(base, Size::new(40.0, 15.0), None);
    tree.position_options = vec![(target, options)];
    let root = tree.push_flex(container(cb_width, 70.0), vec![target]);
    definite_layout(tree, root, cb_width, 70.0);
    target
}

#[test]
fn the_first_fitting_option_wins_wpt_position_try_001() {
    // (case, cb width, anchor, expected box, chosen, overflows)
    let cases = [
        (
            "wider cb: base fits",
            195.0,
            rect(45.0, 20.0, 100.0, 30.0),
            (150.0, 25.0, 40.0, 15.0),
            0,
            false,
        ),
        (
            "anchor further right: --f1",
            190.0,
            rect(50.0, 20.0, 100.0, 30.0),
            (5.0, 25.0, 40.0, 15.0),
            1,
            false,
        ),
        (
            "no spacer: --f2",
            190.0,
            rect(45.0, 0.0, 100.0, 30.0),
            (50.0, 35.0, 40.0, 15.0),
            2,
            false,
        ),
        (
            "two spacers: --f3",
            190.0,
            rect(45.0, 40.0, 100.0, 30.0),
            (50.0, 20.0, 40.0, 15.0),
            3,
            false,
        ),
        (
            "one spacer: --f4",
            190.0,
            rect(45.0, 20.0, 100.0, 30.0),
            (150.0, 25.0, 35.0, 40.0),
            4,
            false,
        ),
        (
            "narrower cb: nothing fits",
            185.0,
            rect(45.0, 20.0, 100.0, 30.0),
            (150.0, 25.0, 40.0, 15.0),
            0,
            true,
        ),
    ];
    for (case, cb_width, anchor_rect, expected, chosen, overflows) in cases {
        let mut tree = TestTree::default();
        let target = try_001(&mut tree, cb_width, anchor_rect);
        assert_box(&tree.layout(target), expected, case);
        let reported = outcome(&tree, target);
        assert_eq!(
            (reported.chosen, reported.overflows),
            (chosen, overflows),
            "{case}"
        );
    }
}

#[test]
fn the_last_successful_option_is_kept_while_it_fits() {
    // Base would fit (wider cb), but --f4 was last successful and fits too.
    let mut tree = TestTree::default();
    tree.last_successful = vec![(0, 4)];
    let target = try_001(&mut tree, 195.0, rect(45.0, 20.0, 100.0, 30.0));
    assert_eq!(target, 0, "the target is the first node pushed");
    assert_box(&tree.layout(target), (150.0, 25.0, 35.0, 40.0), "kept --f4");
    assert_eq!(outcome(&tree, target).chosen, 4);

    // Last successful --f1 no longer fits: the list is walked from the base,
    // skipping --f1, and the base fits.
    let mut tree = TestTree::default();
    tree.last_successful = vec![(0, 1)];
    let target = try_001(&mut tree, 195.0, rect(45.0, 20.0, 100.0, 30.0));
    assert_box(
        &tree.layout(target),
        (150.0, 25.0, 40.0, 15.0),
        "back to base",
    );
    assert_eq!(outcome(&tree, target).chosen, 0);

    // Nothing fits: the current (last successful) option stays.
    let mut tree = TestTree::default();
    tree.last_successful = vec![(0, 2)];
    let target = try_001(&mut tree, 185.0, rect(45.0, 20.0, 100.0, 30.0));
    let reported = outcome(&tree, target);
    assert_eq!((reported.chosen, reported.overflows), (2, true));
    assert_box(
        &tree.layout(target),
        (50.0, 55.0, 40.0, 15.0),
        "stays on --f2",
    );
}

/// WPT `position-try-order-basic.html` (numbers adapted): `most-width` tries
/// the option with the widest inset-modified containing block first.
#[test]
fn position_try_order_sorts_by_imcb_size() {
    let run = |order: PositionTryOrder| {
        let mut tree = with_default_anchor("--a", rect(150.0, 200.0, 150.0, 150.0));
        let base = TestStyle {
            inset: edges(inset_auto(), inset_auto(), inset_auto(), inset_px(450.0)),
            position_try_order: order,
            ..absolute(40.0, 40.0)
        };
        let right = TestStyle {
            inset: edges(
                inset_auto(),
                inset_auto(),
                inset_auto(),
                anchor("", Side::Right, None),
            ),
            ..base.clone()
        };
        let left = TestStyle {
            inset: edges(
                inset_auto(),
                anchor("", Side::Left, None),
                inset_auto(),
                inset_auto(),
            ),
            ..base.clone()
        };
        let target = tree.push_leaf(base, Size::new(40.0, 40.0), None);
        tree.position_options = vec![(target, vec![right, left])];
        let root = tree.push_flex(container(400.0, 400.0), vec![target]);
        definite_layout(&tree, root, 400.0, 400.0);
        (
            tree.layout(target).location.x,
            outcome(&tree, target).chosen,
        )
    };
    assert_eq!(
        run(PositionTryOrder::Normal),
        (300.0, 1),
        "list order: --right"
    );
    assert_eq!(
        run(PositionTryOrder::MostWidth),
        (110.0, 2),
        "--left is wider"
    );
    assert_eq!(
        run(PositionTryOrder::MostInlineSize),
        (110.0, 2),
        "inline is horizontal"
    );
    // Equal heights: the stable sort keeps list order.
    assert_eq!(run(PositionTryOrder::MostHeight), (300.0, 1), "stable");
}

/// §6.5: an option whose inset-modified containing block was negative and
/// corrected to zero does not fit, even for a zero-size margin box.
#[test]
fn a_negative_imcb_never_fits() {
    let mut tree = TestTree::default();
    let base = TestStyle {
        inset: edges(inset_px(0.0), inset_auto(), inset_auto(), inset_px(500.0)),
        ..absolute(0.0, 0.0)
    };
    let negative = TestStyle {
        inset: edges(
            inset_px(0.0),
            inset_px(300.0),
            inset_auto(),
            inset_px(300.0),
        ),
        ..base.clone()
    };
    let fits = TestStyle {
        inset: inset(0.0, 0.0, 0.0, 0.0),
        ..base.clone()
    };
    let target = tree.push_leaf(base, Size::ZERO, None);
    tree.position_options = vec![(target, vec![negative, fits])];
    let root = tree.push_flex(container(400.0, 100.0), vec![target]);
    definite_layout(&tree, root, 400.0, 100.0);
    assert_eq!(outcome(&tree, target).chosen, 2);
}

/// A box with options claims no content independence: which option it is
/// committed with depends on its own size.
#[test]
fn a_box_with_options_is_not_content_independent() {
    let mut tree = TestTree::default();
    tree.enable_cache();
    let target = try_001(&mut tree, 195.0, rect(45.0, 20.0, 100.0, 30.0));
    let input = tree.committed_input(target).expect("committed");
    assert_eq!(input.goal.independence(), Some(Size::new(false, false)));

    // Without options the claim stands.
    let mut tree = with_default_anchor("--a", rect(100.0, 100.0, 100.0, 100.0));
    tree.enable_cache();
    let id = place_in(
        &mut tree,
        container(300.0, 300.0),
        TestStyle {
            position_area: area(K::Top, K::Left),
            ..absolute(10.0, 10.0)
        },
        Size::new(10.0, 10.0),
    );
    let input = tree.committed_input(id).expect("committed");
    assert_eq!(input.goal.independence(), Some(Size::new(true, true)));
}

#[test]
fn trial_options_are_measured_and_only_the_winner_is_committed() {
    let mut tree = TestTree::default();
    tree.enable_cache();
    let target = try_001(&mut tree, 190.0, rect(45.0, 20.0, 100.0, 30.0));
    let input = tree.committed_input(target).expect("committed");
    // --f4's size is handed over as known dimensions.
    assert_eq!(input.known_dimensions, Size::new(Some(35.0), Some(40.0)));
}

// ---------------------------------------------------------------------------
// The outcome (§3.3 compensation, §6.6).

#[test]
fn compensation_follows_the_three_conditions() {
    let anchor_rect = rect(100.0, 100.0, 50.0, 50.0);
    let run = |style: TestStyle, shares: bool| {
        let mut tree = with_default_anchor("--a", anchor_rect);
        tree.anchor_rects.push(("--b", rect(0.0, 0.0, 10.0, 10.0)));
        if shares {
            tree.scrolls_with_default = vec!["--b"];
        }
        let id = place_in(
            &mut tree,
            container(300.0, 300.0),
            style,
            Size::new(10.0, 10.0),
        );
        outcome(&tree, id)
    };
    let with = |inset: Edges<Inset>| TestStyle {
        inset,
        ..absolute(10.0, 10.0)
    };
    let auto = || edges(inset_auto(), inset_auto(), inset_auto(), inset_auto());

    let area = run(
        TestStyle {
            position_area: area(K::Top, K::None),
            ..absolute(10.0, 10.0)
        },
        false,
    );
    assert_eq!(area.compensates, Size::new(true, true), "position-area");
    assert!(area.references_default_anchor && area.default_anchor_resolved);

    let centered = run(
        TestStyle {
            align_self: align(AlignFlags::ANCHOR_CENTER),
            ..absolute(10.0, 10.0)
        },
        false,
    );
    assert_eq!(
        centered.compensates,
        Size::new(false, true),
        "anchor-center"
    );

    let mut default_left = auto();
    default_left.left = anchor("", Side::Right, None);
    assert_eq!(
        run(with(default_left), false).compensates,
        Size::new(true, false),
        "anchor() on the default anchor"
    );

    let mut named_top = auto();
    named_top.top = anchor("--b", Side::Bottom, None);
    assert_eq!(
        run(with(named_top.clone()), true).compensates,
        Size::new(false, true)
    );
    let other = run(with(named_top), false);
    assert_eq!(
        other.compensates,
        Size::new(false, false),
        "another scroller"
    );
    assert!(!other.references_default_anchor);
}

#[test]
fn outcome_rectangles_are_in_padding_box_coordinates() {
    let mut tree = with_default_anchor("--a", rect(100.0, 100.0, 100.0, 100.0));
    let id = place_in(
        &mut tree,
        container(300.0, 300.0),
        TestStyle {
            inset: inset(10.0, 15.0, 20.0, 25.0),
            margin: edges(
                margin_px(1.0),
                margin_px(2.0),
                margin_px(3.0),
                margin_px(4.0),
            ),
            position_area: area(K::Top, K::Left),
            ..absolute(10.0, 10.0)
        },
        Size::new(10.0, 10.0),
    );
    let reported = outcome(&tree, id);
    assert_eq!(reported.imcb, rect(25.0, 10.0, 60.0, 70.0));
    // End-aligned: the margin box's right/bottom edges on the IMCB's.
    assert_eq!(reported.margin_box, rect(69.0, 66.0, 16.0, 14.0));
    assert!(!reported.overflows);
}

#[test]
fn plain_absolute_boxes_report_nothing() {
    let mut tree = with_default_anchor("--a", rect(100.0, 100.0, 100.0, 100.0));
    let id = place_in(
        &mut tree,
        container(300.0, 300.0),
        TestStyle {
            inset: inset(10.0, 15.0, 20.0, 25.0),
            ..absolute(10.0, 10.0)
        },
        Size::new(10.0, 10.0),
    );
    assert!(tree.anchor_outcomes(id).is_empty());
}

// ---------------------------------------------------------------------------
// Grid: the grid area is the pre-modification containing block, the grid
// container's padding box the original one.

#[test]
fn position_area_in_a_grid_area() {
    let mut tree = TestTree::default();
    // Grid: padding 20, columns 200 / 300, rows 150 / 100 (WPT
    // `grid-position-area-basic.html`); the anchor fills cell (2, 2).
    tree.anchor_rects = vec![("--foo", rect(220.0, 170.0, 300.0, 100.0))];
    tree.default_anchor = Some("--foo");
    let positioned = tree.push_leaf(
        TestStyle {
            grid_row: Line::new(grid_line(1), grid_line(2)),
            grid_column: Line::new(grid_line(1), grid_line(2)),
            position_area: area(K::Top, K::Right),
            ..absolute(30.0, 30.0)
        },
        Size::new(30.0, 30.0),
        None,
    );
    let grid = tree.push_grid(
        TestStyle {
            padding: Edges::uniform(npx(20.0)),
            template_columns: track_list(vec![track_px(200.0), track_px(300.0)]),
            template_rows: track_list(vec![track_px(150.0), track_px(100.0)]),
            ..TestStyle::default()
        },
        vec![positioned],
    );
    definite_layout(&tree, grid, 540.0, 290.0);
    // Region (grid-area coordinates): x [500, 500], y [0, 150]; the box
    // aligns toward the anchor (x start, y end). It overflows the empty
    // column, and css-align-3 §4.4.1.2 keeps it inside the overflow limit
    // rect — the grid container's padding box, x [-20, 520] in grid-area
    // coordinates — so it ends at 520: x 490, plus the grid area's 20. (The
    // WPT reference places a narrower box at 500 unshifted; with 20px of
    // room it would not shift here either.)
    assert_box(
        &tree.layout(positioned),
        (510.0, 140.0, 30.0, 30.0),
        "top right of a grid area",
    );
    let reported = outcome(&tree, positioned);
    assert_eq!(reported.imcb, rect(520.0, 20.0, 0.0, 150.0));
}

/// WPT `position-try-fallbacks-no-fit-after-fit.html`: the `flip-block`
/// option (cascaded by the host: `top` ↔ `bottom`, `anchor(outside)` kept)
/// fits first; after the containing block narrows so that nothing fits, the
/// box is back on its base style, because the engine never records a last
/// successful option itself — the host does, after layout (§6.5.1.1).
#[test]
fn nothing_fitting_after_a_fit_keeps_the_base_until_the_host_records_wpt() {
    let run = |width: f32, last_successful: Option<usize>| {
        let mut tree = with_default_anchor("--a", rect(0.0, 280.0, 20.0, 20.0));
        tree.last_successful = last_successful
            .map(|option| (0, option))
            .into_iter()
            .collect();
        let base = TestStyle {
            inset: edges(
                anchor("", Side::Outside, None),
                inset_auto(),
                inset_auto(),
                inset_px(0.0),
            ),
            ..absolute(390.0, 100.0)
        };
        let flipped = TestStyle {
            inset: edges(
                inset_auto(),
                inset_auto(),
                anchor("", Side::Outside, None),
                inset_px(0.0),
            ),
            ..base.clone()
        };
        let target = tree.push_leaf(base, Size::new(390.0, 100.0), None);
        tree.position_options = vec![(target, vec![flipped])];
        let root = tree.push_flex(container(width, 300.0), vec![target]);
        definite_layout(&tree, root, width, 300.0);
        (tree.layout(target).location.y, outcome(&tree, target))
    };
    let (y, fitted) = run(400.0, None);
    assert_eq!((y, fitted.chosen, fitted.overflows), (180.0, 1, false));
    let (y, narrowed) = run(380.0, None);
    assert_eq!((y, narrowed.chosen, narrowed.overflows), (300.0, 0, true));
    // Had the host recorded flip-block, nothing fitting would keep it.
    let (y, recorded) = run(380.0, Some(1));
    assert_eq!((y, recorded.chosen, recorded.overflows), (180.0, 1, true));
}

/// WPT `position-area-in-position-try.html` in spirit: an option may change
/// `position-area`, and (through the host) the default anchor.
#[test]
fn options_change_position_area_and_the_default_anchor() {
    let mut tree = TestTree::default();
    tree.anchor_rects = vec![
        ("--near-top", rect(100.0, 5.0, 50.0, 20.0)),
        ("--low", rect(100.0, 200.0, 50.0, 20.0)),
    ];
    tree.default_anchor = Some("--near-top");
    tree.option_default_anchors = vec![(2, Some("--low"))];
    let base = TestStyle {
        position_area: area(K::Top, K::Center),
        ..absolute(30.0, 30.0)
    };
    let below = TestStyle {
        position_area: area(K::Bottom, K::Center),
        size: Size::new(size_px(30.0), size_px(300.0)),
        ..base.clone()
    };
    let above_low = TestStyle {
        position_area: area(K::Top, K::Center),
        ..base.clone()
    };
    let target = tree.push_leaf(base, Size::new(30.0, 30.0), None);
    tree.position_options = vec![(target, vec![below, above_low])];
    let root = tree.push_flex(container(300.0, 300.0), vec![target]);
    definite_layout(&tree, root, 300.0, 300.0);
    // Above `--near-top` there are 5px; below it a 300px box does not fit;
    // above `--low` (option 2's default anchor) there are 200px.
    assert_box(
        &tree.layout(target),
        (110.0, 170.0, 30.0, 30.0),
        "above --low",
    );
    let reported = outcome(&tree, target);
    assert_eq!(reported.chosen, 2);
    assert_eq!(reported.imcb, rect(100.0, 0.0, 50.0, 200.0));
}

// ---------------------------------------------------------------------------
// Hoisted boxes: laid out by their containing block's absolute pass.

/// A box whose containing block is not its parent — here `fixed` under a
/// non-establishing wrapper — is laid out by the containing block's
/// absolute pass in tree order with the block's own out-of-flow children:
/// after the one before its wrapper, before the one after it. Its static
/// position comes from the wrapper (recorded during the in-flow phase) and
/// its layout goes back into the wrapper's coordinates.
#[test]
fn hoisted_boxes_interleave_with_own_out_of_flow_children_in_tree_order() {
    let mut tree = TestTree::default();
    let before = tree.push_leaf(absolute(5.0, 5.0), Size::new(5.0, 5.0), None);
    let hoisted = tree.push_leaf(
        TestStyle {
            position: PositionProperty::Fixed,
            inset: edges(inset_auto(), inset_auto(), inset_auto(), inset_px(20.0)),
            ..absolute(10.0, 10.0)
        },
        Size::new(10.0, 10.0),
        None,
    );
    let wrapper = tree.push_flex(
        TestStyle {
            size: Size::new(size_px(50.0), size_px(30.0)),
            margin: edges(
                margin_px(0.0),
                margin_px(0.0),
                margin_px(0.0),
                margin_px(7.0),
            ),
            padding: edges(npx(3.0), npx(3.0), npx(3.0), npx(3.0)),
            ..TestStyle::default()
        },
        vec![hoisted],
    );
    let after = tree.push_leaf(absolute(5.0, 5.0), Size::new(5.0, 5.0), None);
    let root = tree.push_flex(container(200.0, 100.0), vec![before, wrapper, after]);
    tree.hoisted = vec![(root, hoisted, 1)];
    definite_layout(&tree, root, 200.0, 100.0);

    assert_eq!(
        *tree.out_of_flow_writes.borrow(),
        vec![before, hoisted, after]
    );
    // `left: 20px` against the containing block is 13px into the wrapper,
    // which sits 7px in; the static `top` is the wrapper's content edge.
    assert_box(&tree.layout(hoisted), (13.0, 3.0, 10.0, 10.0), "hoisted");
}
