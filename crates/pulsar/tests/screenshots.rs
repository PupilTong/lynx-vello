//! Golden screenshot comparison over the full test pipeline:
//! inline-styled `<div>` fragment → `dom` → `pulsar` → headless GPU.
//!
//! Capture, comparison, and golden management belong to `flashbulb`; this file
//! only supplies the document. The reference PNG is committed under
//! `tests/screenshots`. Refresh it with:
//! `FLASHBULB_UPDATE_SNAPSHOTS=1 cargo test -p pulsar --test screenshots`.

mod common;
#[path = "support/html.rs"]
mod html;

use flashbulb::vello::peniko::Color;
use flashbulb::{capture_document, headless_or_skip};

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
    let Some(mut gpu) = headless_or_skip("inline_style_fragment_matches_reference") else {
        return;
    };
    let mut doc = html::parse(FRAGMENT, SCREEN_WIDTH, SCREEN_HEIGHT);
    let actual =
        capture_document(&mut gpu, &mut doc.dom, Color::WHITE).expect("headless screenshot render");

    flashbulb::screenshots_in(env!("CARGO_MANIFEST_DIR"))
        .assert_matches(&["inline-style"], &actual);
}
