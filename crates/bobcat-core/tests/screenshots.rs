//! Public-facade coverage for the offscreen render path.
//!
//! CSS, document, image-store, and tree mutation are deliberately absent from
//! this integration-test boundary. Element construction happens only inside
//! the fetched Element-PAPI script.

mod support;

use std::sync::Arc;

use bobcat_core::resource::ResourceFetcher;
use bobcat_core::{
    OffscreenLynxView, PageConfig, PreparsedDeclaration, PreparsedRule, PreparsedStyleSheet,
};
use flashbulb::{Image, Screenshots};
use support::{FetcherDouble, wait_for_script};

const SCRIPT_URL: &str = "app:///main.js";
const MAIN_THREAD_SCRIPT: &str = r"
globalThis.renderPage = function renderPage() {
  const page = __CreatePage('card', 0);
  __AppendElement(page, __CreateView(0));
};
";

/// Builds a `.card` with a `.badge` child; every visual is author CSS.
const STYLED_SCRIPT: &str = r"
globalThis.renderPage = function renderPage() {
  const page = __CreatePage('card', 0);
  const card = __CreateView(0);
  __SetClasses(card, 'card');
  const badge = __CreateView(0);
  __SetClasses(badge, 'badge');
  __AppendElement(card, badge);
  __AppendElement(page, card);
};
";

/// The `raw-text` carrier written by `__CreateRawText`, painted: a plain run,
/// an inline-styled one, one reached through a wrapper, one whose value
/// carries a literal newline, and one long enough to wrap.
const TEXT_SCRIPT: &str = r"
globalThis.renderPage = function renderPage() {
  const page = __CreatePage('card', 0);
  __SetInlineStyles(
    page,
    'background-color:#ffffff;padding:24px;font-family:Roboto;color:#111827',
  );
  function line(styles, value, throughWrapper) {
    const text = __CreateText(0);
    __SetInlineStyles(text, styles);
    if (throughWrapper) {
      const wrapper = __CreateWrapperElement(0);
      __AppendElement(wrapper, __CreateRawText(value));
      __AppendElement(text, wrapper);
    } else {
      __AppendElement(text, __CreateRawText(value));
    }
    __AppendElement(page, text);
  }
  line('font-size:22px', 'Sphinx of black quartz');
  line('font-size:30px;color:#dc2626', 'Judge my vow');
  line('font-size:18px', 'reached through a wrapper', true);
  line('font-size:18px', 'first line\nsecond line');
  line(
    'font-size:16px;color:#374151',
    'A run long enough to need more than one line wraps inside the text '
      + 'element that carries it, at the width layout gives that element.',
  );
};
";

const IMAGE_URL: &str = "https://example.test/retained-checker.png";
const IMAGE_SCRIPT: &str = r#"
globalThis.renderPage = function renderPage() {
  const page = __CreatePage('card', 0);
  __SetInlineStyles(page, 'background-color:#e5e7eb;padding:16px');
  const image = __CreateView(0);
  __SetInlineStyles(
    image,
    'width:128px;height:96px;border:4px solid #1f2937;background-color:#ffffff;background-image:url("https://example.test/retained-checker.png");background-repeat:no-repeat;background-size:120px 88px;image-rendering:pixelated',
  );
  __AppendElement(page, image);
};
"#;

fn declaration(property: &str, value: &str) -> PreparsedDeclaration {
    PreparsedDeclaration {
        property: property.to_owned(),
        value: value.to_owned(),
        important: false,
    }
}

fn view(source: &[u8]) -> OffscreenLynxView {
    let resources: Arc<dyn ResourceFetcher> =
        Arc::new(FetcherDouble::new(source.to_vec()).resolving_to(SCRIPT_URL));
    OffscreenLynxView::new(
        PageConfig::default(),
        resources,
        Arc::new(|| {}),
        393.0,
        727.0,
        1.0,
    )
    .expect("view")
}

fn screenshots() -> Screenshots {
    flashbulb::screenshots_in(env!("CARGO_MANIFEST_DIR"))
}

/// A store carrying the one checker the image page draws.
fn checker_store() -> Arc<flashbulb::TestImages> {
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
    let images = Arc::new(flashbulb::TestImages::new());
    images.insert_rgba8(IMAGE_URL, 4, 4, rgba);
    images
}

#[tokio::test]
async fn fetched_script_reaches_the_offscreen_draw_target() {
    let mut view = view(MAIN_THREAD_SCRIPT.as_bytes());
    view.attach_offscreen()
        .expect("GPU initialization for the offscreen target");
    view.execute_script(SCRIPT_URL)
        .await
        .expect("fetch and start script");
    wait_for_script(&mut view).expect("script execution");

    let shot = view.capture().expect("capture the committed page");
    assert_eq!(shot.size.width, 393);
    assert_eq!(shot.size.height, 727);
    assert_eq!(
        shot.pixels.len(),
        shot.size.width as usize * shot.size.height as usize * 4
    );
}

/// Requirement: an embedder-owned store reaches the private painter through
/// the whole public path — install the store, load the source through it, and
/// the `background-image: url(...)` layer the script wrote draws those pixels.
#[tokio::test]
async fn an_embedder_image_store_reaches_the_private_painter() {
    let mut view = view(IMAGE_SCRIPT.as_bytes());
    view.set_image_store(checker_store() as Arc<dyn bobcat_core::ImageStore>)
        .expect("available document");
    view.attach_offscreen()
        .expect("GPU initialization for the offscreen target");
    view.execute_script(SCRIPT_URL)
        .await
        .expect("fetch and start script");
    wait_for_script(&mut view).expect("script execution");
    view.load_image(IMAGE_URL).await.expect("published source");

    let shot = view.capture().expect("capture the committed image");
    let image = Image::from_rgba8(shot.size.width, shot.size.height, shot.pixels)
        .expect("captured RGBA image");
    screenshots().assert_matches(&["embedder-image-store"], &image);
}

/// Requirement: text written the only way Lynx can write it — a `raw-text`
/// element carrying its run in an attribute — reaches the painter as glyphs.
#[tokio::test]
async fn raw_text_reaches_the_private_painter_as_glyphs() {
    const ROBOTO: &[u8] = include_bytes!("../../hughie/tests/fixtures/Roboto-Regular.ttf");

    let mut view = view(TEXT_SCRIPT.as_bytes());
    assert_eq!(
        view.register_fonts(ROBOTO).expect("an idle document"),
        1,
        "the vendored Roboto fixture must register exactly one face"
    );
    view.attach_offscreen()
        .expect("GPU initialization for the offscreen target");
    view.execute_script(SCRIPT_URL)
        .await
        .expect("fetch and start script");
    wait_for_script(&mut view).expect("script execution");

    let shot = view.capture().expect("capture the committed page");
    let image = Image::from_rgba8(shot.size.width, shot.size.height, shot.pixels)
        .expect("captured RGBA image");
    screenshots().assert_matches(&["raw-text-runs"], &image);
}

/// Requirement: a class change made by script must restyle against author
/// rules that were never parsed from text. The script paints itself red, then
/// swaps the class and flushes again; only the second commit is captured.
#[tokio::test]
async fn a_scripted_class_change_restyles_against_preparsed_rules() {
    const SWAP_SCRIPT: &str = r"
globalThis.renderPage = function renderPage() {
  const page = __CreatePage('card', 0);
  const box = __CreateView(0);
  __SetClasses(box, 'before');
  __AppendElement(page, box);
  __FlushElementTree();
  __SetClasses(box, 'after');
  __FlushElementTree();
};
";
    let sized = |color: &str| {
        vec![
            declaration("width", "16px"),
            declaration("height", "12px"),
            declaration("background-color", color),
        ]
    };
    let resources: Arc<dyn ResourceFetcher> = Arc::new(
        FetcherDouble::new(SWAP_SCRIPT.as_bytes().to_vec())
            .resolving_to(SCRIPT_URL)
            .with_preparsed_style_sheet(PreparsedStyleSheet {
                rules: vec![
                    PreparsedRule::Style {
                        selectors: ".before".to_owned(),
                        declarations: sized("#ff0000"),
                    },
                    PreparsedRule::Style {
                        selectors: ".after".to_owned(),
                        declarations: sized("#00ff00"),
                    },
                ],
            }),
    );
    let mut view = OffscreenLynxView::new(
        PageConfig::default(),
        resources,
        Arc::new(|| {}),
        32.0,
        24.0,
        1.0,
    )
    .expect("view");

    view.load_style_sheet("app:///author.css")
        .await
        .expect("the pre-parsed sheet mounts");
    view.attach_offscreen()
        .expect("GPU initialization for the offscreen target");
    view.execute_script(SCRIPT_URL)
        .await
        .expect("fetch and start script");
    wait_for_script(&mut view).expect("script execution");

    let shot = view.capture().expect("capture the restyled page");
    let count = |wanted: [u8; 4]| {
        shot.pixels
            .chunks_exact(4)
            .filter(|pixel| *pixel == wanted)
            .count()
    };
    assert_eq!(
        count([0, 255, 0, 255]),
        16 * 12,
        "the class swap must restyle against the author rules"
    );
    assert_eq!(count([255, 0, 0, 255]), 0, "no pre-swap pixels survive");
}

/// The whole pre-parsed ingestion path, end to end: a host-decoded sheet is
/// loaded through the resource provider, a script builds a classed element,
/// and the committed frame is compared against a golden.
#[tokio::test]
async fn a_preparsed_author_sheet_paints() {
    let resources: Arc<dyn ResourceFetcher> = Arc::new(
        FetcherDouble::new(STYLED_SCRIPT.as_bytes().to_vec())
            .resolving_to(SCRIPT_URL)
            .with_preparsed_style_sheet(PreparsedStyleSheet {
                rules: vec![
                    PreparsedRule::Style {
                        selectors: ".card".to_owned(),
                        declarations: vec![
                            declaration("width", "200px"),
                            declaration("height", "120px"),
                            declaration("background-color", "rebeccapurple"),
                            declaration("margin", "40px"),
                        ],
                    },
                    PreparsedRule::Style {
                        selectors: ".card > .badge".to_owned(),
                        declarations: vec![
                            declaration("width", "60px"),
                            declaration("height", "60px"),
                            declaration("background-color", "gold"),
                        ],
                    },
                ],
            }),
    );
    let mut view = OffscreenLynxView::new(
        PageConfig::default(),
        resources,
        Arc::new(|| {}),
        393.0,
        727.0,
        1.0,
    )
    .expect("view");

    view.load_style_sheet("app:///author.css")
        .await
        .expect("the pre-parsed sheet mounts");
    view.attach_offscreen()
        .expect("GPU initialization for the offscreen target");
    view.execute_script(SCRIPT_URL)
        .await
        .expect("fetch and start script");
    wait_for_script(&mut view).expect("script execution");

    let shot = view.capture().expect("capture the styled page");
    let image = Image::from_rgba8(shot.size.width, shot.size.height, shot.pixels)
        .expect("captured RGBA image");
    screenshots().assert_matches(&["preparsed-author-sheet"], &image);
}
