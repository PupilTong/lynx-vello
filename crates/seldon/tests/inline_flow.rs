//! The CSS inline formatting context of a block container holding text runs
//! only (`seldon::text::inline`), on Ahem geometry.
//!
//! Ahem's em square is exact: every glyph advances one font size, the ascent
//! is 0.8em, the descent 0.2em and the line gap zero, so `line-height:
//! normal` is 1em with the baseline 0.8em down.

use parley::PositionedLayoutItem;
use seldon::text::inline::{InlineFlow, InlineFlowItem, InlineFlowStyle, InlineRun};
use seldon::text::paragraph::{
    BlockConstraint, BreakStyle, Direction, IndentKeywords, LineGeometry, LineHeight, OverflowWrap,
    RunStyle, TextAlign, TextSecurity, TextWrap, WhiteSpaceCollapse, WordBreak,
};
use seldon::text::{FontBlob, TextContext};
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
    let small = ahem(10.0, LineHeight::Normal);
    let flow = committed(
        container(ahem(20.0, LineHeight::Normal)),
        &[run(&small, "aa")],
        None,
    );
    let [line] = flow.line_geometry()[..] else {
        panic!("one line expected, got {:?}", flow.line_geometry())
    };
    assert_line(line, 0.0, 20.0, 16.0);
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
    let tall_leading = ahem(10.0, LineHeight::Px(40.0));
    let big = ahem(30.0, LineHeight::Px(30.0));
    let flow = committed(
        container(ahem(10.0, LineHeight::Px(10.0))),
        &[run(&tall_leading, "aa"), run(&big, "b")],
        None,
    );
    let [line] = flow.line_geometry()[..] else {
        panic!("one line expected")
    };
    assert_line(line, 0.0, 41.0, 24.0);
    assert_close(flow.size().height, 41.0);
}

#[test]
fn line_height_normal_and_numbers_resolve_against_each_runs_own_font() {
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
    assert_line(lines[1], 10.0, 30.0, 28.0);
    assert_close(flow.size().height, 40.0);
}

#[test]
fn collapsible_whitespace_is_removed_at_the_start_of_the_paragraph_and_after_a_break() {
    let style = ahem(10.0, LineHeight::Normal);
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
    let metrics = flow.probe(&mut context, BlockConstraint::new(Some(60.0), 0.0));
    assert_close(metrics.size.width, 60.0);
    assert_close(metrics.size.height, 20.0);
    assert_close(flow.min_content_width(&mut context, 0.0), 40.0);
    assert_close(flow.max_content_width(&mut context, 0.0), 90.0);
    let wide = flow.probe(&mut context, BlockConstraint::new(Some(200.0), 0.0));
    assert_close(wide.size.width, 90.0);

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
        assert_eq!(flow.line_geometry(), []);
        assert_close(flow.size().height, 0.0);
        assert_close(flow.size().width, 0.0);
        assert_eq!(flow.first_baseline(), None);
    }
}

#[test]
fn empty_content_that_keeps_a_strut_line_lays_out_one_empty_line() {
    let style = ahem(10.0, LineHeight::Px(24.0));
    for runs in [vec![], vec![run(&style, "  \n\t ")], vec![run(&style, "")]] {
        for width in [Some(100.0), Some(0.0), None] {
            let keeping = InlineFlowStyle {
                strut_line_when_empty: true,
                ..container(style.clone())
            };
            let mut context = text_context();
            let mut flow = InlineFlow::new(&mut context, keeping, &runs);
            flow.commit(&mut context, BlockConstraint::new(width, 0.0));
            let [line] = flow.line_geometry()[..] else {
                panic!("one line expected, got {:?}", flow.line_geometry())
            };
            assert_line(line, 0.0, 24.0, 15.0);
            assert_close(flow.size().width, 0.0);
            assert_close(flow.size().height, 24.0);
            assert_eq!(flow.first_baseline(), Some(15.0));
            assert_eq!(flow.last_baseline(), Some(15.0));
            assert_close(flow.min_content_width(&mut context, 0.0), 0.0);
            assert_close(flow.max_content_width(&mut context, 0.0), 0.0);
            assert_eq!(flow.atoms(), []);
            assert_eq!(flow.fragments(), []);
            assert_eq!(
                flow.range_boxes(0, 0).map(|boxes| boxes.len()),
                Some(1),
                "an empty range has the line's box"
            );
            assert_eq!(
                flow.index_at_point(seldon::geometry::Point::new(2.0, 5.0)),
                None
            );
        }
    }
}

#[test]
fn the_strut_line_is_only_for_empty_content_that_asks_for_it() {
    let style = ahem(10.0, LineHeight::Normal);
    let flow = committed(container(style.clone()), &[], Some(100.0));
    assert_eq!(flow.line_geometry(), []);

    let keeping = InlineFlowStyle {
        strut_line_when_empty: true,
        ..container(style.clone())
    };
    let flow = committed(keeping, &[run(&style, "aa bb")], Some(30.0));
    assert_eq!(flow.line_geometry().len(), 2);
    assert_close(flow.size().width, 30.0);
    assert_close(flow.size().height, 20.0);
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
fn a_width_the_max_content_lines_fit_reuses_them() {
    let style = ahem(10.0, LineHeight::Normal);
    let mut context = text_context();
    let mut flow = InlineFlow::new(
        &mut context,
        container(style.clone()),
        &[run(&style, "aaaa bbbb")],
    );
    let max_content = flow.probe(&mut context, BlockConstraint::new(None, 0.0));
    assert_close(max_content.size.width, 90.0);
    let breaks = flow.break_count();

    assert_eq!(
        flow.probe(&mut context, BlockConstraint::new(Some(120.0), 0.0)),
        max_content
    );
    assert_eq!(
        flow.commit(&mut context, BlockConstraint::new(Some(90.0), 0.0)),
        max_content
    );
    assert_eq!(flow.break_count(), breaks);
    assert!(!flow.is_probe_dirty());
    assert_eq!(flow.line_count(), 1);

    flow.probe(&mut context, BlockConstraint::new(Some(89.0), 0.0));
    assert_eq!(flow.break_count(), breaks + 1);
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

fn line_ranges(flow: &InlineFlow) -> Vec<core::ops::Range<usize>> {
    flow.display()
        .lines()
        .map(|line| line.text_range())
        .collect()
}

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
    let break_spaces = at(WhiteSpaceCollapse::BreakSpaces, 50.0);
    assert_eq!(line_ranges(&break_spaces), [0..5, 5..9]);
    assert_eq!(run_at(&break_spaces, 5), (0.0, 40.0));
    let pre_wrap = at(WhiteSpaceCollapse::Preserve, 50.0);
    assert_eq!(line_ranges(&pre_wrap), [0..6, 6..9]);

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
    assert_close(flow.min_content_width(&mut context, 0.0), 30.0);
}

#[test]
fn an_rtl_container_sets_the_paragraph_level() {
    let style = ahem(10.0, LineHeight::Normal);
    let rtl = |text_align| InlineFlowStyle {
        direction: Direction::Rtl,
        text_align,
        ..container(style.clone())
    };

    let flow = committed(rtl(TextAlign::Start), &[run(&style, "aa")], Some(100.0));
    assert_eq!(line_starts(&flow), [80.0]);
    assert_eq!(flow.first_baseline(), Some(8.0));

    let mixed = "abc \u{5d0}\u{5d1}\u{5d2}";
    let flow = committed(rtl(TextAlign::Start), &[run(&style, mixed)], None);
    assert_close(run_at(&flow, 0).0, 40.0);
    assert_close(run_at(&flow, 4).0, 0.0);
    let ltr = committed(container(style.clone()), &[run(&style, mixed)], None);
    assert_close(run_at(&ltr, 0).0, 0.0);
    assert_close(run_at(&ltr, 4).0, 40.0);

    let hebrew = "\u{5d0}\u{5d0} \u{5d1}\u{5d1}";
    let flow = committed(rtl(TextAlign::Start), &[run(&style, hebrew)], Some(30.0));
    let first = flow.display().get(0).expect("a first line");
    assert_close(first.metrics().hanging_advance, 10.0);
    assert_eq!(run_at(&flow, 0), (0.0, 30.0));
    let flow = committed(rtl(TextAlign::Start), &[run(&style, "aa bb")], Some(30.0));
    let first = flow.display().get(0).expect("a first line");
    assert_close(first.metrics().hanging_advance, 0.0);
    assert_eq!(run_at(&flow, 0), (0.0, 30.0));
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
    let style = ahem(10.0, LineHeight::Normal);
    for (text, advance) in [
        ("\tx", 90.0),
        ("ab\tx", 90.0),
        ("abcdefg\tx", 90.0),
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
    let style = RunStyle {
        letter_spacing: 2.0,
        ..ahem(10.0, LineHeight::Normal)
    };
    let flow = committed(container(style.clone()), &[pre(&style, "a\tx")], None);
    assert_advances(&flow, &[108.0]);
}

#[test]
fn tab_columns_restart_after_a_break_and_count_the_first_line_indent() {
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
    let flow = committed(
        container(style.clone()),
        &[pre(&style, "aaaa aaaa aa\tb")],
        Some(100.0),
    );
    assert_eq!(advances(&flow).len(), 2);
    assert_close(advances(&flow)[1], 90.0);

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

    let flow = committed(
        container(style.clone()),
        &[pre(&style, "aa  bb")],
        Some(30.0),
    );
    assert_eq!(advances(&flow).len(), 2);
    assert_close(advances(&flow)[1], 20.0);

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
    let (intrinsic, breaks, one_pass) =
        intrinsic_widths(container(style.clone()), &[run(&style, " ")], 10.0);
    assert_eq!(intrinsic, (10.0, 10.0));
    assert_eq!(one_pass, (10.0, 10.0), "parley's one pass");
    assert_eq!(breaks, 2);
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
    let style = RunStyle {
        letter_spacing: -3.0,
        ..ahem(10.0, LineHeight::Normal)
    };
    let (intrinsic, breaks, one_pass) = intrinsic_widths(
        container(ahem(10.0, LineHeight::Normal)),
        &[run(&style, "\u{200B}x-y")],
        0.0,
    );
    assert_eq!(intrinsic, (14.0, 21.0));
    assert_eq!(one_pass, (14.0, 21.0), "parley's one pass");
    assert_eq!(breaks, 2);
}

#[test]
fn right_to_left_content_keeps_the_intrinsic_widths_of_two_breaks() {
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
    assert_eq!(intrinsic, (27.0, 75.0));
    assert_eq!(one_pass, (27.0, 75.0), "parley's one pass");
    assert_eq!(breaks, 2);
}

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
