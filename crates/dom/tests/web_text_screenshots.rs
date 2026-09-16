//! Golden screenshots for the `lynx-stack` text cases whose own oracle was a
//! picture.
//!
//! The originals are Playwright full-page diffs at `maxDiffPixelRatio: 0`
//! (`packages/web-platform/web-elements/tests/web-elements.spec.ts`,
//! `packages/web-platform/web-core-e2e/tests/reactlynx.spec.ts`). The metric
//! replicas of those cases — in `crates/dom/tests/web_text_replication.rs`,
//! `crates/bobcat-core/src/main/tree/web_text_replication.rs` and
//! `crates/hughie/tests/web_text_replication.rs` — assert advances, frames and
//! sampled pixels, which is the right oracle for layout and says nothing about
//! what the paragraph *looks like*: whether a run really wears its own colour,
//! whether two sizes hang from one baseline, whether an atomic box is drawn at
//! all. This file restores the originals' oracle for the cases whose claim is
//! visual. Every test here names the sibling metric test it doubles; none of
//! them makes a claim that is not already made there.
//!
//! The reference is **web-core**, not native Lynx (`AGENTS.md`'s compatibility
//! target).
//!
//! Three adaptations run through the file, the first two shared with
//! `crates/dom/tests/web_text_replication.rs`:
//!
//! * `crates/dom` holds no Lynx element vocabulary, so a `<text>` is spelled as the `display:
//!   -lynx-text` box the Lynx UA sheet gives it — here through `support/html.rs`'s `text-block`
//!   class — and a compiled `<raw-text>` as [`RAW_TEXT_SHEET`]'s `display: contents;
//!   white-space-collapse: preserve-breaks`, which is what that element's UA rules amount to
//!   (`crates/bobcat-core/src/main/tree/raw_text.rs:8-11`).
//! * `crates/dom` has no UA sheet at all, so a container that stacks its children says `display:
//!   flex; flex-direction: column` rather than relying on a block-level default. Where a case's
//!   claim is about block-level stacking, it is the metric sibling that carries it.
//! * These fixtures shape **vendored Roboto**, never a host font: a golden exists to be looked at,
//!   and a host face could not have a committed golden at all (`support/screenshot.rs`). Advances
//!   therefore differ from the Ahem-based metric replicas — every width here is chosen so the line
//!   the fixture wants still fits. The one exception is
//!   `a_custom_truncation_marker_is_laid_in_at_the_clamp_in_its_own_colour`, whose whole subject is
//!   *where* a cut falls: it shapes the vendored Ahem face, so the picture is em squares a reviewer
//!   can count instead of letterforms they would have to measure.
//!
//! * The fixture importer drops whitespace-only text nodes outright (`support/html.rs:110-112`), so
//!   the newline-and-indent a browser would collapse to one space between two adjacent runs never
//!   reaches the engine. Where a fixture relies on that space it is written inside the preceding
//!   run instead, so the picture shows the space the reference renders.
//!
//! A golden is committed only where the frame is right. Where this engine
//! renders a case wrongly today, the case appears here only if the fixture can
//! be built so the defective element is absent and the golden shows correct
//! output — `a_block_s_gradient_reaches_the_run_that_inherits_it` is that
//! shape. A case whose whole frame is wrong gets **no test in this file at
//! all**: an `#[ignore]`d screenshot test would be self-healing, because
//! `assert_golden` writes a missing PNG and fails only on that first run
//! (`crates/flashbulb/src/golden.rs:100-103`), so the second run would pass
//! against a golden of the defect. The two visual claims in that position —
//! the marker on a bare `text-maxline` clamp, and a one-line clamp keeping its
//! nested run's colour — are carried by `#[ignore]`d metric siblings:
//! `a_bare_maxline_clamp_marks_its_last_visible_line` in
//! `crates/hughie/tests/web_text_replication.rs` and
//! `a_one_line_clamp_fills_the_available_width_instead_of_breaking_at_a_word`
//! in `crates/bobcat-core/src/main/tree/web_text_replication.rs`. Add the
//! golden here when those close.
//!
//! Refresh with:
//! `FLASHBULB_UPDATE_SNAPSHOTS=1 cargo test -p dom --test web_text_screenshots`.

#[path = "support/html.rs"]
mod html;
mod paint_common;
#[path = "support/screenshot.rs"]
mod screenshot;

use flashbulb::Image;

/// The compiled shape of a dynamic string child of a `<text>`.
///
/// A compiled `ReactLynx` bundle turns such a child to `__CreateRawText`, and the Lynx UA sheet
/// dissolves the carrier while keeping its newline policy
/// (`crates/bobcat-core/src/main/tree/raw_text.rs:8-11`). Only the newline
/// policy is observable in a picture, so that is all this sheet carries; the
/// `text` attribute half is a `crates/bobcat-core` concern.
const RAW_TEXT_SHEET: &str =
    ".raw-text { display: contents; white-space-collapse: preserve-breaks; }";

fn assert_web_text_golden(case: &str, actual: &Image) {
    screenshot::assert_golden(&["web-text", case], actual);
}

/// [`screenshot::capture`] plus one stylesheet and a chosen vendored face.
fn capture_with_sheet(
    test: &str,
    css: &str,
    origin: dom::StylesheetOrigin,
    font: &'static [u8],
    fragment: &str,
    width: f32,
    height: f32,
) -> Image {
    let mut doc = html::parse(fragment, width, height);
    doc.dom.add_stylesheet(css, origin);
    assert_eq!(
        doc.dom.register_fonts(dom::FontBlob::from_static(font)),
        1,
        "the vendored fixture face must register exactly one face"
    );
    screenshot::capture_prebuilt_document(test, &mut doc.dom, &dom::NoImages)
}

/// [`screenshot::capture`] plus a user-agent sheet, for the fixtures that need
/// one — a `@property` registration or a rule that no inline style can express.
fn capture_with_ua_css(test: &str, ua_css: &str, fragment: &str, width: f32, height: f32) -> Image {
    capture_with_sheet(
        test,
        ua_css,
        dom::StylesheetOrigin::UserAgent,
        screenshot::ROBOTO,
        fragment,
        width,
        height,
    )
}

/// [`capture_with_ua_css`] over Ahem, for the one fixture whose subject is
/// where a cut falls rather than what a face looks like.
fn capture_ahem_with_ua_css(
    test: &str,
    ua_css: &str,
    fragment: &str,
    width: f32,
    height: f32,
) -> Image {
    capture_with_sheet(
        test,
        ua_css,
        dom::StylesheetOrigin::UserAgent,
        screenshot::AHEM,
        fragment,
        width,
        height,
    )
}

/// [`capture_with_ua_css`] with an author sheet instead, for the one fixture
/// whose claim *is* the author cascade.
fn capture_with_author_css(
    test: &str,
    author_css: &str,
    fragment: &str,
    width: f32,
    height: f32,
) -> Image {
    capture_with_sheet(
        test,
        author_css,
        dom::StylesheetOrigin::Author,
        screenshot::ROBOTO,
        fragment,
        width,
        height,
    )
}

/// Replicates `x-text/inline-text`
/// (`packages/web-platform/web-elements/tests/fixtures/x-text/inline-text.html`,
/// `packages/web-platform/web-elements/tests/web-elements.spec.ts:148`): a
/// `text` nested directly in a `text` is an inline run of the parent's
/// paragraph — it shares the line box, inherits the weight it was handed, and
/// scopes its own colour to itself.
///
/// Sibling metric test:
/// `a_text_nested_in_a_text_is_an_inline_run_that_scopes_only_its_own_colour`
/// in `crates/bobcat-core/src/main/tree/web_text_replication.rs`, which asserts
/// the collapse of the whitespace between the two runs as an advance and the
/// nested run's colour as computed style. What a computed-style read cannot
/// say, and this golden can, is that the *glyphs* of the second run are red
/// while the first run's stay black on the very same line — a per-run paint
/// claim. A paragraph that painted every glyph with the establishing element's
/// style would satisfy the metric test and fail here.
#[test]
fn a_nested_run_wears_its_own_colour_on_the_parent_s_line() {
    const FRAGMENT: &str = r#"
<div style="display: flex; width: 400px; height: 70px; padding: 10px; box-sizing: border-box; background-color: white; font-family: Roboto">
  <div class="text-block" style="width: 380px; font-size: 24px; font-weight: bold">I am bold
    <span class="text-block" style="font-size: 24px; color: red">and red</span></div>
</div>
"#;
    let actual = screenshot::capture(
        "a_nested_run_wears_its_own_colour_on_the_parent_s_line",
        FRAGMENT,
        400.0,
        70.0,
    );
    assert_web_text_golden("inline-text", &actual);
}

/// Replicates `x-text/text-inline-no-whitespace`
/// (`packages/web-platform/web-elements/tests/fixtures/x-text/text-inline-no-whitespace.html`,
/// `packages/web-platform/web-elements/tests/web-elements.spec.ts:234`): three
/// adjacent runs written with no whitespace between the tags shape as one
/// uninterrupted line — a nested inline `text` introduces neither a space, nor
/// a box edge, nor a break opportunity of its own.
///
/// Sibling metric test:
/// `adjacent_runs_written_without_whitespace_shape_as_one_uninterrupted_line`
/// in `crates/bobcat-core/src/main/tree/web_text_replication.rs`, which asserts
/// the advance is exactly twenty-two Ahem em squares. That number proves no
/// *space* was inserted; it cannot show that the run boundary left no seam in
/// the shaping — proportional Roboto kerns across the boundary or it does not,
/// and only a picture says which. The middle run is left unstyled, as in the
/// fixture, so nothing but the tag marks where it starts.
#[test]
fn adjacent_runs_written_without_whitespace_draw_one_unbroken_word() {
    const FRAGMENT: &str = r#"
<div style="display: flex; width: 360px; height: 60px; padding: 10px; box-sizing: border-box; background-color: white; font-family: Roboto">
  <div class="text-block" style="width: 340px; font-size: 28px">helloworld<span class="text-block">lynxweb</span>hello</div>
</div>
"#;
    let actual = screenshot::capture(
        "adjacent_runs_written_without_whitespace_draw_one_unbroken_word",
        FRAGMENT,
        360.0,
        60.0,
    );
    assert_web_text_golden("text-inline-no-whitespace", &actual);
}

/// Replicates `x-text/text-baseline-alignment`
/// (`packages/web-platform/web-elements/tests/fixtures/x-text/text-baseline-alignment.html`,
/// `packages/web-platform/web-elements/tests/web-elements.spec.ts:167`): the
/// fixture's four arrangements of a 20px and a 30px run — stacked as sibling
/// blocks, placed side by side as flex items, and twice as two runs of one
/// paragraph, where they hang from a common baseline.
///
/// Sibling metric test:
/// `runs_of_different_size_share_a_baseline_while_sibling_text_blocks_do_not`
/// in `crates/bobcat-core/src/main/tree/web_text_replication.rs`, which reads
/// the shared baseline off the line box's height: two runs hanging from one
/// baseline cost the larger run's em box where two stacked baselines would cost
/// the sum. A line box of the right height is necessary and not sufficient —
/// it holds equally if both runs sit on the box's top edge, or its bottom, or
/// each on its own. The golden is where the two rows of glyphs are seen to
/// stand on one line.
///
/// The fixture repeats its third arrangement verbatim as a fourth; both are
/// kept so the picture matches the original's page.
#[test]
fn a_small_and_a_large_run_stand_on_one_baseline() {
    const FRAGMENT: &str = r#"
<div style="display: flex; flex-direction: column; width: 400px; height: 220px; padding: 10px; box-sizing: border-box; background-color: white; font-family: Roboto">
  <div style="display: flex; flex-direction: column">
    <div class="text-block" style="font-size: 20px">hello world</div>
    <div class="text-block" style="font-size: 30px">111</div>
  </div>
  <div style="display: flex; flex-direction: row">
    <div class="text-block" style="font-size: 20px">hello world</div>
    <div class="text-block" style="font-size: 30px">111</div>
  </div>
  <div style="display: flex; flex-direction: column">
    <div class="text-block" style="width: 380px">
      <span class="text-block" style="font-size: 20px">hello world </span>
      <span class="text-block" style="font-size: 30px">111</span>
    </div>
  </div>
  <div style="display: flex; flex-direction: column">
    <div class="text-block" style="width: 380px">
      <span class="text-block" style="font-size: 20px">hello world </span>
      <span class="text-block" style="font-size: 30px">111</span>
    </div>
  </div>
</div>
"#;
    let actual = screenshot::capture(
        "a_small_and_a_large_run_stand_on_one_baseline",
        FRAGMENT,
        400.0,
        220.0,
    );
    assert_web_text_golden("text-baseline-alignment", &actual);
}

/// Replicates `x-text/view-in-text`
/// (`packages/web-platform/web-elements/tests/fixtures/x-text/view-in-text.html`,
/// `packages/web-platform/web-elements/tests/web-elements.spec.ts:140`): a
/// block-level `view` child of a `text` is one atomic inline box that keeps its
/// own specified size and runs its own container algorithm over its children,
/// independently of line breaking.
///
/// Sibling metric tests:
/// `a_view_child_of_a_text_is_one_atomic_inline_box_laid_out_on_its_own` in
/// `crates/bobcat-core/src/main/tree/web_text_replication.rs` for the atom's own
/// 200x200 frame and its three stacked children, and
/// `an_atomic_inline_box_keeps_the_size_it_measured` in
/// `crates/dom/tests/web_text_replication.rs` for the committed box. Both read
/// `location`/`size` off the layout tree, which says nothing about whether the
/// frame builder ever emits the atom: this is the case whose whole point in the
/// original is that a green square with three bordered yellow bars is *drawn*
/// inside a paragraph. The fixture's backgrounds and borders, deliberately left
/// out of the geometric replica as "paint", are the subject here.
///
/// The `text` block is written as an ordinary in-flow child of the wrapper, not
/// as the paint root and not as a stacking context, because those two paths do
/// not descend past the paragraph — a defect that
/// `a_boxed_child_paints_from_a_text_block_that_is_its_own_context` in
/// `crates/dom/tests/web_text_replication.rs` owns
/// (`crates/dom/src/visual/build.rs:563-570`). Keeping the block out of those
/// paths is what lets this golden show only correct output.
#[test]
fn a_view_child_of_a_text_is_drawn_as_an_atomic_box() {
    const FRAGMENT: &str = r#"
<div style="display: flex; width: 240px; height: 220px; padding: 10px; box-sizing: border-box; background-color: white; font-family: Roboto">
  <div class="text-block" style="width: 220px">
    <div style="display: flex; flex-direction: column; width: 200px; height: 200px; background-color: green">
      <div style="display: flex; width: 50px; height: 20px; background-color: yellow; border: 1px solid red"></div>
      <div style="display: flex; width: 50px; height: 20px; background-color: yellow; border: 1px solid red"></div>
      <div style="display: flex; width: 50px; height: 20px; background-color: yellow; border: 1px solid red"></div>
    </div>
  </div>
</div>
"#;
    let actual = screenshot::capture(
        "a_view_child_of_a_text_is_drawn_as_an_atomic_box",
        FRAGMENT,
        240.0,
        220.0,
    );
    assert_web_text_golden("view-in-text", &actual);
}

/// Replicates `x-text/view-flex-in-text`
/// (`packages/web-platform/web-elements/tests/fixtures/x-text/view-flex-in-text.html`,
/// `packages/web-platform/web-elements/tests/web-elements.spec.ts:352`): a
/// `view` child of a `text` that selects flex layout shrink-wraps to its three
/// bordered items and is placed after the preceding run, on the same line.
///
/// Sibling metric test: `a_flex_view_inside_a_text_shrinks_to_fit_and_stays_on_
/// the_line` in `crates/bobcat-core/src/main/tree/web_text_replication.rs`,
/// whose doc comment says in as many words that "the borders and backgrounds
/// the golden shows are paint" and asserts the 162px shrink-to-fit width
/// instead. This is that golden: the run, then three bordered boxes with their
/// margins between them, all on one line.
///
/// The vertical placement *is* part of this picture, and it is the reference's.
/// An atom's top edge comes out of hughie's placement table as
/// `line baseline - the atom's own first baseline`
/// (`crates/hughie/src/text/block/position.rs:93-95`), the atom's baseline
/// being the one its own layout reported
/// (`crates/dom/src/layout/text_block.rs:564`) — which for this flex container
/// is its first item's synthesized baseline, 2px of margin plus a 22px border
/// box above the atom's top. That is the CSS rule. The recorded deviation
/// beside it (`docs/tracking/deviations.md:213-220`) is that the *line* is not
/// grown by the atom's below-baseline part, and this fixture keeps it out of
/// the picture: one line, no background on the block, and nothing under it, so
/// a short line box displaces no pixel.
#[test]
fn a_flex_view_atom_is_drawn_after_the_run_on_the_same_line() {
    const FRAGMENT: &str = r#"
<div style="display: flex; width: 420px; height: 70px; padding: 10px; box-sizing: border-box; background-color: white; font-family: Roboto">
  <div class="text-block" style="width: 400px; font-size: 20px">this is text <span style="display: flex; flex-direction: row">
      <span style="display: flex; width: 50px; height: 20px; border: 1px solid red; margin: 2px; background-color: yellow"></span>
      <span style="display: flex; width: 50px; height: 20px; border: 1px solid red; margin: 2px; background-color: yellow"></span>
      <span style="display: flex; width: 50px; height: 20px; border: 1px solid red; margin: 2px; background-color: yellow"></span>
    </span></div>
</div>
"#;
    let actual = screenshot::capture(
        "a_flex_view_atom_is_drawn_after_the_run_on_the_same_line",
        FRAGMENT,
        420.0,
        70.0,
    );
    assert_web_text_golden("view-flex-in-text", &actual);
}

/// Replicates `text/nest-view`
/// (`packages/web-platform/web-tests/dist/basic-element-text-nest-view/index.web.json`,
/// `packages/web-platform/web-core-e2e/tests/reactlynx.spec.ts:2617`): a
/// padded, bordered, rounded `view` nested in a `text` as an atomic inline box,
/// holding a `text` of its own, with raw strings flowing before and after it.
///
/// Sibling metric test:
/// `a_padded_view_nested_in_a_text_is_an_atom_the_surrounding_runs_flow_around`
/// in `crates/bobcat-core/src/main/tree/web_text_replication.rs`, which is
/// explicitly "geometry only: the border, its radius and the inner run's
/// gradient fill are paint". All three are what this golden is for — the rounded
/// red outline around the atom, and a nested paragraph inside it whose glyphs
/// carry a green-to-yellow ramp while the runs on either side stay black.
///
/// It also shows the thing the metric replica leaves out for being a recorded
/// deviation, and shows it holding: the inner 30px run and the 20px runs
/// outside the atom sit on one baseline, because the atom's top comes out of
/// hughie's placement table as `line baseline - the atom's own first baseline`
/// (`crates/hughie/src/text/block/position.rs:93-95`) and that baseline is the
/// inner paragraph's, carried up through
/// `crates/dom/src/layout/text_block.rs:564`. The deviation
/// (`docs/tracking/deviations.md:213-220`) is that the line is not grown by the
/// atom's below-baseline part — its bottom padding and border — and this
/// fixture keeps it out of the picture the same way the flex-atom golden does:
/// one line, no background on the block, nothing under it.
#[test]
fn a_padded_rounded_view_atom_draws_its_border_and_its_inner_ramp() {
    const FRAGMENT: &str = r#"
<div style="display: flex; width: 640px; height: 90px; padding: 10px; box-sizing: border-box; background-color: white; font-family: Roboto">
  <div class="text-block" style="width: 620px; font-size: 20px">hello world<span style="display: flex; padding: 10px; border: 1px solid red; border-radius: 20px"><span class="text-block" style="font-size: 30px; color: linear-gradient(green, yellow)">sub text</span></span>other text content</div>
</div>
"#;
    let actual = screenshot::capture(
        "a_padded_rounded_view_atom_draws_its_border_and_its_inner_ramp",
        FRAGMENT,
        640.0,
        90.0,
    );
    assert_web_text_golden("nest-view", &actual);
}

/// Replicates `text/with-new-line`
/// (`packages/web-platform/web-tests/dist/basic-element-text-with-new-line/index.web.json`,
/// `packages/web-platform/web-core-e2e/tests/reactlynx.spec.ts:2636`): a literal
/// `\n` in a dynamic string child forces a hard line break — Lynx text does not
/// collapse it the way HTML's `white-space: normal` would.
///
/// Sibling metric test:
/// `a_literal_newline_in_a_dynamic_string_child_breaks_the_line` in
/// `crates/bobcat-core/src/main/tree/web_text_replication.rs`, which asserts the
/// paragraph measures two line boxes and takes the wider word's width. Two line
/// boxes is also what a paragraph that wrapped on width would measure; the
/// golden shows the break falling exactly between `hello` and `world!` with the
/// line's remaining width unused. The control beside it is the same string with
/// the newline replaced by a space, which must draw as one line.
#[test]
fn a_literal_newline_breaks_the_line_where_it_is_written() {
    const FRAGMENT: &str = r#"
<div style="display: flex; flex-direction: column; width: 360px; height: 130px; padding: 10px; gap: 8px; box-sizing: border-box; background-color: white; font-family: Roboto; font-size: 28px">
  <div class="text-block" style="width: 340px"><span class="raw-text">hello
world!</span></div>
  <div class="text-block" style="width: 340px"><span class="raw-text">hello world!</span></div>
</div>
"#;
    let actual = capture_with_ua_css(
        "a_literal_newline_breaks_the_line_where_it_is_written",
        RAW_TEXT_SHEET,
        FRAGMENT,
        360.0,
        130.0,
    );
    assert_web_text_golden("with-new-line", &actual);
}

/// Replicates `text/color`
/// (`packages/web-platform/web-tests/dist/basic-element-text-color/index.web.json`,
/// `packages/web-platform/web-core-e2e/tests/reactlynx.spec.ts:2881`): two
/// equal-specificity class rules on one `text`, where the later one in the
/// sheet wins both the `font-size` and a gradient `color` over a solid one —
/// the class attribute's own order decides nothing.
///
/// Sibling metric test:
/// `the_later_of_two_equal_specificity_class_rules_wins_on_a_text` in
/// `crates/bobcat-core/src/main/tree/web_text_replication.rs`, which asserts the
/// computed `color` is a gradient and the run shapes at 80px. "Is a gradient"
/// is a style read; whether the ramp reaches the *ink*, and in which direction,
/// is the golden's to say — and `a_gradient_color_on_a_nested_run_fills_only_
/// that_run` in `crates/dom/tests/web_text_replication.rs` is the standing
/// proof that a paragraph can hold a gradient in its style and still paint a
/// flat fill.
#[test]
fn the_later_class_rule_s_gradient_and_size_reach_the_ink() {
    const AUTHOR_CSS: &str = ".a { color: red; font-size: 16px }\n\
                              .b { color: linear-gradient(green, #ff0); font-size: 80px }";
    const FRAGMENT: &str = r#"
<div style="display: flex; width: 440px; height: 140px; padding: 10px; box-sizing: border-box; background-color: white; font-family: Roboto">
  <div class="text-block a b" style="display: flex; width: 420px">hello lynx</div>
</div>
"#;
    let actual = capture_with_author_css(
        "the_later_class_rule_s_gradient_and_size_reach_the_ink",
        AUTHOR_CSS,
        FRAGMENT,
        440.0,
        140.0,
    );
    assert_web_text_golden("color", &actual);
}

/// Replicates `text/text-with-linear-gradient`
/// (`packages/web-platform/web-tests/dist/basic-element-text-text-with-linear-gradient/index.web.
/// json`, `packages/web-platform/web-core-e2e/tests/reactlynx.spec.ts:2623`): Lynx's
/// gradient-valued `color` paints glyph ink with the ramp rather than a solid
/// fill.
///
/// Sibling metric test: `gradient_valued_color_paints_glyph_ink_with_the_ramp`
/// in `crates/dom/tests/web_text_replication.rs`, which samples four pixels of
/// two Ahem em squares — the top and bottom of the ramp, a point below the
/// block, and a point inside the block but off the glyphs. Four pixels of a
/// solid rectangle are exactly what a proportional face's glyphs are not: this
/// golden shows the ramp running down real letterforms, with the white of the
/// counters and the sidebearings between them untouched, which is the claim the
/// original's golden made.
#[test]
fn a_gradient_color_runs_down_real_letterforms() {
    const FRAGMENT: &str = r#"
<div style="display: flex; width: 400px; height: 110px; padding: 10px; box-sizing: border-box; background-color: white; font-family: Roboto">
  <div class="text-block" style="width: 380px; font-size: 80px; color: linear-gradient(green, yellow)">sub text</div>
</div>
"#;
    let actual = screenshot::capture(
        "a_gradient_color_runs_down_real_letterforms",
        FRAGMENT,
        400.0,
        110.0,
    );
    assert_web_text_golden("text-with-linear-gradient", &actual);
}

/// Replicates the first and third paragraphs of `text/linear-gradient-color`
/// (`packages/web-platform/web-tests/dist/basic-element-text-linear-gradient-color/index.web.json`,
/// `packages/web-platform/web-core-e2e/tests/reactlynx.spec.ts:2765`): a
/// gradient `color` on the block itself, and the same gradient inherited by an
/// unstyled nested run.
///
/// Sibling metric test: `a_gradient_color_fills_a_block_and_the_run_that_
/// inherits_it` in `crates/dom/tests/web_text_replication.rs`, which carries the
/// same two paragraphs and samples the green and yellow ends of each.
///
/// The fixture's **middle** paragraph — a solid-coloured block with one
/// gradient-coloured nested run — is deliberately absent from this fragment.
/// It does not render correctly today: the gradient tile is resolved once per
/// paragraph from the establishing element's own colour
/// (`crates/dom/src/paint/walker.rs:816-817`), so a nested run's ramp falls back
/// to a flat fill. Its metric half is `#[ignore]`d as
/// `a_gradient_color_on_a_nested_run_fills_only_that_run`; drawing it here would
/// freeze that defect into a committed PNG, so this golden shows only the two
/// paragraphs that are right. This is the same split, and for the same reason,
/// that the metric replica makes.
#[test]
fn a_block_s_gradient_reaches_the_run_that_inherits_it() {
    const FRAGMENT: &str = r#"
<div style="display: flex; flex-direction: column; width: 400px; height: 140px; padding: 10px; gap: 10px; box-sizing: border-box; background-color: white; font-family: Roboto; font-size: 48px">
  <div class="text-block" style="width: 380px; color: linear-gradient(green, yellow)">gradient block</div>
  <div class="text-block" style="width: 380px; color: linear-gradient(green, yellow)"><span class="text-block">inherited run</span></div>
</div>
"#;
    let actual = screenshot::capture(
        "a_block_s_gradient_reaches_the_run_that_inherits_it",
        FRAGMENT,
        400.0,
        140.0,
    );
    assert_web_text_golden("linear-gradient-color", &actual);
}

/// The Lynx UA sheet's truncation clauses, as much of them as a fixture here
/// can wear (`crates/bobcat-core/src/main/tree/text.rs:121-132`): the two
/// registered integer properties a paragraph reads its limit and its marker
/// flag from, `inline-truncation`'s `display: none` default, and the rule that
/// lifts it for a `text`'s **own** child — web-core's `:scope >
/// inline-truncation` scope. `crates/dom` names no Lynx tag, so the `text` half
/// of that child combinator is spelled as `support/html.rs`'s `text-block`
/// class; the marker keeps its real tag name, which nothing in the engine reads.
const TRUNCATION_SHEET: &str = r#"
@property --lynx-text-maxline { syntax: "<integer>"; inherits: false; initial-value: 0; }
@property --lynx-inline-truncation { syntax: "<integer>"; inherits: false; initial-value: 0; }
inline-truncation { display: none; }
.text-block > inline-truncation { display: -lynx-text !important; --lynx-inline-truncation: 1; }
"#;

/// Replicates `x-text/text-maxline-with-custom-truncation`
/// (`packages/web-platform/web-elements/tests/fixtures/x-text/text-maxline-with-custom-truncation.
/// html`, `packages/web-platform/web-elements/tests/web-elements.spec.ts:226`)
/// together with its control,
/// `x-text/text-no-maxline-do-not-show-inline-truncation`
/// (`.../fixtures/x-text/text-no-maxline-do-not-show-inline-truncation.html`,
/// `web-elements.spec.ts:244`): a clamped paragraph that overflows lays its
/// `inline-truncation` child's content in at the end of the last visible line,
/// in that child's own colour and in place of the units a retreat frees for it;
/// a paragraph that does not overflow paints none of it.
///
/// Sibling metric tests: `a_shown_truncation_subtree_is_painted_at_the_clamp`
/// in `crates/dom/tests/web_text_replication.rs`, whose fixture this is — the
/// same 100px break-all paragraph of thirty Ahem squares clamped to two lines,
/// with a one-square marker — and
/// `a_shown_truncation_marker_paints_its_runs_and_a_background_behind_them_only`
/// beside it
/// for the empty-with-nothing-to-clamp half. Both sample single pixels; what
/// they cannot show, and this golden can, is the *whole* clamp line at once:
/// that exactly three black squares are kept, that the red square is the
/// fourth and not somewhere else on the line, that the fifth stays empty
/// because the retreat's floor of two units is wider than the content asked
/// for, and that the four clamped-away lines below leave no ink at all.
///
/// Ahem, not this file's usual Roboto: every claim here is a position on a
/// line, and the reference's own picture — aqua marker text at the end of a
/// clamped paragraph — is one a proportional face would only blur. The sizes
/// are integers and the `line-height` explicit, so the frame is exact
/// arithmetic: a 20px em square, five to a 100px line.
///
/// The fixture's aqua is written red here, as the metric siblings write it: on
/// white, aqua's luminance is close enough to the paragraph's own that a
/// reviewer could not tell the marker from kept text at a glance, which is the
/// one thing this golden exists to show.
#[test]
fn a_custom_truncation_marker_is_laid_in_at_the_clamp_in_its_own_colour() {
    const FRAGMENT: &str = r#"
<div style="display: flex; flex-direction: column; width: 200px; height: 100px; padding: 10px; gap: 10px; box-sizing: border-box; background-color: white; font-family: Ahem; font-size: 20px; line-height: 20px; color: black">
  <div class="text-block" style="width: 100px; word-break: break-all; --lynx-text-maxline: 2">HHHHHHHHHHHHHHHHHHHHHHHHHHHHHH<inline-truncation style="color: red">H</inline-truncation></div>
  <div class="text-block" style="width: 100px; word-break: break-all">HHHHH<inline-truncation style="color: red">H</inline-truncation></div>
</div>
"#;
    let actual = capture_ahem_with_ua_css(
        "a_custom_truncation_marker_is_laid_in_at_the_clamp_in_its_own_colour",
        TRUNCATION_SHEET,
        FRAGMENT,
        200.0,
        100.0,
    );
    assert_web_text_golden("text-maxline-with-custom-truncation", &actual);
}

/// **Not** a `lynx-stack` case: no web-elements fixture puts a background on
/// an inline `text`, so there is no reference picture to replicate. What it
/// pins is the reference *behaviour* — web-core makes a nested
/// `x-text`/`inline-text` `display: inline` and adds nothing to it but
/// `background-clip: inherit`
/// (`packages/web-platform/web-elements/src/elements/XText/x-text.css:52-67`),
/// so the browser paints its background as an inline box's: one fragment per
/// line, over the font's content area rather than the line box. Native Lynx
/// fills the line box instead; the 2026-09-16 ruling follows web-core
/// (`docs/tracking/web-text-test-replication.md`, conflict 8).
///
/// Sibling metric tests: the whole inline-background group in
/// `crates/dom/tests/web_text_replication.rs`, from
/// `a_nested_scope_s_background_paints_behind_its_own_fragments_only`. Those
/// sample single pixels of Ahem em squares with the ink painted transparent,
/// because solid squares would hide the very band they measure. This golden is
/// the other half: real letterforms over a background a reviewer can see is
/// *behind the text and nothing else* — the first paragraph's `line-height:
/// 40px` leaves visible half-leading above and below the band, which is the
/// whole difference from the native geometry, and the second shows the two
/// fragments a wrapped scope paints, each rounded by its own `border-radius`
/// (`box-decoration-break: clone`, an approximation of the web default this
/// engine has no property to express).
#[test]
fn a_nested_scope_paints_a_background_behind_its_own_line_fragments() {
    const FRAGMENT: &str = r#"
<div style="display: flex; flex-direction: column; gap: 16px; width: 400px; height: 200px; padding: 10px; box-sizing: border-box; background-color: white; font-family: Roboto">
  <div class="text-block" style="width: 380px; font-size: 24px; line-height: 40px; font-weight: bold">I am bold <span class="text-block" style="color: red; background-color: #ffe08a">and red</span> again</div>
  <div class="text-block" style="width: 200px; font-size: 24px; line-height: 36px">go <span class="text-block" style="background-image: linear-gradient(90deg, #4a90e2, #e94e77); border-radius: 6px; color: white">over two whole lines of it</span> now</div>
</div>
"#;
    let actual = screenshot::capture(
        "a_nested_scope_paints_a_background_behind_its_own_line_fragments",
        FRAGMENT,
        400.0,
        200.0,
    );
    assert_web_text_golden("inline-text-background", &actual);
}
