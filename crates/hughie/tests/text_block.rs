//! Lynx text-block layout conformance tests, on Ahem geometry.
//!
//! Ahem at `font_size` 10: every glyph (periods and spaces included) advances
//! 10px, ascent 8, descent 2, so a text-only line is 10 tall with its
//! baseline at 8.

use hughie::geometry::{Point, Size};
use hughie::text::block::{
    BlockStyle, Direction, InlineBoxSpec, InlineItem, PlacedBox, RunStyle, SourceItem, TextAlign,
    TextBlock, TextIndent, TextOverflow, TextRunItem, TextWrap, VerticalAlign,
};
use hughie::text::{FontBlob, TextContext};
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

fn ahem_family() -> FontFamily {
    FontFamily {
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
    }
}

fn ahem_style() -> RunStyle {
    RunStyle {
        font_family: ahem_family(),
        font_size: 10.0,
        ..RunStyle::default()
    }
}

fn run<'src>(style: &'src RunStyle, text: &'src str) -> InlineItem<'src> {
    InlineItem::Run(TextRunItem {
        text,
        style,
        preserve_newlines: false,
    })
}

fn raw<'src>(style: &'src RunStyle, text: &'src str) -> InlineItem<'src> {
    InlineItem::Run(TextRunItem {
        text,
        style,
        preserve_newlines: true,
    })
}

fn atom(id: u64, width: f32, height: f32, vertical_align: VerticalAlign) -> InlineItem<'static> {
    InlineItem::Box(InlineBoxSpec {
        id,
        size: Size::new(width, height),
        baseline: None,
        vertical_align,
    })
}

fn laid_out(
    context: &mut TextContext,
    style: BlockStyle,
    items: &[InlineItem<'_>],
    truncation: Option<&[InlineItem<'_>]>,
    width: Option<f32>,
) -> TextBlock {
    let mut block = TextBlock::new(context, style, items, truncation);
    block.layout(context, width);
    block
}

fn visible_box(block: &TextBlock, id: u64) -> (u32, Point<f32>, Size<f32>) {
    block
        .boxes()
        .iter()
        .find_map(|placed| match *placed {
            PlacedBox::Visible {
                id: found,
                line,
                origin,
                size,
            } if found == id => Some((line, origin, size)),
            _ => None,
        })
        .unwrap_or_else(|| panic!("box {id} is visible"))
}

fn assert_hidden(block: &TextBlock, id: u64) {
    assert!(
        block
            .boxes()
            .iter()
            .any(|placed| matches!(placed, PlacedBox::Hidden { id: found } if *found == id)),
        "box {id} is hidden",
    );
}

#[test]
fn flattened_items_keep_source_order_and_identity() {
    let style = ahem_style();
    let items = [
        run(&style, "aa"),
        atom(7, 10.0, 10.0, VerticalAlign::Baseline),
        run(&style, "bb"),
    ];
    let mut context = text_context();
    let block = laid_out(&mut context, BlockStyle::default(), &items, None, None);

    let (line, origin, size) = visible_box(&block, 7);
    assert_eq!(line, 0);
    assert_close(origin.x, 20.0);
    assert_close(size.width, 10.0);
    assert_close(block.size().width, 50.0);
    assert_close(block.size().height, 10.0);
    assert_eq!(block.source_of(0), SourceItem::Content(0));
    assert_eq!(block.source_of(1), SourceItem::Content(2));

    let lines = block.lines();
    assert_eq!(lines.len(), 1);
    assert_eq!((lines[0].source_start, lines[0].source_end), (0, 5));
    assert_eq!(lines[0].ellipsis_count, 0);
    assert_eq!(block.first_baseline(), Some(lines[0].baseline));
    assert!(!block.truncated());
}

#[test]
fn a_box_without_a_baseline_sits_its_bottom_edge_on_the_baseline() {
    let style = ahem_style();
    let items = [
        run(&style, "aaaa"),
        atom(1, 10.0, 30.0, VerticalAlign::Baseline),
    ];
    let mut context = text_context();
    let block = laid_out(&mut context, BlockStyle::default(), &items, None, None);

    // The box contributes its full height as ascent: line height 30,
    // leading 30 − 32 = −2, half −1, baseline 29.
    let line = block.lines()[0];
    assert_close(line.height, 30.0);
    assert_close(line.baseline, 29.0);
    let (_, origin, _) = visible_box(&block, 1);
    assert_close(origin.y + 30.0, line.baseline);
}

#[test]
fn a_box_baseline_hangs_its_remainder_below_the_text_baseline() {
    let style = ahem_style();
    let spec = InlineBoxSpec {
        id: 1,
        size: Size::new(10.0, 30.0),
        baseline: Some(24.0),
        vertical_align: VerticalAlign::Baseline,
    };
    let items = [run(&style, "aaaa"), InlineItem::Box(spec)];
    let mut context = text_context();
    let block = laid_out(&mut context, BlockStyle::default(), &items, None, None);

    // Contribution 24: line height 24, leading −2, baseline 23. The box's
    // below-baseline part (6) hangs without reserving descent.
    let line = block.lines()[0];
    assert_close(line.height, 24.0);
    assert_close(line.baseline, 23.0);
    let (_, origin, _) = visible_box(&block, 1);
    assert_close(origin.y, -1.0);
    assert_close(origin.y + 30.0, line.baseline + 6.0);
}

#[test]
fn every_vertical_align_value_lands_on_its_table_row() {
    let style = ahem_style();
    let mut context = text_context();

    // Contribution 30 for every extent-anchored value: line height 30,
    // baseline 29, block box −1..31, text ascent 8, descent 2.
    let expectations = [
        (VerticalAlign::Baseline, -1.0),
        (VerticalAlign::Sub, 1.0),
        (VerticalAlign::Super, -4.4),
        (VerticalAlign::Top, -1.0),
        (VerticalAlign::TextTop, 21.0),
        (VerticalAlign::Bottom, 1.0),
        (VerticalAlign::TextBottom, 1.0),
        (VerticalAlign::Percent(0.5), -16.0),
    ];
    for (value, expected) in expectations {
        let items = [run(&style, "aaaa"), atom(1, 10.0, 30.0, value)];
        let block = laid_out(&mut context, BlockStyle::default(), &items, None, None);
        let (_, origin, _) = visible_box(&block, 1);
        assert_close(origin.y, expected);
    }

    // Middle centers on half the x-height above the baseline; Center is its
    // alias. Ahem's x-height is 0.8em = 8: y = 29 − 4 − 15.
    for value in [VerticalAlign::Middle, VerticalAlign::Center] {
        let items = [run(&style, "aaaa"), atom(1, 10.0, 30.0, value)];
        let block = laid_out(&mut context, BlockStyle::default(), &items, None, None);
        let (_, origin, _) = visible_box(&block, 1);
        assert_close(origin.y, 10.0);
    }

    // A positive raise grows the line (contribution 35 → baseline 34); a
    // negative one drops the box without growing anything.
    let items = [
        run(&style, "aaaa"),
        atom(1, 10.0, 30.0, VerticalAlign::Length(5.0)),
    ];
    let block = laid_out(&mut context, BlockStyle::default(), &items, None, None);
    assert_close(block.lines()[0].baseline, 34.0);
    let (_, origin, _) = visible_box(&block, 1);
    assert_close(origin.y, -1.0);

    let items = [
        run(&style, "aaaa"),
        atom(1, 10.0, 30.0, VerticalAlign::Length(-5.0)),
    ];
    let block = laid_out(&mut context, BlockStyle::default(), &items, None, None);
    assert_close(block.lines()[0].baseline, 29.0);
    let (_, origin, _) = visible_box(&block, 1);
    assert_close(origin.y, 4.0);
}

#[test]
fn maxline_ellipsis_backs_off_three_units_and_appends_dots() {
    let style = ahem_style();
    let items = [run(&style, "aaaa aaaa aaaa")];
    let block_style = BlockStyle {
        max_lines: core::num::NonZeroU32::new(2),
        overflow: TextOverflow::Ellipsis,
        ..BlockStyle::default()
    };
    let mut context = text_context();
    let block = laid_out(&mut context, block_style, &items, None, Some(50.0));

    assert!(block.truncated());
    let lines = block.lines();
    assert_eq!(lines.len(), 2);
    assert_eq!((lines[0].source_start, lines[0].source_end), (0, 4));
    assert_eq!((lines[1].source_start, lines[1].source_end), (5, 9));
    assert_eq!(lines[0].ellipsis_count, 0);
    assert_eq!(lines[1].ellipsis_count, 3);
    assert_close(block.size().height, 20.0);
    // The visible line end excludes the wrapped space, so the cut backs off
    // from unit 9: "a" plus three dots, four Ahem advances.
    assert_close(lines[1].advance, 40.0);
    // The dots carry their own identity, styled after the cut run.
    let display = block.display();
    let styles = 0..u16::try_from(display.styles().len()).expect("fits");
    assert!(
        styles
            .clone()
            .any(|i| block.source_of(i) == SourceItem::Ellipsis)
    );
    assert!(
        styles
            .clone()
            .all(|i| block.source_of(i) != SourceItem::Truncation(0))
    );
}

#[test]
fn a_short_cut_line_shrinks_the_dots_to_its_own_length() {
    let style = ahem_style();
    let items = [run(&style, "aaaa a aaaa")];
    let block_style = BlockStyle {
        max_lines: core::num::NonZeroU32::new(2),
        overflow: TextOverflow::Ellipsis,
        ..BlockStyle::default()
    };
    let mut context = text_context();
    let block = laid_out(&mut context, block_style, &items, None, Some(50.0));

    // Natural line 2 shows "a" — one visible unit, fewer than three: the cut
    // moves to the line start and the dots shrink to one.
    let lines = block.lines();
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[1].ellipsis_count, 1);
    assert_close(lines[1].advance, 10.0);
}

#[test]
fn maxlength_cuts_without_backing_off_and_keeps_three_dots() {
    let style = ahem_style();
    let items = [run(&style, "aaaa aaaa")];
    let block_style = BlockStyle {
        max_chars: Some(4),
        overflow: TextOverflow::Ellipsis,
        ..BlockStyle::default()
    };
    let mut context = text_context();
    let block = laid_out(&mut context, block_style, &items, None, None);

    assert!(block.truncated());
    let lines = block.lines();
    assert_eq!(lines.len(), 1);
    assert_eq!((lines[0].source_start, lines[0].source_end), (0, 9));
    assert_eq!(lines[0].ellipsis_count, 5);
    // "aaaa" plus three dots.
    assert_close(block.size().width, 70.0);
}

#[test]
fn the_earlier_of_maxlength_and_maxline_wins() {
    let style = ahem_style();
    let items = [run(&style, "aaaa aaaa aaaa")];
    let mut context = text_context();

    let maxlength_wins = laid_out(
        &mut context,
        BlockStyle {
            max_lines: core::num::NonZeroU32::new(2),
            max_chars: Some(2),
            overflow: TextOverflow::Ellipsis,
            ..BlockStyle::default()
        },
        &items,
        None,
        Some(50.0),
    );
    assert_eq!(maxlength_wins.lines().len(), 1);
    assert_eq!(maxlength_wins.lines()[0].ellipsis_count, 2);

    let maxline_wins = laid_out(
        &mut context,
        BlockStyle {
            max_lines: core::num::NonZeroU32::new(2),
            max_chars: Some(2000),
            overflow: TextOverflow::Ellipsis,
            ..BlockStyle::default()
        },
        &items,
        None,
        Some(50.0),
    );
    assert_eq!(maxline_wins.lines().len(), 2);
    assert_eq!(maxline_wins.lines()[1].ellipsis_count, 3);
}

#[test]
fn truncation_content_reserves_its_width_on_the_cut_line() {
    let style = ahem_style();
    let items = [run(&style, "aaaa aaaa aaaa")];
    let tail_style = ahem_style();
    let tail = [run(&tail_style, "XX")];
    let block_style = BlockStyle {
        max_lines: core::num::NonZeroU32::new(2),
        ..BlockStyle::default()
    };
    let mut context = text_context();
    let block = laid_out(&mut context, block_style, &items, Some(&tail), Some(50.0));

    assert!(block.truncated());
    assert!(block.truncation_visible());
    // The fitting walk frees two 10px clusters for the 20px content; the
    // wrapped space is outside the visible range and never counts as freed.
    assert_eq!(block.lines()[1].ellipsis_count, 2);
    assert_close(block.lines()[1].advance, 40.0);
    let display = block.display();
    let styles = 0..u16::try_from(display.styles().len()).expect("fits");
    assert!(
        styles
            .clone()
            .any(|i| block.source_of(i) == SourceItem::Truncation(0))
    );
    assert!(
        styles
            .clone()
            .all(|i| block.source_of(i) != SourceItem::Ellipsis)
    );
}

#[test]
fn truncation_content_wider_than_the_container_is_hidden() {
    let style = ahem_style();
    let items = [run(&style, "aaaa aaaa aaaa")];
    let tail_style = ahem_style();
    let tail = [
        run(&tail_style, "XXXXX"),
        atom(9, 20.0, 10.0, VerticalAlign::Baseline),
    ];
    let block_style = BlockStyle {
        max_lines: core::num::NonZeroU32::new(2),
        overflow: TextOverflow::Ellipsis,
        ..BlockStyle::default()
    };
    let mut context = text_context();
    let block = laid_out(&mut context, block_style, &items, Some(&tail), Some(50.0));

    assert!(block.truncated());
    assert!(!block.truncation_visible());
    assert_hidden(&block, 9);
    // The cut lands at the line start and no dots are appended.
    let lines = block.lines();
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[1].ellipsis_count, 4);
    let display = block.display();
    let styles = 0..u16::try_from(display.styles().len()).expect("fits");
    assert!(
        styles
            .clone()
            .all(|i| block.source_of(i) == SourceItem::Content(0))
    );
}

#[test]
fn retreating_across_a_box_hides_it() {
    let style = ahem_style();
    let items = [
        run(&style, "aaaa "),
        run(&style, "aaa"),
        atom(4, 10.0, 10.0, VerticalAlign::Baseline),
        run(&style, " aaaa"),
    ];
    let tail_style = ahem_style();
    let tail = [run(&tail_style, "XX")];
    let block_style = BlockStyle {
        max_lines: core::num::NonZeroU32::new(2),
        ..BlockStyle::default()
    };
    let mut context = text_context();
    // Natural line 2 is "aaa" + the box + " " (units 5..10); freeing 20px
    // removes the trailing space cluster and the box.
    let block = laid_out(&mut context, block_style, &items, Some(&tail), Some(50.0));

    assert!(block.truncation_visible());
    assert_hidden(&block, 4);
}

#[test]
fn clip_cuts_at_the_line_end_without_dots() {
    let style = ahem_style();
    let items = [run(&style, "aaaa aaaa aaaa")];
    let block_style = BlockStyle {
        max_lines: core::num::NonZeroU32::new(2),
        overflow: TextOverflow::Clip,
        ..BlockStyle::default()
    };
    let mut context = text_context();
    let block = laid_out(&mut context, block_style, &items, None, Some(50.0));

    assert!(block.truncated());
    let lines = block.lines();
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[1].ellipsis_count, 0);
    assert_eq!((lines[1].source_start, lines[1].source_end), (5, 9));
    let display = block.display();
    let styles = 0..u16::try_from(display.styles().len()).expect("fits");
    assert!(
        styles
            .clone()
            .all(|i| block.source_of(i) == SourceItem::Content(0))
    );
}

#[test]
fn justify_stretches_every_line_but_the_last() {
    let style = ahem_style();
    let items = [run(&style, "aa a aaa")];
    let block_style = BlockStyle {
        text_align: TextAlign::Justify,
        ..BlockStyle::default()
    };
    let mut context = text_context();
    let block = laid_out(&mut context, block_style, &items, None, Some(50.0));

    let lines = block.lines();
    assert_eq!(lines.len(), 2);
    // Line 1 content "aa a" is 40px in 50: the inner space stretches by 10.
    // Justification mutates cluster advances, which `LineMetrics::advance`
    // does not reflect, so the glyph runs' right edge is what is measured.
    assert_close(right_edge(block.display(), 0), 60.0);
    assert_close(right_edge(block.display(), 1), 30.0);
}

/// The rightmost glyph edge of one line, trailing whitespace included.
fn right_edge(layout: &parley::Layout<()>, line: usize) -> f32 {
    layout
        .get(line)
        .expect("line exists")
        .items()
        .filter_map(|item| match item {
            parley::PositionedLayoutItem::GlyphRun(run) => Some(run.offset() + run.advance()),
            parley::PositionedLayoutItem::InlineBox(_) => None,
        })
        .fold(0.0f32, f32::max)
}

#[test]
fn start_and_end_resolve_against_the_declared_direction_on_ascii_text() {
    let style = ahem_style();
    let items = [run(&style, "aa")];
    let mut context = text_context();

    for (text_align, direction, expected_offset) in [
        (TextAlign::Start, Direction::Ltr, 0.0),
        (TextAlign::Start, Direction::Rtl, 80.0),
        (TextAlign::End, Direction::Ltr, 80.0),
        (TextAlign::End, Direction::Rtl, 0.0),
        (TextAlign::Center, Direction::Ltr, 40.0),
    ] {
        let block = laid_out(
            &mut context,
            BlockStyle {
                text_align,
                direction,
                ..BlockStyle::default()
            },
            &items,
            None,
            Some(100.0),
        );
        let metrics = *block.display().get(0).expect("one line").metrics();
        assert_close(metrics.offset, expected_offset);
    }
}

#[test]
fn a_paragraph_of_only_boxes_lays_out_and_reports_units() {
    let items = [
        atom(1, 10.0, 10.0, VerticalAlign::Baseline),
        atom(2, 10.0, 12.0, VerticalAlign::Baseline),
    ];
    let mut context = text_context();
    let block = laid_out(&mut context, BlockStyle::default(), &items, None, None);

    let lines = block.lines();
    assert_eq!(lines.len(), 1);
    assert_eq!((lines[0].source_start, lines[0].source_end), (0, 2));
    let (_, first, _) = visible_box(&block, 1);
    let (_, second, _) = visible_box(&block, 2);
    assert_close(first.x, 0.0);
    assert_close(second.x, 10.0);
    assert_close(block.size().width, 20.0);
    assert_close(block.lines()[0].height, 12.0);
}

#[test]
fn a_box_wider_than_the_width_takes_its_own_line() {
    let style = ahem_style();
    let items = [
        run(&style, "aa"),
        atom(3, 30.0, 10.0, VerticalAlign::Baseline),
    ];
    let mut context = text_context();
    let block = laid_out(
        &mut context,
        BlockStyle::default(),
        &items,
        None,
        Some(25.0),
    );

    let lines = block.lines();
    assert_eq!(lines.len(), 2);
    assert_eq!((lines[0].source_start, lines[0].source_end), (0, 2));
    assert_eq!((lines[1].source_start, lines[1].source_end), (2, 3));
    let (line, _, _) = visible_box(&block, 3);
    assert_eq!(line, 1);
}

#[test]
fn nowrap_suppresses_soft_wrapping_but_not_preserved_breaks() {
    let style = ahem_style();
    let mut context = text_context();

    let soft = [run(&style, "aaaa aaaa")];
    let block = laid_out(
        &mut context,
        BlockStyle {
            text_wrap: TextWrap::NoWrap,
            ..BlockStyle::default()
        },
        &soft,
        None,
        Some(30.0),
    );
    assert_eq!(block.lines().len(), 1);
    assert_close(block.size().width, 90.0);

    let hard = [raw(&style, "aa\naa")];
    let block = laid_out(
        &mut context,
        BlockStyle {
            text_wrap: TextWrap::NoWrap,
            ..BlockStyle::default()
        },
        &hard,
        None,
        Some(30.0),
    );
    assert_eq!(block.lines().len(), 2);
}

#[test]
fn text_indent_shifts_and_shortens_the_first_line_only() {
    let style = ahem_style();
    let items = [run(&style, "aaaa aaaa")];
    let block_style = BlockStyle {
        text_indent: TextIndent::Px(10.0),
        ..BlockStyle::default()
    };
    let mut context = text_context();
    let block = laid_out(&mut context, block_style, &items, None, Some(50.0));

    let display = block.display();
    assert_close(display.get(0).expect("first").metrics().offset, 10.0);
    assert_close(display.get(1).expect("second").metrics().offset, 0.0);

    let percent = laid_out(
        &mut context,
        BlockStyle {
            text_indent: TextIndent::Percent(0.2),
            ..BlockStyle::default()
        },
        &items,
        None,
        Some(50.0),
    );
    assert_close(
        percent.display().get(0).expect("first").metrics().offset,
        10.0,
    );
}

#[test]
fn content_widths_cover_words_and_boxes() {
    let style = ahem_style();
    let mut context = text_context();

    let words = laid_out(
        &mut context,
        BlockStyle::default(),
        &[run(&style, "aaaa aa")],
        None,
        None,
    );
    let widths = words.content_widths();
    assert_close(widths.min, 40.0);
    assert_close(widths.max, 70.0);

    let with_box = laid_out(
        &mut context,
        BlockStyle::default(),
        &[
            run(&style, "aaaa aa"),
            atom(1, 55.0, 10.0, VerticalAlign::Baseline),
        ],
        None,
        None,
    );
    assert_close(with_box.content_widths().min, 55.0);
}

#[test]
fn resizing_a_box_rebreaks_into_the_same_geometry_as_a_fresh_build() {
    let style = ahem_style();
    let small = [
        run(&style, "aaaa "),
        atom(5, 10.0, 10.0, VerticalAlign::Baseline),
        run(&style, " aaaa"),
    ];
    let grown = [
        run(&style, "aaaa "),
        InlineItem::Box(InlineBoxSpec {
            id: 5,
            size: Size::new(25.0, 25.0),
            baseline: Some(20.0),
            vertical_align: VerticalAlign::Baseline,
        }),
        run(&style, " aaaa"),
    ];
    let mut context = text_context();

    let mut resized = TextBlock::new(&mut context, BlockStyle::default(), &small, None);
    resized.layout(&mut context, Some(50.0));
    resized.set_box_size(5, Size::new(25.0, 25.0), Some(20.0));
    resized.layout(&mut context, Some(50.0));

    let fresh = laid_out(
        &mut context,
        BlockStyle::default(),
        &grown,
        None,
        Some(50.0),
    );

    assert_eq!(resized.lines(), fresh.lines());
    assert_eq!(resized.boxes(), fresh.boxes());
    assert_close(resized.size().width, fresh.size().width);
    assert_close(resized.size().height, fresh.size().height);
}

#[test]
fn max_chars_zero_cuts_everything() {
    let style = ahem_style();
    let items = [run(&style, "aaaa")];
    let mut context = text_context();

    let ellipsis = laid_out(
        &mut context,
        BlockStyle {
            max_chars: Some(0),
            overflow: TextOverflow::Ellipsis,
            ..BlockStyle::default()
        },
        &items,
        None,
        None,
    );
    assert!(ellipsis.truncated());
    assert_eq!(ellipsis.lines().len(), 1);
    assert_eq!(ellipsis.lines()[0].ellipsis_count, 4);
    assert_close(ellipsis.size().width, 30.0);

    let clip = laid_out(
        &mut context,
        BlockStyle {
            max_chars: Some(0),
            ..BlockStyle::default()
        },
        &items,
        None,
        None,
    );
    assert!(clip.truncated());
    assert_eq!(clip.lines().len(), 1);
    assert_close(clip.size().width, 0.0);
    // The empty display borrows the natural line's geometry for the report.
    assert_close(clip.lines()[0].height, 10.0);

    let beyond = laid_out(
        &mut context,
        BlockStyle {
            max_chars: Some(4),
            ..BlockStyle::default()
        },
        &items,
        None,
        None,
    );
    assert!(!beyond.truncated());
}

#[test]
fn a_maxlength_cut_inside_a_surrogate_pair_rounds_down() {
    let style = ahem_style();
    let items = [run(&style, "a🙂b")];
    let mut context = text_context();
    let block = laid_out(
        &mut context,
        BlockStyle {
            max_chars: Some(2),
            overflow: TextOverflow::Ellipsis,
            ..BlockStyle::default()
        },
        &items,
        None,
        None,
    );

    assert!(block.truncated());
    // Four source units; the cut at unit 2 rounds down to the pair start.
    assert_eq!(block.lines()[0].ellipsis_count, 2);
    assert_eq!(
        (block.lines()[0].source_start, block.lines()[0].source_end),
        (0, 4)
    );
}

#[test]
fn content_that_exactly_fills_max_lines_is_not_truncated() {
    let style = ahem_style();
    let mut context = text_context();

    // A trailing preserved newline leaves parley one renderless line short
    // of done; that must not read as overflow.
    let trailing_break = laid_out(
        &mut context,
        BlockStyle {
            max_lines: core::num::NonZeroU32::new(2),
            overflow: TextOverflow::Ellipsis,
            ..BlockStyle::default()
        },
        &[raw(&style, "a\nb\n")],
        None,
        Some(100.0),
    );
    assert!(!trailing_break.truncated());
    assert_eq!(trailing_break.lines().len(), 2);
    assert_eq!(trailing_break.lines()[1].ellipsis_count, 0);

    let single = laid_out(
        &mut context,
        BlockStyle {
            max_lines: core::num::NonZeroU32::new(1),
            overflow: TextOverflow::Ellipsis,
            ..BlockStyle::default()
        },
        &[raw(&style, "a\n")],
        None,
        None,
    );
    assert!(!single.truncated());
    assert_close(single.size().width, 10.0);

    // A hanging trailing space overflows the width without being content.
    let hanging = laid_out(
        &mut context,
        BlockStyle {
            max_lines: core::num::NonZeroU32::new(1),
            overflow: TextOverflow::Ellipsis,
            ..BlockStyle::default()
        },
        &[run(&style, "aaaa ")],
        None,
        Some(40.0),
    );
    assert!(!hanging.truncated());
    assert_eq!(hanging.lines().len(), 1);
}

#[test]
fn a_box_left_past_max_lines_still_counts_as_overflow() {
    let style = ahem_style();
    let mut context = text_context();
    let items = [
        run(&style, "aaa"),
        atom(4, 10.0, 10.0, VerticalAlign::Baseline),
    ];
    let block = laid_out(
        &mut context,
        BlockStyle {
            max_lines: core::num::NonZeroU32::new(1),
            overflow: TextOverflow::Ellipsis,
            ..BlockStyle::default()
        },
        &items,
        None,
        Some(30.0),
    );

    assert!(block.truncated());
    assert_hidden(&block, 4);
    assert_eq!(block.lines().len(), 1);
    assert_eq!(block.lines()[0].ellipsis_count, 3);
}

#[test]
fn a_cut_between_adjacent_boxes_keeps_the_earlier_one() {
    let style = ahem_style();
    let items = [
        run(&style, "a"),
        atom(1, 10.0, 10.0, VerticalAlign::Baseline),
        atom(2, 10.0, 10.0, VerticalAlign::Baseline),
    ];
    let mut context = text_context();
    let block = laid_out(
        &mut context,
        BlockStyle {
            max_chars: Some(2),
            overflow: TextOverflow::Ellipsis,
            ..BlockStyle::default()
        },
        &items,
        None,
        None,
    );

    // Both boxes share one byte position; the cut is a unit decision.
    let (_, origin, _) = visible_box(&block, 1);
    assert_close(origin.x, 10.0);
    assert_hidden(&block, 2);
    assert_eq!(block.lines()[0].ellipsis_count, 1);
}

#[test]
fn a_box_directly_before_the_cut_survives_it() {
    let style = ahem_style();
    let items = [
        run(&style, "ab"),
        atom(9, 10.0, 10.0, VerticalAlign::Baseline),
        run(&style, "cd"),
    ];
    let mut context = text_context();
    let block = laid_out(
        &mut context,
        BlockStyle {
            max_chars: Some(3),
            overflow: TextOverflow::Ellipsis,
            ..BlockStyle::default()
        },
        &items,
        None,
        None,
    );

    // The box and 'c' share byte 2, but the box's unit (2) precedes the cut
    // (3): the render is "ab", the box, then the dots.
    let (_, origin, _) = visible_box(&block, 9);
    assert_close(origin.x, 20.0);
    assert_eq!(block.lines()[0].ellipsis_count, 2);
    assert!(
        !block
            .boxes()
            .iter()
            .any(|placed| matches!(placed, PlacedBox::Hidden { .. })),
        "no box is hidden by this cut",
    );
}

#[test]
fn truncation_content_never_shows_on_a_pure_maxlength_cut() {
    let style = ahem_style();
    let items = [run(&style, "aaaa aaaa")];
    let tail_style = ahem_style();
    let tail = [
        run(&tail_style, "XX"),
        atom(9, 10.0, 10.0, VerticalAlign::Baseline),
    ];
    let mut context = text_context();
    let block = laid_out(
        &mut context,
        BlockStyle {
            max_chars: Some(4),
            overflow: TextOverflow::Ellipsis,
            ..BlockStyle::default()
        },
        &items,
        Some(&tail),
        None,
    );

    assert!(block.truncated());
    assert!(!block.truncation_visible());
    assert_hidden(&block, 9);
    // The presence of truncation content also suppresses the dots.
    assert_close(block.size().width, 40.0);
}

#[test]
fn an_untruncated_block_reports_its_truncation_boxes_hidden() {
    let style = ahem_style();
    let items = [run(&style, "aaaa")];
    let tail_style = ahem_style();
    let tail = [
        run(&tail_style, "XX"),
        atom(9, 10.0, 10.0, VerticalAlign::Baseline),
    ];
    let mut context = text_context();
    let block = laid_out(
        &mut context,
        BlockStyle::default(),
        &items,
        Some(&tail),
        None,
    );

    assert!(!block.truncated());
    assert!(!block.truncation_visible());
    assert_hidden(&block, 9);
}

#[test]
fn an_empty_block_lays_out_to_nothing() {
    let mut context = text_context();
    let block = laid_out(
        &mut context,
        BlockStyle {
            max_lines: core::num::NonZeroU32::new(1),
            ..BlockStyle::default()
        },
        &[],
        None,
        Some(50.0),
    );

    assert!(block.lines().is_empty());
    assert!(block.boxes().is_empty());
    assert_close(block.size().width, 0.0);
    assert_close(block.size().height, 0.0);
    assert_eq!(block.first_baseline(), None);
    assert!(!block.truncated());
}

#[test]
fn the_paragraph_direction_is_detected_from_the_content() {
    let style = ahem_style();
    let mut context = text_context();

    let hebrew = laid_out(
        &mut context,
        BlockStyle::default(),
        &[run(&style, "אב")],
        None,
        Some(100.0),
    );
    assert!(hebrew.display().is_rtl());

    let ascii = laid_out(
        &mut context,
        BlockStyle::default(),
        &[run(&style, "ab")],
        None,
        Some(100.0),
    );
    assert!(!ascii.display().is_rtl());
}

#[test]
fn content_widths_track_box_resizes_and_survive_justification() {
    let style = ahem_style();
    let mut context = text_context();

    let items = [
        run(&style, "aaaa aa"),
        atom(1, 10.0, 10.0, VerticalAlign::Baseline),
    ];
    let mut block = TextBlock::new(&mut context, BlockStyle::default(), &items, None);
    block.layout(&mut context, None);
    assert_close(block.content_widths().min, 40.0);
    block.set_box_size(1, Size::new(55.0, 10.0), None);
    block.layout(&mut context, None);
    assert_close(block.content_widths().min, 55.0);

    // Justification mutates cluster advances on the retained layout; the
    // reported widths come from the unjustified pass and stay put.
    let mut justified = TextBlock::new(
        &mut context,
        BlockStyle {
            text_align: TextAlign::Justify,
            ..BlockStyle::default()
        },
        &[run(&style, "aa aa aa")],
        None,
    );
    justified.layout(&mut context, Some(70.0));
    assert_close(justified.content_widths().max, 80.0);
    justified.layout(&mut context, Some(60.0));
    assert_close(justified.content_widths().max, 80.0);
}
