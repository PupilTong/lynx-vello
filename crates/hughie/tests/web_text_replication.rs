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
//! (`crates/hughie/src/text/block/truncate.rs:95`, `:135`), and that gating is
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
//! The fixtures are re-authored against the vendored Ahem face: every glyph
//! (CJK and punctuation included) advances one em, so a run of `n` characters
//! at `font_size` is exactly `n * font_size` wide and every break point is
//! exact. Font sizes and line heights are the fixtures' own; container widths
//! are the fixtures' own except where a note says otherwise.

use hughie::geometry::{Point, Size};
use hughie::text::block::{
    BlockConstraint, BlockStyle, InlineBoxSpec, InlineItem, LineHeight, PlacedBox, RunStyle,
    SourceItem, TextBlock, TextOverflow, TextRunItem, TextWrap, VerticalAlign, WordBreak,
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

/// Ahem at one declared `font-size`, with `line-height: normal`.
fn ahem_at(font_size: f32) -> RunStyle {
    RunStyle {
        font_family: ahem_family(),
        font_size,
        ..RunStyle::default()
    }
}

/// Ahem at one declared `font-size` / `line-height` pair, in px, the way the
/// ReactLynx-emitted fixtures write them.
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
        preserve_newlines: false,
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
    // A test reads the laid-out result, which is a commit's to produce.
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

/// The paint identity of every style the display layout carries.
fn sources(block: &TextBlock) -> Vec<SourceItem> {
    (0..u16::try_from(block.display().styles().len()).expect("fits"))
        .map(|index| block.source_of(index))
        .collect()
}

/// The four `.text` paragraphs of the `text-maxline-basic` family, as the one
/// flattened run sequence they reach a paragraph engine as: the leading raw
/// text, a pink inline text, a `3em` one and a `letter-spacing: 7px` one,
/// separated by the collapsing inter-child whitespace the fixtures'
/// indentation produces.
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

/// The leading raw text of the `text-maxline` paragraphs. Its digit prefix is
/// what identifies the clamp in the fixtures' screenshots.
fn maxline_lead(clamp: u32) -> String {
    format!("{clamp}hello world, this is a long enough text without any limitation.")
}

// ---------------------------------------------------------------------------
// `x-text` structure: what the paragraph is made of.
// ---------------------------------------------------------------------------

/// Replicates `x-text/text-maxline-basic-with-lynx-wrapper`
/// (`web-elements/tests/fixtures/x-text/text-maxline-basic-with-lynx-wrapper.html`,
/// `web-elements.spec.ts:212`): a `lynx-wrapper` around two of the inline runs
/// adds no box and no break to the inline formatting context, mixed run
/// metrics drive per-line heights, and `word-break: break-all` breaks inside a
/// word.
///
/// The wrapper is `display: contents`, so it reaches this layer only as the
/// absence of anything: the runs it held arrive flattened, in source order,
/// with no atomic inline box of their own. The marker question the same clamp
/// raises is deliberately factored out — the fixture declares no
/// `text-overflow`, and `x-text/text-maxline-basic` below is where that is
/// pinned — so this test keeps the initial `clip` and asserts the clamp
/// geometry, which the marker does not move.
///
/// Recorded model deviation for the `clamp == 1` block: the web target does
/// not wrap it and then keep one line, it makes the inner box
/// `white-space: nowrap` outright (`x-text.css:223-230`, `:239-241`) and lets
/// `text-overflow` clip it, so its one line consumes the whole paragraph
/// rather than ending at a break. `lines().len() == 1` coincides; the line's
/// source range does not, which is why none is asserted for that clamp.
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
                ..BlockStyle::default()
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
            // The 3em run raises the line box it lands in from one em to three.
            assert_close(lines[0].height, 10.0);
            assert_close(lines[3].height, 30.0);
            // Under break-all a break falls inside a word, and consecutive
            // ranges then touch; a break at a space leaves that space in
            // neither line's range.
            assert_eq!(lines[3].source_end, lines[4].source_start);
            assert!(lines[2].source_end < lines[3].source_start);
        }
    }
}

/// Replicates `x-text/avatar-text-inline`
/// (`web-elements/tests/fixtures/x-text/avatar-text-inline.html`,
/// `web-elements.spec.ts:277`): an 18x18 avatar view and a nine-glyph run
/// measure as one line inside 375px, so a two-line clamp does not overflow and
/// the custom truncation subtree is never laid in.
///
/// The avatar's `margin-right: 7px` is not a box-protocol input, so the atom
/// carries the 25px inline advance the box and the margin add up to. The web
/// probes — the shadow inner box's `-webkit-line-clamp`, and the
/// `x-show-inline-truncation` attribute the JS pass would set — are
/// `truncated()` and `truncation_visible()` here.
#[test]
fn an_avatar_atom_and_a_short_run_fit_one_line_under_a_two_line_clamp() {
    let quote = ahem_at_line_height(15.0, 23.0);
    let items = [
        atom(1, 25.0, 18.0, VerticalAlign::Middle),
        run(&quote, "清透缎光妆感自然。"),
    ];
    // The `quote-mark-slot` is `width: 17px; margin-left: 4px`, so its inline
    // advance is 21 — folded into the atom the way every atom in this file
    // folds its margins. The dots run carries `margin-left: -4px`, which a run
    // cannot express here (a `RunStyle` has no margin); it is left unmodelled
    // because this test never lays the truncation in — the atom is asserted
    // hidden and `truncation_visible()` false.
    let truncation = [run(&quote, "..."), atom(2, 21.0, 17.0, VerticalAlign::Top)];
    let mut context = text_context();
    let block = laid_out(
        &mut context,
        BlockStyle {
            max_lines: core::num::NonZeroU32::new(2),
            ..BlockStyle::default()
        },
        &items,
        Some(&truncation),
        Some(375.0),
    );

    assert_eq!(block.lines().len(), 1);
    // 25px of avatar plus nine 15px glyphs, well inside 375.
    assert_close(block.size().width, 160.0);
    assert_close(block.lines()[0].height, 23.0);
    let (line, origin, _) = visible_box(&block, 1);
    assert_eq!(line, 0);
    assert_close(origin.x, 0.0);
    assert!(!block.truncated());
    assert!(!block.truncation_visible());
    assert_hidden(&block, 2);
}

/// Replicates `x-text/text-maxline-truncation-in-view`
/// (`web-elements/tests/fixtures/x-text/text-maxline-truncation-in-view.html`,
/// `web-elements.spec.ts:175`): custom-truncation eligibility is a
/// direct-child test, so a truncation node nested inside an inline view never
/// registers — the block falls back to the cheap `text-maxline` path, the
/// empty view still occupies one atomic inline box at the head of line 1, and
/// the clamp point carries the platform's own tail.
///
/// The nesting is expressed by handing the block no truncation content, which
/// is what the eligibility test decides; the `text` attribute's synthesized
/// raw text follows the parsed element child, as it does in the fixture's run
/// order.
///
/// **2026-09-14 ruling.** On the web this paragraph ends in a marker: with
/// `text-maxline="1"`, default attributes and no *valid* inline truncation,
/// `#doExpensiveLineLayoutCalculation` is false (`XTextTruncation.ts:100-105`)
/// so no JS truncation runs at all, and the marker is the browser's own
/// `text-overflow: ellipsis` on a `white-space: nowrap` inner box
/// (`x-text.css:223-230`), which fits one `…` and drops however many glyphs
/// that takes. This engine deliberately shows none: the fixture declares no
/// `text-overflow`, its initial value is `clip`, and the maxline retreat is
/// gated on `ellipsis` (`crates/hughie/src/text/block/truncate.rs:95`) — the
/// ruled behavior, and native Lynx's (Harmony `text_shadow_node.cc:63-66`;
/// Android `TextRenderer.java:185-200`; iOS `LynxTextRenderer.m:1013-1021`).
/// So the one kept line keeps every unit that fits it: the empty view is unit 0
/// and adds no advance, and thirty 10px glyphs fill the 300px width exactly,
/// which is the whole line's advance with nothing backed off. The marked form
/// of the same clamp is covered by
/// `a_bare_maxline_clamp_marks_its_last_visible_line`.
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
            ..BlockStyle::default()
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
    // No retreat, so the clamped line keeps the zero-width view and all thirty
    // 10px glyphs that fit beside it: units 0..31, advancing the full 300px.
    assert_eq!(
        (block.lines()[0].source_start, block.lines()[0].source_end),
        (0, 31),
        "the unmarked cut keeps every unit the line fits",
    );
    assert_eq!(block.lines()[0].ellipsis_count, 0);
    assert_close(block.lines()[0].advance, 300.0);
    assert!(!sources(&block).contains(&SourceItem::Ellipsis));

    // Gated open on this fixture's own items, not a sibling's: the interaction
    // under test is a registered-but-unshown inline view at the head of a
    // clamped line, and only this shape has one. Three 10px dots displace three
    // of the glyphs, so the line keeps units 0..28 and still fills its 300px.
    let marked = laid_out(
        &mut context,
        BlockStyle {
            word_break: WordBreak::BreakAll,
            max_lines: core::num::NonZeroU32::new(1),
            overflow: TextOverflow::Ellipsis,
            ..BlockStyle::default()
        },
        &items,
        None,
        Some(300.0),
    );
    assert_eq!(marked.lines().len(), 1);
    assert_eq!(marked.lines().len(), 1);
    // A reported range is the NATURAL layout's span and `ellipsis_count` is
    // `source_end - cut` (`crates/hughie/src/text/block/mod.rs:66-79`), so the
    // pair says the cut fell at unit 28: the same 0..31 the clip pass reports,
    // with the last three units given up to the dots.
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
    assert!(sources(&marked).contains(&SourceItem::Ellipsis));
    // Three 10px glyphs out, three 10px dots in: the line still fills 300px,
    // which is why the advance alone cannot tell the two passes apart and
    // `ellipsis_count` is what carries the claim.
    assert_close(marked.lines()[0].advance, 300.0);
    assert!(!marked.truncation_visible());
}

// ---------------------------------------------------------------------------
// `x-text` truncation: where the cut lands and what marks it.
// ---------------------------------------------------------------------------

/// The nine `a`..`i` rows the two `text-maxlength` fixtures share, as (row,
/// `text-maxlength`, the flattened runs of the row's nested text children, the
/// width `web-core` renders once the three-dot tail is in, the width this
/// engine renders for the fixture's own declarations — the cut alone, with no
/// tail, by the 2026-09-14 ruling).
///
/// Every glyph is one 10px em, so the tailed width is
/// `min(maxlength, units) * 10 + 30` on a row that cuts and the untailed one is
/// `min(maxlength, units) * 10`. Row `a` cuts nothing and is 10 either way; row
/// `i` cuts everything, so it is three dots wide tailed and empty untailed.
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

/// Replicates `x-text/text-maxlength`
/// (`web-elements/tests/fixtures/x-text/text-maxlength.html`,
/// `web-elements.spec.ts:195`): `text-maxlength` counts UTF-16 units over the
/// flattened run — nested text containers are transparent — cuts the node
/// holding index N at that offset, empties everything after it, and appends a
/// three-dot tail. A run no longer than N is left alone (row `a`), and
/// `text-maxlength="0"` leaves the dots alone (row `i`).
///
/// Every row is laid out unconstrained, as the fixture's rows are: they carry
/// no width and nothing wraps, so the rendered glyph sequence is the block's
/// width. The fixture declares no `font-size` anywhere, so every row gives its
/// nested children identical metrics and which element owns the dots — here
/// the outer `<text>`, since the tail is the inner box's `::after`
/// (`x-text.css:191-194`) — moves nothing measurable. That is a property of
/// the fixture, not a weakening: no row of it can discriminate ownership, and
/// giving one mixed metrics would be inventing geometry the fixture does not
/// have. `the_truncation_tail_takes_the_block_style_not_the_cut_runs` carries
/// a fixture that does discriminate, and is where ownership is pinned.
///
/// **2026-09-14 ruling.** `web-core` appends the three dots to every one of
/// these rows unconditionally — they are the inner box's `::after`
/// (`x-text.css:191-194`), which reads no `text-overflow` — and so does native
/// Lynx, verbatim: "Ellipsis will be appended disregarding the overflowing
/// mode." (Android `TextRenderer.java:126-135`). This engine deliberately does
/// not: the tail is gated on `text-overflow: ellipsis`
/// (`crates/hughie/src/text/block/truncate.rs:135`), the fixture declares none,
/// and the initial value is `clip`, so each row renders its cut alone. On
/// `text-maxlength` that matches *neither* reference — it is a Lynx-vello
/// behavior, deliberate under the ruling, not an oversight — which is why the
/// untailed width is spelled out per row rather than derived from the tailed
/// one. The second pass sets `text-overflow: ellipsis` and asserts the
/// reference widths, so the tail's own machinery stays covered.
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
            ..BlockStyle::default()
        };
        let source_len: u32 = parts
            .iter()
            .map(|part| u32::try_from(part.chars().count()).expect("fits"))
            .sum();

        // The fixture's own declarations: the cut, with no tail after it.
        let block = laid_out(&mut context, style(TextOverflow::Clip), &items, None, None);
        assert_eq!(block.truncated(), max_chars < source_len, "row {row}");
        assert_eq!(block.lines().len(), 1, "row {row}");
        assert!(
            !sources(&block).contains(&SourceItem::Ellipsis),
            "row {row} appends no dots without `text-overflow: ellipsis`",
        );
        assert_close(block.size().width, untailed);

        // Gated open: the width both references render for this row.
        let tailed_block = laid_out(
            &mut context,
            style(TextOverflow::Ellipsis),
            &items,
            None,
            None,
        );
        assert_eq!(
            sources(&tailed_block).contains(&SourceItem::Ellipsis),
            tailed_block.truncated(),
            "row {row} tails exactly the cuts it makes",
        );
        assert_eq!(tailed_block.lines().len(), 1, "row {row}");
        assert_close(tailed_block.size().width, tailed);
    }
}

/// Replicates `x-text/text-maxlength-with-tail-color-convert-false`
/// (`web-elements/tests/fixtures/x-text/text-maxlength-with-tail-color-convert-false.html`,
/// `web-elements.spec.ts:199`): with `tail-color-convert="false"` the same
/// nine cuts are made, but the three dots stop being a marker the block owns
/// and become real inline content spliced into the run at the cut, carrying
/// their own paint identity and the styling of the span they land in.
///
/// The attribute itself is dropped: nothing here reads it, and these are the
/// semantics the engine implements for every cut. Each row of this fixture
/// gives its nested children the same metrics, so no row of it discriminates
/// the two ownership models on its own: a tail owned by the block and a tail
/// spliced into the cut run measure the same when every span is 10px. So after
/// the nine shared rows this test lays out row `e` adapted — the same
/// `1`/`2`/`3`/`4`/`5` cut at two units, with the run holding the cut given a
/// 20px font. `tail-color-convert="false"` makes the dots real inline content
/// appended inside the cut node's own parent
/// (`XTextTruncation.ts:289-303`, `new Array(ellipsisLength).fill('.')`), so
/// they wear that run's 20px: 10 + 20 + 3x20 = 90. The default path, where the
/// tail is the inner box's `::after` in the block's own styling
/// (`x-text.css:191-194`), would put the same row at 10 + 20 + 3x10 = 60.
///
/// **2026-09-14 ruling.** Whether the dots exist at all is a separate question
/// from whose style they wear, and this test now reports on both. `web-core`
/// splices them in unconditionally, as does native Lynx on `text-maxlength`
/// ("Ellipsis will be appended disregarding the overflowing mode.", Android
/// `TextRenderer.java:126-135`); this engine deliberately does not, because the
/// tail is gated on `text-overflow: ellipsis`
/// (`crates/hughie/src/text/block/truncate.rs:135`) and the fixture declares
/// none — a behavior matching neither reference, and Lynx-vello-specific under
/// the ruling. So the first pass asserts each row's bare cut, and the second
/// sets `text-overflow: ellipsis` and keeps the ownership claim the fixture
/// exists for: the dots are real inline content wearing the cut run's metrics,
/// which the adapted row measures at 90px against the 60px a block-owned tail
/// would give. The converse ownership case — the *default*
/// `tail-color-convert`, where the tail should take the block's style and does
/// not — is the `tail-color-convert` gap (A1 in
/// `docs/tracking/web-text-test-replication.md`), still open, and stays pinned by
/// `the_truncation_tail_takes_the_block_style_not_the_cut_runs`.
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
            ..BlockStyle::default()
        };

        // The fixture's own declarations: the cut keeps every unit it spares
        // and nothing is spliced in after it.
        let block = laid_out(&mut context, style(TextOverflow::Clip), &items, None, None);
        assert_close(block.size().width, untailed);
        // Row `i` cuts all five units, and there the block reports two widths
        // that disagree: `size()` is the display layout's 0, while `lines()[0]`
        // falls back to the natural layout's five 10px units. `assemble` takes
        // the width from `final_layout` but can take the lines from the
        // fallback (`crates/hughie/src/text/block/mod.rs:1074-1079`), so the two
        // are not held to each other. That is an UNRULED deviation, recorded
        // here rather than endorsed: the 2026-09-14 ruling settled only whether
        // a marker is emitted, not which of the two widths an empty cut should
        // report. Pinned so a change to either one is noticed.
        assert_close(
            block.lines()[0].advance,
            if row == 'i' { 50.0 } else { untailed },
        );
        assert!(
            !sources(&block).contains(&SourceItem::Ellipsis),
            "row {row} splices in no dots without `text-overflow: ellipsis`",
        );

        // Gated open: the dots ride in the line's own advance, not past it.
        let spliced = laid_out(
            &mut context,
            style(TextOverflow::Ellipsis),
            &items,
            None,
            None,
        );
        assert_close(spliced.lines()[0].advance, tailed);
        assert_eq!(
            sources(&spliced).contains(&SourceItem::Ellipsis),
            spliced.truncated(),
            "row {row} carries the dots as inline content",
        );
    }

    // Row `e` adapted so the two ownership models separate: the run holding
    // the cut carries a 20px font, and the spliced dots wear it.
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
        ..BlockStyle::default()
    };
    // Ungated, the same cut is the kept glyphs alone: '1' at 10px and '2' at
    // 20px.
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

/// Replicates `x-text/text-maxline-basic`
/// (`web-elements/tests/fixtures/x-text/text-maxline-basic.html`,
/// `web-elements.spec.ts:206`): a bare `text-maxline` clamp keeps exactly N
/// line boxes and marks the last visible one, with no author `text-overflow`
/// anywhere — the browser's own line-clamp ellipsis for N >= 2, and the UA
/// sheet's single-line tail ellipsis for N == 1.
///
/// **2026-09-14 ruling.** `web-core` renders this fixture with a marker on the
/// last visible line — the browser's line-clamp ellipsis for N >= 2 and the UA
/// sheet's tail ellipsis for N == 1, neither of which reads `text-overflow`
/// (`x-text.css:216-230`, `:239-241`; `XTextTruncation.ts:349-366`). This
/// engine deliberately does not: the maxline retreat is gated on
/// `text-overflow: ellipsis` (`crates/hughie/src/text/block/truncate.rs:95`),
/// the fixture declares none, and the initial value is `clip` — so the clamp
/// keeps N line boxes with no marker and no units given up. That gating is the
/// ruled behavior, and it is what native Lynx does (Harmony
/// `text_shadow_node.cc:63-66`, `:162-164`; Android `TextRenderer.java:185-200`;
/// iOS `LynxTextRenderer.m:1013-1021`), so on maxline the divergence is from
/// `web-core` alone. The second pass sets `text-overflow: ellipsis` and is the
/// marker's own cover: there the last line does give up units, three of them
/// (`ELLIPSIS_UNITS`), where the web target instead fits a single `…` — a
/// recorded deviation in the marker's width, not in whether it appears.
///
/// The advances are pure Ahem geometry at a 300px width, one em per glyph. The
/// clamp 1 and 2 lines are thirty 10px units (`0..30`, `30..60`), so an unmarked
/// line advances 300 and a marked one keeps 27 of them plus three 10px dots,
/// also 300. The clamp 4 line is `90..117`: twenty-six 10px units of the pink
/// run and the 3em run's first 30px glyph, 290 unmarked; marked, it retreats
/// three units to 114, which leaves 24 10px units and puts the dots in the 10px
/// run, 270. The clamp 6 line is ten 30px units (`127..137`), 300 unmarked and
/// seven kept units plus three 30px dots marked, also 300.
///
/// Recorded model deviation for the `clamp == 1` block, shared with
/// `a_lynx_wrapper_adds_no_box_and_no_break_to_a_clamped_paragraph`: the web
/// target reaches one line by making the inner box `white-space: nowrap`
/// (`x-text.css:223-230`, `:239-241`) rather than by wrapping and clamping, so
/// its single line consumes the whole paragraph. The line count coincides and
/// no source range is asserted for that clamp.
#[test]
fn a_bare_maxline_clamp_marks_its_last_visible_line() {
    let base = ahem_at(10.0);
    let three_em = ahem_at(30.0);
    let tracked = RunStyle {
        letter_spacing: 7.0,
        ..ahem_at(10.0)
    };
    let mut context = text_context();

    // (clamp, the last visible line's advance unmarked, and marked).
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
            ..BlockStyle::default()
        };

        // The fixture's own declarations: `text-overflow` at its initial `clip`.
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
            !sources(&block).contains(&SourceItem::Ellipsis),
            "clamp {clamp} draws no dots without `text-overflow: ellipsis`",
        );

        // Gated open: the marker the references draw unconditionally.
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
        assert!(sources(&marked_block).contains(&SourceItem::Ellipsis));
    }
}

/// Replicates `x-text/text-maxline-with-tail-color-convert-false`
/// (`web-elements/tests/fixtures/x-text/text-maxline-with-tail-color-convert-false.html`,
/// `web-elements.spec.ts:219`): `tail-color-convert="false"` takes the
/// expensive path even with no custom truncation content — the last visible
/// line is cut three units short of its natural end and three real `.`
/// characters are laid in at the cut, wearing the metrics of the run the cut
/// landed in.
///
/// The attribute is dropped: the engine offers only these semantics. Clamp 1
/// and clamp 6 are where ownership is measurable: the first cut falls in the
/// 10px lead run and the second in the 3em run, and in both the dots refill
/// the 300px line exactly, which dots wearing one shared size could not do.
///
/// **2026-09-14 ruling.** The fixture declares no `text-overflow`, and this
/// engine gates the maxline retreat on `text-overflow: ellipsis`
/// (`crates/hughie/src/text/block/truncate.rs:95`), so for the fixture's own
/// declarations it renders the clamp with no retreat and no dots — where
/// `web-core` lays the three `.` characters in unconditionally
/// (`XTextTruncation.ts:289-303`, which never reads `text-overflow`). On
/// `text-maxline` the gate is what native Lynx does (Harmony
/// `text_shadow_node.cc:63-66`; Android `TextRenderer.java:185-200`; iOS
/// `LynxTextRenderer.m:1013-1021`), so the divergence is from `web-core` alone
/// and is deliberate. The first pass therefore asserts the unretreated clamp —
/// the last visible line keeps every unit that fits it, 300px of 10px glyphs at
/// clamps 1 and 2, 290px at clamp 4 (twenty-six 10px units plus the 3em run's
/// first 30px glyph) and 300px of ten 30px glyphs at clamp 6 — and the second
/// sets `text-overflow: ellipsis`, which is where the fixture's own claim, the
/// three-unit back-off with the dots wearing the cut run's metrics, is kept.
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
            ..BlockStyle::default()
        };

        // The fixture's own declarations: no retreat, so no dots to own.
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
            !sources(&block).contains(&SourceItem::Ellipsis),
            "clamp {clamp} lays in no dots without `text-overflow: ellipsis`",
        );

        // Gated open: the three-unit back-off, and whose metrics the dots wear.
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
        assert!(sources(&block).contains(&SourceItem::Ellipsis));
        advances.push((last.advance, marked.advance));
    }

    // Unretreated, the last visible line keeps every unit that fits: thirty
    // 10px glyphs at clamp 1, and ten 30px ones at clamp 6.
    assert_close(advances[0].0, 300.0);
    assert_close(advances[3].0, 300.0);
    // Retreated, clamp 1 is 27 kept 10px glyphs and three 10px dots, and clamp
    // 6 seven kept 30px glyphs and three 30px dots. Dots wearing one shared
    // size could not refill both lines exactly.
    assert_close(advances[0].1, 300.0);
    assert_close(advances[3].1, 300.0);
}

/// Replicates `x-text/text-maxline-with-custom-truncation`
/// (`web-elements/tests/fixtures/x-text/text-maxline-with-custom-truncation.html`,
/// `web-elements.spec.ts:226`): a direct-child `inline-truncation` shows only
/// when the clamp actually overflows, and then it replaces the marker — the
/// cut retreats until the discarded tail is at least as wide as the truncation
/// content, which is painted in its place with no dots anywhere.
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

    // The fixture's first block declares no maxline, and no digit prefix: the
    // analysis never runs and the truncation content stays unpainted.
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
            ..BlockStyle::default()
        },
        &unclamped_items,
        Some(&truncation),
        Some(300.0),
    );
    assert!(!unclamped.truncated());
    assert!(!unclamped.truncation_visible());
    assert!(!sources(&unclamped).contains(&SourceItem::Truncation(0)));

    // How many units each clamp gives up to the 170px truncation run: the
    // retreat walks back from the cut line's end until the discarded tail
    // covers the content's width, so the answer is set by how wide the units
    // at that line's end are. Clamps 1 and 2 end in the 10px base run (17),
    // clamp 4 in the 10px-plus-7px-tracked run (15), clamp 6 in the 30px run
    // (6).
    for (clamp, freed) in [(1u32, 17u32), (2, 17), (4, 15), (6, 6)] {
        let lead = maxline_lead(clamp);
        let items = maxline_basic_items(&lead, &base, &base, &three_em, &tracked);
        let block = laid_out(
            &mut context,
            BlockStyle {
                word_break: WordBreak::BreakAll,
                max_lines: core::num::NonZeroU32::new(clamp),
                ..BlockStyle::default()
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
            !source.contains(&SourceItem::Ellipsis),
            "clamp {clamp} shows no dots beside the custom content",
        );
        // The retreat frees exactly the units the 170px truncation run needs,
        // and the content lands inside the 300px width.
        let last = *block.lines().last().expect("a clamped line");
        assert_eq!(last.ellipsis_count, freed, "clamp {clamp}");
        assert!(last.advance <= 300.0 + EPSILON);
    }
}

/// Replicates `x-text/text-maxline-just-fit-do-not-show-custom-truncation`
/// (`web-elements/tests/fixtures/x-text/text-maxline-just-fit-do-not-show-custom-truncation.html`,
/// `web-elements.spec.ts:265`): overflow is "does a line at index maxline
/// exist", not "does the content reach maxline lines" — content that fills
/// exactly two lines skips the custom-truncation branch entirely, so the 12x12
/// truncation image is never laid in.
///
/// Negative companion to `x-text/inline-truncation-with-inline-image`, which
/// differs only in `text-maxline`. The container is the 390px viewport the
/// webkit project renders the width-less block at.
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
            ..BlockStyle::default()
        },
        &items,
        Some(&truncation),
        Some(390.0),
    );

    assert!(!block.truncated());
    assert!(!block.truncation_visible());
    assert_eq!(block.lines().len(), 2);
    assert_eq!(block.lines()[1].ellipsis_count, 0);
    assert!(!sources(&block).contains(&SourceItem::Ellipsis));
    assert_hidden(&block, 1);
}

/// Replicates `x-text/inline-truncation-with-inline-image`
/// (`web-elements/tests/fixtures/x-text/inline-truncation-with-inline-image.html`,
/// `web-elements.spec.ts:271`): truncation content whose only child is a
/// replaced element is fitted by its laid-out box width, not by a text
/// advance — the cut retreats from the last visible line's end by at least two
/// units and keeps retreating until the discarded tail covers the image's
/// 12px, which then paints inline at the end of the single line.
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
            ..BlockStyle::default()
        },
        &items,
        Some(&truncation),
        Some(390.0),
    );

    assert!(block.truncated());
    assert!(block.truncation_visible());
    assert_eq!(block.lines().len(), 1);
    assert_close(block.lines()[0].height, 20.0);
    // One 13px cluster already frees more than the 12px image, and the walk
    // still gives up two: the web's loop decrements before its first width
    // check.
    assert_eq!(block.lines()[0].ellipsis_count, 2);
    assert!(!sources(&block).contains(&SourceItem::Ellipsis));
    let (line, origin, size) = visible_box(&block, 1);
    assert_eq!(line, 0);
    assert!(
        origin.x + size.width <= 390.0 + EPSILON,
        "the image stays inside the block width",
    );
}

/// Replicates `x-text/text-maxline-with-inline-view-and-custom-truncation`
/// (`web-elements/tests/fixtures/x-text/text-maxline-with-inline-view-and-custom-truncation.html`,
/// `web-elements.spec.ts:302`): a paragraph that opens with an empty inline
/// view chain and closes with another one still finds its cut. On the web the
/// assertion is that no exception reached the console — a DOM Range whose
/// index fell inside an element — which becomes a positive geometry assertion
/// here, where a cut addresses boxes in unit space and a box shares its byte
/// with the character after it.
///
/// The fixture's `x-text` is `width: 100%` and carries no width of its own, so
/// its container is the webkit project's viewport — iPhone 12 Pro, 390px
/// (`playwright-fixtures/src/playwright.common.ts:62-66`). Its content is 25px
/// of avatar (18px plus a 7px right margin), 44 units at 15px, and a 15px
/// trailing view (17px with `margin-left: -2px`): 700px, which two 390px lines
/// hold. So `web-core` does **not** overflow this paragraph — `getLineInfo(2)`
/// is undefined, nothing is cut, and the `inline-truncation` subtree stays
/// unlaid. The markup's pre-set `x-show-inline-truncation` is inert:
/// `#revertTruncatedTextNodes()` strips it at the top of every
/// `#layoutTextInner()` before the decision is re-made
/// (`XTextTruncation.ts:167-172`, `:128-141`).
///
/// The fixture's own render is therefore the first half of this test. The
/// second half narrows the block to 300px — a *constructed* probe, not the
/// fixture's geometry — because that is what forces the cut this case exists to
/// guard, and a cut on a paragraph fenced by empty inline views is the thing
/// that used to throw. The truncation content's trailing view carries its
/// `margin-left: 4px` in the atom's 21px inline advance.
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
        ..BlockStyle::default()
    };

    // The fixture's own geometry: 700px of content over two 390px lines.
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
    // Both of the paragraph's own views are placed; neither truncation item is.
    let (line, origin, size) = visible_box(&fits, 2);
    assert_eq!(line, 1);
    assert!(origin.x + size.width <= 390.0 + EPSILON);
    assert_hidden(&fits, 3);
    assert!(!sources(&fits).contains(&SourceItem::Truncation(0)));

    // Constructed: the same paragraph narrowed until the clamp does overflow,
    // which is the cut-point search the fixture guards.
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
    // The leading view is unit 0 and survives; the trailing one sits past the
    // cut and leaves the paint list.
    let (line, origin, _) = visible_box(&block, 1);
    assert_eq!(line, 0);
    assert_close(origin.x, 0.0);
    assert_hidden(&block, 2);
    // The truncation content's own view lands on the cut line, inside 300px.
    let (line, origin, size) = visible_box(&block, 3);
    assert_eq!(line, 1);
    assert!(origin.x + size.width <= 300.0 + EPSILON);
    assert!(sources(&block).contains(&SourceItem::Truncation(0)));
}

/// Replicates `x-text/truncation-first-element-is-image`
/// (`web-elements/tests/fixtures/x-text/truncation-first-element-is-image.html`,
/// `web-elements.spec.ts:385`): when the first unit of the run is a replaced
/// element rather than text, the leading image is node 0 / unit 0 — a valid
/// seed for line 0, not "no previous node" — and the retreat consumes only
/// trailing content, so the image survives at the head of line 1 while the
/// truncation content ('更多' plus a 12x12 icon) closes line 3 inside 300px.
///
/// The leading image's `margin-right: 4px` rides in the atom's 36px inline
/// advance. The fixture's own run is `大学` five times, the markup's newline and
/// indentation (one collapsed space), then `大学` fifty times: 111 units.
///
/// The fixture's 32px line height is asserted by
/// `a_runs_line_height_is_not_taken_from_the_span_after_it`, which fails today
/// and carries its own GAP; this test is the cut-point search the case is
/// assigned, and that part works.
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
            ..BlockStyle::default()
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
    // The cut falls on the last line and nowhere earlier.
    assert_eq!(block.lines()[0].ellipsis_count, 0);
    assert_eq!(block.lines()[1].ellipsis_count, 0);
    // The truncation content measures 2x14 + 12 = 40px and the retreat frees
    // 24px a unit, but the web's loop decrements before its first width check
    // (`XTextTruncation.ts:222-232`), so it gives up exactly two.
    assert_eq!(block.lines()[2].ellipsis_count, 2);
    let source = sources(&block);
    assert!(source.contains(&SourceItem::Truncation(0)));
    assert!(!source.contains(&SourceItem::Ellipsis));
    let (line, origin, size) = visible_box(&block, 2);
    assert_eq!(line, 2);
    assert!(origin.x + size.width <= 300.0 + EPSILON);
}

/// The line-height half of `x-text/truncation-first-element-is-image`, split
/// out because it does not hold yet. The fixture's paragraph declares
/// `line-height: 32px` and only the `inline-truncation` subtree declares
/// `22px`, so on the web every one of the three line boxes is 32px tall: a line
/// box is at least as tall as the block's own strut, and the 22px truncation
/// span — which reaches only the last line in the first place — is shorter than
/// it, so it raises nothing and lowers nothing (the fixture's screenshot is
/// recorded as "3 lines at 24px/32px").
///
/// This block reports 22/22/22 instead, and the same block built with no
/// truncation content reports 32/32/32. The cause is not the cut: a shaped run
/// takes its `line-height` from the style of the span that *follows* it, so
/// whichever span is pushed last governs every line the preceding run lands on.
/// Two ordinary runs with different `line-height` in one paragraph and no
/// truncation anywhere reproduce it; the truncation content is only the last
/// span here. `line-height: normal` hides it, which is why the mixed-metric
/// assertions in `a_lynx_wrapper_adds_no_box_and_no_break_to_a_clamped_paragraph`
/// hold.
#[test]
#[ignore = "GAP: a run's line-height is read from the style of the NEXT span, \
            so the last span's line-height governs every line \
            (parley-0.11.0/src/shape/mod.rs:127-128 advances \
            `item.style_index` before flushing the pending item, and \
            parley-0.11.0/src/layout/data.rs:435-436 reads the run's \
            line-height through it; reached from \
            crates/hughie/src/text/block/shape.rs:61-67)"]
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
            ..BlockStyle::default()
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

    // The same defect with no truncation in sight: two runs, two declared
    // line heights, one line each. Line 0 holds only the 24px/32px run.
    let plain = [run(&body, "AAAAAAAAAA"), run(&more, "BBBBB")];
    let plain = laid_out(
        &mut context,
        BlockStyle::default(),
        &plain,
        None,
        Some(240.0),
    );
    assert_eq!(plain.lines().len(), 2);
    assert_close(plain.lines()[0].height, 32.0);
    assert_close(plain.lines()[1].height, 22.0);
}

// ---------------------------------------------------------------------------
// `x-text` layout: clipping, and the CSS `text-overflow` trigger.
// ---------------------------------------------------------------------------

/// Replicates `x-text/text-overflow-inherit`
/// (`web-elements/tests/fixtures/x-text/text-overflow-inherit.html`,
/// `web-elements.spec.ts:321`): `text-overflow: ellipsis` on a `nowrap` block
/// whose available inline size is smaller than the run's max-content width
/// replaces the overflowing glyphs with a marker at the clip edge, with the
/// 36px line height governing the line box. No `text-maxline` and no
/// `text-maxlength` take part — overflowing a line box is the trigger, which
/// is CSS-UI's own `text-overflow` and a W3C feature in its own right.
#[test]
#[ignore = "GAP: there is no overflow-driven ellipsis path — a cut needs a \
            maxline clamp or a maxlength cut, and a single nowrap line \
            consumes all its source (crates/hughie/src/text/block/mod.rs:625)"]
fn an_overflowing_nowrap_line_is_marked_by_text_overflow_ellipsis() {
    let body = ahem_at_line_height(15.0, 36.0);
    let items = [run(&body, "曼联积分榜最新排行榜")];
    let mut context = text_context();
    let block = laid_out(
        &mut context,
        BlockStyle {
            text_wrap: TextWrap::NoWrap,
            overflow: TextOverflow::Ellipsis,
            ..BlockStyle::default()
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

/// Replicates `x-text/text-clipped-display-important`
/// (`web-elements/tests/fixtures/x-text/text-clipped-display-important.html`,
/// `web-elements.spec.ts:364`): an atomic inline box that falls past a
/// two-line clamp must not paint, however strongly its `display` is declared —
/// on the web an `!important` rule marks it `display: none` over the
/// `inline-flex !important` that made it an atomic box, here it simply leaves
/// the paint list — and the custom `inline-truncation` content takes the
/// marker's place.
///
/// The truncation run is 8px so that its ten glyphs measure 80px inside the
/// 100px block, the proportion the fixture's own face gives them; truncation
/// content as wide as its container is hidden instead, which is a different
/// rule.
#[test]
fn an_atom_clipped_past_the_clamp_leaves_the_paint_list() {
    let base = ahem_at(10.0);
    let small = ahem_at(8.0);
    let items = [
        run(&base, "this is text I know you your past your future"),
        // The inline-flex row of three 50px atoms, each with a 1px border and
        // 2px margins.
        atom(1, 168.0, 24.0, VerticalAlign::Baseline),
    ];
    let truncation = [run(&small, "truncation")];
    let mut context = text_context();
    let block = laid_out(
        &mut context,
        BlockStyle {
            max_lines: core::num::NonZeroU32::new(2),
            ..BlockStyle::default()
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
    assert!(!source.contains(&SourceItem::Ellipsis));
    assert!(block.size().width <= 100.0 + EPSILON);
}

// ---------------------------------------------------------------------------
// ReactLynx `<text>`: the same attributes, reached through the element PAPI.
// ---------------------------------------------------------------------------

/// One row of the `ReactLynx` `text-maxlength` fixture: its letter, its
/// `text-maxlength` attribute, the flattened runs of its nested text children,
/// the width the references render with their tail in, and the width of the cut
/// alone — which is what this engine renders for the fixture's own
/// declarations: N 10px units, or the whole 244-unit paragraph where nothing
/// cuts.
type MaxlengthRow<'src> = (char, Option<u32>, Vec<&'src str>, f32, f32);

/// The 244-character English paragraph the `ReactLynx` text fixtures share.
/// Only its length and the absence of a cut point before unit 200 matter to
/// the clamps below, so it is spelled here rather than copied.
fn paragraph() -> String {
    let text: String = "lorem ipsum dolor sit amet "
        .repeat(10)
        .chars()
        .take(244)
        .collect();
    assert_eq!(text.chars().count(), 244);
    text
}

/// Replicates `text/maxlength`
/// (`web-tests/dist/basic-element-text-maxlength/index.web.json`,
/// `web-core-e2e/tests/reactlynx.spec.ts:2725`): `text-maxlength` set through
/// the element PAPI clamps at 1, 2, 3 and 200 units across ASCII, CJK and runs
/// split over nested inline `<text>` children, appending the three-dot tail; a
/// limit longer than the run, and a row with no attribute at all, keep
/// everything.
///
/// The rows are laid out unconstrained, as the fixture's flex column leaves
/// them, and each is spelled as a [`MaxlengthRow`].
///
/// **2026-09-14 ruling.** Both references append the tail to every cut row
/// regardless of `text-overflow` — `web-core` through the inner box's `::after`
/// (`x-text.css:191-194`) and native Lynx explicitly ("Ellipsis will be
/// appended disregarding the overflowing mode.", Android
/// `TextRenderer.java:126-135`). This engine deliberately gates it on
/// `text-overflow: ellipsis` (`crates/hughie/src/text/block/truncate.rs:135`),
/// which the fixture never declares and whose initial value is `clip`, so each
/// row renders exactly its first N units and nothing else. On `text-maxlength`
/// that matches neither reference; it is a Lynx-vello behavior, ruled correct,
/// so both widths are carried per row: the cut alone, and — in the second pass,
/// which sets `text-overflow: ellipsis` and keeps the tail covered — the cut
/// plus the 30px of dots the references show.
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
            ..BlockStyle::default()
        };

        // The fixture's own declarations: the cut, untailed.
        let block = laid_out(&mut context, style(TextOverflow::Clip), &items, None, None);
        assert_eq!(block.truncated(), max_chars.is_some(), "row {row}");
        assert!(
            !sources(&block).contains(&SourceItem::Ellipsis),
            "row {row} appends no tail without `text-overflow: ellipsis`",
        );
        assert_close(block.size().width, untailed);

        // Gated open: the width both references render.
        let tailed_block = laid_out(
            &mut context,
            style(TextOverflow::Ellipsis),
            &items,
            None,
            None,
        );
        assert_eq!(
            sources(&tailed_block).contains(&SourceItem::Ellipsis),
            max_chars.is_some(),
            "row {row} tails exactly the cuts it makes",
        );
        assert_close(tailed_block.size().width, tailed);
    }
}

/// Replicates `text/tail-color-convert`
/// (`web-tests/dist/basic-element-text-tail-color-convert/index.web.json`,
/// `web-core-e2e/tests/reactlynx.spec.ts:2734`): with `tail-color-convert`
/// left at its default the truncation tail belongs to the outer `<text>`, not
/// to the run the cut landed in — the block paints it, in the block's own
/// styling, past the last surviving inline span.
///
/// Ownership is a colour claim in the fixture (a coloured outer block against
/// a red nested run) and this layer carries no colour, so it is put as the
/// metric this layer does carry: the nested run gets a 20px font against the
/// outer run's 10px, and the tail's advance says which of the two owns it.
/// The fixture's other two rows add a limit past the end of the run, which is
/// asserted, and an outer colour, which moves no geometry.
///
/// This test sets `text-overflow: ellipsis` so it reports on ownership alone,
/// which the fixture does not declare: in `web-core` the tail is
/// unconditional. Clearing this test's GAP therefore does not make the
/// fixture's screenshot reachable — two gates stand between them, and the
/// first one, the tail being gated on `text-overflow` at all, is pinned
/// separately by `maxlength_counts_units_across_nested_runs_and_scripts`.
/// This test reports on the second gate only.
#[test]
#[ignore = "GAP: `tail-color-convert` is unparsed, and the dots always inherit \
            the run holding the cut rather than the block \
            (crates/hughie/src/text/block/truncate.rs:226)"]
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
            ..BlockStyle::default()
        },
        &items,
        None,
        None,
    );
    assert!(cut.truncated());
    // '简' at 10px, '体中' at 20px, then three dots the block owns: 10 + 40 +
    // 30. Dots wearing the nested run's 20px would make it 110.
    assert_close(cut.size().width, 80.0);

    let intact = laid_out(
        &mut context,
        BlockStyle {
            max_chars: Some(10),
            overflow: TextOverflow::Ellipsis,
            ..BlockStyle::default()
        },
        &items,
        None,
        None,
    );
    assert!(!intact.truncated());
    assert_close(intact.size().width, 70.0);
}

/// Replicates `text/word-break`
/// (`web-tests/dist/basic-element-text-word-break/index.web.json`,
/// `web-core-e2e/tests/reactlynx.spec.ts:2876`): the line-breaking policy of a
/// 100px `<text>` over unbroken digits, space-separated digits, unbroken CJK,
/// unbroken Latin, space-separated Latin and space-separated CJK.
///
/// An unbroken Latin word breaks mid-word because the web target declares
/// `overflow-wrap: break-word` on `x-text`, which this engine hardcodes rather
/// than exposing as an input (`crates/hughie/src/text/block/style.rs:265`). A
/// gap between consecutive ranges is the space a soft wrap consumed; touching
/// ranges are a break inside a word.
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
        let block = laid_out(
            &mut context,
            BlockStyle::default(),
            &items,
            None,
            Some(100.0),
        );

        let ranges = block
            .lines()
            .iter()
            .map(|line| (line.source_start, line.source_end))
            .collect::<Vec<_>>();
        assert_eq!(ranges, expected, "row {row}");
        assert!(!block.truncated(), "row {row}");
    }
}

/// Replicates `web-core/testing-library-port`'s "normalizes whitespace"
/// (`web-core/tests/testing-library-port.spec.ts:245`): raw-text content keeps
/// its two leading spaces verbatim through creation and flush — collapsing is
/// not a storage behavior — so whatever collapsing the rendered line shows is
/// the shaping path's alone. The storage half of the case belongs to the
/// element PAPI; this is the shaping half.
///
/// Collapsing a run of spaces to one and removing a collapsible space at the
/// start of a line are two steps of the same white-space processing, so
/// `"  Step 1 of 4"` shapes to eleven advances, not twelve, while the source
/// units the line reports stay the pre-collapse thirteen.
#[test]
#[ignore = "GAP: a leading collapsible whitespace span emits its space instead \
            of being removed at the start of the line \
            (crates/hughie/src/text/block/content.rs:359)"]
fn collapsible_whitespace_is_removed_at_the_start_of_a_line() {
    let base = ahem_at(10.0);
    let items = [run(&base, "  Step 1 of 4")];
    let mut context = text_context();
    let block = laid_out(&mut context, BlockStyle::default(), &items, None, None);

    assert_eq!(block.lines().len(), 1);
    assert_eq!(
        (block.lines()[0].source_start, block.lines()[0].source_end),
        (0, 13),
        "the source range counts pre-collapse units",
    );
    assert_close(block.size().width, 110.0);
}
