//! Public-facade coverage for the offscreen render path.
//!
//! CSS, document, image-store, and tree mutation are deliberately absent from
//! this integration-test boundary. Element construction happens only inside
//! the fetched Element-PAPI script.

mod support;

use std::rc::Rc;
use std::sync::Arc;

use bobcat_core::{
    DrawTarget, FontBlob, LynxView, NoWakeup, PreparsedDeclaration, PreparsedRule,
    PreparsedStyleSheet, ViewSources,
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

/// A booted, offscreen-attached view over `source`, waited out so the frame
/// it captured is the one its entry module committed.
fn fetcher(source: &[u8]) -> impl FnOnce(bobcat_core::ImageReports) -> Rc<FetcherDouble> {
    let source = source.to_vec();
    move |_sink| Rc::new(FetcherDouble::new(source).resolving_to(SCRIPT_URL))
}

async fn booted(
    resources: impl FnOnce(bobcat_core::ImageReports) -> Rc<FetcherDouble>,
    sources: ViewSources,
) -> LynxView<Rc<FetcherDouble>> {
    let mut view = LynxView::new(
        Arc::new(NoWakeup),
        393.0,
        727.0,
        1.0,
        DrawTarget::Offscreen,
        resources,
        sources,
    )
    .await
    .expect("view");
    wait_for_script(&mut view).expect("script execution");
    view
}

/// A view whose stylesheet request the double answers pre-parsed.
async fn booted_with_sheet(
    source: &[u8],
    sheet: PreparsedStyleSheet,
) -> LynxView<Rc<FetcherDouble>> {
    booted_with_sheet_at(source, sheet, 393.0, 727.0).await
}

async fn booted_with_sheet_at(
    source: &[u8],
    sheet: PreparsedStyleSheet,
    width: f32,
    height: f32,
) -> LynxView<Rc<FetcherDouble>> {
    let mut view = LynxView::new(
        Arc::new(NoWakeup),
        width,
        height,
        1.0,
        DrawTarget::Offscreen,
        |_sink| {
            Rc::new(
                FetcherDouble::new(source.to_vec())
                    .resolving_to(SCRIPT_URL)
                    .with_preparsed_style_sheet(sheet),
            )
        },
        ViewSources {
            style_sheets: vec!["app:///author.css".to_owned()],
            ..ViewSources::new(SCRIPT_URL)
        },
    )
    .await
    .expect("view");
    wait_for_script(&mut view).expect("script execution");
    view
}

/// Drives the view until the painter has resolved a frame that draws an
/// image.
///
/// The store's retain log is the precise signal: it is written by the
/// painter's resolve pass, so a non-empty working set means a committed frame
/// actually named an image and the painter read its pixels. Each round is a
/// forced tick, which is the one call that waits for the commit behind it.
fn settle_images(view: &mut LynxView<Rc<FetcherDouble>>, images: &flashbulb::TestImages) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        let _ = view.tick(true);
        if images.retained().iter().any(|set| !set.is_empty()) {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "no committed frame ever drew an image"
        );
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
}

fn screenshots() -> Screenshots {
    flashbulb::screenshots_in(env!("CARGO_MANIFEST_DIR"))
}

/// A store carrying the one checker the image page draws.
fn checker_store() -> Rc<flashbulb::TestImages> {
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
    let images = Rc::new(flashbulb::TestImages::new());
    images.insert_rgba8(IMAGE_URL, 4, 4, rgba);
    images
}

#[tokio::test]
async fn fetched_script_reaches_the_offscreen_draw_target() {
    let mut view = booted(
        fetcher(MAIN_THREAD_SCRIPT.as_bytes()),
        ViewSources::new(SCRIPT_URL),
    )
    .await;

    let shot = view.capture().expect("capture the committed page");
    assert_eq!(shot.size.width, 393);
    assert_eq!(shot.size.height, 727);
    assert_eq!(
        shot.pixels.len(),
        shot.size.width as usize * shot.size.height as usize * 4
    );
}

/// Requirement: an embedder-owned store reaches the painter through the whole
/// public path, and does so **without the host asking**.
///
/// Nothing here loads the image. The paint walk meets the script's
/// `background-image: url(...)`, reports the source, the painter names it
/// against the store and reports the completed load back, the document
/// records it and republishes, and only then does a frame carry the image for
/// the painter to resolve. That whole round trip is what this asserts, and
/// nothing covered it before: the old shape needed an explicit
/// `view.load_image(...)` from the embedder.
#[tokio::test]
async fn an_embedder_image_store_reaches_the_private_painter() {
    let images = checker_store();
    let mut view = booted(
        |sink| {
            Rc::new(
                FetcherDouble::new(IMAGE_SCRIPT.as_bytes().to_vec())
                    .resolving_to(SCRIPT_URL)
                    .with_images(Rc::clone(&images))
                    .serving(sink),
            )
        },
        ViewSources::new(SCRIPT_URL),
    )
    .await;
    settle_images(&mut view, &images);

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

    // Selecting the face by name is what proves the container registered: an
    // unknown default family fails the construction.
    let mut view = booted(
        fetcher(TEXT_SCRIPT.as_bytes()),
        ViewSources {
            fonts: vec![FontBlob::from_static(ROBOTO)],
            default_font_family: Some("Roboto".to_owned()),
            ..ViewSources::new(SCRIPT_URL)
        },
    )
    .await;

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
    let mut view = booted_with_sheet_at(
        SWAP_SCRIPT.as_bytes(),
        PreparsedStyleSheet {
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
        },
        32.0,
        24.0,
    )
    .await;

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
    let mut view = booted_with_sheet(
        STYLED_SCRIPT.as_bytes(),
        PreparsedStyleSheet {
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
        },
    )
    .await;

    let shot = view.capture().expect("capture the styled page");
    let image = Image::from_rgba8(shot.size.width, shot.size.height, shot.pixels)
        .expect("captured RGBA image");
    screenshots().assert_matches(&["preparsed-author-sheet"], &image);
}
