//! The CSS inline formatting context of a block container holding text runs
//! only (`hughie::text::inline`), on Ahem geometry.
//!
//! Ahem's em square is exact: every glyph advances one font size, the ascent
//! is 0.8em, the descent 0.2em and the line gap zero, so `line-height:
//! normal` is 1em with the baseline 0.8em down.

use hughie::text::inline::{InlineFlow, InlineFlowStyle, InlineRun};
use hughie::text::paragraph::{
    BlockConstraint, BreakStyle, IndentKeywords, LineGeometry, LineHeight, OverflowWrap, RunStyle,
    TextAlign, TextWrap, WhiteSpaceCollapse, WordBreak,
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
) -> InlineRun<'src> {
    InlineRun {
        text,
        style,
        white_space,
        breaking,
    }
}

fn run<'src>(style: &'src RunStyle, text: &'src str) -> InlineRun<'src> {
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
fn committed(style: InlineFlowStyle, runs: &[InlineRun<'_>], width: Option<f32>) -> InlineFlow {
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
    // container's baseline. Parley alone would make the line 10px tall.
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
    assert_close(line.shift, 8.0);
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
    // css-text-3 §4.1.2 rule 1. The Lynx block keeps one space here (gap C2).
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
