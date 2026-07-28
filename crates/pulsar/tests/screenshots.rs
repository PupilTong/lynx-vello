//! Golden screenshot comparison for **box rendering** — backgrounds, borders,
//! radii and nested flex geometry — over the full test pipeline:
//! inline-styled fragment → `dom` → `pulsar` → headless GPU.
//!
//! Text rendering has its own binary, `tests/text_screenshots.rs`; both write
//! into the same crate-level `tests/screenshots` golden tree. Capture,
//! comparison and golden management belong to `flashbulb` — these files only
//! supply the documents. Refresh with:
//! `FLASHBULB_UPDATE_SNAPSHOTS=1 cargo test -p pulsar --test screenshots`.

mod common;
#[path = "support/html.rs"]
mod html;
#[path = "support/screenshot.rs"]
mod screenshot;

// lynx-stack's Playwright Chromium project uses the Pixel 5 viewport, and its
// tracked viewport screenshots are 393 × 727 CSS pixels.
const SCREEN_WIDTH: f32 = 393.0;
const SCREEN_HEIGHT: f32 = 727.0;
const FRAGMENT: &str = r#"
<div style="display: flex; width: 393px; height: 727px; padding: 12px; gap: 12px; box-sizing: border-box; background-color: #e5e7eb">
  <div style="display: flex; flex-direction: column; width: 78px; height: 104px; padding: 8px; gap: 8px; box-sizing: border-box; border: 4px solid #2563eb; background-color: white">
    <div style="display: flex; width: 54px; height: 28px; background-color: #14b8a6"></div>
    <div style="display: flex; width: 54px; height: 40px; gap: 6px">
      <div style="display: flex; width: 24px; height: 40px; background-color: #8b5cf6"></div>
      <div style="display: flex; width: 24px; height: 40px; background-color: #f59e0b"></div>
    </div>
  </div>
  <div style="display: flex; flex-direction: column; width: 78px; height: 104px; padding: 8px; gap: 8px; box-sizing: border-box; border: 4px solid #1f2937; background-color: white">
    <div style="display: flex; width: 54px; height: 28px; background-color: #ef4444"></div>
    <div style="display: flex; width: 54px; height: 40px; gap: 6px">
      <div style="display: flex; width: 24px; height: 40px; background-color: #0f766e"></div>
      <div style="display: flex; width: 24px; height: 40px; background-color: #8b5cf6"></div>
    </div>
  </div>
</div>
"#;

#[test]
fn inline_style_fragment_matches_reference() {
    let Some(actual) = screenshot::capture(
        "inline_style_fragment_matches_reference",
        FRAGMENT,
        SCREEN_WIDTH,
        SCREEN_HEIGHT,
    ) else {
        return;
    };
    screenshot::assert_golden(&["inline-style"], &actual);
}
