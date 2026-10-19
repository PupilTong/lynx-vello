//! Inline boxes in a CSS block container's inline formatting context
//! (`hughie::text::inline`): non-atomic inline boxes with their margin,
//! border and padding, and atomic inline boxes, on Ahem geometry.
//!
//! Ahem's em square is exact: every glyph advances one font size, the ascent
//! is 0.8em, the descent 0.2em and the line gap zero, so at 10px a line of
//! `line-height: normal` is 10px with the baseline 8px down. Glyphs Ahem has
//! no outline for — the Hebrew and Han below — still advance 1em.

#![allow(clippy::float_cmp)]

use hughie::geometry::{Point, Rect, Size};
use hughie::text::block::PlacedBox;
use hughie::text::inline::{
    FlowSource, InlineAtom, InlineBoxOpen, InlineEdges, InlineFlow, InlineFlowItem,
    InlineFlowStyle, InlineFragment, InlineRun,
};
use hughie::text::paragraph::{
    BlockConstraint, BreakStyle, Direction, IndentKeywords, LineHeight, OverflowWrap, RunStyle,
    TextWrap, VerticalAlign, WhiteSpaceCollapse,
};
use hughie::text::{FontBlob, TextContext};
use parley::PositionedLayoutItem;
use stylo::Atom;
use stylo::values::computed::font::{
    FamilyName, FontFamily, FontFamilyList, FontFamilyNameSyntax, SingleFontFamily,
};

const AHEM: &[u8] = include_bytes!("fixtures/Ahem.ttf");

fn text_context() -> TextContext {
    let mut context = TextContext::without_system_fonts();
    assert_eq!(context.register_fonts(FontBlob::from_static(AHEM)), 1);
    context
}

fn ahem() -> RunStyle {
    RunStyle {
        font_family: FontFamily {
            families: FontFamilyList {
                list: stylo::ArcSlice::from_iter(std::iter::once(SingleFontFamily::FamilyName(
                    FamilyName {
                        name: Atom::from("Ahem"),
                        syntax: FontFamilyNameSyntax::Identifiers,
                    },
                ))),
            },
            is_system_font: false,
            is_initial: false,
        },
        font_size: 10.0,
        line_height: LineHeight::Normal,
        ..RunStyle::default()
    }
}

fn run<'src>(style: &'src RunStyle, text: &'src str) -> InlineFlowItem<'src> {
    InlineFlowItem::Run(InlineRun {
        text,
        style,
        white_space: WhiteSpaceCollapse::Collapse,
        breaking: BreakStyle::default(),
    })
}

/// An inline box with `edges`, in `direction`.
fn open_in(style: &RunStyle, direction: Direction, edges: InlineEdges) -> InlineFlowItem<'_> {
    InlineFlowItem::Open(InlineBoxOpen {
        style,
        breaking: BreakStyle::default(),
        direction,
        edges,
    })
}

/// A left-to-right inline box whose start and end edges are `start` and
/// `end` px of padding.
fn open(style: &RunStyle, start: f32, end: f32) -> InlineFlowItem<'_> {
    open_in(
        style,
        Direction::Ltr,
        InlineEdges {
            start_inset: start,
            end_inset: end,
            ..InlineEdges::default()
        },
    )
}

const CLOSE: InlineFlowItem<'static> = InlineFlowItem::Close;

fn container(direction: Direction) -> InlineFlowStyle {
    InlineFlowStyle {
        strut: ahem(),
        direction,
        ..InlineFlowStyle::default()
    }
}

/// A left-to-right flow of `items` committed at `width`.
fn committed(items: &[InlineFlowItem<'_>], width: Option<f32>) -> InlineFlow {
    committed_in(Direction::Ltr, items, width)
}

fn committed_in(
    direction: Direction,
    items: &[InlineFlowItem<'_>],
    width: Option<f32>,
) -> InlineFlow {
    let mut context = text_context();
    let mut flow = InlineFlow::new(&mut context, container(direction), items);
    flow.commit(&mut context, BlockConstraint::new(width, 0.0));
    flow
}

/// The `(line, offset, advance)` of every glyph run shaped from run item
/// `item`.
fn runs_of(flow: &InlineFlow, item: u32) -> Vec<(usize, f32, f32)> {
    let mut found = Vec::new();
    for (index, line) in flow.display().lines().enumerate() {
        for entry in line.items() {
            if let PositionedLayoutItem::GlyphRun(glyph_run) = entry
                && flow.source_of(glyph_run.style_index()) == FlowSource::Run(item)
            {
                found.push((index, glyph_run.offset(), glyph_run.advance()));
            }
        }
    }
    found
}

/// The fragments of the inline box whose `Open` is item `item`.
fn fragments_of(flow: &InlineFlow, item: u32) -> Vec<InlineFragment> {
    flow.fragments()
        .iter()
        .filter(|fragment| fragment.item == item)
        .copied()
        .collect()
}

/// A fragment's inline extent, rounded to a hundredth.
fn span(fragment: &InlineFragment) -> (f32, f32) {
    let round = |value: f32| (value * 100.0).round() / 100.0;
    (
        round(fragment.rect.origin.x),
        round(fragment.rect.origin.x + fragment.rect.size.width),
    )
}

#[test]
fn a_box_s_edges_are_advance_before_and_after_its_content() {
    let style = ahem();
    // aa <span>bb</span> cc, the span with a 2px start margin, a 1px border
    // and 5px padding at its start, 3px of padding and a 4px margin at its
    // end, and 1px and 2px of border and padding above and below.
    let edges = InlineEdges {
        start_margin: 2.0,
        start_inset: 6.0,
        end_inset: 3.0,
        end_margin: 4.0,
        top_inset: 1.0,
        bottom_inset: 2.0,
    };
    let items = [
        run(&style, "aa"),
        open_in(&style, Direction::Ltr, edges),
        run(&style, "bb"),
        CLOSE,
        run(&style, "cc"),
    ];
    let flow = committed(&items, None);
    assert_eq!(runs_of(&flow, 0), [(0, 0.0, 20.0)]);
    // The glyphs after an edge move by it: 20 + 2 + 6, then 48 + 3 + 4.
    assert_eq!(runs_of(&flow, 2), [(0, 28.0, 20.0)]);
    assert_eq!(runs_of(&flow, 4), [(0, 55.0, 20.0)]);
    assert_eq!(flow.size(), Size::new(75.0, 10.0));
    // The border box: from the start border edge to the end padding edge's
    // border, horizontally; the content area grown by the vertical border
    // and padding, which do not touch the 10px line box.
    let [fragment] = fragments_of(&flow, 1)[..] else {
        panic!("one fragment expected, got {:?}", flow.fragments())
    };
    assert_eq!(span(&fragment), (22.0, 51.0));
    assert_eq!(fragment.rect.origin.y, -1.0);
    assert_eq!(fragment.rect.size.height, 13.0);
    assert!(fragment.first && fragment.last);
    assert_eq!(flow.line_geometry()[0].height, 10.0);
}

#[test]
fn slice_puts_the_start_edge_on_the_first_fragment_and_the_end_edge_on_the_last() {
    let style = ahem();
    // aa <span>bb cc</span> dd at 70px: "aa ", the 10px start edge and
    // "bb " fill the first line; "cc", the end edge and " dd" the second.
    let items = [
        run(&style, "aa "),
        open(&style, 10.0, 10.0),
        run(&style, "bb cc"),
        CLOSE,
        run(&style, " dd"),
    ];
    let flow = committed(&items, Some(70.0));
    assert_eq!(flow.line_geometry().len(), 2);
    let fragments = fragments_of(&flow, 1);
    assert_eq!(fragments.len(), 2);
    // The first holds the start edge and "bb", not the space hanging after
    // it at the line's end (css-text-3 §4.1.3).
    assert_eq!((fragments[0].line, span(&fragments[0])), (0, (30.0, 60.0)));
    assert!(fragments[0].first && !fragments[0].last);
    // The second holds "cc" and the end edge.
    assert_eq!((fragments[1].line, span(&fragments[1])), (1, (0.0, 30.0)));
    assert!(!fragments[1].first && fragments[1].last);
    assert_eq!(fragments[1].rect.origin.y, 10.0);
    assert_eq!(runs_of(&flow, 4), [(1, 30.0, 30.0)]);
}

#[test]
fn nested_boxes_hold_their_children_and_their_children_s_margins() {
    let style = ahem();
    let outer = InlineEdges {
        start_margin: 3.0,
        start_inset: 4.0,
        end_inset: 4.0,
        end_margin: 3.0,
        ..InlineEdges::default()
    };
    let items = [
        run(&style, "a"),
        open_in(&style, Direction::Ltr, outer),
        run(&style, "b"),
        open(&style, 2.0, 2.0),
        run(&style, "c"),
        CLOSE,
        run(&style, "d"),
        CLOSE,
        run(&style, "e"),
    ];
    let flow = committed(&items, None);
    assert_eq!(runs_of(&flow, 2)[0].1, 17.0);
    assert_eq!(runs_of(&flow, 4)[0].1, 29.0);
    assert_eq!(runs_of(&flow, 6)[0].1, 41.0);
    assert_eq!(runs_of(&flow, 8)[0].1, 58.0);
    // The outer box's border box leaves its margins out and holds all of
    // the inner box; fragments come outer first, in tree order.
    let fragments = flow.fragments();
    assert_eq!(fragments.len(), 2);
    assert_eq!((fragments[0].item, span(&fragments[0])), (1, (13.0, 55.0)));
    assert_eq!((fragments[1].item, span(&fragments[1])), (3, (27.0, 41.0)));
}

#[test]
fn an_empty_box_with_padding_takes_part_in_the_line() {
    let style = ahem();
    // CSS 2.2 §9.4.2: a line holding an inline box with nonzero padding is
    // no zero-height line box, so it keeps the strut.
    let alone = committed(&[open(&style, 5.0, 5.0), CLOSE], None);
    assert_eq!(alone.size(), Size::new(10.0, 10.0));
    let [fragment] = alone.fragments() else {
        panic!("one fragment expected")
    };
    assert_eq!(span(fragment), (0.0, 10.0));
    assert_eq!(fragment.rect.size.height, 10.0);

    // Between two runs it is advance and no opportunity.
    let between = committed(
        &[
            run(&style, "aa"),
            open(&style, 5.0, 5.0),
            CLOSE,
            run(&style, "bb"),
        ],
        Some(30.0),
    );
    assert_eq!(between.line_geometry().len(), 1);
    assert_eq!(runs_of(&between, 3), [(0, 30.0, 20.0)]);
    assert_eq!(span(&between.fragments()[0]), (20.0, 30.0));

    // Without an edge it is nothing at all: no line, no fragment.
    let bare = committed(&[open(&style, 0.0, 0.0), CLOSE], None);
    assert_eq!(bare.size().height, 0.0);
    assert!(bare.fragments().is_empty());
}

#[test]
fn an_rtl_box_starts_on_the_right_of_its_content() {
    let style = ahem();
    let rtl = |start, end| {
        open_in(
            &style,
            Direction::Rtl,
            InlineEdges {
                start_inset: start,
                end_inset: end,
                ..InlineEdges::default()
            },
        )
    };
    // א <span dir=rtl>בב</span> ג in a right-to-left paragraph: reading
    // from the right, א, the 5px start edge, בב, the 3px end edge, ג.
    let items = [
        run(&style, "\u{5d0}"),
        rtl(5.0, 3.0),
        run(&style, "\u{5d1}\u{5d1}"),
        CLOSE,
        run(&style, "\u{5d2}"),
    ];
    let flow = committed_in(Direction::Rtl, &items, None);
    assert_eq!(flow.size().width, 48.0);
    assert_eq!(runs_of(&flow, 4)[0].1, 0.0);
    assert_eq!(runs_of(&flow, 2)[0].1, 13.0);
    assert_eq!(runs_of(&flow, 0)[0].1, 38.0);
    let [fragment] = flow.fragments() else {
        panic!("one fragment expected")
    };
    assert_eq!(span(fragment), (10.0, 38.0));
}

#[test]
fn whitespace_hangs_past_a_right_to_left_box_s_fragments() {
    let style = ahem();
    let rtl = |start, end| {
        open_in(
            &style,
            Direction::Rtl,
            InlineEdges {
                start_inset: start,
                end_inset: end,
                ..InlineEdges::default()
            },
        )
    };
    // אא <span>בב</span> גג in a right-to-left paragraph, the span with 5px
    // edges, wrapping before גג.
    let items = [
        run(&style, "\u{5d0}\u{5d0} "),
        rtl(5.0, 5.0),
        run(&style, "\u{5d1}\u{5d1}"),
        CLOSE,
        run(&style, " \u{5d2}\u{5d2}"),
    ];
    // At 65px the first line's 60px of content is aligned to the right edge
    // without the space after the box, which hangs past the line's left end
    // (css-text-3 §4.1.3): the fragment runs from its end edge at 5 to its
    // start edge at 35.
    let flow = committed_in(Direction::Rtl, &items, Some(65.0));
    assert_eq!(flow.line_geometry().len(), 2);
    let [fragment] = fragments_of(&flow, 1)[..] else {
        panic!("one fragment expected, got {:?}", flow.fragments())
    };
    assert_eq!((fragment.line, span(&fragment)), (0, (5.0, 35.0)));
    assert!(fragment.first && fragment.last);
    assert_eq!(runs_of(&flow, 4), [(0, -5.0, 10.0), (1, 45.0, 20.0)]);

    // At 40px the box has a line of its own, right-aligned, with the space
    // after it hanging on its left, not in the fragment.
    let flow = committed_in(Direction::Rtl, &items, Some(40.0));
    assert_eq!(flow.line_geometry().len(), 3);
    let [fragment] = fragments_of(&flow, 1)[..] else {
        panic!("one fragment expected, got {:?}", flow.fragments())
    };
    assert_eq!((fragment.line, span(&fragment)), (1, (10.0, 40.0)));
    assert_eq!(runs_of(&flow, 4), [(1, 0.0, 10.0), (2, 20.0, 20.0)]);
}

#[test]
fn a_box_s_edges_follow_the_logical_start_and_end_of_mixed_content() {
    let style = ahem();
    // a <span>בב</span> b in a left-to-right paragraph: the box's content
    // is right-to-left, and its start edge sits on its first character's
    // logical-before side — its right — and its end edge on its last
    // character's left.
    let items = [
        run(&style, "a"),
        open(&style, 5.0, 3.0),
        run(&style, "\u{5d1}\u{5d1}"),
        CLOSE,
        run(&style, "b"),
    ];
    let flow = committed(&items, None);
    assert_eq!(runs_of(&flow, 0)[0].1, 0.0);
    assert_eq!(runs_of(&flow, 2)[0].1, 13.0);
    assert_eq!(runs_of(&flow, 4)[0].1, 38.0);
    assert_eq!(span(&flow.fragments()[0]), (10.0, 38.0));
}

#[test]
fn an_edge_is_never_a_soft_wrap_opportunity() {
    let style = ahem();
    // aaa<span>bbb</span>: no opportunity anywhere, so the 70px of content
    // overflows a 50px line rather than breaking at the box's start, where
    // an atomic box would have let the line break.
    let glued = committed(
        &[
            run(&style, "aaa"),
            open(&style, 10.0, 0.0),
            run(&style, "bbb"),
            CLOSE,
        ],
        Some(50.0),
    );
    assert_eq!(glued.line_geometry().len(), 1);
    assert_eq!(glued.size().width, 70.0);

    // aaa <span>bbb</span>: the opportunity after the space is the box's
    // start; the start edge goes with "bbb" to the second line.
    let spaced = committed(
        &[
            run(&style, "aaa "),
            open(&style, 10.0, 0.0),
            run(&style, "bbb"),
            CLOSE,
        ],
        Some(50.0),
    );
    assert_eq!(spaced.line_geometry().len(), 2);
    assert_eq!(runs_of(&spaced, 2), [(1, 10.0, 30.0)]);

    // Between two Han characters UAX #14 has an opportunity, and it stays
    // before the start edge: 中中 | <span>中中</span>.
    let han = committed(
        &[
            run(&style, "\u{4e2d}\u{4e2d}"),
            open(&style, 10.0, 0.0),
            run(&style, "\u{4e2d}\u{4e2d}"),
            CLOSE,
        ],
        Some(35.0),
    );
    assert_eq!(han.line_geometry().len(), 2);
    assert_eq!(runs_of(&han, 0), [(0, 0.0, 20.0)]);
    assert_eq!(runs_of(&han, 2), [(1, 10.0, 20.0)]);

    // An end edge stays with the content before it: the line breaks after
    // it, at the space.
    let ended = committed(
        &[
            run(&style, "aa"),
            open(&style, 0.0, 10.0),
            run(&style, "bb"),
            CLOSE,
            run(&style, " cc"),
        ],
        Some(55.0),
    );
    assert_eq!(ended.line_geometry().len(), 2);
    let [fragment] = ended.fragments() else {
        panic!("one fragment expected")
    };
    assert_eq!((fragment.line, span(fragment)), (0, (20.0, 50.0)));
}

#[test]
fn a_box_starting_at_a_regional_indicator_keeps_the_flags_paired() {
    let style = ahem();
    // 🇺🇸<span>🇫🇷</span> at 35px: UAX #14 LB30a breaks between the two
    // flags and inside neither, so the box and its 10px start edge go to the
    // second line whole.
    let between = committed(
        &[
            run(&style, "\u{1F1FA}\u{1F1F8}"),
            open(&style, 10.0, 0.0),
            run(&style, "\u{1F1EB}\u{1F1F7}"),
            CLOSE,
        ],
        Some(35.0),
    );
    assert_eq!(between.line_geometry().len(), 2);
    assert_eq!(runs_of(&between, 0), [(0, 0.0, 20.0)]);
    assert_eq!(runs_of(&between, 2), [(1, 10.0, 20.0)]);

    // 🇺<span>🇸🇫🇷</span> at 35px: the box starts inside the first flag.
    // The edge keeps UAX #14's count, so nothing breaks inside 🇫🇷, as it
    // would if the edge had restarted the count. The opportunity between
    // the flags is lost (`docs/tracking/deviations.md`): the shaper pairs
    // the box's indicators from its own run's start, 🇸 with 🇫, and the
    // opportunity falls inside that cluster. The line overflows.
    let inside = committed(
        &[
            run(&style, "\u{1F1FA}"),
            open(&style, 10.0, 0.0),
            run(&style, "\u{1F1F8}\u{1F1EB}\u{1F1F7}"),
            CLOSE,
        ],
        Some(35.0),
    );
    assert_eq!(inside.line_geometry().len(), 1);
    assert_eq!(runs_of(&inside, 2), [(0, 20.0, 30.0)]);
}

#[test]
fn a_box_ending_with_a_forced_break_ends_on_the_line_the_break_ends() {
    let style = ahem();
    // <span>ab\n</span>cd, preserved: the break ends the first line, and
    // the box's 10px end edge stays on it, after "ab", rather than starting
    // the second line before "cd".
    let items = [
        open(&style, 0.0, 10.0),
        InlineFlowItem::Run(InlineRun {
            text: "ab\n",
            style: &style,
            white_space: WhiteSpaceCollapse::Preserve,
            breaking: BreakStyle::default(),
        }),
        CLOSE,
        run(&style, "cd"),
    ];
    let flow = committed(&items, None);
    assert_eq!(flow.line_geometry().len(), 2);
    let [fragment] = fragments_of(&flow, 0)[..] else {
        panic!("one fragment expected, got {:?}", flow.fragments())
    };
    assert_eq!((fragment.line, span(&fragment)), (0, (0.0, 30.0)));
    assert!(fragment.first && fragment.last);
    assert_eq!(runs_of(&flow, 3), [(1, 0.0, 20.0)]);
}

#[test]
fn an_out_of_flow_box_s_static_position_is_where_the_content_after_it_begins() {
    let style = ahem();
    // aa |bbb at 35px, wrapping after the space: before the second run is
    // the start of the second line; past every item, the end of "bbb".
    let wrapped = committed(&[run(&style, "aa "), run(&style, "bbb")], Some(35.0));
    assert_eq!(wrapped.static_position(0), Point::new(0.0, 0.0));
    assert_eq!(wrapped.static_position(1), Point::new(0.0, 10.0));
    assert_eq!(wrapped.static_position(2), Point::new(30.0, 10.0));

    // Around an inline box with a 10px start edge and a 5px end edge: before
    // its start is before the edge, inside it after the edge, before its end
    // before the end edge.
    let boxed = committed(
        &[
            run(&style, "aa"),
            open(&style, 10.0, 5.0),
            run(&style, "bb"),
            CLOSE,
            run(&style, "c"),
        ],
        None,
    );
    let at = |item| boxed.static_position(item).x;
    assert_eq!(
        (at(1), at(2), at(3), at(4), at(5)),
        (20.0, 30.0, 50.0, 55.0, 65.0)
    );

    // Before an atom is before it.
    let with = with_atom(VerticalAlign::Baseline);
    assert_eq!(with.static_position(1).x, 10.0);
    assert_eq!(with.static_position(2).x, 30.0);

    // Right-to-left content: אא fills 80..100; before it is its right edge,
    // after it its left.
    let rtl = committed_in(
        Direction::Rtl,
        &[run(&style, "\u{5d0}\u{5d0}")],
        Some(100.0),
    );
    assert_eq!(rtl.static_position(0), Point::new(100.0, 0.0));
    assert_eq!(rtl.static_position(1), Point::new(80.0, 0.0));

    // No line box: the origin.
    let empty = committed(&[], Some(100.0));
    assert_eq!(empty.static_position(0), Point::new(0.0, 0.0));
}

/// "a", an atom, "b", the atom 20px square with no baseline of its own.
fn with_atom(vertical_align: VerticalAlign) -> InlineFlow {
    let style = ahem();
    let items = [
        run(&style, "a"),
        InlineFlowItem::Atom(InlineAtom { vertical_align }),
        run(&style, "b"),
    ];
    let mut context = text_context();
    let mut flow = InlineFlow::new(&mut context, container(Direction::Ltr), &items);
    flow.set_atom_size(1, Size::new(20.0, 20.0), None);
    flow.commit(&mut context, BlockConstraint::new(None, 0.0));
    flow
}

fn placed(flow: &InlineFlow) -> (u64, Point<f32>, Size<f32>) {
    match flow.atoms() {
        [
            PlacedBox::Visible {
                id, origin, size, ..
            },
        ] => (*id, *origin, *size),
        other => panic!("one placed atom expected, got {other:?}"),
    }
}

#[test]
fn an_atom_is_placed_by_its_margin_box_and_vertical_align() {
    // `baseline` with no baseline of its own: the bottom margin edge on the
    // line's baseline, 20px above it, the strut's 2px below.
    let baseline = with_atom(VerticalAlign::Baseline);
    let line = baseline.line_geometry()[0];
    assert_eq!((line.height, line.baseline), (22.0, 20.0));
    assert_eq!(
        placed(&baseline),
        (1, Point::new(10.0, 0.0), Size::new(20.0, 20.0))
    );
    assert_eq!(runs_of(&baseline, 2)[0].1, 30.0);

    // A length raises it: 5px up moves the line's baseline 5px down.
    let raised = with_atom(VerticalAlign::Length(5.0));
    let line = raised.line_geometry()[0];
    assert_eq!((line.height, line.baseline), (27.0, 25.0));
    assert_eq!(placed(&raised).1, Point::new(10.0, 0.0));

    // A percentage is one of the root inline box's 10px line height.
    let percent = with_atom(VerticalAlign::Percent(0.5));
    assert_eq!(percent.line_geometry()[0], raised.line_geometry()[0]);

    // `top` puts it against the line box's top, which the strut shares.
    let top = with_atom(VerticalAlign::Top);
    let line = top.line_geometry()[0];
    assert_eq!((line.height, line.baseline), (20.0, 8.0));
    assert_eq!(placed(&top).1, Point::new(10.0, 0.0));
}

#[test]
fn an_edge_beside_an_atom_is_part_of_its_advance() {
    let style = ahem();
    // a <span>[atom]</span> b: the start edge sits before the atom, the
    // end edge after it, and both move nothing but the atom and "b".
    let items = [
        run(&style, "a"),
        open(&style, 5.0, 7.0),
        InlineFlowItem::Atom(InlineAtom {
            vertical_align: VerticalAlign::Baseline,
        }),
        CLOSE,
        run(&style, "b"),
    ];
    let mut context = text_context();
    let mut flow = InlineFlow::new(&mut context, container(Direction::Ltr), &items);
    flow.set_atom_size(2, Size::new(20.0, 20.0), None);
    flow.commit(&mut context, BlockConstraint::new(None, 0.0));
    assert_eq!(placed(&flow).1, Point::new(15.0, 0.0));
    assert_eq!(runs_of(&flow, 4)[0].1, 42.0);
    // The box's fragment holds the atom and its own edges, and its content
    // area sits on the line's baseline, 20px down.
    let [fragment] = flow.fragments() else {
        panic!("one fragment expected")
    };
    assert_eq!(span(fragment), (10.0, 42.0));
    assert_eq!(fragment.rect.origin.y, 12.0);
    // Its min-content width keeps the atom and its edges together.
    assert_eq!(flow.min_content_width(&mut context, 0.0), 32.0);
}

#[test]
fn range_queries_select_content_and_never_an_edge() {
    let style = ahem();
    // aa <span>bb</span> cc, 5px edges: aa 0–20, start edge 20–25, bb 25–45,
    // end edge 45–50, cc 50–70.
    let items = [
        run(&style, "aa"),
        open(&style, 5.0, 5.0),
        run(&style, "bb"),
        CLOSE,
        run(&style, "cc"),
    ];
    let flow = committed(&items, None);
    assert_eq!(
        flow.range_boxes(2, 4),
        Some(vec![Rect::new(
            Point::new(25.0, 0.0),
            Size::new(20.0, 10.0)
        )])
    );
    assert_eq!(
        flow.range_boxes(0, 6),
        Some(vec![Rect::new(Point::new(0.0, 0.0), Size::new(70.0, 10.0))])
    );
    assert_eq!(flow.index_at_point(Point::new(22.0, 5.0)), None);
    assert_eq!(flow.index_at_point(Point::new(30.0, 5.0)), Some(2));
    assert_eq!(flow.index_at_point(Point::new(47.0, 5.0)), None);
    assert_eq!(flow.index_at_point(Point::new(55.0, 5.0)), Some(4));
    assert_eq!(flow.item_source_range(1), Some(2..2));
    assert_eq!(flow.item_source_range(2), Some(2..4));
    // The edges count toward the intrinsic widths.
    let mut context = text_context();
    let mut flow = InlineFlow::new(&mut context, container(Direction::Ltr), &items);
    assert_eq!(flow.max_content_width(&mut context, 0.0), 70.0);
    assert_eq!(flow.min_content_width(&mut context, 0.0), 70.0);
}

fn styled<'src>(
    style: &'src RunStyle,
    text: &'src str,
    white_space: WhiteSpaceCollapse,
) -> InlineFlowItem<'src> {
    InlineFlowItem::Run(InlineRun {
        text,
        style,
        white_space,
        breaking: BreakStyle::default(),
    })
}

/// Awkward combinations — edges around preserved breaks and tabs, boxes of
/// collapsible white space only, empty boxes nested in each other, atoms
/// carrying edges on both sides, a box spanning a forced break, every one in
/// both directions — laid out at several widths, with every range and a
/// grid of points asked about. The answers are not checked; that the source
/// map and the fragments hold together everywhere is.
#[test]
fn awkward_boxes_lay_out_and_answer_every_query() {
    let style = ahem();
    let atom = || {
        InlineFlowItem::Atom(InlineAtom {
            vertical_align: VerticalAlign::Middle,
        })
    };
    let pre = |text| styled(&style, text, WhiteSpaceCollapse::Preserve);
    let cases: Vec<Vec<InlineFlowItem<'_>>> = vec![
        vec![
            open(&style, 5.0, 5.0),
            pre("\nfoo"),
            CLOSE,
            run(&style, "bar"),
        ],
        vec![
            run(&style, "a"),
            open(&style, 5.0, 5.0),
            run(&style, "   "),
            CLOSE,
            run(&style, " b"),
        ],
        vec![
            open(&style, 2.0, 3.0),
            open(&style, 4.0, 5.0),
            CLOSE,
            CLOSE,
            run(&style, "x"),
        ],
        vec![
            open(&style, 5.0, 7.0),
            atom(),
            CLOSE,
            open(&style, 1.0, 2.0),
            atom(),
            run(&style, "y"),
            atom(),
            CLOSE,
        ],
        vec![open(&style, 5.0, 5.0), pre("a\tb\tc"), CLOSE],
        vec![
            run(&style, "aa"),
            open(&style, 3.0, 3.0),
            pre("b\nc"),
            CLOSE,
            run(&style, "d"),
        ],
        vec![
            run(&style, "aa "),
            open(&style, 3.0, 3.0),
            run(&style, "b "),
            CLOSE,
            open(&style, 3.0, 3.0),
            run(&style, " c"),
            CLOSE,
        ],
        vec![
            open(&style, 3.0, 3.0),
            run(&style, "\u{5d0} a \u{5d1}"),
            CLOSE,
            run(&style, " \u{4e2d}\u{4e2d}"),
        ],
        vec![open(&style, 3.0, 3.0), CLOSE],
        vec![open(&style, 3.0, 3.0)],
    ];
    for direction in [Direction::Ltr, Direction::Rtl] {
        for items in &cases {
            for width in [None, Some(0.0), Some(25.0), Some(45.0)] {
                ask_everything(direction, items, width);
            }
        }
    }
}

/// Lays `items` out at `width` with every atom 12×14 and asks the flow every
/// range of its source and a grid of points over its size.
fn ask_everything(direction: Direction, items: &[InlineFlowItem<'_>], width: Option<f32>) {
    let mut context = text_context();
    let mut flow = InlineFlow::new(&mut context, container(direction), items);
    for (index, item) in items.iter().enumerate() {
        if matches!(item, InlineFlowItem::Atom(_)) {
            let item = u32::try_from(index).expect("fits");
            flow.set_atom_size(item, Size::new(12.0, 14.0), Some(10.0));
        }
    }
    flow.commit(&mut context, BlockConstraint::new(width, 0.0));
    let units = (0..u32::try_from(items.len()).expect("fits"))
        .filter_map(|item| flow.item_source_range(item))
        .map(|range| range.end)
        .max()
        .unwrap_or(0);
    for start in 0..=units {
        for end in start..=units {
            assert!(flow.range_boxes(start, end).is_some());
        }
    }
    let size = flow.size();
    for x in 0..=10_u8 {
        for y in 0..=10_u8 {
            let point = Point::new(
                size.width * f32::from(x) / 10.0,
                size.height * f32::from(y) / 10.0,
            );
            let _ = flow.index_at_point(point);
        }
    }
    for fragment in flow.fragments() {
        assert!(fragment.rect.size.width >= 0.0, "{fragment:?}");
        assert!((fragment.line as usize) < flow.line_geometry().len());
    }
    let _ = flow.min_content_width(&mut context, 0.0);
}

/// An atom's size is the host's to change between a probe and a commit — a
/// CSS inline formatting context shrinks an atom to the width it lays the
/// line out at — so putting the committed layout back puts the committed
/// sizes back with it.
#[test]
fn restoring_the_committed_layout_restores_the_committed_atom_sizes() {
    let style = ahem();
    let items = [
        run(&style, "a"),
        InlineFlowItem::Atom(InlineAtom {
            vertical_align: VerticalAlign::Baseline,
        }),
        run(&style, "b"),
    ];
    let mut context = text_context();
    let mut flow = InlineFlow::new(&mut context, container(Direction::Ltr), &items);
    flow.set_atom_size(1, Size::new(40.0, 20.0), None);
    flow.commit(&mut context, BlockConstraint::new(Some(100.0), 0.0));
    assert_eq!(runs_of(&flow, 2)[0].1, 50.0);

    // A min-content probe with the atom at its min-content size: "a", the
    // 30px atom over the strut's 2px descent, and "b", one line each.
    flow.set_atom_size(1, Size::new(15.0, 30.0), None);
    let probed = flow.probe(&mut context, BlockConstraint::new(Some(0.0), 0.0));
    assert_eq!(probed.size.height, 52.0);
    assert!(flow.is_probe_dirty());

    assert!(flow.restore_committed(&mut context));
    assert!(!flow.is_probe_dirty());
    assert_eq!(
        placed(&flow),
        (1, Point::new(10.0, 0.0), Size::new(40.0, 20.0))
    );
    assert_eq!(runs_of(&flow, 2)[0].1, 50.0);
    assert_eq!(flow.size().height, 22.0);
    // Nothing to put back the second time.
    assert!(!flow.restore_committed(&mut context));
}

/// The flow's intrinsic widths at `indent` with its atoms at `atoms`'
/// widths, how many breaks finding them took, and parley's one-pass content
/// widths of the same content at the same indent, read off a layout of it.
fn intrinsic_widths(
    style: InlineFlowStyle,
    items: &[InlineFlowItem<'_>],
    atoms: &[(u32, f32)],
    indent: f32,
) -> ((f32, f32), u32, (f32, f32)) {
    let mut context = text_context();
    let mut flow = InlineFlow::new(&mut context, style, items);
    for &(item, width) in atoms {
        flow.set_atom_size(item, Size::new(width, 10.0), None);
    }
    let intrinsic = (
        flow.min_content_width(&mut context, indent),
        flow.max_content_width(&mut context, indent),
    );
    let breaks = flow.break_count();
    flow.commit(&mut context, BlockConstraint::new(None, indent));
    let one_pass = flow.display().calculate_content_widths();
    (intrinsic, breaks, (one_pass.min, one_pass.max))
}

const ATOM: InlineFlowItem<'static> = InlineFlowItem::Atom(InlineAtom {
    vertical_align: VerticalAlign::Baseline,
});

#[test]
fn a_hanging_indent_keeps_the_intrinsic_widths_of_two_breaks() {
    // The break at zero width ends the atom's line with a soft wrap, and the
    // empty line after it takes the hanging indent; the one pass has no
    // line there.
    let style = InlineFlowStyle {
        indent: IndentKeywords {
            each_line: false,
            hanging: true,
        },
        ..container(Direction::Ltr)
    };
    let (intrinsic, breaks, one_pass) = intrinsic_widths(style, &[ATOM], &[(0, 5.0)], 10.0);
    assert_eq!(intrinsic, (10.0, 10.0));
    assert_eq!(one_pass, (5.0, 5.0), "parley's one pass");
    assert_eq!(breaks, 2);
}

#[test]
fn overflow_wrap_anywhere_keeps_the_intrinsic_widths_of_two_breaks() {
    // A space that is `nowrap` and `anywhere` before an atom: the one pass
    // breaks before the space, as `anywhere` allows; the break at zero width
    // keeps one line, since the space fits by hanging and the atom after it
    // follows a `nowrap` run.
    let style = ahem();
    let items = [
        run(&style, "bbb"),
        InlineFlowItem::Run(InlineRun {
            text: " ",
            style: &style,
            white_space: WhiteSpaceCollapse::Collapse,
            breaking: BreakStyle {
                text_wrap: TextWrap::NoWrap,
                overflow_wrap: OverflowWrap::Anywhere,
                ..BreakStyle::default()
            },
        }),
        ATOM,
    ];
    let (intrinsic, breaks, one_pass) =
        intrinsic_widths(container(Direction::Ltr), &items, &[(2, 5.0)], 0.0);
    assert_eq!(intrinsic, (45.0, 45.0));
    assert_eq!(one_pass, (30.0, 45.0), "parley's one pass");
    assert_eq!(breaks, 2);
}

#[test]
fn a_negative_advance_keeps_the_intrinsic_widths_of_two_breaks() {
    // An atom narrower than nothing, and an inline box start edge of −35px:
    // the one pass skips a wrap opportunity while its running width is
    // negative.
    let (intrinsic, breaks, one_pass) = intrinsic_widths(
        container(Direction::Ltr),
        &[ATOM, ATOM],
        &[(0, -5.0), (1, 25.0)],
        0.0,
    );
    assert_eq!(intrinsic, (25.0, 25.0));
    assert_eq!(one_pass, (20.0, 20.0), "parley's one pass");
    assert_eq!(breaks, 2);

    let style = ahem();
    let items = [
        open_in(
            &style,
            Direction::Ltr,
            InlineEdges {
                start_margin: -35.0,
                ..InlineEdges::default()
            },
        ),
        run(&style, "aa bb"),
        CLOSE,
    ];
    let (intrinsic, breaks, one_pass) =
        intrinsic_widths(container(Direction::Ltr), &items, &[], 0.0);
    assert_eq!(intrinsic, (20.0, 20.0));
    assert_eq!(one_pass, (15.0, 15.0), "parley's one pass");
    assert_eq!(breaks, 2);
}

/// An inline box's edges are text spliced into the flow with no source
/// unit: an edge byte reads as the unit after it, and every unit lands on
/// its character, past the edge.
#[test]
fn an_edge_byte_has_no_unit_of_its_own() {
    let style = ahem();
    let items = [
        run(&style, "aa"),
        open(&style, 4.0, 4.0),
        run(&style, "bb"),
        CLOSE,
        run(&style, "cc"),
    ];
    let flow = committed(&items, None);
    let map = flow.offset_map();
    let text = map.display_text();
    assert!(text.len() > 6, "edges are spliced in: {text:?}");
    assert_eq!(map.source_len(), 6);
    let b = map.unit_to_byte(2).unwrap();
    assert!(text[b..].starts_with("bb"), "{text:?} at {b}");
    let c = map.unit_to_byte(4).unwrap();
    assert!(text[c..].starts_with("cc"), "{text:?} at {c}");
    // The start edge lies between `aa` and `bb`.
    assert!(b > 2);
    assert_eq!(map.byte_to_unit(2), 2, "the start edge's byte");
    assert_eq!(map.byte_to_unit(b - 1), 2);
    assert_eq!(map.unit_before(2), 2);
    assert_eq!(map.unit_to_byte_or_end(6), Some(text.len()));
}
