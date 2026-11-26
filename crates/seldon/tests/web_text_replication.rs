//! Replications of lynx-stack's web-platform text fixtures, on Ahem geometry.
//!
//! Each test names the lynx-stack case it replicates and asserts what
//! `web-core` renders for it — the compatibility target of `AGENTS.md`, not
//! what this engine does today. A replica whose reference behavior does not
//! hold yet is `#[ignore]`d with the cause, never weakened.
//!
//! The one standing exception is the truncation marker, by the user's ruling of
//! 2026-09-14: this engine gates the `text-maxline` retreat and the
//! `text-maxlength` tail on `text-overflow: ellipsis`
//! (`crates/seldon/src/text/block/truncate.rs:95`, `:135`), and that gating is
//! now the intended behavior rather than a gap. `text-overflow`'s initial value
//! is `clip` and the Lynx UA sheet never declares it, so a bare `text-maxline`
//! or `text-maxlength` clamps or cuts with no marker at all. The tests below
//! that cover a marker therefore assert the gated behavior for the fixture's own
//! declarations and keep the reference's on record in prose: on `text-maxline`
//! this matches native Lynx (Harmony `text_shadow_node.cc:63-66`, `:162-164`;
//! Android `TextRenderer.java:185-200`; iOS `LynxTextRenderer.m:1013-1021`) and
//! diverges from `web-core`, which marks unconditionally (`x-text.css:216-230`,
//! `:239-241`; `XTextTruncation.ts:349-366`, which never reads `text-overflow`);
//! on `text-maxlength` it matches neither reference, since both append the tail
//! unconditionally (Android `TextRenderer.java:126-135`, "Ellipsis will be
//! appended disregarding the overflowing mode."; `x-text.css:191-194`). Each
//! such test also lays the same fixture out a second time with
//! `text-overflow: ellipsis` set, so the gated-open path — the marker the
//! references show — stays covered.
//!
//! A second exception is white space, by the user's ruling of 2026-10-16:
//! a `<text>` keeps its text verbatim, as every native backend does, where
//! `web-core` hands it to the browser's CSS white-space processing. Every
//! replica is laid out with that verbatim processing (`text_style`), and the
//! one whose claim is the browser's collapsing is ignored as a DEVIATION with
//! a native companion beside it.
//!
//! The fixtures are re-authored against the vendored Ahem face: every glyph
//! (CJK and punctuation included) advances one em, so a run of `n` characters
//! at `font_size` is exactly `n * font_size` wide and every break point is
//! exact. Font sizes and line heights are the fixtures' own; container widths
//! are the fixtures' own except where a note says otherwise.

use seldon::geometry::{Point, Size};
use seldon::text::block::{
    BlockConstraint, BlockStyle, InlineBoxSpec, InlineItem, LineHeight, PlacedBox, RunStyle,
    SourceItem, TextBlock, TextOverflow, TextRunItem, TextSecurity, TextWrap, VerticalAlign,
    WhiteSpaceCollapse, WordBreak,
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

fn ahem_at(font_size: f32) -> RunStyle {
    RunStyle {
        font_family: ahem_family(),
        font_size,
        ..RunStyle::default()
    }
}

fn ahem_at_line_height(font_size: f32, line_height: f32) -> RunStyle {
    RunStyle {
        line_height: LineHeight::Px(line_height),
        ..ahem_at(font_size)
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

fn atom(id: u64, width: f32, height: f32, vertical_align: VerticalAlign) -> InlineItem<'static> {
    InlineItem::Box(InlineBoxSpec {
        id,
        size: Size::new(width, height),
        baseline: None,
        vertical_align,
    })
}

fn text_style() -> BlockStyle {
    BlockStyle::default()
}

fn laid_out(
    context: &mut TextContext,
    style: BlockStyle,
    items: &[InlineItem<'_>],
    truncation: Option<&[InlineItem<'_>]>,
    width: Option<f32>,
) -> TextBlock {
    let mut block = TextBlock::new(context, style, items, truncation);
    block.commit(context, BlockConstraint::new(width, 0.0));
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

fn sources(block: &TextBlock) -> Vec<SourceItem> {
    (0..u16::try_from(block.display().styles().len()).expect("fits"))
        .map(|index| block.source_of(index))
        .collect()
}

fn has_ellipsis(sources: &[SourceItem]) -> bool {
    sources
        .iter()
        .any(|item| matches!(item, SourceItem::Ellipsis { .. }))
}

fn maxline_basic_items<'src>(
    lead: &'src str,
    base: &'src RunStyle,
    pink: &'src RunStyle,
    three_em: &'src RunStyle,
    tracked: &'src RunStyle,
) -> Vec<InlineItem<'src>> {
    vec![
        run(base, lead),
        run(base, " "),
        run(pink, "we could use inline-text to set color of some text"),
        run(base, " "),
        run(three_em, "also, font-size could be different"),
        run(base, " "),
        run(tracked, "additionally, letter-space could be different"),
    ]
}

fn maxline_lead(clamp: u32) -> String {
    format!("{clamp}hello world, this is a long enough text without any limitation.")
}

#[test]
fn a_lynx_wrapper_adds_no_box_and_no_break_to_a_clamped_paragraph() {
    let base = ahem_at(10.0);
    let three_em = ahem_at(30.0);
    let tracked = RunStyle {
        letter_spacing: 7.0,
        ..ahem_at(10.0)
    };
    let mut context = text_context();

    for clamp in [1u32, 2, 4, 6] {
        let lead = maxline_lead(clamp);
        let items = maxline_basic_items(&lead, &base, &base, &three_em, &tracked);
        let block = laid_out(
            &mut context,
            BlockStyle {
                word_break: WordBreak::BreakAll,
                max_lines: core::num::NonZeroU32::new(clamp),
                ..text_style()
            },
            &items,
            None,
            Some(300.0),
        );

        assert!(block.truncated(), "clamp {clamp} overflows 300px");
        assert_eq!(block.lines().len(), clamp as usize);
        assert!(
            block.boxes().is_empty(),
            "the wrapper contributes no atomic inline box",
        );

        if clamp == 6 {
            let lines = block.lines();
            assert_close(lines[0].height, 10.0);
            assert_close(lines[3].height, 30.0);
            assert_eq!(lines[3].source_end, lines[4].source_start);
            assert!(lines[2].source_end < lines[3].source_start);
        }
    }
}

#[test]
fn an_avatar_atom_and_a_short_run_fit_one_line_under_a_two_line_clamp() {
    let quote = ahem_at_line_height(15.0, 23.0);
    let items = [
        atom(1, 25.0, 18.0, VerticalAlign::Middle),
        run(&quote, "清透缎光妆感自然。"),
    ];
    let truncation = [run(&quote, "..."), atom(2, 21.0, 17.0, VerticalAlign::Top)];
    let mut context = text_context();
    let block = laid_out(
        &mut context,
        BlockStyle {
            max_lines: core::num::NonZeroU32::new(2),
            ..text_style()
        },
        &items,
        Some(&truncation),
        Some(375.0),
    );

    assert_eq!(block.lines().len(), 1);
    assert_close(block.size().width, 160.0);
    assert_close(block.lines()[0].height, 23.0);
    let (line, origin, _) = visible_box(&block, 1);
    assert_eq!(line, 0);
    assert_close(origin.x, 0.0);
    assert!(!block.truncated());
    assert!(!block.truncation_visible());
    assert_hidden(&block, 2);
}

#[test]
fn a_truncation_node_inside_an_inline_view_never_registers() {
    let base = ahem_at(10.0);
    let items = [
        atom(1, 0.0, 0.0, VerticalAlign::Baseline),
        run(
            &base,
            "4hello world, this is a long enough text without any limitation.",
        ),
    ];
    let mut context = text_context();
    let block = laid_out(
        &mut context,
        BlockStyle {
            word_break: WordBreak::BreakAll,
            max_lines: core::num::NonZeroU32::new(1),
            ..text_style()
        },
        &items,
        None,
        Some(300.0),
    );

    let (line, origin, _) = visible_box(&block, 1);
    assert_eq!(line, 0);
    assert_close(origin.x, 0.0);
    assert!(block.truncated());
    assert!(!block.truncation_visible());
    assert_eq!(block.lines().len(), 1);
    assert_eq!(
        (block.lines()[0].source_start, block.lines()[0].source_end),
        (0, 31),
        "the unmarked cut keeps every unit the line fits",
    );
    assert_eq!(block.lines()[0].ellipsis_count, 0);
    assert_close(block.lines()[0].advance, 300.0);
    assert!(!has_ellipsis(&sources(&block)));

    let marked = laid_out(
        &mut context,
        BlockStyle {
            word_break: WordBreak::BreakAll,
            max_lines: core::num::NonZeroU32::new(1),
            overflow: TextOverflow::Ellipsis,
            ..text_style()
        },
        &items,
        None,
        Some(300.0),
    );
    assert_eq!(marked.lines().len(), 1);
    assert_eq!(marked.lines().len(), 1);
    assert_eq!(
        (marked.lines()[0].source_start, marked.lines()[0].source_end),
        (0, 31),
        "the range is the pre-truncation span either way",
    );
    assert_eq!(
        marked.lines()[0].ellipsis_count,
        3,
        "and the cut is three units back from its end",
    );
    assert!(has_ellipsis(&sources(&marked)));
    assert_close(marked.lines()[0].advance, 300.0);
    assert!(!marked.truncation_visible());
}

const MAXLENGTH_ROWS: [(char, u32, &[&str], f32, f32); 9] = [
    ('a', 1, &["1"], 10.0, 10.0),
    ('b', 1, &["12"], 40.0, 10.0),
    ('c', 1, &["123"], 40.0, 10.0),
    ('d', 1, &["1", "2", "3"], 40.0, 10.0),
    ('e', 2, &["1", "2", "3", "4", "5"], 50.0, 20.0),
    ('f', 3, &["简", "体", "中文"], 60.0, 30.0),
    ('g', 3, &["1", "2", "3", "4", "5"], 60.0, 30.0),
    ('h', 4, &["1", "2", "3", "4", "5"], 70.0, 40.0),
    ('i', 0, &["1", "2", "3", "4", "5"], 30.0, 0.0),
];

#[test]
fn maxlength_cuts_the_flattened_run_and_tails_it_with_three_dots() {
    let base = ahem_at(10.0);
    let mut context = text_context();

    for (row, max_chars, parts, tailed, untailed) in MAXLENGTH_ROWS {
        let items = parts
            .iter()
            .map(|part| run(&base, part))
            .collect::<Vec<_>>();
        let style = |overflow| BlockStyle {
            max_chars: Some(max_chars),
            overflow,
            ..text_style()
        };
        let source_len: u32 = parts
            .iter()
            .map(|part| u32::try_from(part.chars().count()).expect("fits"))
            .sum();

        let block = laid_out(&mut context, style(TextOverflow::Clip), &items, None, None);
        assert_eq!(block.truncated(), max_chars < source_len, "row {row}");
        assert_eq!(block.lines().len(), 1, "row {row}");
        assert!(
            !has_ellipsis(&sources(&block)),
            "row {row} appends no dots without `text-overflow: ellipsis`",
        );
        assert_close(block.size().width, untailed);

        let tailed_block = laid_out(
            &mut context,
            style(TextOverflow::Ellipsis),
            &items,
            None,
            None,
        );
        assert_eq!(
            has_ellipsis(&sources(&tailed_block)),
            tailed_block.truncated(),
            "row {row} tails exactly the cuts it makes",
        );
        assert_eq!(tailed_block.lines().len(), 1, "row {row}");
        assert_close(tailed_block.size().width, tailed);
    }
}

#[test]
fn tail_color_convert_false_splices_the_dots_into_the_cut_run() {
    let base = ahem_at(10.0);
    let mut context = text_context();

    for (row, max_chars, parts, tailed, untailed) in MAXLENGTH_ROWS {
        let items = parts
            .iter()
            .map(|part| run(&base, part))
            .collect::<Vec<_>>();
        let style = |overflow| BlockStyle {
            max_chars: Some(max_chars),
            overflow,
            ..text_style()
        };

        let block = laid_out(&mut context, style(TextOverflow::Clip), &items, None, None);
        assert_close(block.size().width, untailed);
        assert_close(
            block.lines()[0].advance,
            if row == 'i' { 50.0 } else { untailed },
        );
        assert!(
            !has_ellipsis(&sources(&block)),
            "row {row} splices in no dots without `text-overflow: ellipsis`",
        );

        let spliced = laid_out(
            &mut context,
            style(TextOverflow::Ellipsis),
            &items,
            None,
            None,
        );
        assert_close(spliced.lines()[0].advance, tailed);
        assert_eq!(
            has_ellipsis(&sources(&spliced)),
            spliced.truncated(),
            "row {row} carries the dots as inline content",
        );
    }

    let wide = ahem_at(20.0);
    let adapted = [
        run(&base, "1"),
        run(&wide, "2"),
        run(&base, "3"),
        run(&base, "4"),
        run(&base, "5"),
    ];
    let adapted_style = |overflow| BlockStyle {
        max_chars: Some(2),
        overflow,
        ..text_style()
    };
    let block = laid_out(
        &mut context,
        adapted_style(TextOverflow::Clip),
        &adapted,
        None,
        None,
    );
    assert!(block.truncated());
    assert_close(block.size().width, 30.0);

    let block = laid_out(
        &mut context,
        adapted_style(TextOverflow::Ellipsis),
        &adapted,
        None,
        None,
    );
    assert!(block.truncated());
    assert_close(block.size().width, 90.0);
}

#[test]
fn a_bare_maxline_clamp_marks_its_last_visible_line() {
    let base = ahem_at(10.0);
    let three_em = ahem_at(30.0);
    let tracked = RunStyle {
        letter_spacing: 7.0,
        ..ahem_at(10.0)
    };
    let mut context = text_context();

    for (clamp, clipped, marked) in [
        (1u32, 300.0f32, 300.0f32),
        (2, 300.0, 300.0),
        (4, 290.0, 270.0),
        (6, 300.0, 300.0),
    ] {
        let lead = maxline_lead(clamp);
        let items = maxline_basic_items(&lead, &base, &base, &three_em, &tracked);
        let style = |overflow| BlockStyle {
            word_break: WordBreak::BreakAll,
            max_lines: core::num::NonZeroU32::new(clamp),
            overflow,
            ..text_style()
        };

        let block = laid_out(
            &mut context,
            style(TextOverflow::Clip),
            &items,
            None,
            Some(300.0),
        );
        assert!(block.truncated(), "clamp {clamp}");
        assert_eq!(block.lines().len(), clamp as usize);
        let last = *block.lines().last().expect("a clamped line");
        assert_eq!(
            last.ellipsis_count, 0,
            "clamp {clamp} gives up no unit to a marker it does not draw",
        );
        assert_close(last.advance, clipped);
        assert!(
            !has_ellipsis(&sources(&block)),
            "clamp {clamp} draws no dots without `text-overflow: ellipsis`",
        );

        let marked_block = laid_out(
            &mut context,
            style(TextOverflow::Ellipsis),
            &items,
            None,
            Some(300.0),
        );
        assert_eq!(marked_block.lines().len(), clamp as usize);
        let last = *marked_block.lines().last().expect("a clamped line");
        assert_eq!(
            last.ellipsis_count, 3,
            "clamp {clamp} backs the cut off by the three units the dots take",
        );
        assert_close(last.advance, marked);
        assert!(has_ellipsis(&sources(&marked_block)));
    }
}

#[test]
fn tail_color_convert_false_backs_the_maxline_cut_off_by_three_units() {
    let base = ahem_at(10.0);
    let three_em = ahem_at(30.0);
    let tracked = RunStyle {
        letter_spacing: 7.0,
        ..ahem_at(10.0)
    };
    let mut context = text_context();

    let mut advances = Vec::new();
    for clamp in [1u32, 2, 4, 6] {
        let lead = maxline_lead(clamp);
        let items = maxline_basic_items(&lead, &base, &base, &three_em, &tracked);
        let style = |overflow| BlockStyle {
            word_break: WordBreak::BreakAll,
            max_lines: core::num::NonZeroU32::new(clamp),
            overflow,
            ..text_style()
        };

        let block = laid_out(
            &mut context,
            style(TextOverflow::Clip),
            &items,
            None,
            Some(300.0),
        );
        let last = *block.lines().last().expect("a clamped line");
        assert_eq!(
            last.ellipsis_count, 0,
            "clamp {clamp} reserves nothing for dots it does not lay in",
        );
        assert!(
            !has_ellipsis(&sources(&block)),
            "clamp {clamp} lays in no dots without `text-overflow: ellipsis`",
        );

        let block = laid_out(
            &mut context,
            style(TextOverflow::Ellipsis),
            &items,
            None,
            Some(300.0),
        );
        let marked = *block.lines().last().expect("a clamped line");
        assert_eq!(
            marked.ellipsis_count, 3,
            "clamp {clamp} reserves three units"
        );
        assert!(has_ellipsis(&sources(&block)));
        advances.push((last.advance, marked.advance));
    }

    assert_close(advances[0].0, 300.0);
    assert_close(advances[3].0, 300.0);
    assert_close(advances[0].1, 300.0);
    assert_close(advances[3].1, 300.0);
}

#[test]
fn custom_truncation_content_replaces_the_marker_at_the_clamp() {
    let base = ahem_at(10.0);
    let three_em = ahem_at(30.0);
    let tracked = RunStyle {
        letter_spacing: 7.0,
        ..ahem_at(10.0)
    };
    let truncation = [run(&base, "inline-truncation")];
    let mut context = text_context();

    let unclamped_items = maxline_basic_items(
        "hello world, this is a long enough text without any limitation.",
        &base,
        &base,
        &three_em,
        &tracked,
    );
    let unclamped = laid_out(
        &mut context,
        BlockStyle {
            word_break: WordBreak::BreakAll,
            ..text_style()
        },
        &unclamped_items,
        Some(&truncation),
        Some(300.0),
    );
    assert!(!unclamped.truncated());
    assert!(!unclamped.truncation_visible());
    assert!(!sources(&unclamped).contains(&SourceItem::Truncation(0)));

    for (clamp, freed) in [(1u32, 17u32), (2, 17), (4, 15), (6, 6)] {
        let lead = maxline_lead(clamp);
        let items = maxline_basic_items(&lead, &base, &base, &three_em, &tracked);
        let block = laid_out(
            &mut context,
            BlockStyle {
                word_break: WordBreak::BreakAll,
                max_lines: core::num::NonZeroU32::new(clamp),
                ..text_style()
            },
            &items,
            Some(&truncation),
            Some(300.0),
        );

        assert!(block.truncated(), "clamp {clamp}");
        assert!(block.truncation_visible(), "clamp {clamp}");
        assert_eq!(block.lines().len(), clamp as usize);
        let source = sources(&block);
        assert!(source.contains(&SourceItem::Truncation(0)), "clamp {clamp}");
        assert!(
            !has_ellipsis(&source),
            "clamp {clamp} shows no dots beside the custom content",
        );
        let last = *block.lines().last().expect("a clamped line");
        assert_eq!(last.ellipsis_count, freed, "clamp {clamp}");
        assert!(last.advance <= 300.0 + EPSILON);
    }
}

#[test]
fn content_that_just_fits_the_clamp_leaves_the_truncation_content_unused() {
    let body = ahem_at_line_height(13.0, 20.0);
    let items = [run(
        &body,
        "活动规则：在04.17-05.17期间，带话题#暑假科普手抄报 发布优质原创图文作品，图片数量≥3张，上榜作者能赢取周边",
    )];
    let truncation = [atom(1, 12.0, 12.0, VerticalAlign::Baseline)];
    let mut context = text_context();
    let block = laid_out(
        &mut context,
        BlockStyle {
            max_lines: core::num::NonZeroU32::new(2),
            overflow: TextOverflow::Ellipsis,
            ..text_style()
        },
        &items,
        Some(&truncation),
        Some(390.0),
    );

    assert!(!block.truncated());
    assert!(!block.truncation_visible());
    assert_eq!(block.lines().len(), 2);
    assert_eq!(block.lines()[1].ellipsis_count, 0);
    assert!(!has_ellipsis(&sources(&block)));
    assert_hidden(&block, 1);
}

#[test]
fn a_truncation_image_is_fitted_by_its_box_width() {
    let body = ahem_at_line_height(13.0, 20.0);
    let items = [run(
        &body,
        "活动规则：在04.17-05.17期间，带话题#暑假科普手抄报 发布优质原创图文作品，图片数量≥3张，上榜作者能赢取周边",
    )];
    let truncation = [atom(1, 12.0, 12.0, VerticalAlign::Baseline)];
    let mut context = text_context();
    let block = laid_out(
        &mut context,
        BlockStyle {
            max_lines: core::num::NonZeroU32::new(1),
            overflow: TextOverflow::Ellipsis,
            ..text_style()
        },
        &items,
        Some(&truncation),
        Some(390.0),
    );

    assert!(block.truncated());
    assert!(block.truncation_visible());
    assert_eq!(block.lines().len(), 1);
    assert_close(block.lines()[0].height, 20.0);
    assert_eq!(block.lines()[0].ellipsis_count, 2);
    assert!(!has_ellipsis(&sources(&block)));
    let (line, origin, size) = visible_box(&block, 1);
    assert_eq!(line, 0);
    assert!(
        origin.x + size.width <= 390.0 + EPSILON,
        "the image stays inside the block width",
    );
}

#[test]
fn a_paragraph_that_opens_with_inline_view_boxes_still_finds_its_cut() {
    let quote = ahem_at_line_height(15.0, 23.0);
    let items = [
        atom(1, 25.0, 18.0, VerticalAlign::Middle),
        run(
            &quote,
            "零跑C10开了17000公里，8295车机流畅，智享版辅助驾驶完全够用，没必要硬上顶配。",
        ),
        atom(2, 15.0, 17.0, VerticalAlign::Top),
    ];
    let truncation = [run(&quote, "..."), atom(3, 21.0, 17.0, VerticalAlign::Top)];
    let mut context = text_context();
    let style = || BlockStyle {
        max_lines: core::num::NonZeroU32::new(2),
        overflow: TextOverflow::Ellipsis,
        ..text_style()
    };

    let fits = laid_out(
        &mut context,
        style(),
        &items,
        Some(&truncation),
        Some(390.0),
    );
    assert!(!fits.truncated());
    assert!(!fits.truncation_visible());
    assert_eq!(fits.lines().len(), 2);
    let (line, origin, _) = visible_box(&fits, 1);
    assert_eq!(line, 0);
    assert_close(origin.x, 0.0);
    let (line, origin, size) = visible_box(&fits, 2);
    assert_eq!(line, 1);
    assert!(origin.x + size.width <= 390.0 + EPSILON);
    assert_hidden(&fits, 3);
    assert!(!sources(&fits).contains(&SourceItem::Truncation(0)));

    let block = laid_out(
        &mut context,
        style(),
        &items,
        Some(&truncation),
        Some(300.0),
    );

    assert!(block.truncated());
    assert!(block.truncation_visible());
    assert_eq!(block.lines().len(), 2);
    let (line, origin, _) = visible_box(&block, 1);
    assert_eq!(line, 0);
    assert_close(origin.x, 0.0);
    assert_hidden(&block, 2);
    let (line, origin, size) = visible_box(&block, 3);
    assert_eq!(line, 1);
    assert!(origin.x + size.width <= 300.0 + EPSILON);
    assert!(sources(&block).contains(&SourceItem::Truncation(0)));
}

#[test]
fn a_leading_image_survives_a_cut_that_retreats_only_trailing_content() {
    let body = ahem_at_line_height(24.0, 32.0);
    let more = ahem_at_line_height(14.0, 22.0);
    let text = "大学".repeat(5) + " " + &"大学".repeat(50);
    let items = [
        atom(1, 36.0, 18.0, VerticalAlign::Baseline),
        run(&body, &text),
    ];
    let truncation = [
        run(&more, "更多"),
        atom(2, 12.0, 12.0, VerticalAlign::Baseline),
    ];
    let mut context = text_context();
    let block = laid_out(
        &mut context,
        BlockStyle {
            max_lines: core::num::NonZeroU32::new(3),
            overflow: TextOverflow::Ellipsis,
            ..text_style()
        },
        &items,
        Some(&truncation),
        Some(300.0),
    );

    assert!(block.truncated());
    assert!(block.truncation_visible());
    assert_eq!(block.lines().len(), 3);
    let (line, origin, _) = visible_box(&block, 1);
    assert_eq!(line, 0, "the leading image heads the first line");
    assert_close(origin.x, 0.0);
    assert_eq!(block.lines()[0].ellipsis_count, 0);
    assert_eq!(block.lines()[1].ellipsis_count, 0);
    assert_eq!(block.lines()[2].ellipsis_count, 2);
    let source = sources(&block);
    assert!(source.contains(&SourceItem::Truncation(0)));
    assert!(!has_ellipsis(&source));
    let (line, origin, size) = visible_box(&block, 2);
    assert_eq!(line, 2);
    assert!(origin.x + size.width <= 300.0 + EPSILON);
}

#[test]
fn a_runs_line_height_is_not_taken_from_the_span_after_it() {
    let body = ahem_at_line_height(24.0, 32.0);
    let more = ahem_at_line_height(14.0, 22.0);
    let text = "大学".repeat(5) + " " + &"大学".repeat(50);
    let items = [
        atom(1, 36.0, 18.0, VerticalAlign::Baseline),
        run(&body, &text),
    ];
    let truncation = [
        run(&more, "更多"),
        atom(2, 12.0, 12.0, VerticalAlign::Baseline),
    ];
    let mut context = text_context();
    let block = laid_out(
        &mut context,
        BlockStyle {
            max_lines: core::num::NonZeroU32::new(3),
            overflow: TextOverflow::Ellipsis,
            ..text_style()
        },
        &items,
        Some(&truncation),
        Some(300.0),
    );

    assert!(block.truncation_visible());
    assert_eq!(block.lines().len(), 3);
    let heights = block
        .lines()
        .iter()
        .map(|line| line.height)
        .collect::<Vec<_>>();
    assert_eq!(heights, vec![32.0, 32.0, 32.0]);

    let plain = [run(&body, "AAAAAAAAAA"), run(&more, "BBBBB")];
    let plain = laid_out(
        &mut context,
        BlockStyle {
            strut: Some(ahem_at_line_height(10.0, 10.0)),
            ..text_style()
        },
        &plain,
        None,
        Some(240.0),
    );
    assert_eq!(plain.lines().len(), 2);
    assert_close(plain.lines()[0].height, 32.0);
    assert_close(plain.lines()[1].height, 22.0);
}

#[test]
fn an_overflowing_nowrap_line_is_marked_by_text_overflow_ellipsis() {
    let body = ahem_at_line_height(15.0, 36.0);
    let items = [run(&body, "曼联积分榜最新排行榜")];
    let mut context = text_context();
    let block = laid_out(
        &mut context,
        BlockStyle {
            text_wrap: TextWrap::NoWrap,
            overflow: TextOverflow::Ellipsis,
            ..text_style()
        },
        &items,
        None,
        Some(100.0),
    );

    assert_eq!(block.lines().len(), 1);
    assert_close(block.lines()[0].height, 36.0);
    assert!(block.truncated(), "the 150px run overflows its 100px box");
    assert!(
        block.lines()[0].advance <= 100.0 + EPSILON,
        "the marked line fits the clip edge, got {}",
        block.lines()[0].advance,
    );
    assert!(block.lines()[0].ellipsis_count > 0);
}

#[test]
fn an_atom_clipped_past_the_clamp_leaves_the_paint_list() {
    let base = ahem_at(10.0);
    let small = ahem_at(8.0);
    let items = [
        run(&base, "this is text I know you your past your future"),
        atom(1, 168.0, 24.0, VerticalAlign::Baseline),
    ];
    let truncation = [run(&small, "truncation")];
    let mut context = text_context();
    let block = laid_out(
        &mut context,
        BlockStyle {
            max_lines: core::num::NonZeroU32::new(2),
            ..text_style()
        },
        &items,
        Some(&truncation),
        Some(100.0),
    );

    assert!(block.truncated());
    assert!(block.truncation_visible());
    assert_eq!(block.lines().len(), 2);
    assert_hidden(&block, 1);
    let source = sources(&block);
    assert!(source.contains(&SourceItem::Truncation(0)));
    assert!(!has_ellipsis(&source));
    assert!(block.size().width <= 100.0 + EPSILON);
}

type MaxlengthRow<'src> = (char, Option<u32>, Vec<&'src str>, f32, f32);

fn paragraph() -> String {
    let text: String = "lorem ipsum dolor sit amet "
        .repeat(10)
        .chars()
        .take(244)
        .collect();
    assert_eq!(text.chars().count(), 244);
    text
}

#[test]
fn maxlength_counts_units_across_nested_runs_and_scripts() {
    let base = ahem_at(10.0);
    let para = paragraph();
    let mut context = text_context();

    let rows: [MaxlengthRow<'_>; 9] = [
        ('a', Some(1), vec![para.as_str()], 40.0, 10.0),
        ('b', Some(1), vec!["简体中文"], 40.0, 10.0),
        ('c', Some(2), vec![para.as_str()], 50.0, 20.0),
        ('d', Some(2), vec!["简体中文"], 50.0, 20.0),
        ('e', Some(3), vec!["简", "体", "中文"], 60.0, 30.0),
        ('f', Some(3), vec!["简", "体中", "文"], 60.0, 30.0),
        ('g', Some(3), vec!["简", "体中文"], 60.0, 30.0),
        ('h', Some(200), vec![para.as_str()], 2030.0, 2000.0),
        ('i', None, vec![para.as_str()], 2440.0, 2440.0),
    ];
    for (row, max_chars, parts, tailed, untailed) in rows {
        let items = parts
            .iter()
            .map(|part| run(&base, part))
            .collect::<Vec<_>>();
        let style = |overflow| BlockStyle {
            max_chars,
            overflow,
            ..text_style()
        };

        let block = laid_out(&mut context, style(TextOverflow::Clip), &items, None, None);
        assert_eq!(block.truncated(), max_chars.is_some(), "row {row}");
        assert!(
            !has_ellipsis(&sources(&block)),
            "row {row} appends no tail without `text-overflow: ellipsis`",
        );
        assert_close(block.size().width, untailed);

        let tailed_block = laid_out(
            &mut context,
            style(TextOverflow::Ellipsis),
            &items,
            None,
            None,
        );
        assert_eq!(
            has_ellipsis(&sources(&tailed_block)),
            max_chars.is_some(),
            "row {row} tails exactly the cuts it makes",
        );
        assert_close(tailed_block.size().width, tailed);
    }
}

#[test]
#[ignore = "DEVIATION: this engine follows native Lynx, by the user's ruling \
            of 2026-09-15. `tail-color-convert` is a boolean that defaults to \
            false (Android `TextRenderer.convertTailColor`, iOS \
            `LynxTextRenderer.m overrideTruncatedAttrIfNeed`), and even when \
            it is true only the marker's foreground colour is swapped — the \
            dots keep the font of the run the cut landed in, so the advance \
            here is 110 rather than web-core's 80. \
            `the_truncation_tail_keeps_the_cut_run_s_font_under_the_native_default` \
            asserts that geometry and passes."]
fn the_truncation_tail_takes_the_block_style_not_the_cut_runs() {
    let outer = ahem_at(10.0);
    let nested = ahem_at(20.0);
    let items = [run(&outer, "简"), run(&nested, "体中文")];
    let mut context = text_context();

    let cut = laid_out(
        &mut context,
        BlockStyle {
            max_chars: Some(3),
            overflow: TextOverflow::Ellipsis,
            ..text_style()
        },
        &items,
        None,
        None,
    );
    assert!(cut.truncated());
    assert_close(cut.size().width, 80.0);

    let intact = laid_out(
        &mut context,
        BlockStyle {
            max_chars: Some(10),
            overflow: TextOverflow::Ellipsis,
            ..text_style()
        },
        &items,
        None,
        None,
    );
    assert!(!intact.truncated());
    assert_close(intact.size().width, 70.0);
}

#[test]
fn the_truncation_tail_keeps_the_cut_run_s_font_under_the_native_default() {
    let outer = ahem_at(10.0);
    let nested = ahem_at(20.0);
    let items = [run(&outer, "简"), run(&nested, "体中文")];
    let mut context = text_context();

    let cut = laid_out(
        &mut context,
        BlockStyle {
            max_chars: Some(3),
            overflow: TextOverflow::Ellipsis,
            ..text_style()
        },
        &items,
        None,
        None,
    );
    assert!(cut.truncated());
    assert_close(cut.size().width, 110.0);
    assert!(
        sources(&cut).contains(&SourceItem::Ellipsis { item: 1 }),
        "the marker names the nested run it was shaped in, which is what the \
         painter resolves its colour through",
    );
}

#[test]
fn word_break_policy_at_a_hundred_pixel_width() {
    let base = ahem_at(10.0);
    let mut context = text_context();

    for (row, text, expected) in [
        ('a', "12345678901234567890", &[(0u32, 10u32), (10, 20)][..]),
        (
            'b',
            "12345 67890 12345 67890",
            &[(0, 5), (6, 11), (12, 17), (18, 23)][..],
        ),
        (
            'c',
            "你好世界你好世界你好世界你好世界",
            &[(0, 10), (10, 16)][..],
        ),
        (
            'd',
            "abcdefghijklmnopqrstu",
            &[(0, 10), (10, 20), (20, 21)][..],
        ),
        (
            'e',
            "abcde fghij klmno pqrstu",
            &[(0, 5), (6, 11), (12, 17), (18, 24)][..],
        ),
        (
            'f',
            "你好世界 你好世界 你好世界 你好世界",
            &[(0, 9), (10, 19)][..],
        ),
    ] {
        let items = [run(&base, text)];
        let block = laid_out(&mut context, text_style(), &items, None, Some(100.0));

        let ranges = block
            .lines()
            .iter()
            .map(|line| (line.source_start, line.source_end))
            .collect::<Vec<_>>();
        assert_eq!(ranges, expected, "row {row}");
        assert!(!block.truncated(), "row {row}");
    }
}

#[test]
#[ignore = "DEVIATION: this engine follows native Lynx, by the user's ruling \
            of 2026-10-16. A `<text>` does no CSS white-space processing on \
            any native backend (`raw_text_element.cc:16-46`, Android \
            `BaseTextShadowNode.java:494-498`, iOS \
            `LynxBaseTextShadowNode.m:240-249`), so the two leading spaces stay \
            and the line is 130 wide rather than web-core's 110. \
            `leading_spaces_stay_in_a_text_the_way_native_lynx_keeps_them` \
            asserts that geometry and passes."]
fn collapsible_whitespace_is_removed_at_the_start_of_a_line() {
    let base = ahem_at(10.0);
    let items = [run(&base, "  Step 1 of 4")];
    let mut context = text_context();
    let block = laid_out(&mut context, text_style(), &items, None, None);

    assert_eq!(block.lines().len(), 1);
    assert_eq!(
        (block.lines()[0].source_start, block.lines()[0].source_end),
        (0, 13),
        "the source range counts pre-collapse units",
    );
    assert_close(block.size().width, 110.0);
}

#[test]
fn leading_spaces_stay_in_a_text_the_way_native_lynx_keeps_them() {
    let base = ahem_at(10.0);
    let items = [run(&base, "  Step 1 of 4")];
    let mut context = text_context();
    let block = laid_out(&mut context, text_style(), &items, None, None);

    assert_eq!(block.lines().len(), 1);
    assert_eq!(
        (block.lines()[0].source_start, block.lines()[0].source_end),
        (0, 13),
    );
    assert_close(block.size().width, 130.0);
}
