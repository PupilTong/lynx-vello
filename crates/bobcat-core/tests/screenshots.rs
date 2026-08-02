#![cfg(feature = "quickjs")]

//! Golden screenshot comparison over the whole runtime pipeline:
//! main-thread script → Element PAPI → `lynx-element` → `dom` → `pulsar` →
//! headless GPU.
//!
//! This is the Rust analogue of lynx-stack's `web-core-e2e` Playwright suite:
//! a fixture drives the Element PAPI, the result is captured at the same
//! Pixel 5 CSS viewport their Chromium project uses, and the image is compared
//! to a committed golden with `pixelmatch` tolerances.
//!
//! Refresh the goldens with:
//! `FLASHBULB_UPDATE_SNAPSHOTS=1 cargo test -p bobcat-core --test screenshots`.

use bobcat_core::pulsar::gpu::Headless;
use bobcat_core::{ElementTree, MainThreadRuntime, PageConfig, Viewport};
use flashbulb::vello::peniko::{Blob, Color, ImageAlphaType, ImageData, ImageFormat};
use flashbulb::{Image, Screenshots, capture_scene, headless};

/// lynx-stack's Playwright Chromium project emulates a Pixel 5, whose CSS
/// viewport is 393 × 727; `toHaveScreenshot` captures in CSS pixels, so their
/// tracked goldens are exactly that size.
const VIEWPORT: Viewport = Viewport::new(393.0, 727.0);

/// Styling reaches the tree through an author stylesheet rather than through
/// the PAPI: `__SetClasses`, `__AddInlineStyle`, and `__SetCSSId` are not
/// implemented, so every rule below selects on tag and position — which is
/// also how a decoded `.web.bundle` `StyleInfo` section will address elements
/// once its lowering exists.
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

/// A card root in the shape a real `lepusCode.root` has: it assigns
/// `renderPage` onto `globalThis` and builds the tree from inside it.
const MAIN_THREAD_SCRIPT: &str = r"
globalThis.renderPage = function renderPage() {
  const page = __CreatePage('card', 0);
  const rows = [3, 2, 1];
  for (const cells of rows) {
    const row = __CreateView(0);
    __AppendElement(page, row);
    for (let index = 0; index < cells; index += 1) {
      __AppendElement(row, __CreateView(0));
    }
  }
};
";

fn screenshots() -> Screenshots {
    flashbulb::screenshots_in(env!("CARGO_MANIFEST_DIR"))
}

fn capture_elements(gpu: &mut Headless, elements: &mut ElementTree) -> Image {
    elements.render();
    let scene = elements.scene();
    capture_scene(gpu, elements.document(), &scene, Color::WHITE).expect("capture")
}

#[test]
fn a_main_thread_script_renders_its_element_tree() {
    let mut gpu = headless("a_main_thread_script_renders_its_element_tree");

    let mut runtime = MainThreadRuntime::new(ElementTree::new(VIEWPORT, PageConfig::default()))
        .expect("QuickJS realm");
    runtime.elements_mut().add_author_stylesheet(STYLE);
    runtime
        .run_main_thread_script(MAIN_THREAD_SCRIPT)
        .expect("main-thread script");

    let image = {
        let mut elements = runtime.elements_mut();
        capture_elements(&mut gpu, &mut elements)
    };

    assert_eq!(image.width(), 393);
    assert_eq!(image.height(), 727);
    screenshots().assert_matches(&["create-view-append-element"], &image);
}

/// A child larger than its parent, rendered under both settings of the
/// `defaultOverflowVisible` page config.
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
globalThis.renderPage = function renderPage() {
  const page = __CreatePage('card', 0);
  for (let row = 0; row < 2; row += 1) {
    const box = __CreateView(0);
    __AppendElement(page, box);
    __AppendElement(box, __CreateView(0));
  }
};
";

fn render_overflow(config: PageConfig, test: &str, golden: &str) {
    let mut gpu = headless(test);
    let mut runtime =
        MainThreadRuntime::new(ElementTree::new(VIEWPORT, config)).expect("QuickJS realm");
    runtime.elements_mut().add_author_stylesheet(OVERFLOW_STYLE);
    runtime
        .run_main_thread_script(OVERFLOW_SCRIPT)
        .expect("main-thread script");

    let image = {
        let mut elements = runtime.elements_mut();
        capture_elements(&mut gpu, &mut elements)
    };
    screenshots().assert_matches(&[golden], &image);
}

/// `defaultOverflowVisible: true` — the bundled default. The oversized child
/// spills past its parent's border box.
#[test]
fn overflow_visible_lets_a_child_spill_out_of_its_parent() {
    render_overflow(
        PageConfig::default(),
        "overflow_visible_lets_a_child_spill_out_of_its_parent",
        "overflow-visible",
    );
}

/// `defaultOverflowVisible: false` — the UA sheet emits `overflow: hidden`,
/// and the same child is clipped to its parent's padding box. Rendering both
/// keeps the page-config switch honest end to end: if it stopped reaching the
/// cascade, these two goldens would converge.
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
  __AppendElement(page, __CreateView(0));
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

/// The document owns the image registry beside its private painter. This
/// golden fails visibly if `ElementTree::images_mut` and `Document::render`
/// stop referring to the same store: the checker disappears and only the
/// white fallback background remains.
#[test]
fn document_image_store_reaches_the_private_painter() {
    let mut gpu = headless("document_image_store_reaches_the_private_painter");

    let mut runtime = MainThreadRuntime::new(ElementTree::new(VIEWPORT, PageConfig::default()))
        .expect("QuickJS realm");
    {
        let mut elements = runtime.elements_mut();
        elements.add_author_stylesheet(IMAGE_STYLE);
        elements.images_mut().insert_url(IMAGE_URL, checker_image());
    }
    runtime
        .run_main_thread_script(IMAGE_SCRIPT)
        .expect("main-thread script");

    let image = {
        let mut elements = runtime.elements_mut();
        capture_elements(&mut gpu, &mut elements)
    };
    screenshots().assert_matches(&["retained-image-store"], &image);
}
