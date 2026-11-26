//! Lynx text-block layout conformance tests, on Ahem geometry.
//!
//! Ahem at `font_size` 10: every glyph (periods and spaces included) advances
//! 10px, ascent 8, descent 2, so a text-only line is 10 tall with its
//! baseline at 8.

use seldon::geometry::{Point, Rect, Size};
use seldon::text::block::{
    BlockConstraint, BlockStyle, Direction, InlineBoxSpec, InlineItem, PlacedBox, RunStyle,
    SourceItem, TextAlign, TextBlock, TextIndent, TextOverflow, TextRunItem, TextSecurity,
    TextWrap, VerticalAlign, WhiteSpaceCollapse,
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
        white_space: WhiteSpaceCollapse::Collapse,
        text_security: TextSecurity::None,
    })
}

fn raw<'src>(style: &'src RunStyle, text: &'src str) -> InlineItem<'src> {
    InlineItem::Run(TextRunItem {
        text,
        style,
        white_space: WhiteSpaceCollapse::PreserveBreaks,
        text_security: TextSecurity::None,
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
    laid_out_at(
        context,
        style,
        items,
        truncation,
        BlockConstraint::new(width, 0.0),
    )
}

fn laid_out_at(
    context: &mut TextContext,
    style: BlockStyle,
    items: &[InlineItem<'_>],
    truncation: Option<&[InlineItem<'_>]>,
    constraint: BlockConstraint,
) -> TextBlock {
    let mut block = TextBlock::new(context, style, items, truncation);
    block.commit(context, constraint);
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
    assert_close(block.size().height, 12.0);
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

    let line = block.lines()[0];
    assert_close(line.height, 32.0);
    assert_close(line.baseline, 30.0);
    let (_, origin, _) = visible_box(&block, 1);
    assert_close(origin.y + 30.0, line.baseline);
}

#[test]
fn a_box_baseline_reserves_its_remainder_below_the_text_baseline() {
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

    let line = block.lines()[0];
    assert_close(line.height, 30.0);
    assert_close(line.baseline, 24.0);
    let (_, origin, _) = visible_box(&block, 1);
    assert_close(origin.y, 0.0);
    assert_close(origin.y + 30.0, line.baseline + 6.0);
}

#[test]
fn every_vertical_align_value_lands_where_css_places_it() {
    let style = RunStyle {
        line_height: seldon::text::block::LineHeight::Px(40.0),
        ..ahem_style()
    };
    let mut context = text_context();
    for (value, top) in [
        (VerticalAlign::Baseline, 17.0),
        (VerticalAlign::Sub, 19.0),
        (VerticalAlign::Super, 23.0 - 10.0 / 3.0 - 6.0),
        (VerticalAlign::Top, 0.0),
        (VerticalAlign::TextTop, 15.0),
        (VerticalAlign::Bottom, 34.0),
        (VerticalAlign::TextBottom, 19.0),
        (VerticalAlign::Middle, 16.0),
        (VerticalAlign::Center, 16.0),
        (VerticalAlign::Length(5.0), 12.0),
        (VerticalAlign::Length(-5.0), 22.0),
    ] {
        let items = [run(&style, "aaaa"), atom(1, 10.0, 6.0, value)];
        let block = laid_out(&mut context, BlockStyle::default(), &items, None, None);
        let (_, origin, _) = visible_box(&block, 1);
        let line = block.lines()[0];
        assert_close(line.height, 40.0);
        assert_close(line.baseline, 23.0);
        assert_close(origin.y, top);
    }

    let items = [
        run(&style, "aaaa"),
        atom(1, 10.0, 6.0, VerticalAlign::Percent(0.5)),
    ];
    let block = laid_out(&mut context, BlockStyle::default(), &items, None, None);
    let (_, origin, _) = visible_box(&block, 1);
    assert_close(block.lines()[0].baseline, 26.0);
    assert_close(block.lines()[0].height, 43.0);
    assert_close(origin.y, 0.0);
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
    assert_close(lines[1].advance, 40.0);
    let display = block.display();
    let styles = 0..u16::try_from(display.styles().len()).expect("fits");
    assert!(
        styles
            .clone()
            .any(|i| matches!(block.source_of(i), SourceItem::Ellipsis { .. }))
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
    assert_close(block.size().width, 70.0);
}

#[test]
fn maxlength_clip_at_a_line_boundary_does_not_retain_an_empty_line() {
    let style = ahem_style();
    let mut context = text_context();
    for text in ["aaaa", "aa aa"] {
        let block = laid_out(
            &mut context,
            BlockStyle {
                max_chars: Some(2),
                ..BlockStyle::default()
            },
            &[run(&style, text)],
            None,
            Some(20.0),
        );
        assert!(block.truncated());
        assert_eq!(block.lines().len(), 1);
        assert_close(block.size().height, 10.0);
        assert_close(block.size().width, 20.0);
    }
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
            .all(|i| !matches!(block.source_of(i), SourceItem::Ellipsis { .. }))
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
    let block = laid_out(&mut context, block_style, &items, Some(&tail), Some(50.0));

    assert!(block.truncation_visible());
    assert_hidden(&block, 4);
    assert_eq!(block.lines()[1].ellipsis_count, 2);
    assert_close(block.lines()[1].advance, 40.0);
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
    assert_close(right_edge(block.display(), 0), 60.0);
    assert_close(right_edge(block.display(), 1), 30.0);
}

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
fn nowrap_suppresses_soft_wrapping() {
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
}

#[test]
fn text_indent_shifts_and_shortens_the_first_line_only() {
    let style = ahem_style();
    let items = [run(&style, "aaaa aaaa")];
    let mut context = text_context();
    let resolve = |indent: TextIndent, basis: f32| match indent {
        TextIndent::Px(value) => value,
        TextIndent::Percent(fraction) => fraction * basis,
    };
    let block = laid_out_at(
        &mut context,
        BlockStyle::default(),
        &items,
        None,
        BlockConstraint::new(Some(50.0), resolve(TextIndent::Px(10.0), 50.0)),
    );

    let display = block.display();
    assert_close(display.get(0).expect("first").metrics().offset, 10.0);
    assert_close(display.get(1).expect("second").metrics().offset, 0.0);

    let percent = laid_out_at(
        &mut context,
        BlockStyle::default(),
        &items,
        None,
        BlockConstraint::new(Some(50.0), resolve(TextIndent::Percent(0.2), 50.0)),
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
    resized.commit(&mut context, BlockConstraint::new(Some(50.0), 0.0));
    resized.set_box_size(5, Size::new(25.0, 25.0), Some(20.0));
    resized.commit(&mut context, BlockConstraint::new(Some(50.0), 0.0));

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

    assert_eq!(block.lines(), []);
    assert_eq!(block.boxes(), []);
    assert_close(block.size().width, 0.0);
    assert_close(block.size().height, 0.0);
    assert_eq!(block.first_baseline(), None);
    assert!(!block.truncated());
}

#[test]
fn an_empty_block_that_keeps_a_strut_line_lays_out_one_empty_line() {
    let mut context = text_context();
    let strut = RunStyle {
        line_height: seldon::text::block::LineHeight::Px(24.0),
        ..ahem_style()
    };
    for max_lines in [None, core::num::NonZeroU32::new(1)] {
        for max_chars in [None, Some(0)] {
            for text_wrap in [TextWrap::Wrap, TextWrap::NoWrap] {
                let style = BlockStyle {
                    strut: Some(strut.clone()),
                    strut_line_when_empty: true,
                    max_lines,
                    max_chars,
                    text_wrap,
                    overflow: TextOverflow::Ellipsis,
                    ..BlockStyle::default()
                };
                let empty = run(&strut, "");
                for items in [&[][..], &[empty][..]] {
                    let block = laid_out(&mut context, style.clone(), items, None, Some(50.0));
                    let [line] = block.lines() else {
                        panic!("one line expected, got {:?}", block.lines())
                    };
                    assert_eq!((line.source_start, line.source_end), (0, 0));
                    assert_eq!(line.ellipsis_count, 0);
                    assert_close(line.top, 0.0);
                    assert_close(line.height, 24.0);
                    assert_close(line.baseline, 15.0);
                    assert_close(block.size().width, 0.0);
                    assert_close(block.size().height, 24.0);
                    assert_eq!(block.first_baseline(), Some(15.0));
                    assert_eq!(block.last_baseline(), Some(15.0));
                    assert!(!block.truncated());
                    assert_eq!(block.boxes(), []);
                    for index in 0..block.style_count() {
                        assert_eq!(
                            block.source_of(u16::try_from(index).unwrap()),
                            SourceItem::Strut
                        );
                    }
                    assert_eq!(
                        block.range_boxes(0, 0),
                        Some(vec![Rect::new(Point::new(0.0, 0.0), Size::new(0.0, 24.0))])
                    );
                    assert_eq!(block.range_boxes(0, 1), None);
                    assert_eq!(block.index_at_point(Point::new(2.0, 5.0)), None);
                    let widths = block.content_widths();
                    assert_eq!((widths.min, widths.max), (0.0, 0.0));
                }
            }
        }
    }

    let block = laid_out(
        &mut context,
        BlockStyle {
            strut_line_when_empty: true,
            ..BlockStyle::default()
        },
        &[],
        None,
        Some(50.0),
    );
    assert_eq!(block.lines().len(), 1);
    assert_close(block.size().width, 0.0);
}

#[test]
fn the_empty_strut_line_aligns_as_a_zero_wide_line() {
    let mut context = text_context();
    let strut = ahem_style();
    for (text_align, direction, expected) in [
        (TextAlign::Start, Direction::Ltr, 0.0),
        (TextAlign::Left, Direction::Ltr, 0.0),
        (TextAlign::Center, Direction::Ltr, 95.0),
        (TextAlign::Right, Direction::Ltr, 190.0),
        (TextAlign::End, Direction::Ltr, 190.0),
        (TextAlign::Justify, Direction::Ltr, 0.0),
        (TextAlign::End, Direction::Rtl, 0.0),
    ] {
        let style = BlockStyle {
            strut: Some(strut.clone()),
            strut_line_when_empty: true,
            text_align,
            direction,
            ..BlockStyle::default()
        };
        let block = laid_out(&mut context, style, &[], None, Some(190.0));
        let layout = block.display();
        let line = layout.get(0).expect("the one line");
        assert_close(line.metrics().offset, expected);
        let caret = parley::Cursor::from_byte_index(layout, 0, parley::Affinity::Downstream)
            .geometry(layout, 1.0);
        assert!((caret.x0 - f64::from(expected)).abs() <= f64::from(EPSILON));
        assert_close(block.size().width, 0.0);
    }
}

#[test]
fn the_strut_line_is_only_for_empty_content() {
    let style = ahem_style();
    let mut context = text_context();
    let keeping = BlockStyle {
        strut: Some(style.clone()),
        strut_line_when_empty: true,
        ..BlockStyle::default()
    };
    let block = laid_out(
        &mut context,
        keeping.clone(),
        &[run(&style, "aa")],
        None,
        Some(50.0),
    );
    assert_eq!(block.lines().len(), 1);
    assert_eq!(block.size(), Size::new(20.0, 10.0));
    let block = laid_out(
        &mut context,
        keeping.clone(),
        &[run(&style, "a\n")],
        None,
        Some(50.0),
    );
    assert_eq!(block.lines().len(), 2);
    for keeps in [true, false] {
        let block = laid_out(
            &mut context,
            BlockStyle {
                max_chars: Some(0),
                strut_line_when_empty: keeps,
                ..keeping.clone()
            },
            &[run(&style, "abc")],
            None,
            Some(50.0),
        );
        assert!(block.truncated());
        assert_eq!(block.lines().len(), 1, "keeps {keeps}");
        assert_eq!(block.size(), Size::new(0.0, 10.0), "keeps {keeps}");
    }

    let block = laid_out(
        &mut context,
        BlockStyle {
            strut: Some(style),
            ..BlockStyle::default()
        },
        &[],
        None,
        Some(50.0),
    );
    assert_eq!(block.lines(), []);
    assert_close(block.size().height, 0.0);
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

    let left = laid_out(
        &mut context,
        BlockStyle {
            text_align: TextAlign::Start,
            direction: Direction::Ltr,
            ..BlockStyle::default()
        },
        &[run(&style, "אב")],
        None,
        Some(100.0),
    );
    let right = laid_out(
        &mut context,
        BlockStyle {
            text_align: TextAlign::Start,
            direction: Direction::Rtl,
            ..BlockStyle::default()
        },
        &[run(&style, "אב")],
        None,
        Some(100.0),
    );
    let left_offset = left.display().get(0).expect("one line").metrics().offset;
    let right_offset = right.display().get(0).expect("one line").metrics().offset;
    assert_close(left_offset, 0.0);
    assert!(
        right_offset > left_offset + 20.0,
        "declared Rtl right-aligns the detected-RTL line",
    );
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
    block.commit(&mut context, BlockConstraint::new(None, 0.0));
    assert_close(block.content_widths().min, 40.0);
    block.set_box_size(1, Size::new(55.0, 10.0), None);
    block.commit(&mut context, BlockConstraint::new(None, 0.0));
    assert_close(block.content_widths().min, 55.0);

    let mut justified = TextBlock::new(
        &mut context,
        BlockStyle {
            text_align: TextAlign::Justify,
            ..BlockStyle::default()
        },
        &[run(&style, "aa aa aa")],
        None,
    );
    justified.commit(&mut context, BlockConstraint::new(Some(70.0), 0.0));
    assert_close(justified.content_widths().max, 80.0);
    justified.commit(&mut context, BlockConstraint::new(Some(60.0), 0.0));
    assert_close(justified.content_widths().max, 80.0);
}

#[test]
fn a_maxlength_cut_on_a_wrapped_box_keeps_the_dots() {
    let style = ahem_style();
    let items = [
        run(&style, "aa"),
        atom(5, 10.0, 10.0, VerticalAlign::Baseline),
        run(&style, "bb"),
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
        Some(20.0),
    );

    assert!(block.truncated());
    assert_hidden(&block, 5);
    let lines = block.lines();
    assert_eq!(lines.len(), 2);
    assert_eq!((lines[1].source_start, lines[1].source_end), (2, 3));
    assert_eq!(lines[1].ellipsis_count, 1);
    let display = block.display();
    let styles = 0..u16::try_from(display.styles().len()).expect("fits");
    assert!(
        styles
            .clone()
            .any(|i| matches!(block.source_of(i), SourceItem::Ellipsis { .. }))
    );
}

#[test]
fn a_wrapped_box_does_not_widen_the_dots_back_off() {
    let style = ahem_style();
    let items = [
        run(&style, "aaaa"),
        atom(5, 10.0, 10.0, VerticalAlign::Baseline),
        run(&style, "bb"),
    ];
    let mut context = text_context();
    let block = laid_out(
        &mut context,
        BlockStyle {
            max_lines: core::num::NonZeroU32::new(1),
            overflow: TextOverflow::Ellipsis,
            ..BlockStyle::default()
        },
        &items,
        None,
        Some(40.0),
    );

    assert!(block.truncated());
    let lines = block.lines();
    assert_eq!(lines.len(), 1);
    assert_eq!((lines[0].source_start, lines[0].source_end), (0, 4));
    assert_eq!(lines[0].ellipsis_count, 3);
    assert_close(lines[0].advance, 40.0);
    assert_hidden(&block, 5);
}

#[test]
fn the_fitting_walk_removes_at_least_two_units() {
    let style = ahem_style();
    let items = [run(&style, "aaaa aaaa aaaa")];
    let tail_style = ahem_style();
    let tail = [run(&tail_style, "X")];
    let mut context = text_context();
    let block = laid_out(
        &mut context,
        BlockStyle {
            max_lines: core::num::NonZeroU32::new(2),
            ..BlockStyle::default()
        },
        &items,
        Some(&tail),
        Some(50.0),
    );

    assert!(block.truncation_visible());
    assert_eq!(block.lines()[1].ellipsis_count, 2);
    assert_close(block.lines()[1].advance, 30.0);
}

#[test]
fn a_remembered_constraint_costs_no_break() {
    let style = ahem_style();
    let items = [run(&style, "aaaa bbbb cccc dddd")];
    let mut context = text_context();
    let mut block = TextBlock::new(&mut context, BlockStyle::default(), &items, None);

    let wide = BlockConstraint::new(Some(200.0), 0.0);
    let first = block.probe(&mut context, wide);
    let after_first = block.break_count();
    let again = block.probe(&mut context, wide);

    assert_eq!(first, again);
    assert_eq!(
        block.break_count(),
        after_first,
        "a repeat of a remembered constraint must not re-enter parley"
    );
}

#[test]
fn the_memo_absorbs_the_constraint_cycle_a_container_produces() {
    let style = ahem_style();
    let items = [run(&style, "aaaa bbbb cccc dddd")];
    let mut context = text_context();
    let mut block = TextBlock::new(&mut context, BlockStyle::default(), &items, None);

    let cycle = [
        BlockConstraint::new(None, 0.0),
        BlockConstraint::new(Some(40.0), 0.0),
        BlockConstraint::new(Some(120.0), 0.0),
    ];
    for constraint in cycle {
        block.probe(&mut context, constraint);
    }
    let after_cold = block.break_count();

    for constraint in [cycle[0], cycle[2], cycle[1], cycle[0]] {
        block.probe(&mut context, constraint);
    }
    assert_eq!(
        block.break_count(),
        after_cold,
        "three remembered constraints must absorb the cycle a container asks in"
    );
}

#[test]
fn a_commit_is_never_served_from_the_memo() {
    let style = ahem_style();
    let items = [run(&style, "aaaa bbbb cccc")];
    let mut context = text_context();
    let mut block = TextBlock::new(&mut context, BlockStyle::default(), &items, None);

    let narrow = BlockConstraint::new(Some(40.0), 0.0);
    let wide = BlockConstraint::new(None, 0.0);

    let probed = block.probe(&mut context, narrow);
    block.probe(&mut context, wide);
    let after_probes = block.break_count();

    let committed = block.commit(&mut context, narrow);
    assert!(
        block.break_count() > after_probes,
        "a commit must re-break when the retained layout sits at another constraint"
    );
    assert_eq!(
        committed, probed,
        "and it must land on the same answer the probe reported"
    );
    assert!(block.has_committed());
    assert!(!block.is_probe_dirty());

    let settled = block.break_count();
    block.commit(&mut context, narrow);
    assert_eq!(block.break_count(), settled);
}

#[test]
fn a_probe_that_never_commits_is_handed_back_to_its_committed_break() {
    let style = ahem_style();
    let items = [run(&style, "aaaa bbbb cccc dddd")];
    let mut context = text_context();
    let mut block = TextBlock::new(&mut context, BlockStyle::default(), &items, None);

    let used = BlockConstraint::new(Some(80.0), 0.0);
    let committed = block.commit(&mut context, used);
    let committed_lines = block.lines().len();

    block.probe(&mut context, BlockConstraint::new(None, 0.0));
    assert!(
        block.is_probe_dirty(),
        "a probe away from the committed constraint owes a restore"
    );

    assert!(block.restore_committed(&mut context));
    assert!(!block.is_probe_dirty());
    assert_eq!(block.size(), committed.size);
    assert_eq!(block.lines().len(), committed_lines);
    assert!(
        !block.restore_committed(&mut context),
        "a block already at its committed constraint restores nothing"
    );
}

#[test]
fn a_block_reports_no_commit_until_one_has_happened() {
    let style = ahem_style();
    let items = [run(&style, "aaaa")];
    let mut context = text_context();
    let mut block = TextBlock::new(&mut context, BlockStyle::default(), &items, None);

    assert!(!block.has_committed());
    assert!(
        !block.is_probe_dirty(),
        "with nothing committed there is nothing to be dirty against"
    );

    block.probe(&mut context, BlockConstraint::new(Some(40.0), 0.0));
    assert!(
        !block.has_committed(),
        "a probe must not make a block look painted"
    );

    block.commit(&mut context, BlockConstraint::new(Some(40.0), 0.0));
    assert!(block.has_committed());
}

#[test]
fn re_asserting_an_unchanged_box_size_costs_nothing() {
    let style = ahem_style();
    let items = [
        run(&style, "aa"),
        InlineItem::Box(InlineBoxSpec {
            id: 1,
            size: Size::new(10.0, 10.0),
            baseline: None,
            vertical_align: VerticalAlign::Baseline,
        }),
    ];
    let mut context = text_context();
    let mut block = TextBlock::new(&mut context, BlockStyle::default(), &items, None);

    let constraint = BlockConstraint::new(Some(100.0), 0.0);
    block.commit(&mut context, constraint);
    let after_commit = block.break_count();

    block.set_box_size(1, Size::new(10.0, 10.0), None);
    block.probe(&mut context, constraint);
    assert_eq!(
        block.break_count(),
        after_commit,
        "a host re-asserting the same box size must not defeat the memo"
    );
    assert!(!block.is_probe_dirty());

    block.set_box_size(1, Size::new(30.0, 10.0), None);
    assert!(
        block.has_committed(),
        "a resized atom is not an unpainted block"
    );
    assert!(block.is_probe_dirty(), "and it owes a re-layout");
}

#[test]
fn min_content_width_accounts_for_a_resized_atom() {
    let style = ahem_style();
    let items = [
        run(&style, "aa"),
        InlineItem::Box(InlineBoxSpec {
            id: 1,
            size: Size::new(10.0, 10.0),
            baseline: None,
            vertical_align: VerticalAlign::Baseline,
        }),
    ];
    let mut context = text_context();
    let mut block = TextBlock::new(&mut context, BlockStyle::default(), &items, None);

    let cold = block.min_content_width(&mut context, 0.0);
    block.set_box_size(1, Size::new(80.0, 10.0), None);
    let after = block.min_content_width(&mut context, 0.0);

    assert!(
        after > cold,
        "a wider unbreakable atom raises the min-content width: {cold} -> {after}"
    );
}

fn pre<'src>(style: &'src RunStyle, text: &'src str) -> InlineItem<'src> {
    InlineItem::Run(TextRunItem {
        text,
        style,
        white_space: WhiteSpaceCollapse::Preserve,
        text_security: TextSecurity::None,
    })
}

fn only_box(block: &TextBlock, start: u32, end: u32) -> Rect<f32> {
    let boxes = block.range_boxes(start, end).expect("a valid range");
    assert_eq!(boxes.len(), 1, "one line's worth of [{start}, {end})");
    boxes[0]
}

#[test]
fn a_truncated_display_keeps_the_tab_stops() {
    let style = ahem_style();
    let mut context = text_context();
    let block = laid_out(
        &mut context,
        BlockStyle {
            max_chars: Some(2),
            overflow: TextOverflow::Ellipsis,
            ..BlockStyle::default()
        },
        &[pre(&style, "\tabc")],
        None,
        None,
    );
    assert!(block.truncated());
    assert_close(only_box(&block, 1, 2).origin.x, 20.0);
    assert_close(block.size().width, 60.0);
}

#[test]
fn the_last_baseline_is_the_last_reported_line_s() {
    let style = ahem_style();
    let mut context = text_context();

    let one = laid_out(
        &mut context,
        BlockStyle::default(),
        &[run(&style, "aaaa")],
        None,
        None,
    );
    assert_eq!(one.last_baseline(), Some(8.0));
    assert_eq!(one.last_baseline(), one.first_baseline());

    let mut block = TextBlock::new(
        &mut context,
        BlockStyle::default(),
        &[run(&style, "aaaa bbbb cccc")],
        None,
    );
    let metrics = block.commit(&mut context, BlockConstraint::new(Some(50.0), 0.0));
    assert_eq!(block.lines().len(), 3);
    assert_eq!(metrics.first_baseline, Some(8.0));
    assert_eq!(metrics.last_baseline, Some(28.0));
    assert_eq!(block.last_baseline(), Some(28.0));

    let clamped = laid_out(
        &mut context,
        BlockStyle {
            max_lines: core::num::NonZeroU32::new(2),
            ..BlockStyle::default()
        },
        &[run(&style, "aaaa bbbb cccc")],
        None,
        Some(50.0),
    );
    assert_eq!(clamped.lines().len(), 2);
    assert_eq!(clamped.last_baseline(), Some(18.0));

    let empty = laid_out(&mut context, BlockStyle::default(), &[], None, Some(50.0));
    assert_eq!(empty.last_baseline(), None);
}

#[test]
fn the_strut_is_the_root_inline_box_every_line_starts_with() {
    let small = RunStyle {
        font_size: 5.0,
        ..ahem_style()
    };
    let with_strut = BlockStyle {
        strut: Some(ahem_style()),
        ..BlockStyle::default()
    };
    let mut context = text_context();
    let block = laid_out(
        &mut context,
        with_strut.clone(),
        &[run(&small, "aa")],
        None,
        None,
    );
    assert_close(block.lines()[0].height, 10.0);
    assert_close(block.lines()[0].baseline, 8.0);
    let block = laid_out(
        &mut context,
        BlockStyle::default(),
        &[run(&small, "aa")],
        None,
        None,
    );
    assert_close(block.lines()[0].height, 5.0);

    let items = [atom(1, 10.0, 6.0, VerticalAlign::Baseline)];
    let block = laid_out(&mut context, with_strut, &items, None, None);
    assert_close(block.lines()[0].height, 10.0);
    assert_close(block.lines()[0].baseline, 8.0);
    let (_, origin, _) = visible_box(&block, 1);
    assert_close(origin.y, 2.0);
    let block = laid_out(&mut context, BlockStyle::default(), &items, None, None);
    assert_close(block.lines()[0].height, 6.0);
}

#[test]
fn preserved_runs_keep_spaces_and_their_forced_breaks() {
    let mut context = text_context();
    let style = ahem_style();
    for (text, width, lines) in [
        ("a  b", 40.0, 1),
        ("a\nb", 10.0, 2),
        ("a \nb", 10.0, 2),
        ("a\r\nb", 10.0, 2),
        ("a\rb", 30.0, 1),
    ] {
        let block = laid_out(
            &mut context,
            BlockStyle::default(),
            &[pre(&style, text)],
            None,
            Some(200.0),
        );
        assert_eq!(block.lines().len(), lines, "{text:?}");
        assert_close(block.size().width, width);
        assert_eq!(
            block.offset_map().display_text().len(),
            text.len(),
            "{text:?}"
        );
    }
}

#[test]
fn a_verbatim_offset_map_walks_units_and_bytes_together() {
    let mut context = text_context();
    let style = ahem_style();
    let text = "a  b\tc😀d";
    let block = laid_out(
        &mut context,
        BlockStyle::default(),
        &[run(&style, text)],
        None,
        Some(400.0),
    );
    let map = block.offset_map();
    assert_eq!(map.display_text().len(), text.len());
    assert_eq!(map.source_len(), 9);
    assert!(!map.truncated());
    assert_eq!(map.cut_byte(), text.len());
    let mut unit = 0;
    for (byte, character) in text.char_indices() {
        assert_eq!(map.unit_to_byte(unit), Some(byte), "unit {unit}");
        assert_eq!(map.byte_to_unit(byte), unit, "byte {byte}");
        assert_eq!(map.unit_before(byte), unit, "byte {byte}");
        unit += u32::try_from(character.len_utf16()).unwrap();
    }
    assert_eq!(map.unit_to_byte(7), Some(6));
    assert_eq!(map.unit_to_byte(9), None, "the end has no character");
    assert_eq!(map.unit_to_byte_or_end(9), Some(text.len()));
    assert_eq!(map.unit_to_byte_or_end(10), None);
    assert_eq!(map.byte_to_unit(text.len()), 9);
    assert_eq!(map.unit_before(text.len()), 9);
}

#[test]
fn a_truncated_offset_map_reports_its_cut() {
    let mut context = text_context();
    let style = ahem_style();
    let block = laid_out(
        &mut context,
        BlockStyle {
            max_lines: core::num::NonZeroU32::new(1),
            overflow: TextOverflow::Ellipsis,
            ..BlockStyle::default()
        },
        &[run(&style, "aaaa bbbb cccc")],
        None,
        Some(60.0),
    );
    assert!(block.truncated());
    let map = block.offset_map();
    assert!(map.truncated());
    assert!(map.cut_byte() < map.display_text().len());
    assert_eq!(map.unit_to_byte(0), Some(0));
    assert_eq!(
        map.unit_to_byte(12),
        Some(12),
        "past the cut: a content byte"
    );
}

#[test]
fn a_justified_clamp_frees_unjustified_width_for_the_truncation_content() {
    let mut context = text_context();
    let style = ahem_style();
    let block = laid_out(
        &mut context,
        BlockStyle {
            max_lines: core::num::NonZeroU32::new(1),
            text_align: TextAlign::Justify,
            ..BlockStyle::default()
        },
        &[run(&style, "aa bb cc")],
        Some(&[atom(9, 32.0, 10.0, VerticalAlign::Baseline)]),
        Some(55.0),
    );
    assert!(block.truncation_visible());
    let lines = block.lines();
    assert_eq!(lines.len(), 1);
    assert_eq!((lines[0].source_start, lines[0].source_end), (0, 5));
    assert_eq!(lines[0].ellipsis_count, 4, "cut after the first `a`");
}

#[test]
fn an_atomic_box_is_its_object_in_the_offset_map() {
    let mut context = text_context();
    let style = ahem_style();
    let block = laid_out(
        &mut context,
        BlockStyle::default(),
        &[
            run(&style, "ab"),
            atom(1, 10.0, 10.0, VerticalAlign::Baseline),
            run(&style, "cd"),
        ],
        None,
        Some(400.0),
    );
    let map = block.offset_map();
    assert_eq!(map.display_text(), "abcd");
    assert_eq!(map.source_len(), 5);
    assert!(!map.truncated());
    assert_eq!(map.cut_byte(), 7, "the text and the box's object");
    let layout = block.display();
    for (unit, byte) in [(0, 0), (1, 1), (2, 2), (3, 5), (4, 6)] {
        assert_eq!(map.unit_to_byte(unit), Some(byte), "unit {unit}");
        assert_eq!(map.byte_to_unit(byte), unit, "byte {byte}");
        let cluster = parley::Cluster::from_byte_index(layout, byte).expect("a cluster");
        assert_eq!(cluster.text_range().start, byte, "unit {unit}");
    }
    assert_eq!(map.unit_to_byte_or_end(5), Some(7));
    assert_eq!(map.byte_to_unit(3), 2);
    assert_eq!(map.byte_to_unit(7), 5);
    for (byte, unit) in [(2, 2), (3, 2), (5, 3), (6, 4), (7, 5)] {
        assert_eq!(map.unit_before(byte), unit, "byte {byte}");
    }
}

#[test]
fn a_first_line_offset_map_shows_no_unit_past_its_line() {
    let mut context = text_context();
    let style = ahem_style();
    let block = laid_out(
        &mut context,
        BlockStyle {
            text_wrap: TextWrap::NoWrap,
            ..BlockStyle::default()
        },
        &[run(&style, "ab\ncd")],
        None,
        Some(400.0),
    );
    let map = block.offset_map();
    assert_eq!(map.display_text(), "ab\n");
    assert_eq!(map.source_len(), 5);
    assert_eq!(map.unit_to_byte(2), Some(2));
    assert_eq!(map.unit_to_byte_or_end(3), Some(3));
    assert_eq!(map.unit_to_byte_or_end(4), None);
    assert_eq!(map.unit_to_byte_or_end(5), None);
}
