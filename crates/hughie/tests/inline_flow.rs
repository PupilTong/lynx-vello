//! The CSS inline formatting context of a block container holding text runs
//! only (`hughie::text::inline`), on Ahem geometry.
//!
//! Ahem's em square is exact: every glyph advances one font size, the ascent
//! is 0.8em, the descent 0.2em and the line gap zero, so `line-height:
//! normal` is 1em with the baseline 0.8em down.

use hughie::text::inline::{InlineFlow, InlineFlowItem, InlineFlowStyle, InlineRun};
use hughie::text::paragraph::{
    BlockConstraint, BreakStyle, Direction, IndentKeywords, LineGeometry, LineHeight, OverflowWrap,
    RunStyle, TextAlign, TextSecurity, TextWrap, WhiteSpaceCollapse, WordBreak,
};
use hughie::text::{FontBlob, TextContext};
use parley::PositionedLayoutItem;
use stylo::Atom;
use stylo::values::computed::font::{
    FamilyName, FontFamily, FontFamilyList, FontFamilyNameSyntax, SingleFontFamily,
};

const AHEM: &[u8] = include_bytes!("fixtures/Ahem.ttf");
const EPSILON: f32 = 0.01;

fn assert_close(actual: f32, expected: f32) {
    assert!(
        (actual - expected).abs() <= EPSILON,
        "expected {expected}, got {actual}"
    );
}

fn assert_line(line: LineGeometry, top: f32, height: f32, baseline: f32) {
    assert_close(line.top, top);
    assert_close(line.height, height);
    assert_close(line.baseline, baseline);
}

fn text_context() -> TextContext {
    let mut context = TextContext::without_system_fonts();
    assert_eq!(context.register_fonts(FontBlob::from_static(AHEM)), 1);
    context
}

fn ahem(font_size: f32, line_height: LineHeight) -> RunStyle {
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
        font_size,
        line_height,
        ..RunStyle::default()
    }
}

fn styled<'src>(
    style: &'src RunStyle,
    text: &'src str,
    white_space: WhiteSpaceCollapse,
    breaking: BreakStyle,
) -> InlineFlowItem<'src> {
    InlineFlowItem::Run(InlineRun {
        text,
        style,
        white_space,
        text_security: TextSecurity::None,
        breaking,
    })
}

fn run<'src>(style: &'src RunStyle, text: &'src str) -> InlineFlowItem<'src> {
    styled(
        style,
        text,
        WhiteSpaceCollapse::Collapse,
        BreakStyle::default(),
    )
}

fn container(strut: RunStyle) -> InlineFlowStyle {
    InlineFlowStyle {
        strut,
        ..InlineFlowStyle::default()
    }
}

/// A flow committed at `width` with no indent.
fn committed(
    style: InlineFlowStyle,
    runs: &[InlineFlowItem<'_>],
    width: Option<f32>,
) -> InlineFlow {
    let mut context = text_context();
    let mut flow = InlineFlow::new(&mut context, style, runs);
    flow.commit(&mut context, BlockConstraint::new(width, 0.0));
    flow
}

/// The x of every line's first glyph run, in line order.
fn line_starts(flow: &InlineFlow) -> Vec<f32> {
    flow.display()
        .lines()
        .map(|line| {
            line.items()
                .find_map(|item| match item {
                    PositionedLayoutItem::GlyphRun(glyph_run) => Some(glyph_run.offset()),
                    PositionedLayoutItem::InlineBox(_) => None,
                })
                .unwrap_or(f32::NAN)
        })
        .collect()
}

#[test]
fn the_strut_holds_a_line_of_smaller_text_at_the_containers_line_height() {
    // CSS 2.2 §10.8.1: the strut — the container's 20px Ahem at `normal` —
    // starts every line box, so 10px text sits on a 20px line with the
    // container's baseline. Parley builds that line box itself, and draws
    // the glyphs on its baseline.
    let small = ahem(10.0, LineHeight::Normal);
    let flow = committed(
        container(ahem(20.0, LineHeight::Normal)),
        &[run(&small, "aa")],
        None,
    );
    let [line] = flow.line_geometry() else {
        panic!("one line expected, got {:?}", flow.line_geometry())
    };
    assert_line(*line, 0.0, 20.0, 16.0);
    let glyph_baseline = flow
        .display()
        .lines()
        .next()
        .and_then(|line| {
            line.items().find_map(|item| match item {
                PositionedLayoutItem::GlyphRun(glyph_run) => Some(glyph_run.baseline()),
                PositionedLayoutItem::InlineBox(_) => None,
            })
        })
        .expect("a glyph run");
    assert_close(glyph_baseline, 16.0);
    assert_close(flow.size().height, 20.0);
    assert_close(flow.size().width, 20.0);
    assert_eq!(flow.first_baseline(), Some(16.0));
}

#[test]
fn each_run_adds_its_own_half_leading_around_the_shared_baseline() {
    // Strut: 10px at 10px → 8 above, 2 below.
    // "aa": 10px at 40px → leading 30 → 23 above, 17 below.
    // "b": 30px at 30px → no leading → 24 above, 6 below.
    // The line box is the highest top to the lowest bottom: 24 + 17 = 41,
    // more than any one run's line-height — the case parley's one leading
    // per line cannot produce.
    let tall_leading = ahem(10.0, LineHeight::Px(40.0));
    let big = ahem(30.0, LineHeight::Px(30.0));
    let flow = committed(
        container(ahem(10.0, LineHeight::Px(10.0))),
        &[run(&tall_leading, "aa"), run(&big, "b")],
        None,
    );
    let [line] = flow.line_geometry() else {
        panic!("one line expected")
    };
    assert_line(*line, 0.0, 41.0, 24.0);
    assert_close(flow.size().height, 41.0);
}

#[test]
fn line_height_normal_and_numbers_resolve_against_each_runs_own_font() {
    // `normal` is ascent + descent + line gap of the first available font:
    // 1em for Ahem. A number multiplies the run's own font size.
    let normal = committed(
        container(ahem(20.0, LineHeight::Normal)),
        &[run(&ahem(20.0, LineHeight::Normal), "a")],
        None,
    );
    assert_line(normal.line_geometry()[0], 0.0, 20.0, 16.0);

    let number = committed(
        container(ahem(10.0, LineHeight::Number(2.0))),
        &[run(&ahem(10.0, LineHeight::Number(2.0)), "a")],
        None,
    );
    // 20px line, 10px of leading split 5/5 around the 8/2 content area.
    assert_line(number.line_geometry()[0], 0.0, 20.0, 13.0);
}

#[test]
fn lines_stack_by_their_own_heights() {
    let style = ahem(10.0, LineHeight::Normal);
    let tall = ahem(10.0, LineHeight::Px(30.0));
    let flow = committed(
        container(ahem(10.0, LineHeight::Normal)),
        &[styled(
            &style,
            "aa\n",
            WhiteSpaceCollapse::PreserveBreaks,
            BreakStyle::default(),
        )],
        None,
    );
    assert_eq!(
        flow.line_geometry().len(),
        1,
        "a trailing break adds no line"
    );

    let flow = committed(
        container(ahem(10.0, LineHeight::Normal)),
        &[
            styled(
                &style,
                "aa\n",
                WhiteSpaceCollapse::PreserveBreaks,
                BreakStyle::default(),
            ),
            run(&tall, "bb"),
        ],
        None,
    );
    let lines = flow.line_geometry();
    assert_eq!(lines.len(), 2);
    assert_line(lines[0], 0.0, 10.0, 8.0);
    // 30px line-height around the 8/2 content area: 18 above, 12 below.
    assert_line(lines[1], 10.0, 30.0, 28.0);
    assert_close(flow.size().height, 40.0);
}

#[test]
fn collapsible_whitespace_is_removed_at_the_start_of_the_paragraph_and_after_a_break() {
    let style = ahem(10.0, LineHeight::Normal);
    // css-text-3 §4.1.2 rule 1. A Lynx `<text>` keeps the whole run
    // verbatim.
    let flow = committed(
        container(style.clone()),
        &[run(&style, "  \n  aa bb")],
        None,
    );
    assert_close(flow.size().width, 50.0);
    assert_eq!(line_starts(&flow), [0.0]);

    let flow = committed(
        container(style.clone()),
        &[styled(
            &style,
            "aa\n   bb",
            WhiteSpaceCollapse::PreserveBreaks,
            BreakStyle::default(),
        )],
        None,
    );
    assert_eq!(line_starts(&flow), [0.0, 0.0]);
    assert_close(flow.size().width, 20.0);
}

#[test]
fn whitespace_collapses_across_runs() {
    let style = ahem(10.0, LineHeight::Normal);
    let flow = committed(
        container(style.clone()),
        &[run(&style, "aa  "), run(&style, "  bb")],
        None,
    );
    assert_close(flow.size().width, 50.0);
}

#[test]
fn pre_preserve_breaks_and_nowrap_keep_what_they_name() {
    let style = ahem(10.0, LineHeight::Normal);
    let pre = committed(
        container(style.clone()),
        &[styled(
            &style,
            "a  b\nc",
            WhiteSpaceCollapse::Preserve,
            BreakStyle::default(),
        )],
        None,
    );
    assert_eq!(pre.line_geometry().len(), 2);
    assert_close(pre.size().width, 40.0);

    let pre_line = committed(
        container(style.clone()),
        &[styled(
            &style,
            "a  b\nc",
            WhiteSpaceCollapse::PreserveBreaks,
            BreakStyle::default(),
        )],
        None,
    );
    assert_eq!(pre_line.line_geometry().len(), 2);
    assert_close(pre_line.size().width, 30.0);

    let nowrap = BreakStyle {
        text_wrap: TextWrap::NoWrap,
        ..BreakStyle::default()
    };
    let flow = committed(
        container(style.clone()),
        &[styled(
            &style,
            "aaaa bbbb",
            WhiteSpaceCollapse::Collapse,
            nowrap,
        )],
        Some(50.0),
    );
    assert_eq!(flow.line_geometry().len(), 1);
    // Min-content is max-content under `nowrap`, so the fit-content width
    // is the whole line, overflowing the 50px offered.
    assert_close(flow.size().width, 90.0);
}

#[test]
fn a_definite_width_reports_the_fit_content_width_not_the_widest_line() {
    let style = ahem(10.0, LineHeight::Normal);
    let mut context = text_context();
    let mut flow = InlineFlow::new(
        &mut context,
        container(style.clone()),
        &[run(&style, "aaaa bbbb")],
    );
    // max-content 90, min-content 40: at 60 the box is 60 wide although its
    // widest line is 40 (css-sizing-3 fit-content size).
    let metrics = flow.probe(&mut context, BlockConstraint::new(Some(60.0), 0.0));
    assert_close(metrics.size.width, 60.0);
    assert_close(metrics.size.height, 20.0);
    assert_close(flow.min_content_width(&mut context, 0.0), 40.0);
    assert_close(flow.max_content_width(&mut context, 0.0), 90.0);
    // Wider than max-content, the box shrinks to it.
    let wide = flow.probe(&mut context, BlockConstraint::new(Some(200.0), 0.0));
    assert_close(wide.size.width, 90.0);

    // Laid out at that width, lines align within it.
    for (text_align, start) in [(TextAlign::Center, 10.0), (TextAlign::End, 20.0)] {
        let flow = committed(
            InlineFlowStyle {
                text_align,
                ..container(style.clone())
            },
            &[run(&style, "aaaa bbbb")],
            Some(60.0),
        );
        assert_eq!(line_starts(&flow), [start, start], "{text_align:?}");
    }
}

#[test]
fn intrinsic_widths_are_the_narrowest_and_widest_lines() {
    let style = ahem(10.0, LineHeight::Normal);
    let mut context = text_context();
    let mut flow = InlineFlow::new(
        &mut context,
        container(style.clone()),
        &[run(&style, "aaaa bb")],
    );
    assert_close(flow.min_content_width(&mut context, 0.0), 40.0);
    assert_close(flow.max_content_width(&mut context, 0.0), 70.0);
    let breaks = flow.break_count();
    assert_close(flow.min_content_width(&mut context, 0.0), 40.0);
    assert_eq!(flow.break_count(), breaks, "the widths are remembered");
}

#[test]
fn text_indent_counts_toward_the_intrinsic_widths() {
    let style = ahem(10.0, LineHeight::Normal);
    let mut context = text_context();
    let mut flow = InlineFlow::new(
        &mut context,
        container(style.clone()),
        &[run(&style, "aaaa bb")],
    );
    // The first line is indented: "aaaa" + 10 at min-content, the whole
    // line + 10 at max-content.
    assert_close(flow.min_content_width(&mut context, 10.0), 50.0);
    assert_close(flow.max_content_width(&mut context, 10.0), 80.0);

    let mut hanging = InlineFlow::new(
        &mut context,
        InlineFlowStyle {
            indent: IndentKeywords {
                each_line: false,
                hanging: true,
            },
            ..container(style.clone())
        },
        &[run(&style, "aaaa bb")],
    );
    // `hanging`: every line but the first — "bb" + 10 — is indented.
    assert_close(hanging.min_content_width(&mut context, 10.0), 40.0);
    assert_close(hanging.max_content_width(&mut context, 10.0), 70.0);
}

#[test]
fn text_indent_keywords_choose_the_indented_lines() {
    let style = ahem(10.0, LineHeight::Normal);
    let lines = |indent: IndentKeywords, white_space, text, width| {
        let mut context = text_context();
        let mut flow = InlineFlow::new(
            &mut context,
            InlineFlowStyle {
                indent,
                ..container(style.clone())
            },
            &[styled(&style, text, white_space, BreakStyle::default())],
        );
        flow.commit(&mut context, BlockConstraint::new(width, 10.0));
        line_starts(&flow)
    };
    let collapse = WhiteSpaceCollapse::Collapse;
    let breaks = WhiteSpaceCollapse::PreserveBreaks;
    let first = IndentKeywords::default();
    let each_line = IndentKeywords {
        each_line: true,
        hanging: false,
    };
    let hanging = IndentKeywords {
        each_line: false,
        hanging: true,
    };
    assert_eq!(lines(first, breaks, "aa\nbb", None), [10.0, 0.0]);
    assert_eq!(lines(each_line, breaks, "aa\nbb", None), [10.0, 10.0]);
    assert_eq!(lines(hanging, collapse, "aa bb", Some(30.0)), [0.0, 10.0]);
}

#[test]
fn a_tab_stop_counts_the_indent_of_the_line_the_tab_is_on() {
    // css-text-3 §4.1.2: stops are measured from the content edge. With
    // `each-line` the second line starts 30px in, so the tab at x 30 runs to
    // the stop at 80 — a 50px tab, not the 80px one a tab at the line start
    // would take.
    let style = ahem(10.0, LineHeight::Normal);
    let mut context = text_context();
    let mut flow = InlineFlow::new(
        &mut context,
        InlineFlowStyle {
            indent: IndentKeywords {
                each_line: true,
                hanging: false,
            },
            ..container(style.clone())
        },
        &[styled(
            &style,
            "a\n\tb",
            WhiteSpaceCollapse::Preserve,
            BreakStyle::default(),
        )],
    );
    flow.commit(&mut context, BlockConstraint::new(None, 30.0));
    let second = flow.display().get(1).expect("two lines");
    assert_close(second.metrics().advance, 60.0);
}

#[test]
fn overflow_wrap_normal_lets_a_long_word_overflow() {
    // `overflow-wrap` is not a Lynx property, so every element computes the
    // initial `normal`: a word wider than the line overflows it.
    let style = ahem(10.0, LineHeight::Normal);
    let normal = committed(
        container(style.clone()),
        &[run(&style, "aaaaaaaaaa")],
        Some(50.0),
    );
    assert_eq!(normal.line_geometry().len(), 1);
    assert_close(normal.size().width, 100.0);

    let break_word = BreakStyle {
        overflow_wrap: OverflowWrap::BreakWord,
        ..BreakStyle::default()
    };
    let broken = committed(
        container(style.clone()),
        &[styled(
            &style,
            "aaaaaaaaaa",
            WhiteSpaceCollapse::Collapse,
            break_word,
        )],
        Some(50.0),
    );
    assert_eq!(broken.line_geometry().len(), 2);
}

#[test]
fn line_breaking_values_apply_per_run() {
    let style = ahem(10.0, LineHeight::Normal);
    let nowrap = BreakStyle {
        text_wrap: TextWrap::NoWrap,
        ..BreakStyle::default()
    };
    // The nowrap run keeps "aaa bbb" on one line; the wrapping run after it
    // still breaks.
    let flow = committed(
        container(style.clone()),
        &[
            styled(&style, "aaa bbb", WhiteSpaceCollapse::Collapse, nowrap),
            run(&style, " ccc ddd"),
        ],
        Some(50.0),
    );
    assert_eq!(flow.line_geometry().len(), 3);

    let break_all = BreakStyle {
        word_break: WordBreak::BreakAll,
        ..BreakStyle::default()
    };
    // A `break-all` run breaks between its letters; the normal run after it
    // keeps its word whole.
    let flow = committed(
        container(style.clone()),
        &[
            styled(&style, "aaaa", WhiteSpaceCollapse::Collapse, break_all),
            run(&style, " bbbb"),
        ],
        Some(25.0),
    );
    assert_eq!(flow.line_geometry().len(), 3);
}

#[test]
fn empty_and_whitespace_only_content_has_no_line_box() {
    let style = ahem(10.0, LineHeight::Normal);
    for runs in [vec![], vec![run(&style, "  \n\t ")]] {
        let flow = committed(container(style.clone()), &runs, Some(100.0));
        assert!(flow.line_geometry().is_empty());
        assert_close(flow.size().height, 0.0);
        assert_close(flow.size().width, 0.0);
        assert_eq!(flow.first_baseline(), None);
    }
}

#[test]
fn a_probe_never_moves_the_committed_lines_and_a_restore_puts_them_back() {
    let style = ahem(10.0, LineHeight::Normal);
    let mut context = text_context();
    let mut flow = InlineFlow::new(
        &mut context,
        container(style.clone()),
        &[run(&style, "aaaa bbbb")],
    );
    flow.commit(&mut context, BlockConstraint::new(Some(60.0), 0.0));
    assert_eq!(flow.line_geometry().len(), 2);
    flow.probe(&mut context, BlockConstraint::new(None, 0.0));
    assert!(flow.is_probe_dirty());
    assert!(flow.restore_committed(&mut context));
    assert!(!flow.is_probe_dirty());
    assert_eq!(flow.line_geometry().len(), 2);
    let breaks = flow.break_count();
    flow.probe(&mut context, BlockConstraint::new(Some(60.0), 0.0));
    assert_eq!(flow.break_count(), breaks, "a remembered constraint");
}

#[test]
fn a_new_alignment_applies_to_the_same_flow() {
    let style = ahem(10.0, LineHeight::Normal);
    let mut context = text_context();
    let mut flow = InlineFlow::new(
        &mut context,
        container(style.clone()),
        &[run(&style, "aaaa bbbb")],
    );
    flow.commit(&mut context, BlockConstraint::new(Some(60.0), 0.0));
    assert_eq!(line_starts(&flow), [0.0, 0.0]);
    flow.set_style(InlineFlowStyle {
        text_align: TextAlign::Center,
        ..container(style.clone())
    });
    flow.commit(&mut context, BlockConstraint::new(Some(60.0), 0.0));
    assert_eq!(line_starts(&flow), [10.0, 10.0]);
}

/// Each line's text, as the byte range parley laid on it.
fn line_ranges(flow: &InlineFlow) -> Vec<core::ops::Range<usize>> {
    flow.display()
        .lines()
        .map(|line| line.text_range())
        .collect()
}

/// The inline offset and advance of the glyph run whose text holds `byte`.
fn run_at(flow: &InlineFlow, byte: usize) -> (f32, f32) {
    flow.display()
        .lines()
        .flat_map(|line| line.items().collect::<Vec<_>>())
        .find_map(|item| match item {
            PositionedLayoutItem::GlyphRun(glyph_run)
                if glyph_run.run().text_range().contains(&byte) =>
            {
                Some((glyph_run.offset(), glyph_run.advance()))
            }
            _ => None,
        })
        .unwrap_or_else(|| panic!("no glyph run holds byte {byte}"))
}

/// css-text-3 §4.1.3 and §5.1: under `break-spaces` a wrap opportunity
/// follows every preserved space, and none of them hangs, so the spaces a line
/// cannot hold wrap onto the next line like content. Under `pre-wrap`
/// (`preserve`) the whole sequence hangs at the line end instead.
#[test]
fn break_spaces_wraps_after_each_space_and_hangs_none() {
    let style = ahem(10.0, LineHeight::Normal);
    let at = |white_space, width| {
        committed(
            container(style.clone()),
            &[styled(
                &style,
                "aaa   bbb",
                white_space,
                BreakStyle::default(),
            )],
            Some(width),
        )
    };
    // 50px holds "aaa" and two spaces; the third starts the next line.
    let break_spaces = at(WhiteSpaceCollapse::BreakSpaces, 50.0);
    assert_eq!(line_ranges(&break_spaces), [0..5, 5..9]);
    assert_eq!(run_at(&break_spaces, 5), (0.0, 40.0));
    // `pre-wrap` hangs all three spaces after "aaa".
    let pre_wrap = at(WhiteSpaceCollapse::Preserve, 50.0);
    assert_eq!(line_ranges(&pre_wrap), [0..6, 6..9]);

    // Trailing spaces take width: a 30px line holds "ab " and wraps the two
    // spaces after it, where `pre-wrap` hangs them on one line.
    let trailing = |white_space| {
        committed(
            container(style.clone()),
            &[styled(&style, "ab   ", white_space, BreakStyle::default())],
            Some(30.0),
        )
    };
    let break_spaces = trailing(WhiteSpaceCollapse::BreakSpaces);
    assert_eq!(line_ranges(&break_spaces), [0..3, 3..5]);
    assert_close(break_spaces.size().height, 20.0);
    let pre_wrap = trailing(WhiteSpaceCollapse::Preserve);
    assert_eq!(line_ranges(&pre_wrap).len(), 1);
    assert_eq!(line_ranges(&pre_wrap)[0], 0..5);
    // And they count toward the max-content width, which the hanging
    // `pre-wrap` sequence at a forced end also does only where it fits.
    let mut context = text_context();
    let mut flow = InlineFlow::new(
        &mut context,
        container(style.clone()),
        &[styled(
            &style,
            "ab   ",
            WhiteSpaceCollapse::BreakSpaces,
            BreakStyle::default(),
        )],
    );
    assert_close(flow.max_content_width(&mut context, 0.0), 50.0);
    // Its min-content width is the widest piece between two opportunities,
    // "ab " with the space it ends in: a `break-spaces` space never hangs.
    assert_close(flow.min_content_width(&mut context, 0.0), 30.0);
}

/// The container's `direction` is the paragraph's bidi level (UAX #9 P3
/// overridden, css-writing-modes-3 §2.1), not the first strong character's:
/// `start` aligns to the right, a line mixing Latin and Hebrew orders its
/// runs right to left, and a trailing space hangs past the left edge.
#[test]
fn an_rtl_container_sets_the_paragraph_level() {
    let style = ahem(10.0, LineHeight::Normal);
    let rtl = |text_align| InlineFlowStyle {
        direction: Direction::Rtl,
        text_align,
        ..container(style.clone())
    };

    // `start` is the right edge.
    let flow = committed(rtl(TextAlign::Start), &[run(&style, "aa")], Some(100.0));
    assert_eq!(line_starts(&flow), [80.0]);
    assert_eq!(flow.first_baseline(), Some(8.0));

    // "abc אבג": the Latin run comes first logically, so an RTL paragraph
    // puts it at the right end, and the Hebrew run to its left. An LTR
    // container of the same text, which first-strong detection would also
    // give, keeps the Latin run on the left.
    let mixed = "abc \u{5d0}\u{5d1}\u{5d2}";
    let flow = committed(rtl(TextAlign::Start), &[run(&style, mixed)], None);
    // Latin on the right.
    assert_close(run_at(&flow, 0).0, 40.0);
    // Hebrew on the left.
    assert_close(run_at(&flow, 4).0, 0.0);
    let ltr = committed(container(style.clone()), &[run(&style, mixed)], None);
    // Latin on the left.
    assert_close(run_at(&ltr, 0).0, 0.0);
    // Hebrew on the right.
    assert_close(run_at(&ltr, 4).0, 40.0);

    // A wrap leaves "אא " on the first line. An RTL paragraph's end edge is
    // its left one, so that is where the trailing space hangs: the run starts
    // at the left edge with the space, and the letters after it — 10px in,
    // past the hanging space — sit flush with the right edge.
    let hebrew = "\u{5d0}\u{5d0} \u{5d1}\u{5d1}";
    let flow = committed(rtl(TextAlign::Start), &[run(&style, hebrew)], Some(30.0));
    let first = flow.display().get(0).expect("a first line");
    assert_close(first.metrics().hanging_advance, 10.0);
    assert_eq!(run_at(&flow, 0), (0.0, 30.0));
    // A space ending a Latin run in the same RTL paragraph does not hang:
    // parley keeps it at the run's level, inside the line on the right,
    // where UAX #9 L1 would move it to the paragraph level on the left
    // (`docs/tracking/deviations.md`, Text layout).
    let flow = committed(rtl(TextAlign::Start), &[run(&style, "aa bb")], Some(30.0));
    let first = flow.display().get(0).expect("a first line");
    assert_close(first.metrics().hanging_advance, 0.0);
    assert_eq!(run_at(&flow, 0), (0.0, 30.0));
    // An LTR paragraph aligned right hangs that space past the right edge:
    // "aa" is flush with it.
    let flow = committed(
        InlineFlowStyle {
            text_align: TextAlign::Right,
            ..container(style.clone())
        },
        &[run(&style, "aa bb")],
        Some(30.0),
    );
    assert_eq!(run_at(&flow, 0), (10.0, 30.0));
}

/// The last line box's baseline, for the layout pass's `last baseline`
/// alignment: the first's on one line, the last line's own on several, and
/// none without a line box.
#[test]
fn the_last_baseline_is_the_last_line_box_s() {
    let style = ahem(10.0, LineHeight::Normal);
    let one = committed(container(style.clone()), &[run(&style, "aaaa")], None);
    assert_eq!(one.first_baseline(), Some(8.0));
    assert_eq!(one.last_baseline(), Some(8.0));

    let mut context = text_context();
    let mut flow = InlineFlow::new(
        &mut context,
        container(style.clone()),
        &[run(&style, "aaaa bbbb cccc")],
    );
    let metrics = flow.commit(&mut context, BlockConstraint::new(Some(50.0), 0.0));
    assert_eq!(flow.line_geometry().len(), 3);
    assert_eq!(metrics.first_baseline, Some(8.0));
    assert_eq!(metrics.last_baseline, Some(28.0));
    assert_eq!(flow.last_baseline(), Some(28.0));

    let empty = committed(container(style.clone()), &[], Some(50.0));
    assert_eq!(empty.last_baseline(), None);
}

fn pre<'src>(style: &'src RunStyle, text: &'src str) -> InlineFlowItem<'src> {
    styled(
        style,
        text,
        WhiteSpaceCollapse::Preserve,
        BreakStyle::default(),
    )
}

/// The advance of every line, in line order: its content from its start,
/// the indent excluded.
fn advances(flow: &InlineFlow) -> Vec<f32> {
    flow.display()
        .lines()
        .map(|line| line.metrics().advance)
        .collect()
}

fn assert_advances(flow: &InlineFlow, expected: &[f32]) {
    let actual = advances(flow);
    assert_eq!(actual.len(), expected.len(), "lines: {actual:?}");
    for (actual, expected) in actual.into_iter().zip(expected) {
        assert_close(actual, *expected);
    }
}

#[test]
fn a_tab_advances_to_the_next_stop_of_eight_spaces() {
    // css-text-3 §4.2: `tab-size: 8`, so Ahem 10 stops every 80.
    let style = ahem(10.0, LineHeight::Normal);
    for (text, advance) in [
        ("\tx", 90.0),
        ("ab\tx", 90.0),
        ("abcdefg\tx", 90.0),
        // A tab on a stop moves a whole tab size.
        ("abcdefgh\tx", 170.0),
    ] {
        for text_wrap in [TextWrap::Wrap, TextWrap::NoWrap] {
            let flow = committed(
                container(style.clone()),
                &[styled(
                    &style,
                    text,
                    WhiteSpaceCollapse::Preserve,
                    BreakStyle {
                        text_wrap,
                        ..BreakStyle::default()
                    },
                )],
                None,
            );
            assert_advances(&flow, &[advance]);
        }
    }
}

#[test]
fn the_tab_size_counts_letter_spacing() {
    // A space advances 12, so the stops are every 96; `a` ends at 12, and
    // `x` after the tab adds its own 12.
    let style = RunStyle {
        letter_spacing: 2.0,
        ..ahem(10.0, LineHeight::Normal)
    };
    let flow = committed(container(style.clone()), &[pre(&style, "a\tx")], None);
    assert_advances(&flow, &[108.0]);
}

#[test]
fn tab_columns_restart_after_a_break_and_count_the_first_line_indent() {
    // Stops are measured from the content edge: the 30px indent is part of
    // the first line's position, so its tab is 50 wide; the second line
    // starts at the edge, so its tab is a whole 80.
    let style = ahem(10.0, LineHeight::Normal);
    let mut context = text_context();
    let mut flow = InlineFlow::new(
        &mut context,
        container(style.clone()),
        &[pre(&style, "\tx\n\ty")],
    );
    flow.commit(&mut context, BlockConstraint::new(None, 30.0));
    assert_advances(&flow, &[60.0, 90.0]);
}

#[test]
fn a_soft_wrap_moves_the_tab_stop_with_it() {
    let style = ahem(10.0, LineHeight::Normal);
    // On its own line the tab sits at 20, not at the 120 of the unwrapped
    // paragraph, so `b` lands on the second line's first stop.
    let flow = committed(
        container(style.clone()),
        &[pre(&style, "aaaa aaaa aa\tb")],
        Some(100.0),
    );
    assert_eq!(advances(&flow).len(), 2);
    assert_close(advances(&flow)[1], 90.0);

    // A tab whose shift overflows the line hangs at its end like a space,
    // and what follows it wraps to the start of the next line.
    let flow = committed(
        container(style.clone()),
        &[pre(&style, "aaaa aaaa\tb")],
        Some(100.0),
    );
    assert_eq!(advances(&flow).len(), 2);
    assert_close(advances(&flow)[1], 10.0);
}

#[test]
fn preserved_spaces_are_kept_and_hang_at_a_soft_wrap() {
    let style = ahem(10.0, LineHeight::Normal);
    let flow = committed(container(style.clone()), &[pre(&style, "    x")], None);
    assert_advances(&flow, &[50.0]);

    // The preserved spaces stay on the first line; the one that overflows
    // the measure hangs past it rather than wrapping.
    let flow = committed(
        container(style.clone()),
        &[pre(&style, "aa  bb")],
        Some(30.0),
    );
    assert_eq!(advances(&flow).len(), 2);
    assert_close(advances(&flow)[1], 20.0);

    // `nowrap` takes the wrap opportunity away: one line.
    let flow = committed(
        container(style.clone()),
        &[styled(
            &style,
            "aa  bb",
            WhiteSpaceCollapse::Preserve,
            BreakStyle {
                text_wrap: TextWrap::NoWrap,
                ..BreakStyle::default()
            },
        )],
        Some(30.0),
    );
    assert_advances(&flow, &[60.0]);
}

#[test]
fn a_preserved_break_is_forced_and_keeps_the_indentation_after_it() {
    let style = ahem(10.0, LineHeight::Normal);
    let flow = committed(container(style.clone()), &[pre(&style, "a  \n  b")], None);
    assert_eq!(advances(&flow).len(), 2);
    assert_close(advances(&flow)[1], 30.0);

    // A preserved space before the break keeps the break, wrapping or not.
    for text_wrap in [TextWrap::Wrap, TextWrap::NoWrap] {
        let flow = committed(
            container(style.clone()),
            &[styled(
                &style,
                "a \nb",
                WhiteSpaceCollapse::Preserve,
                BreakStyle {
                    text_wrap,
                    ..BreakStyle::default()
                },
            )],
            Some(200.0),
        );
        assert_eq!(advances(&flow).len(), 2, "{text_wrap:?}");
    }
}

#[test]
fn a_preserved_newline_that_ends_the_text_opens_no_line() {
    // css-text-3: a preserved newline that ends the text opens no line box,
    // unlike a Lynx `<text>`, which shows an empty last line.
    let style = ahem(10.0, LineHeight::Normal);
    let flow = committed(
        container(style.clone()),
        &[styled(
            &style,
            "aaa\n",
            WhiteSpaceCollapse::PreserveBreaks,
            BreakStyle::default(),
        )],
        None,
    );
    assert_eq!(flow.line_geometry().len(), 1);
}

/// The flow's intrinsic widths at `indent`, how many breaks finding them
/// took, and parley's one-pass content widths of the same content at the
/// same indent, read off a layout of it.
fn intrinsic_widths(
    style: InlineFlowStyle,
    items: &[InlineFlowItem<'_>],
    indent: f32,
) -> ((f32, f32), u32, (f32, f32)) {
    let mut context = text_context();
    let mut flow = InlineFlow::new(&mut context, style, items);
    let intrinsic = (
        flow.min_content_width(&mut context, indent),
        flow.max_content_width(&mut context, indent),
    );
    let breaks = flow.break_count();
    flow.commit(&mut context, BlockConstraint::new(None, indent));
    let one_pass = flow.display().calculate_content_widths();
    (intrinsic, breaks, (one_pass.min, one_pass.max))
}

#[test]
fn intrinsic_widths_take_no_break_where_parley_measures_them_in_one_pass() {
    // `overflow-wrap: break-word` included: its arbitrary breaks count
    // toward neither width, in one pass as in two breaks.
    let style = ahem(10.0, LineHeight::Normal);
    for breaking in [
        BreakStyle::default(),
        BreakStyle {
            overflow_wrap: OverflowWrap::BreakWord,
            ..BreakStyle::default()
        },
    ] {
        let items = [styled(
            &style,
            "aaaa bb",
            WhiteSpaceCollapse::Collapse,
            breaking,
        )];
        let (intrinsic, breaks, one_pass) = intrinsic_widths(container(style.clone()), &items, 0.0);
        assert_eq!(intrinsic, (40.0, 70.0), "{breaking:?}");
        assert_eq!(one_pass, intrinsic, "{breaking:?}");
        assert_eq!(breaks, 0, "{breaking:?}");
    }
}

#[test]
fn a_negative_indent_keeps_the_intrinsic_widths_of_two_breaks() {
    // Parley's widest line counts a negative indent as none; its one-pass
    // measure subtracts it.
    let style = ahem(10.0, LineHeight::Normal);
    let (intrinsic, breaks, one_pass) =
        intrinsic_widths(container(style.clone()), &[run(&style, "aaaa")], -10.0);
    assert_eq!(intrinsic, (40.0, 40.0));
    assert_eq!(one_pass, (30.0, 30.0), "parley's one pass");
    assert_eq!(breaks, 2);
}

#[test]
fn a_positive_indent_keeps_the_intrinsic_widths_of_two_breaks() {
    let style = ahem(10.0, LineHeight::Normal);
    // White space processing removes the lone space: the empty flow still
    // breaks into one line, which takes the indent, and the one pass
    // measures no line.
    let (intrinsic, breaks, one_pass) =
        intrinsic_widths(container(style.clone()), &[run(&style, " ")], 10.0);
    assert_eq!(intrinsic, (10.0, 10.0));
    assert_eq!(one_pass, (0.0, 0.0), "parley's one pass");
    assert_eq!(breaks, 2);
    // A break takes no wrap opportunity after content of no advance, so the
    // zero width space and "aa" share the indented first line; the one pass
    // ends a fragment after the zero width space.
    let (intrinsic, breaks, one_pass) = intrinsic_widths(
        container(style.clone()),
        &[run(&style, "\u{200B}aa bb")],
        10.0,
    );
    assert_eq!(intrinsic, (30.0, 60.0));
    assert_eq!(one_pass, (20.0, 60.0), "parley's one pass");
    assert_eq!(breaks, 2);
}

#[test]
fn a_preserved_tab_keeps_the_intrinsic_widths_of_two_breaks() {
    // Each break settles the tab's advance from where it falls on its line:
    // at zero width the tab starts a line of its own and reaches the first
    // stop, 80px, while the unconstrained line holds it after "a ".
    let style = ahem(10.0, LineHeight::Normal);
    let items = [styled(
        &style,
        "a \tbbb",
        WhiteSpaceCollapse::BreakSpaces,
        BreakStyle::default(),
    )];
    let (intrinsic, breaks, one_pass) = intrinsic_widths(container(style.clone()), &items, 0.0);
    assert_eq!(intrinsic, (80.0, 110.0));
    assert_eq!(one_pass, (60.0, 110.0), "parley's one pass");
    assert_eq!(breaks, 2);
}

#[test]
fn a_negative_letter_spacing_keeps_the_intrinsic_widths_of_two_breaks() {
    // The zero width space advances −3px: the one pass skips the wrap
    // opportunity after it while its running width is negative.
    let style = RunStyle {
        letter_spacing: -3.0,
        ..ahem(10.0, LineHeight::Normal)
    };
    let (intrinsic, breaks, one_pass) = intrinsic_widths(
        container(ahem(10.0, LineHeight::Normal)),
        &[run(&style, "\u{200B}x-y")],
        0.0,
    );
    assert_eq!(intrinsic, (14.0, 18.0));
    assert_eq!(one_pass, (11.0, 18.0), "parley's one pass");
    assert_eq!(breaks, 2);
}

#[test]
fn right_to_left_content_keeps_the_intrinsic_widths_of_two_breaks() {
    // Parley's one pass lets only white space at the paragraph's own bidi
    // level hang, and documents its widths as possibly wrong for mixed
    // directions. A left-to-right paragraph holding right-to-left text
    // takes the two breaks as well; no input found makes them differ there.
    let spaced = RunStyle {
        letter_spacing: 0.5,
        ..ahem(13.0, LineHeight::Normal)
    };
    let wide = RunStyle {
        letter_spacing: 2.0,
        ..ahem(10.0, LineHeight::Normal)
    };
    let items = [
        styled(
            &spaced,
            "c \n  ",
            WhiteSpaceCollapse::BreakSpaces,
            BreakStyle::default(),
        ),
        styled(
            &wide,
            "  dd",
            WhiteSpaceCollapse::Preserve,
            BreakStyle::default(),
        ),
    ];
    let (intrinsic, breaks, one_pass) = intrinsic_widths(
        InlineFlowStyle {
            direction: Direction::Rtl,
            ..container(ahem(10.0, LineHeight::Normal))
        },
        &items,
        0.0,
    );
    assert_eq!(intrinsic, (37.5, 75.0));
    assert_eq!(one_pass, (27.0, 75.0), "parley's one pass");
    assert_eq!(breaks, 2);
}

/// The offset bridge of a collapsing flow: a unit inside a collapsed span
/// lands on the one byte the span emitted, the byte after the span starts
/// at the unit after it, and the end answers the text's end.
#[test]
fn the_offset_map_snaps_a_collapsed_span_to_its_emitted_byte() {
    let style = ahem(10.0, LineHeight::Normal);
    let flow = committed(container(style.clone()), &[run(&style, "a  b")], None);
    let map = flow.offset_map();
    assert_eq!(map.display_text(), "a b");
    assert_eq!(map.source_len(), 4);
    assert!(!map.truncated());
    assert_eq!(map.unit_to_byte(0), Some(0));
    assert_eq!(map.unit_to_byte(1), Some(1));
    assert_eq!(map.unit_to_byte(2), Some(1), "inside the collapsed span");
    assert_eq!(map.unit_to_byte(3), Some(2));
    assert_eq!(map.unit_to_byte_or_end(4), Some(3));
    assert_eq!(map.byte_to_unit(1), 1, "the span's first unit");
    assert_eq!(map.byte_to_unit(2), 3);
    assert_eq!(map.unit_before(2), 3, "the whole span is before `b`");
    assert_eq!(map.byte_to_unit(3), 4);
}
