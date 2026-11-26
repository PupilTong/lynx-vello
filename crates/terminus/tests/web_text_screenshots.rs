//! Golden screenshots for the `lynx-stack` text cases whose own oracle was a
//! picture.
//!
//! The originals are Playwright full-page diffs at `maxDiffPixelRatio: 0`
//! (`packages/web-platform/web-elements/tests/web-elements.spec.ts`,
//! `packages/web-platform/web-core-e2e/tests/reactlynx.spec.ts`). The metric
//! replicas of those cases — in `crates/terminus/tests/web_text_replication.rs`,
//! `crates/bobcat-core/src/main/tree/web_text_replication.rs` and
//! `crates/seldon/tests/web_text_replication.rs` — assert advances, frames and
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
//! `crates/terminus/tests/web_text_replication.rs`:
//!
//! * `crates/terminus` holds no Lynx element vocabulary, so a `<text>` is spelled as an element
//!   wearing `support/html.rs`'s `text-block` class, which that harness creates as a Lynx text
//!   element — and a compiled `<raw-text>` as [`RAW_TEXT_SHEET`]'s `display: contents;
//!   white-space-collapse: preserve-breaks`, which is what that element's UA rules amount to
//!   (`crates/bobcat-core/src/main/tree/raw_text.rs:8-11`).
//! * `crates/terminus` has no UA sheet at all, so a container that stacks its children says
//!   `display: flex; flex-direction: column` rather than relying on a block-level default. Where a
//!   case's claim is about block-level stacking, it is the metric sibling that carries it.
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
//! (`crates/prime-radiant/src/golden.rs:100-103`), so the second run would pass
//! against a golden of the defect. The two visual claims in that position —
//! the marker on a bare `text-maxline` clamp, and a one-line clamp keeping its
//! nested run's colour — are carried by `#[ignore]`d metric siblings:
//! `a_bare_maxline_clamp_marks_its_last_visible_line` in
//! `crates/seldon/tests/web_text_replication.rs` and
//! `a_one_line_clamp_fills_the_available_width_instead_of_breaking_at_a_word`
//! in `crates/bobcat-core/src/main/tree/web_text_replication.rs`. Add the
//! golden here when those close.
//!
//! Refresh with:
//! `PRIME_RADIANT_UPDATE_SNAPSHOTS=1 cargo test -p terminus --test web_text_screenshots`.

#[path = "support/html.rs"]
mod html;
mod paint_common;
#[path = "support/screenshot.rs"]
mod screenshot;

use prime_radiant::Image;

const RAW_TEXT_SHEET: &str =
    ".raw-text { display: contents; white-space-collapse: preserve-breaks; }";

fn assert_web_text_golden(case: &str, actual: &Image) {
    screenshot::assert_golden(&["web-text", case], actual);
}

fn capture_with_sheet(
    test: &str,
    css: &str,
    origin: terminus::link::StylesheetOrigin,
    font: &'static [u8],
    fragment: &str,
    width: f32,
    height: f32,
) -> Image {
    let mut doc = html::parse(fragment, width, height);
    doc.dom.add_stylesheet(css, origin);
    assert_eq!(
        doc.dom
            .register_fonts(terminus::link::FontBlob::from_static(font)),
        1,
        "the vendored fixture face must register exactly one face"
    );
    screenshot::capture_prebuilt_document(test, &mut doc.dom, &terminus::link::NoImages)
}

fn capture_with_ua_css(test: &str, ua_css: &str, fragment: &str, width: f32, height: f32) -> Image {
    capture_with_sheet(
        test,
        ua_css,
        terminus::link::StylesheetOrigin::UserAgent,
        screenshot::ROBOTO,
        fragment,
        width,
        height,
    )
}

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
        terminus::link::StylesheetOrigin::UserAgent,
        screenshot::AHEM,
        fragment,
        width,
        height,
    )
}

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
        terminus::link::StylesheetOrigin::Author,
        screenshot::ROBOTO,
        fragment,
        width,
        height,
    )
}

#[test]
fn a_nested_run_wears_its_own_colour_on_the_parent_s_line() {
    const FRAGMENT: &str = r#"
<div style="display: flex; width: 400px; height: 70px; padding: 10px; box-sizing: border-box; background-color: white; font-family: Roboto">
  <div class="text-block" style="width: 380px; font-size: 24px; font-weight: bold">I am bold <span class="text-block" style="font-size: 24px; color: red">and red</span></div>
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

const TRUNCATION_SHEET: &str = r#"
@property --lynx-text-maxline { syntax: "<integer>"; inherits: false; initial-value: 0; }
@property --lynx-inline-truncation { syntax: "<integer>"; inherits: false; initial-value: 0; }
:not(.text-block) > inline-truncation { display: none; }
.text-block > inline-truncation { --lynx-inline-truncation: 1; }
"#;

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
