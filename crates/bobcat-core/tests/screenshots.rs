#![cfg(feature = "quickjs")]

//! Golden screenshot comparison over the whole runtime pipeline:
//! main-thread script → Element PAPI → `lynx-element` → `dom` →
//! headless GPU.
//!
//! This is the Rust analogue of lynx-stack's `web-core-e2e` Playwright suite:
//! a fixture drives the Element PAPI, the result is captured at the same
//! Pixel 5 CSS viewport their Chromium project uses, and the image is compared
//! to a committed golden with `pixelmatch` tolerances.
//!
//! Refresh the goldens with:
//! `FLASHBULB_UPDATE_SNAPSHOTS=1 cargo test -p bobcat-core --test screenshots`.

use bobcat_core::engine::OffscreenEngine;
use flashbulb::vello::peniko::{Blob, ImageAlphaType, ImageData, ImageFormat};
use flashbulb::{Image, Screenshots};
use lynx_element::PageConfig;

fn engine(config: PageConfig) -> OffscreenEngine {
    OffscreenEngine::new(config, VIEWPORT_WIDTH, VIEWPORT_HEIGHT, 1.0).expect("engine")
}

const VIEWPORT_WIDTH: f32 = 393.0;
const VIEWPORT_HEIGHT: f32 = 727.0;

const STYLE: &str = r"
page {
  background-color: #e5e7eb;
  padding: 16px;
}
page > view {
  linear-direction: row;
  height: 96px;
  margin-bottom: 16px;
}
page > view > view {
  width: 96px;
  height: 96px;
  margin-right: 16px;
  border: 4px solid #1f2937;
}
page > view:nth-child(1) > view:nth-child(1) { background-color: #2563eb; }
page > view:nth-child(1) > view:nth-child(2) { background-color: #14b8a6; }
page > view:nth-child(1) > view:nth-child(3) { background-color: #f59e0b; }
page > view:nth-child(2) > view:nth-child(1) { background-color: #ef4444; }
page > view:nth-child(2) > view:nth-child(2) { background-color: #8b5cf6; }
page > view:nth-child(3) > view:nth-child(1) { background-color: #0f766e; }
";

const MAIN_THREAD_SCRIPT: &str = r"
globalThis.elements = [];
globalThis.renderPage = function renderPage() {
  const page = __CreatePage('card', 0);
  const rows = [3, 2, 1];
  for (const cells of rows) {
    const row = __CreateView(0);
    elements.push(row);
    __AppendElement(page, row);
    for (let index = 0; index < cells; index += 1) {
      const cell = __CreateView(0);
      elements.push(cell);
      __AppendElement(row, cell);
    }
  }
};
";

fn screenshots() -> Screenshots {
    flashbulb::screenshots_in(env!("CARGO_MANIFEST_DIR"))
}

#[track_caller]
fn capture_engine(engine: &mut OffscreenEngine, test: &str) -> Image {
    engine
        .attach_offscreen()
        .unwrap_or_else(|error| panic!("{test}: GPU initialization failed: {error}"));
    let shot = engine.capture().expect("capture");
    Image::from_rgba8(shot.size.width, shot.size.height, shot.pixels).expect("image")
}

#[test]
fn a_main_thread_script_renders_its_element_tree() {
    let mut engine = engine(PageConfig::default());
    engine.add_author_stylesheet(STYLE);
    engine
        .run_script(MAIN_THREAD_SCRIPT)
        .expect("main-thread script");

    let image = capture_engine(&mut engine, "a_main_thread_script_renders_its_element_tree");

    assert_eq!(image.width(), 393);
    assert_eq!(image.height(), 727);
    screenshots().assert_matches(&["create-view-append-element"], &image);
}

const OVERFLOW_STYLE: &str = r"
page {
  background-color: #e5e7eb;
  padding: 16px;
}
page > view {
  width: 120px;
  height: 120px;
  margin-bottom: 24px;
  background-color: #ffffff;
  border: 4px solid #1f2937;
}
page > view > view {
  width: 220px;
  height: 90px;
  background-color: #2563eb;
}
";

const OVERFLOW_SCRIPT: &str = r"
globalThis.elements = [];
globalThis.renderPage = function renderPage() {
  const page = __CreatePage('card', 0);
  for (let row = 0; row < 2; row += 1) {
    const box = __CreateView(0);
    const child = __CreateView(0);
    elements.push(box, child);
    __AppendElement(page, box);
    __AppendElement(box, child);
  }
};
";

fn render_overflow(config: PageConfig, test: &str, golden: &str) {
    let mut engine = engine(config);
    engine.add_author_stylesheet(OVERFLOW_STYLE);
    engine
        .run_script(OVERFLOW_SCRIPT)
        .expect("main-thread script");

    let image = capture_engine(&mut engine, test);
    screenshots().assert_matches(&[golden], &image);
}

#[test]
fn overflow_visible_lets_a_child_spill_out_of_its_parent() {
    render_overflow(
        PageConfig::default(),
        "overflow_visible_lets_a_child_spill_out_of_its_parent",
        "overflow-visible",
    );
}

#[test]
fn overflow_hidden_clips_a_child_to_its_parent() {
    render_overflow(
        PageConfig {
            default_overflow_visible: false,
            ..PageConfig::default()
        },
        "overflow_hidden_clips_a_child_to_its_parent",
        "overflow-hidden",
    );
}

const IMAGE_URL: &str = "https://example.test/retained-checker.png";

const IMAGE_STYLE: &str = r#"
page {
  background-color: #e5e7eb;
  padding: 16px;
}
page > view {
  width: 128px;
  height: 96px;
  border: 4px solid #1f2937;
  background-color: #ffffff;
  background-image: url("https://example.test/retained-checker.png");
  background-repeat: no-repeat;
  background-size: 120px 88px;
  image-rendering: pixelated;
}
"#;

const IMAGE_SCRIPT: &str = r"
globalThis.renderPage = function renderPage() {
  const page = __CreatePage('card', 0);
  globalThis.imageView = __CreateView(0);
  __AppendElement(page, imageView);
};
";

fn checker_image() -> ImageData {
    let mut rgba = Vec::with_capacity(4 * 4 * 4);
    for y in 0..4 {
        for x in 0..4 {
            let pixel = match (x < 2, y < 2) {
                (true, true) => [239, 68, 68, 255],
                (false, true) => [34, 197, 94, 255],
                (true, false) => [37, 99, 235, 255],
                (false, false) => [250, 204, 21, 255],
            };
            rgba.extend_from_slice(&pixel);
        }
    }
    ImageData {
        data: Blob::from(rgba),
        format: ImageFormat::Rgba8,
        alpha_type: ImageAlphaType::Alpha,
        width: 4,
        height: 4,
    }
}

#[test]
fn document_image_store_reaches_the_private_painter() {
    let mut engine = engine(PageConfig::default());
    engine.add_author_stylesheet(IMAGE_STYLE);
    engine.with_images(|images| images.insert_url(IMAGE_URL, checker_image()));
    engine.run_script(IMAGE_SCRIPT).expect("main-thread script");

    let image = capture_engine(
        &mut engine,
        "document_image_store_reaches_the_private_painter",
    );
    screenshots().assert_matches(&["retained-image-store"], &image);
}
