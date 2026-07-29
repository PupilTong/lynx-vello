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
//! `FLASHBULB_UPDATE_SNAPSHOTS=1 cargo test -p bobcat-quickjs --test screenshots`.

use bobcat_quickjs::MainThreadRuntime;
use flashbulb::vello::peniko::Color;
use flashbulb::{ImageStore, Screenshots, capture_frame, headless_or_skip};
use lynx_element::{PageConfig, Viewport};

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

#[test]
fn a_main_thread_script_renders_its_element_tree() {
    let Some(mut gpu) = headless_or_skip("a_main_thread_script_renders_its_element_tree") else {
        return;
    };

    let mut runtime =
        MainThreadRuntime::new(VIEWPORT, PageConfig::default()).expect("QuickJS realm");
    runtime.elements_mut().add_author_stylesheet(STYLE);
    runtime
        .run_main_thread_script(MAIN_THREAD_SCRIPT)
        .expect("main-thread script");

    let image = {
        let mut elements = runtime.elements_mut();
        let frame = elements.paint_order();
        capture_frame(
            &mut gpu,
            elements.document(),
            &frame,
            Color::WHITE,
            &ImageStore::new(),
        )
        .expect("capture")
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
    let Some(mut gpu) = headless_or_skip(test) else {
        return;
    };
    let mut runtime = MainThreadRuntime::new(VIEWPORT, config).expect("QuickJS realm");
    runtime.elements_mut().add_author_stylesheet(OVERFLOW_STYLE);
    runtime
        .run_main_thread_script(OVERFLOW_SCRIPT)
        .expect("main-thread script");

    let image = {
        let mut elements = runtime.elements_mut();
        let frame = elements.paint_order();
        capture_frame(
            &mut gpu,
            elements.document(),
            &frame,
            Color::WHITE,
            &ImageStore::new(),
        )
        .expect("capture")
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
