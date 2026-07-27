//! Golden screenshot comparison over the full test pipeline:
//! inline-styled `<div>` fragment → `w3c-dom` → `pulsar` → headless GPU.
//!
//! The reference PNG is committed under `tests/screenshots`. Refresh it with:
//! `PULSAR_UPDATE_SCREENSHOTS=1 cargo test -p pulsar --test screenshots`.

mod common;
#[path = "support/html.rs"]
mod html;

use std::fs;

use pulsar::gpu::{GpuError, Headless};
use pulsar::vello::peniko::Color;
use pulsar::{ImageStore, Painter};

// lynx-stack's Playwright Chromium project uses the Pixel 5 viewport, and its
// tracked viewport screenshots are 393 × 727 pixels.
const SCREEN_WIDTH: f32 = 393.0;
const SCREEN_HEIGHT: f32 = 727.0;
const IMAGE_WIDTH: u32 = 393;
const IMAGE_HEIGHT: u32 = 727;
const UPDATE_ENV: &str = "PULSAR_UPDATE_SCREENSHOTS";
const REFERENCE_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/screenshots/inline-style.png"
);
const REFERENCE_PNG: &[u8] = include_bytes!("screenshots/inline-style.png");
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
    let mut doc = html::parse(FRAGMENT, SCREEN_WIDTH, SCREEN_HEIGHT);
    let frame = doc.dom.paint_order();
    let mut painter = Painter::new();
    let scene = painter.paint(&doc.dom, &frame, &ImageStore::new());

    let mut gpu = match Headless::new() {
        Ok(gpu) => gpu,
        Err(GpuError::NoAdapter) => {
            eprintln!("skipping screenshot comparison: no usable GPU adapter");
            return;
        }
        Err(error) => panic!("GPU init failed: {error}"),
    };
    let actual = gpu
        .render(scene, IMAGE_WIDTH, IMAGE_HEIGHT, Color::WHITE)
        .expect("headless screenshot render");
    let actual_png = encode_png(&actual);

    if std::env::var(UPDATE_ENV).as_deref() == Ok("1") {
        fs::write(REFERENCE_PATH, actual_png).expect("write screenshot reference");
        eprintln!("updated {REFERENCE_PATH}");
        return;
    }

    assert!(
        actual_png == REFERENCE_PNG,
        "inline-style screenshot differs from {REFERENCE_PATH}; \
         set {UPDATE_ENV}=1 to accept it"
    );
}

fn encode_png(pixels: &[u8]) -> Vec<u8> {
    let mut bytes = Vec::new();
    let mut encoder = png::Encoder::new(&mut bytes, IMAGE_WIDTH, IMAGE_HEIGHT);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder
        .write_header()
        .expect("encode screenshot header")
        .write_image_data(pixels)
        .expect("encode screenshot pixels");
    bytes
}
