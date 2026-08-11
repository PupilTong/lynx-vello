//! Golden screenshots of DOM's private text painter.
//!
//! Each case exists to be *looked at* as much as to be diffed: glyph
//! rasterization quality, synthesis, and how decorations sit against real
//! letterforms are all things a numeric assertion cannot judge but a reviewer
//! can see at a glance. `tests/gpu_pixels.rs` covers the complementary half —
//! Ahem's solid em squares make individual pixels assertable, at the cost of
//! showing nothing about rendering quality.
//!
//! Every fixture renders **vendored Roboto**, never a host font; see
//! `support/screenshot.rs` for why that is not optional. Refresh with:
//! `FLASHBULB_UPDATE_SNAPSHOTS=1 cargo test -p dom --test text_screenshots`.

#[path = "support/html.rs"]
mod html;
mod paint_common;
#[path = "support/screenshot.rs"]
mod screenshot;

use flashbulb::Image;

fn assert_text_golden(case: &str, actual: &Image) {
    screenshot::assert_golden(&["text", case], actual);
}

#[test]
fn text_specimen() {
    const FRAGMENT: &str = r#"
<div style="display: flex; flex-direction: column; width: 420px; height: 300px; padding: 16px 20px; gap: 2px; box-sizing: border-box; background-color: white; font-family: Roboto; color: #111827">
  <div style="display: flex; font-size: 11px">Sphinx of black quartz, judge my vow — 11px</div>
  <div style="display: flex; font-size: 13px">Sphinx of black quartz, judge my vow — 13px</div>
  <div style="display: flex; font-size: 16px">Sphinx of black quartz — 16px</div>
  <div style="display: flex; font-size: 22px">Sphinx of black — 22px</div>
  <div style="display: flex; font-size: 32px">Hamburgefonstiv</div>
  <div style="display: flex; font-size: 44px">Hamburg 0123</div>
  <div style="display: flex; font-size: 20px; font-weight: 700">Synthetic bold 700</div>
  <div style="display: flex; font-size: 20px; font-style: italic">Synthetic oblique</div>
  <div style="display: flex; font-size: 20px; color: #dc2626">Colored fill (#dc2626)</div>
</div>
"#;
    let actual = screenshot::capture("text_specimen", FRAGMENT, 420.0, 300.0);
    assert_text_golden("specimen", &actual);
}

#[test]
fn text_decorations() {
    const FRAGMENT: &str = r#"
<div style="display: flex; flex-direction: column; width: 460px; height: 430px; padding: 16px 20px; gap: 6px; box-sizing: border-box; background-color: white; font-family: Roboto; font-size: 26px; color: #111827">
  <div style="display: flex; text-decoration: underline">Underline solid</div>
  <div style="display: flex; text-decoration: underline double">Underline double</div>
  <div style="display: flex; text-decoration: underline dotted">Underline dotted</div>
  <div style="display: flex; text-decoration: underline dashed">Underline dashed</div>
  <div style="display: flex; text-decoration: underline wavy">Underline wavy</div>
  <div style="display: flex; text-decoration: line-through">Line-through solid</div>
  <div style="display: flex; text-decoration: line-through double">Line-through double</div>
  <div style="display: flex; text-decoration: underline line-through">Both lines at once</div>
  <div style="display: flex; text-decoration: underline #2563eb wavy">Decoration color wins</div>
  <div style="display: flex; text-decoration: underline #16a34a">
    <span style="display: flex; color: #dc2626">Propagated from the parent</span>
  </div>
</div>
"#;
    let actual = screenshot::capture("text_decorations", FRAGMENT, 460.0, 430.0);
    assert_text_golden("decorations", &actual);
}

#[test]
fn text_shadow_and_stroke() {
    const FRAGMENT: &str = r#"
<div style="display: flex; flex-direction: column; width: 460px; height: 330px; padding: 16px 20px; gap: 12px; box-sizing: border-box; background-color: white; font-family: Roboto; font-size: 28px; color: #111827">
  <div style="display: flex; text-shadow: 4px 4px 0px #93c5fd">Offset shadow, no blur</div>
  <div style="display: flex; text-shadow: 4px 4px 6px #9ca3af">Blur radius is ignored</div>
  <div style="display: flex; color: white; text-shadow: 2px 2px 0px #111827">White fill, dark shadow</div>
  <div style="display: flex; text-stroke: 1px #2563eb">Thin stroke over fill</div>
  <div style="display: flex; color: #fde68a; text-stroke: 3px #b45309">Heavy stroke over fill</div>
  <div style="display: flex; text-decoration: underline; text-shadow: 4px 4px 0px #fca5a5">Shadow takes the underline</div>
</div>
"#;
    let actual = screenshot::capture("text_shadow_and_stroke", FRAGMENT, 460.0, 330.0);
    assert_text_golden("shadow-and-stroke", &actual);
}

#[test]
fn text_paragraph() {
    const FRAGMENT: &str = r#"
<div style="display: flex; flex-direction: column; width: 420px; height: 420px; padding: 16px 20px; gap: 10px; box-sizing: border-box; background-color: white; font-family: Roboto; font-size: 15px; color: #1f2937">
  <div style="display: flex; width: 380px; line-height: 1.5">Typography is the craft of arranging type to make written language legible, readable and appealing when displayed.</div>
  <div style="display: flex; width: 380px; line-height: 2.2; background-color: #f3f4f6">Loose line-height at 2.2 spreads these wrapped lines apart, and the band of background behind them shows where each line box sits.</div>
  <div style="display: flex; width: 380px; letter-spacing: 2px">Letter-spacing of two pixels</div>
  <div style="display: flex; width: 380px; text-align: center; background-color: #f3f4f6">Centered text, wrapped over enough lines that the last one has room left over to be centered in.</div>
  <div style="display: flex; width: 380px; text-align: right; background-color: #f3f4f6">Right-aligned text, wrapped over enough lines that the last one has room left over to sit against the right edge.</div>
  <div style="display: flex; width: 380px; text-indent: 28px; background-color: #f3f4f6">Indented first line, then the rest of the paragraph wraps back to the left edge of the box.</div>
</div>
"#;
    let actual = screenshot::capture("text_paragraph", FRAGMENT, 420.0, 420.0);
    assert_text_golden("paragraph", &actual);
}

#[test]
fn text_color_gradient() {
    const FRAGMENT: &str = r#"
<div style="display: flex; flex-direction: column; width: 440px; height: 400px; padding: 16px 20px; gap: 12px; box-sizing: border-box; background-color: white; font-family: Roboto; font-size: 32px; font-weight: 700">
  <div style="display: flex; color: linear-gradient(90deg, #f43f5e, #6366f1, #14b8a6)">Linear ramp</div>
  <div style="display: flex; color: radial-gradient(circle, #f59e0b, #7c3aed)">Radial ramp</div>
  <div style="display: flex; color: conic-gradient(#ef4444, #22c55e, #3b82f6, #ef4444)">Conic ramp</div>
  <div style="display: flex; padding-left: 40px; color: linear-gradient(90deg, #f43f5e, #14b8a6)">Padded</div>
  <div style="display: flex; width: 320px; font-size: 22px; color: linear-gradient(180deg, #f43f5e, #14b8a6)">This wraps onto three lines, so a vertical ramp has to walk rose to teal down them.</div>
  <div style="display: flex; font-size: 26px; text-decoration: underline; color: linear-gradient(90deg, #f59e0b, #ef4444)">Underline stays solid</div>
</div>
"#;
    let actual = screenshot::capture("text_color_gradient", FRAGMENT, 440.0, 400.0);
    assert_text_golden("color-gradient", &actual);
}

#[test]
fn text_color_gradient_over_background() {
    const FRAGMENT: &str = r#"
<div style="display: flex; flex-direction: column; width: 460px; height: 400px; padding: 16px 20px; gap: 14px; box-sizing: border-box; background-color: white; font-family: Roboto; font-size: 30px; font-weight: 700">
  <div style="display: flex; background-color: #111827; color: linear-gradient(90deg, #fde68a, #f43f5e)">Solid backdrop</div>
  <div style="display: flex; border: 8px solid #0f766e; padding: 4px; background-color: #111827; color: linear-gradient(90deg, #fde68a, #f43f5e)">Border moves it</div>
  <div style="display: flex; background-image: linear-gradient(90deg, #1e3a8a, #111827); color: linear-gradient(270deg, #fde68a, #f43f5e)">Two ramps, opposed</div>
  <div style="display: flex; padding: 2px 10px; border-radius: 16px; background-color: #4c1d95; color: linear-gradient(90deg, #fde68a, #5eead4)">Rounded backdrop</div>
  <div style="display: flex; background-clip: text; background-color: #111827; color: linear-gradient(90deg, #fbbf24, #a855f7)">Clip plus gradient</div>
</div>
"#;
    let actual = screenshot::capture(
        "text_color_gradient_over_background",
        FRAGMENT,
        460.0,
        400.0,
    );
    assert_text_golden("color-gradient-over-background", &actual);
}

#[test]
fn text_background_clip() {
    const FRAGMENT: &str = r#"
<div style="display: flex; flex-direction: column; width: 440px; height: 260px; padding: 16px 20px; gap: 14px; box-sizing: border-box; background-color: white; font-family: Roboto; font-size: 40px; font-weight: 700">
  <div style="display: flex; color: transparent; background-clip: text; background-image: linear-gradient(90deg, #f43f5e, #6366f1, #14b8a6)">Gradient ink</div>
  <div style="display: flex; color: transparent; background-clip: text; background-color: #7c3aed">Solid ink</div>
  <div style="display: flex; color: transparent; text-decoration: underline #111827; background-clip: text; background-image: linear-gradient(90deg, #f59e0b, #ef4444)">Bare underline</div>
</div>
"#;
    let actual = screenshot::capture("text_background_clip", FRAGMENT, 440.0, 260.0);
    assert_text_golden("background-clip", &actual);
}
