//! The Lynx `<text>` block's verbatim white space (`BlockStyle::processing`):
//! native Lynx's no-processing text, on Ahem geometry.
//!
//! Ahem at `font_size` 10: every glyph, the space included, advances 10px,
//! and a line is 10 tall. Every native backend hands a `<text>`'s string to
//! its platform engine unchanged (`lynx/core/renderer/dom/fiber/
//! raw_text_element.cc:16-46`, Android `BaseTextShadowNode.java:494-498`, iOS
//! `LynxBaseTextShadowNode.m:240-249`, textra `text_layout_textra.cc:569-575`),
//! so spaces, tabs and newlines all stay; these tests pin what that means
//! here.

use seldon::geometry::Size;
use seldon::text::block::{
    BlockConstraint, BlockStyle, InlineBoxSpec, InlineItem, PlacedBox, RunStyle, TextBlock,
    TextOverflow, TextRunItem, TextSecurity, TextWrap, VerticalAlign, WhiteSpaceCollapse,
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
        ..RunStyle::default()
    }
}

/// A run whose own `white-space-collapse` is CSS's initial `collapse`: the
/// value a verbatim paragraph does not read.
fn run<'src>(style: &'src RunStyle, text: &'src str) -> InlineItem<'src> {
    InlineItem::Run(TextRunItem {
        text,
        style,
        white_space: WhiteSpaceCollapse::Collapse,
        text_security: TextSecurity::None,
    })
}

fn atom(id: u64) -> InlineItem<'static> {
    InlineItem::Box(InlineBoxSpec {
        id,
        size: Size::new(10.0, 10.0),
        baseline: None,
        vertical_align: VerticalAlign::Baseline,
    })
}

fn verbatim() -> BlockStyle {
    BlockStyle::default()
}

fn nowrap() -> BlockStyle {
    BlockStyle {
        text_wrap: TextWrap::NoWrap,
        ..verbatim()
    }
}

fn laid_out(style: BlockStyle, items: &[InlineItem<'_>], width: Option<f32>) -> TextBlock {
    let mut context = text_context();
    let mut block = TextBlock::new(&mut context, style, items, None);
    block.commit(&mut context, BlockConstraint::new(width, 0.0));
    block
}

#[test]
fn leading_spaces_are_kept_and_measured() {
    let style = ahem();
    let block = laid_out(verbatim(), &[run(&style, "  ab")], None);
    assert_close(block.size().width, 40.0);
}

#[test]
fn space_runs_are_kept() {
    let style = ahem();
    let block = laid_out(verbatim(), &[run(&style, "a   b")], None);
    assert_close(block.size().width, 50.0);
}

#[test]
fn every_newline_is_a_forced_break_and_the_spaces_after_it_stay() {
    let style = ahem();
    // A `collapse` run: its newline still breaks, its indent still shows.
    let block = laid_out(verbatim(), &[run(&style, "a\n  b")], None);
    let lines = block.lines();
    assert_eq!(lines.len(), 2);
    assert_eq!((lines[1].source_start, lines[1].source_end), (2, 5));
    assert_close(block.size().width, 30.0);
    assert_close(block.size().height, 20.0);
}

#[test]
fn a_soft_wrap_ends_the_space_run_on_the_previous_line() {
    let style = ahem();
    let block = laid_out(verbatim(), &[run(&style, "aaa   bbb")], Some(50.0));
    let lines = block.lines();
    assert_eq!(lines.len(), 2);
    // The spaces hang at the end of the first line — consumed by it, outside
    // its content — and the second line starts at the first non-space.
    assert_eq!((lines[0].source_start, lines[0].source_end), (0, 3));
    assert_eq!(lines[1].source_start, 6);
    assert_close(block.size().width, 30.0);
    assert_eq!(
        block.range_boxes(3, 6),
        Some(Vec::new()),
        "hanging spaces show on no line"
    );
}

#[test]
fn trailing_spaces_hang_and_add_no_width() {
    let style = ahem();
    let block = laid_out(verbatim(), &[run(&style, "ab   ")], None);
    assert_close(block.size().width, 20.0);
}

#[test]
fn a_tab_advances_to_the_next_twenty_pixel_stop() {
    let style = ahem();
    // From x 10 the next stop is 20; "b" ends at 30.
    assert_close(
        laid_out(verbatim(), &[run(&style, "a\tb")], None)
            .size()
            .width,
        30.0,
    );
    // A tab already on a stop moves to the next one: from 20 to 40.
    assert_close(
        laid_out(verbatim(), &[run(&style, "aa\tb")], None)
            .size()
            .width,
        50.0,
    );
    // A tab at the line start runs to 20.
    assert_close(
        laid_out(verbatim(), &[run(&style, "\tb")], None)
            .size()
            .width,
        30.0,
    );
}

/// `width` with a resolved `text-indent` of `indent`.
fn laid_out_indented(items: &[InlineItem<'_>], width: Option<f32>, indent: f32) -> TextBlock {
    let mut context = text_context();
    let mut block = TextBlock::new(&mut context, verbatim(), items, None);
    block.commit(&mut context, BlockConstraint::new(width, indent));
    block
}

/// Where source unit `unit` starts on its line.
fn unit_x(block: &TextBlock, unit: u32) -> f32 {
    let boxes = block.range_boxes(unit, unit + 1).expect("a valid range");
    let [glyph] = boxes[..] else {
        panic!("one box for unit {unit}, got {boxes:?}")
    };
    glyph.origin.x
}

/// Lynx applies `text-indent` as Android's `LeadingMarginSpan.Standard(indent,
/// 0)` (`TextShadowNode.java:295-298`), a margin outside the text Android's
/// `TextLine` measures tab positions in: the stops of an indented line sit at
/// the indent plus multiples of 20, so a tab at the start of a 15px-indented
/// line runs to 35, not to 20.
#[test]
fn a_tab_stop_is_measured_from_the_text_start_after_the_indent() {
    let style = ahem();
    let block = laid_out_indented(&[run(&style, "\tb")], None, 15.0);
    assert_close(unit_x(&block, 1), 35.0);
    // From 10 into the text, the next stop is 20 into it: 35 from the edge.
    let block = laid_out_indented(&[run(&style, "a\tb")], None, 15.0);
    assert_close(unit_x(&block, 2), 35.0);
}

/// A soft-wrapped line is not indented, so it measures its stops from 0.
#[test]
fn a_soft_wrapped_line_has_no_indent_and_measures_its_stops_from_zero() {
    let style = ahem();
    // "aaaa " fills the 5px-indented first line of 50; "b\tc" wraps.
    let block = laid_out_indented(&[run(&style, "aaaa b\tc")], Some(50.0), 5.0);
    assert_eq!(block.lines().len(), 2);
    assert_close(unit_x(&block, 0), 5.0);
    assert_close(unit_x(&block, 5), 0.0);
    assert_close(unit_x(&block, 7), 20.0);
}

/// Native Lynx applies `text-indent` as a `LeadingMarginSpan` over the whole
/// text, and Android gives its first-line margin to the first line of every
/// paragraph: a `<text>` indents every line that starts after a `\n`, as
/// css-text-3's `each-line` does (user ruling 2026-10-17). Each such line
/// measures its tab stops from its own text start, after the indent.
#[test]
fn every_line_after_a_newline_is_indented_and_measures_its_stops_after_it() {
    let style = ahem();
    // `a\nb` at 15px: both lines start at 15.
    let block = laid_out_indented(&[run(&style, "a\nb")], None, 15.0);
    assert_close(unit_x(&block, 0), 15.0);
    assert_close(unit_x(&block, 2), 15.0);
    // Both lines count their indent toward the unconstrained width, which is
    // the max-content width a host measures.
    assert_close(block.size().width, 25.0);
    // Units: a b-side tab and a d-side tab, each 10 into its line's text,
    // run to that line's 20, which is 35 from the edge on both lines.
    let block = laid_out_indented(&[run(&style, "a\tb\nc\td")], None, 15.0);
    assert_eq!(block.lines().len(), 2);
    assert_close(unit_x(&block, 2), 35.0);
    assert_close(unit_x(&block, 6), 35.0);
    // A soft wrap after a newline-started line is still not indented.
    let block = laid_out_indented(&[run(&style, "a\nbbb ccc")], Some(60.0), 15.0);
    assert_eq!(block.lines().len(), 3);
    assert_close(unit_x(&block, 2), 15.0);
    assert_close(unit_x(&block, 6), 0.0);
    // A clamp's display layout keeps the rule: the last visible line, after a
    // newline, is still indented.
    let mut context = text_context();
    let mut clamped = TextBlock::new(
        &mut context,
        BlockStyle {
            max_lines: core::num::NonZeroU32::new(2),
            ..verbatim()
        },
        &[run(&style, "a\nb\nc")],
        None,
    );
    clamped.commit(&mut context, BlockConstraint::new(None, 15.0));
    assert!(clamped.truncated());
    assert_eq!(clamped.lines().len(), 2);
    assert_close(unit_x(&clamped, 2), 15.0);
}

#[test]
fn nowrap_shows_nothing_after_the_first_newline() {
    let style = ahem();
    let block = laid_out(nowrap(), &[run(&style, "ab\ncd\nef")], None);
    let [line] = block.lines() else {
        panic!("one line expected, got {:?}", block.lines())
    };
    assert_eq!((line.source_start, line.source_end), (0, 2));
    assert_close(block.size().width, 20.0);
    assert_close(block.size().height, 10.0);
    assert!(
        !block.truncated(),
        "the cut is no truncation and has no marker"
    );
}

#[test]
fn nowrap_with_the_newline_first_shows_one_empty_line() {
    let style = ahem();
    let block = laid_out(nowrap(), &[run(&style, "\nab")], None);
    assert_eq!(block.lines().len(), 1);
    assert_close(block.size().width, 0.0);
    assert_close(block.size().height, 10.0);
}

#[test]
fn nowrap_hides_a_box_after_the_cut_and_keeps_one_before_it() {
    let style = ahem();
    let block = laid_out(nowrap(), &[atom(1), run(&style, "ab\ncd"), atom(2)], None);
    assert!(
        block
            .boxes()
            .iter()
            .any(|placed| matches!(placed, PlacedBox::Visible { id: 1, .. }))
    );
    assert!(
        block
            .boxes()
            .iter()
            .any(|placed| matches!(placed, PlacedBox::Hidden { id: 2 }))
    );
    assert_close(block.size().width, 30.0);
}

#[test]
fn nowrap_still_cuts_its_one_line_at_the_width_with_text_overflow_ellipsis() {
    let style = ahem();
    let block = laid_out(
        BlockStyle {
            overflow: TextOverflow::Ellipsis,
            ..nowrap()
        },
        &[run(&style, "abcdef\nxyz")],
        Some(40.0),
    );
    assert!(block.truncated());
    // "a" plus the three-dot marker fills the 40px line.
    assert_close(block.size().width, 40.0);
    assert_eq!(block.lines().len(), 1);
}

#[test]
fn a_line_clamp_sees_no_overflow_in_what_the_cut_dropped() {
    let style = ahem();
    let block = laid_out(
        BlockStyle {
            overflow: TextOverflow::Ellipsis,
            max_lines: core::num::NonZeroU32::new(1),
            ..nowrap()
        },
        &[run(&style, "ab\ncd")],
        None,
    );
    assert!(!block.truncated());
    assert_close(block.size().width, 20.0);
}

#[test]
fn maxlength_cuts_only_inside_what_the_newline_cut_leaves() {
    let style = ahem();
    let ellipsis = |max_chars| BlockStyle {
        overflow: TextOverflow::Ellipsis,
        max_chars: Some(max_chars),
        ..nowrap()
    };
    // Past the newline: Android clips to the length, appends its ellipsis,
    // then deletes from the newline — the ellipsis with it.
    let past = laid_out(ellipsis(5), &[run(&style, "abc\ndef")], None);
    assert!(!past.truncated());
    assert_close(past.size().width, 30.0);
    // Before it: the length cut and its marker stand.
    let before = laid_out(ellipsis(2), &[run(&style, "abc\ndef")], None);
    assert!(before.truncated());
    assert_close(before.size().width, 50.0);
}

#[test]
fn source_ranges_still_count_what_the_cut_dropped() {
    let style = ahem();
    // Units: a b \n c d.
    let block = laid_out(nowrap(), &[run(&style, "ab\ncd")], None);
    assert_eq!(block.item_source_range(0), Some(0..5));
    let shown = block.range_boxes(0, 2).expect("a valid range");
    assert_eq!(shown.len(), 1);
    assert_close(shown[0].size.width, 20.0);
    let dropped = block.range_boxes(3, 5).expect("a valid range");
    assert!(dropped.is_empty(), "dropped text shows no content");
    assert_eq!(block.range_boxes(0, 5).expect("a valid range").len(), 1);
    assert_eq!(block.range_boxes(0, 6), None, "past the source");
}

#[test]
fn wrapping_text_keeps_every_line() {
    let style = ahem();
    let block = laid_out(verbatim(), &[run(&style, "ab\ncd")], None);
    assert_eq!(block.lines().len(), 2);
}

/// Native hangs every space at a line end, and so does a `<text>` here:
/// each run is shaped with parley's `collapse` hanging, which hangs a whole
/// sequence of overflowing spaces past the line end, before a newline too. No
/// line starts with the spaces a wrap ended, and no line holds only them.
#[test]
fn a_run_of_spaces_hangs_whole_at_a_wrap() {
    let style = ahem();
    for (text, width, expected) in [
        // The run follows a line that fills the width exactly.
        ("aaa   bbb", 30.0, vec![(0, 3), (6, 9)]),
        ("aaa      bbb", 30.0, vec![(0, 3), (9, 12)]),
        // The run ends the paragraph: no line of spaces after it.
        ("aaa bbb   ", 30.0, vec![(0, 3), (4, 7)]),
        ("aaa bbb   ", 45.0, vec![(0, 3), (4, 7)]),
        // The run sits before a newline.
        ("aaa bbb   \nccc", 30.0, vec![(0, 3), (4, 7), (11, 14)]),
    ] {
        let block = laid_out(verbatim(), &[run(&style, text)], Some(width));
        let lines: Vec<_> = block
            .lines()
            .iter()
            .map(|line| (line.source_start, line.source_end))
            .collect();
        assert_eq!(lines, expected, "{text:?} at {width}");
        assert_close(block.size().width, 30.0);
    }
}

/// Every space of a run advances by its own glyph, `letter-spacing` included,
/// and a range over the run selects all of them.
#[test]
fn a_run_of_spaces_keeps_its_full_advance() {
    let mut style = ahem();
    let block = laid_out(verbatim(), &[run(&style, "a    b")], None);
    assert_close(block.size().width, 60.0);
    let boxes = block.range_boxes(1, 5).expect("a valid range");
    assert_eq!(boxes.len(), 1);
    assert_close(boxes[0].size.width, 40.0);

    style.letter_spacing = 2.0;
    let spaced = laid_out(verbatim(), &[run(&style, "a    b")], None);
    // Six clusters of 12px; parley's last cluster keeps its spacing too.
    assert_close(spaced.size().width, 6.0 * 12.0);
}

/// Spaces on both sides of an atomic box are all measured, and so are spaces
/// on both sides of a run boundary: two spaces, a 10px box and two spaces
/// between `a` and `b` make 70, and the same spaces with no box make 60.
#[test]
fn spaces_on_both_sides_of_a_box_are_all_measured() {
    let style = ahem();
    let block = laid_out(
        verbatim(),
        &[run(&style, "a  "), atom(1), run(&style, "  b")],
        None,
    );
    assert_close(block.size().width, 70.0);
    let joined = laid_out(verbatim(), &[run(&style, "a  "), run(&style, "  b")], None);
    assert_close(joined.size().width, 60.0);
}

/// Android's `StaticLayout` ends text that ends with a newline with an empty
/// line, and Lynx measures its height (`TextRenderer.java:720`): the newline
/// adds a line.
#[test]
fn a_newline_that_ends_the_text_adds_an_empty_last_line() {
    let style = ahem();
    for (text, width, expected) in [
        ("aaa\n", None, vec![(0, 3), (4, 4)]),
        ("\n", None, vec![(0, 0), (1, 1)]),
        ("aaa\n\n", None, vec![(0, 3), (4, 4), (5, 5)]),
        // A wrapped line before it changes nothing about the empty line.
        ("aaa bbb\n", Some(30.0), vec![(0, 3), (4, 7), (8, 8)]),
        // Hanging spaces before the newline neither.
        ("aaa   \n", Some(30.0), vec![(0, 3), (7, 7)]),
    ] {
        let block = laid_out(verbatim(), &[run(&style, text)], width);
        let lines: Vec<_> = block
            .lines()
            .iter()
            .map(|line| (line.source_start, line.source_end))
            .collect();
        assert_eq!(lines, expected, "{text:?}");
        #[expect(clippy::cast_precision_loss, reason = "a handful of lines")]
        let height = expected.len() as f32 * 10.0;
        assert_close(block.size().height, height);
        let last = block.lines().last().expect("a line");
        assert_eq!(last.ellipsis_count, 0);
        assert_close(last.height, 10.0);
    }
    // The empty line shows no content: a range over the whole text selects
    // the first line only, and an empty range at its start answers its
    // zero box.
    let block = laid_out(verbatim(), &[run(&style, "aaa\n")], None);
    let boxes = block.range_boxes(0, 4).expect("a valid range");
    assert_eq!(boxes.len(), 1);
    assert_close(boxes[0].size.width, 30.0);
    let caret = block.range_boxes(4, 4).expect("a valid range");
    assert_close(caret[0].size.height, 10.0);
    assert_eq!(block.item_source_range(0), Some(0..4));
}

/// Under `nowrap` the text ends at its first newline, and what follows it —
/// an empty line included — is not shown: one line, as Android's deletion
/// from the newline leaves.
#[test]
fn nowrap_shows_no_empty_line_after_the_newline_it_cuts_at() {
    let style = ahem();
    for text in ["aaa\n", "aaa\nbbb\n", "aaa\n\n"] {
        let block = laid_out(nowrap(), &[run(&style, text)], None);
        assert_eq!(block.lines().len(), 1, "{text:?}");
        assert_close(block.size().height, 10.0);
        assert_close(block.size().width, 30.0);
    }
}

/// A line clamp counts the empty line like any other.
#[test]
fn a_line_clamp_counts_the_empty_last_line() {
    let style = ahem();
    let clamped = |max_lines| BlockStyle {
        overflow: TextOverflow::Ellipsis,
        max_lines: core::num::NonZeroU32::new(max_lines),
        ..verbatim()
    };
    let one = laid_out(clamped(1), &[run(&style, "aaa\n")], None);
    assert_eq!(one.lines().len(), 1);
    assert!(!one.truncated(), "the newline is consumed; nothing is cut");
    assert_close(one.size().height, 10.0);
    let two = laid_out(clamped(2), &[run(&style, "aaa\n")], None);
    assert_eq!(two.lines().len(), 2);
    assert!(!two.truncated());
    assert_close(two.size().height, 20.0);
}

/// `TextBlock::min_content_width` answers at the indent it is asked for, laid
/// out at that indent when cold, and remembers each answer with its indent.
#[test]
fn min_content_is_measured_and_remembered_at_the_indent_asked_for() {
    let style = ahem();
    let mut context = text_context();
    let mut block = TextBlock::new(&mut context, verbatim(), &[run(&style, "a b")], None);
    assert_close(block.min_content_width(&mut context, 15.0), 25.0);
    assert_close(block.min_content_width(&mut context, 0.0), 10.0);
    assert_close(block.min_content_width(&mut context, 15.0), 25.0);
    let mut block = TextBlock::new(&mut context, verbatim(), &[run(&style, "a\nb")], None);
    assert_close(block.min_content_width(&mut context, 15.0), 25.0);
}

/// Parley 0.12's content-width pass takes the indent rule: the indent counts
/// toward the min- and max-content widths on the lines it applies to. In a
/// verbatim block that is the first line and every line after a `\n`, never
/// a soft-wrapped one.
#[test]
fn the_intrinsic_widths_count_the_indent_on_the_indented_lines() {
    let style = ahem();
    // Min-content breaks "a b" into "a" on the indented first line and "b"
    // on an unindented soft-wrapped one: 15 + 10. Max-content is one line.
    let block = laid_out_indented(&[run(&style, "a b")], None, 15.0);
    assert_close(block.content_widths().min, 25.0);
    assert_close(block.content_widths().max, 45.0);
    // After a newline the second line is indented too.
    let block = laid_out_indented(&[run(&style, "a\nb")], None, 15.0);
    assert_close(block.content_widths().min, 25.0);
    assert_close(block.content_widths().max, 25.0);
}
