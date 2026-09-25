//! Public-facade coverage for the offscreen render path.
//!
//! CSS, document, image-store, and tree mutation are deliberately absent from
//! this integration-test boundary. Element construction happens only inside
//! the fetched Element-PAPI script.

mod support;

use std::rc::Rc;
use std::sync::Arc;

use bobcat_core::{
    DrawTarget, FontBlob, LynxView, NoWakeup, Painter, PreparsedDeclaration, PreparsedRule,
    PreparsedStyleSheet, ViewSources,
};
use flashbulb::{Image, Screenshots};
use support::{FetcherDouble, solo_view, wait_for_script};

/// The screen these tests' views report, as a host with no screen to measure
/// names it. None of them reads `SystemInfo`.
const SCREEN: bobcat_core::ScreenMetrics =
    bobcat_core::ScreenMetrics::for_viewport(32.0, 24.0, 1.0);

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

/// The Lynx `<image>` element written the only way script can write one:
/// `__CreateImage` mints the tag, `__SetAttribute` names the source. The three
/// boxes are the three sizing cases the tag has — an authored box, an image
/// asked to fill a container's cross axis, and one with no definite axis at
/// all, which Lynx renders as nothing.
const IMAGE_ELEMENT_SCRIPT: &str = r"
globalThis.renderPage = function renderPage() {
  const page = __CreatePage('card', 0);
  __SetInlineStyles(page, 'background-color:#e5e7eb;padding:16px');
  function picture(styles) {
    const image = __CreateImage(0);
    __SetInlineStyles(image, styles);
    __SetAttribute(image, 'src', 'https://example.test/retained-checker.png');
    __AppendElement(page, image);
    return image;
  }
  picture('width:128px;height:96px;image-rendering:pixelated');
  picture('height:48px;margin-top:16px;image-rendering:pixelated');
  picture('margin-top:16px;image-rendering:pixelated');
};
";

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
) -> (LynxView<Rc<FetcherDouble>>, Painter) {
    let (mut view, painter) = solo_view(
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
    (view, painter)
}

/// A view whose stylesheet request the double answers pre-parsed.
async fn booted_with_sheet(
    source: &[u8],
    sheet: PreparsedStyleSheet,
) -> (LynxView<Rc<FetcherDouble>>, Painter) {
    booted_with_sheet_at(source, sheet, 393.0, 727.0).await
}

async fn booted_with_sheet_at(
    source: &[u8],
    sheet: PreparsedStyleSheet,
    width: f32,
    height: f32,
) -> (LynxView<Rc<FetcherDouble>>, Painter) {
    booted_with_sheet_sources(
        source,
        sheet,
        width,
        height,
        ViewSources {
            style_sheets: vec!["app:///author.css".to_owned()],
            ..ViewSources::new("app:///", SCRIPT_URL, SCREEN)
        },
    )
    .await
}

/// [`booted_with_sheet_at`] with the view's own `ViewSources` — the shape a
/// fixture needs when the sheet it mounts styles *text*, since the face has
/// to be registered at construction.
async fn booted_with_sheet_sources(
    source: &[u8],
    sheet: PreparsedStyleSheet,
    width: f32,
    height: f32,
    sources: ViewSources,
) -> (LynxView<Rc<FetcherDouble>>, Painter) {
    let (mut view, painter) = solo_view(
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
        sources,
    )
    .await
    .expect("view");
    wait_for_script(&mut view).expect("script execution");
    (view, painter)
}

/// Drives both halves until `ready` answers true, or fails naming what it was
/// waiting for.
///
/// The view's turn comes first in every round. It is the one call that
/// services the resource protocol at all, and the image reports it sends ride
/// the same command FIFO the tick's `BeginFrame` then follows — so the
/// acknowledgement the tick waits for implies a commit that saw them.
fn settle(
    view: &mut LynxView<Rc<FetcherDouble>>,
    painter: &mut Painter,
    waiting_for: &str,
    ready: impl Fn() -> bool,
) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        let _ = view.pump();
        let _ = painter.tick(true);
        if ready() {
            return;
        }
        assert!(std::time::Instant::now() < deadline, "{waiting_for}");
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
}

/// Drives both halves until the painter has resolved a frame that draws an
/// image.
///
/// The store's retain log is the precise signal: it is written by the
/// painter's resolve pass, so a non-empty working set means a committed frame
/// actually named an image and the painter read its pixels.
fn settle_images(
    view: &mut LynxView<Rc<FetcherDouble>>,
    painter: &mut Painter,
    images: &flashbulb::TestImages,
) {
    settle(
        view,
        painter,
        "no committed frame ever drew an image",
        || images.retained().iter().any(|set| !set.is_empty()),
    );
}

/// Drives both halves until the newest composed frame draws exactly
/// `expected`, in any order.
///
/// Stronger than [`settle_images`], and the wait every `<image>` case below
/// needs: which bitmaps a frame drew is the whole subject of those pictures,
/// and a source held pending or failed by the store is one the retain log
/// must never name. It is also what makes the capture land after the page
/// epilogue that dispatched a `load` — the mutation a listener makes rides
/// the same commit as the frame that first draws the bitmap it announced.
fn settle_drawing(
    view: &mut LynxView<Rc<FetcherDouble>>,
    painter: &mut Painter,
    images: &flashbulb::TestImages,
    expected: &[&str],
) {
    settle(
        view,
        painter,
        &format!("no committed frame ever drew exactly {expected:?}"),
        || {
            images.retained().last().is_some_and(|frame| {
                frame.len() == expected.len()
                    && expected
                        .iter()
                        .all(|source| frame.iter().any(|drawn| &**drawn == *source))
            })
        },
    );
}

fn screenshots() -> Screenshots {
    flashbulb::screenshots_in(env!("CARGO_MANIFEST_DIR"))
}

/// The painter's newest frame, ready to compare with a golden.
fn captured(painter: &mut Painter) -> Image {
    let shot = painter.capture().expect("capture the committed page");
    Image::from_rgba8(shot.size.width, shot.size.height, shot.pixels).expect("captured RGBA image")
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
    let (_view, mut painter) = booted(
        fetcher(MAIN_THREAD_SCRIPT.as_bytes()),
        ViewSources::new("app:///", SCRIPT_URL, SCREEN),
    )
    .await;

    let shot = painter.capture().expect("capture the committed page");
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
    let (mut view, mut painter) = booted(
        |sink| {
            Rc::new(
                FetcherDouble::new(IMAGE_SCRIPT.as_bytes().to_vec())
                    .resolving_to(SCRIPT_URL)
                    .with_images(Rc::clone(&images))
                    .serving(sink),
            )
        },
        ViewSources::new("app:///", SCRIPT_URL, SCREEN),
    )
    .await;
    settle_images(&mut view, &mut painter, &images);

    screenshots().assert_matches(&["embedder-image-store"], &captured(&mut painter));
}

/// Requirement: the Lynx `<image>` element loads and paints from its `src`
/// alone, with the embedder asked for nothing.
///
/// The whole path is under test: `__SetAttribute` raises the tag's component,
/// which makes the element replaced content and binds its source; the painter
/// names that source against the store and reports the load back; the document
/// records it and republishes; the next frame draws the bitmap.
///
/// The golden also pins the sizing rule the tag exists for. Lynx gives
/// `<image>` no measurement of its own, so only a definite axis produces one:
/// the first box is 128x96 because it says so, the second is 48 tall and
/// stretches across the page's cross axis, and the third — which authors
/// neither axis — is zero-height and draws nothing at all. An `<img>` would
/// have sized all three from the 4x4 checker.
#[tokio::test]
async fn an_image_element_loads_and_paints_from_its_src() {
    let images = checker_store();
    let (mut view, mut painter) = booted(
        |sink| {
            Rc::new(
                FetcherDouble::new(IMAGE_ELEMENT_SCRIPT.as_bytes().to_vec())
                    .resolving_to(SCRIPT_URL)
                    .with_images(Rc::clone(&images))
                    .serving(sink),
            )
        },
        ViewSources::new("app:///", SCRIPT_URL, SCREEN),
    )
    .await;
    settle_images(&mut view, &mut painter, &images);

    screenshots().assert_matches(&["image-element"], &captured(&mut painter));
}

/// Requirement: text written the only way Lynx can write it — a `raw-text`
/// element carrying its run in an attribute — reaches the painter as glyphs.
#[tokio::test]
async fn raw_text_reaches_the_private_painter_as_glyphs() {
    const ROBOTO: &[u8] = include_bytes!("../../hughie/tests/fixtures/Roboto-Regular.ttf");

    // Selecting the face by name is what proves the container registered: an
    // unknown default family fails the construction.
    let (_view, mut painter) = booted(
        fetcher(TEXT_SCRIPT.as_bytes()),
        ViewSources {
            fonts: vec![FontBlob::from_static(ROBOTO)],
            default_font_family: Some("Roboto".to_owned()),
            ..ViewSources::new("app:///", SCRIPT_URL, SCREEN)
        },
    )
    .await;

    screenshots().assert_matches(&["raw-text-runs"], &captured(&mut painter));
}

/// Both layout and glyph painting must lose the second line. At DPR 2 the
/// same 21px line height makes the two boxes 84 and 42 device pixels tall.
#[tokio::test]
async fn text_maxline_halves_the_rendered_box_at_device_pixel_ratio_two() {
    const AHEM: &[u8] = include_bytes!("../../hughie/tests/fixtures/Ahem.ttf");
    const SOURCE: &str = r"
globalThis.renderPage = function () {
  const page = __CreatePage('card', 0);
  __SetInlineStyles(page, 'display:flex;flex-direction:row;align-items:flex-start;gap:10px;background-color:white');
  for (const limited of [false, true]) {
    const text = __CreateText(0);
    __SetInlineStyles(text, 'width:100px;flex-shrink:0;font-family:Ahem;font-size:20px;line-height:21px;background-color:#0080ff');
    __AppendElement(text, __CreateRawText('abc\ndef'));
    if (limited) __SetAttribute(text, 'text-maxline', 1);
    __AppendElement(page, text);
  }
};
";
    let (mut view, mut painter) = solo_view(
        Arc::new(NoWakeup),
        220.0,
        60.0,
        2.0,
        DrawTarget::Offscreen,
        fetcher(SOURCE.as_bytes()),
        ViewSources {
            fonts: vec![FontBlob::from_static(AHEM)],
            default_font_family: Some("Ahem".to_owned()),
            ..ViewSources::new("app:///", SCRIPT_URL, SCREEN)
        },
    )
    .await
    .expect("view");
    wait_for_script(&mut view).expect("script execution");
    let shot = painter.capture().expect("capture both text boxes");
    assert_eq!((shot.size.width, shot.size.height), (440, 120));
    let pixel = |x: usize, y: usize| {
        let offset = (y * shot.size.width as usize + x) * 4;
        &shot.pixels[offset..offset + 4]
    };
    for (x, height) in [(180, 84), (400, 42)] {
        let blue_rows = (0..shot.size.height as usize)
            .filter(|&y| pixel(x, y) == [0, 128, 255, 255])
            .count();
        assert_eq!(blue_rows, height, "box height in device pixels at x={x}");
    }
    assert_eq!(pixel(20, 20), [0, 0, 0, 255], "unrestricted first line");
    assert_eq!(pixel(240, 20), [0, 0, 0, 255], "restricted first line");
    assert_eq!(pixel(20, 62), [0, 0, 0, 255], "unrestricted second line");
    assert_eq!(
        pixel(240, 62),
        [255, 255, 255, 255],
        "restricted second line is gone"
    );

    let image = Image::from_rgba8(shot.size.width, shot.size.height, shot.pixels)
        .expect("captured RGBA image");
    screenshots().assert_matches(&["text-maxline"], &image);
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
    let (_view, mut painter) = booted_with_sheet_at(
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

    let shot = painter.capture().expect("capture the restyled page");
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
    let (_view, mut painter) = booted_with_sheet(
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

    screenshots().assert_matches(&["preparsed-author-sheet"], &captured(&mut painter));
}

// The Lynx `<image>` surface in pixels: `mode`, `placeholder`, `auto-size`
// and the `load`/`error` events, each written the only way a card can write
// them — `__CreateImage`, `__SetAttribute`, `__AddEvent` — and read back off
// the committed frame. `crates/bobcat-core/src/main/tree/image.rs` carries the
// model these pictures are of; `blur-radius` is deliberately absent: it
// paints as a whole-element `filter: blur()` today, ruled an interim state
// until a bitmap-only blur lands, and a golden would pin the interim.

/// The sources the `<image>` pages below name, in the three states a host can
/// leave one in: settled with pixels, settled as a failure, and never
/// answered at all.
const LANDSCAPE_SOURCE: &str = "https://example.test/landscape.png";
const PORTRAIT_SOURCE: &str = "https://example.test/portrait.png";
const HOLDING_SOURCE: &str = "https://example.test/holding.png";
const PUNCHED_SOURCE: &str = "https://example.test/punched.png";
const PENDING_SOURCE: &str = "https://example.test/never-answered.png";
const BROKEN_SOURCE: &str = "https://example.test/broken.png";

/// The frame every quadrant bitmap carries, in source pixels. Cropping eats
/// it on the axis a fit overflows, which is what makes `aspectFill` and
/// `center` readable against `aspectFit`.
const BITMAP_FRAME: u32 = 2;

/// A bitmap whose four quadrants and framed edge make its orientation, its
/// letterboxing and its cropping readable at a glance: `quadrants` is
/// top-left, top-right, bottom-left, bottom-right.
fn quadrant_rgba8(
    width: u32,
    height: u32,
    frame: [u8; 4],
    quadrants: [[u8; 4]; 4],
) -> (u32, u32, Vec<u8>) {
    let mut rgba = Vec::with_capacity((width * height * 4) as usize);
    for y in 0..height {
        for x in 0..width {
            let framed = x < BITMAP_FRAME
                || y < BITMAP_FRAME
                || x + BITMAP_FRAME >= width
                || y + BITMAP_FRAME >= height;
            let quadrant = usize::from(y >= height / 2) * 2 + usize::from(x >= width / 2);
            rgba.extend_from_slice(if framed { &frame } else { &quadrants[quadrant] });
        }
    }
    (width, height, rgba)
}

/// The picture a `src` names: a 2:1 landscape in the palette the checker of
/// `an_image_element_loads_and_paints_from_its_src` already uses.
fn landscape_bitmap() -> (u32, u32, Vec<u8>) {
    quadrant_rgba8(
        40,
        20,
        [17, 24, 39, 255],
        [
            [239, 68, 68, 255],
            [34, 197, 94, 255],
            [37, 99, 235, 255],
            [250, 204, 21, 255],
        ],
    )
}

/// The same picture turned 1:2, so a box sized from it is unmistakably not
/// the one the landscape sized.
fn portrait_bitmap() -> (u32, u32, Vec<u8>) {
    quadrant_rgba8(
        20,
        40,
        [17, 24, 39, 255],
        [
            [239, 68, 68, 255],
            [34, 197, 94, 255],
            [37, 99, 235, 255],
            [250, 204, 21, 255],
        ],
    )
}

/// What a `placeholder` names: the same shape in a palette no `src` uses, so
/// which of an element's two sources is on screen needs no measuring.
fn holding_bitmap() -> (u32, u32, Vec<u8>) {
    quadrant_rgba8(
        40,
        20,
        [248, 250, 252, 255],
        [
            [124, 58, 237, 255],
            [249, 115, 22, 255],
            [20, 184, 166, 255],
            [236, 72, 153, 255],
        ],
    )
}

/// A `src` whose right half is fully transparent.
///
/// It is the one bitmap that can say a loaded source suppressed its
/// placeholder *for good* rather than merely covering it: anything drawn
/// underneath would show through the hole, and what has to show there is the
/// element's own background.
fn punched_bitmap() -> (u32, u32, Vec<u8>) {
    let (width, height) = (40, 20);
    let mut rgba = Vec::with_capacity((width * height * 4) as usize);
    for y in 0..height {
        for x in 0..width {
            let pixel = match (x < width / 2, y < height / 2) {
                (false, _) => [0, 0, 0, 0],
                (true, true) => [239, 68, 68, 255],
                (true, false) => [37, 99, 235, 255],
            };
            rgba.extend_from_slice(&pixel);
        }
    }
    (width, height, rgba)
}

/// The store the `<image>` pages draw from, warmed before the view exists.
///
/// Three sources have pixels, one has failed, and the two the pages name and
/// this never publishes — `PENDING_SOURCE` and, until a test publishes it,
/// `PORTRAIT_SOURCE` — stay pending for the whole run. Nothing here is
/// timing: which state a source is in is the test's to choose.
fn picture_store() -> Rc<flashbulb::TestImages> {
    let images = Rc::new(flashbulb::TestImages::new());
    for (source, bitmap) in [
        (LANDSCAPE_SOURCE, landscape_bitmap()),
        (HOLDING_SOURCE, holding_bitmap()),
        (PUNCHED_SOURCE, punched_bitmap()),
    ] {
        let (width, height, rgba) = bitmap;
        images.insert_rgba8(source, width, height, rgba);
    }
    images.fail(BROKEN_SOURCE);
    images
}

/// One `<image>` page, with the source names it writes bound as JavaScript
/// constants — so a URL a page draws and a URL a test publishes cannot drift
/// apart.
fn image_page(body: &str) -> String {
    format!(
        "const LANDSCAPE = {LANDSCAPE_SOURCE:?};\n\
         const PORTRAIT = {PORTRAIT_SOURCE:?};\n\
         const HOLDING = {HOLDING_SOURCE:?};\n\
         const PUNCHED = {PUNCHED_SOURCE:?};\n\
         const PENDING = {PENDING_SOURCE:?};\n\
         const BROKEN = {BROKEN_SOURCE:?};\n\
         {body}"
    )
}

/// A booted, offscreen-attached view of exactly `width` x `height` CSS pixels
/// at a device pixel ratio of 1, serving `images`.
async fn booted_with_images(
    source: String,
    images: &Rc<flashbulb::TestImages>,
    width: f32,
    height: f32,
) -> (LynxView<Rc<FetcherDouble>>, Painter) {
    let images = Rc::clone(images);
    let (mut view, painter) = solo_view(
        Arc::new(NoWakeup),
        width,
        height,
        1.0,
        DrawTarget::Offscreen,
        move |sink| {
            Rc::new(
                FetcherDouble::new(source.into_bytes())
                    .resolving_to(SCRIPT_URL)
                    .with_images(images)
                    .serving(sink),
            )
        },
        ViewSources::new("app:///", SCRIPT_URL, SCREEN),
    )
    .await
    .expect("view");
    wait_for_script(&mut view).expect("script execution");
    (view, painter)
}

/// Requirement: `mode` decides how the bitmap meets the box, in pixels.
///
/// Eight boxes over the same 2:1 bitmap, each on a slate background so what
/// the bitmap does not cover is visible — six with an 80x60 content box, and
/// two with a 60x40 one inside a frame. Reading the picture left to right,
/// top to bottom:
///
/// 1. no `mode` at all, `scaleToFill`, and `aspectFit` — the first two are the initial `fill` and
///    stretch the bitmap over the whole box; the third contains it, 80x40 centred between two 10px
///    slate bands.
/// 2. `aspectFill`, `center`, and the unrecognised `aspectfit` — cover crops the bitmap's left and
///    right edges away at 120x60, `center` draws it at one source pixel per CSS pixel in the middle
///    of the box, and an unknown value is the initial `fill` again, exactly like box 1.
/// 3. the two overflowing modes again inside a 10px padding, a 5px border and a 30px radius, which
///    pins that the bitmap is drawn into the *content* box and clipped to the rounded content shape
///    rather than to the border box.
#[tokio::test]
async fn the_image_mode_attribute_fits_the_bitmap_to_the_content_box() {
    const BODY: &str = r"
globalThis.renderPage = function renderPage() {
  const page = __CreatePage('card', 0);
  __SetInlineStyles(page, 'background-color:#e5e7eb;padding:10px');
  function row(styles) {
    const view = __CreateView(0);
    __SetInlineStyles(view, 'display:flex;flex-direction:row;' + styles);
    __AppendElement(page, view);
    return view;
  }
  function picture(parent, mode, styles) {
    const image = __CreateImage(0);
    __SetInlineStyles(
      image,
      'background-color:#334155;image-rendering:pixelated;margin-right:10px;' + styles,
    );
    if (mode) __SetAttribute(image, 'mode', mode);
    __SetAttribute(image, 'src', LANDSCAPE);
    __AppendElement(parent, image);
  }
  const plain = 'width:80px;height:60px';
  const framed =
    'width:90px;height:70px;padding:10px;border:5px solid #dc2626;border-radius:30px';
  const first = row('');
  picture(first, '', plain);
  picture(first, 'scaleToFill', plain);
  picture(first, 'aspectFit', plain);
  const second = row('margin-top:10px');
  picture(second, 'aspectFill', plain);
  picture(second, 'center', plain);
  picture(second, 'aspectfit', plain);
  const third = row('margin-top:10px');
  picture(third, 'aspectFill', framed);
  picture(third, 'center', framed);
};
";
    let images = picture_store();
    let (mut view, mut painter) = booted_with_images(image_page(BODY), &images, 290.0, 230.0).await;
    settle_drawing(&mut view, &mut painter, &images, &[LANDSCAPE_SOURCE]);

    screenshots().assert_matches(&["image-mode"], &captured(&mut painter));
}

/// Requirement: the two sources are concurrent, and which of them an element
/// draws follows `dom`'s rule rather than a fallback chain.
///
/// Four 80x60 boxes, all with the same ready placeholder:
///
/// 1. no `src` at all — the placeholder is what there is.
/// 2. a `src` the host never answers — the placeholder holds the box.
/// 3. a ready `src` whose right half is transparent — the source wins outright, and the hole in it
///    shows the element's own slate background rather than the placeholder underneath.
/// 4. a `src` that failed — a failure is terminal and reaches for nothing, so the placeholder that
///    was already loaded stays.
#[tokio::test]
async fn an_image_draws_its_placeholder_until_its_own_source_has_pixels() {
    const BODY: &str = r"
globalThis.renderPage = function renderPage() {
  const page = __CreatePage('card', 0);
  __SetInlineStyles(
    page,
    'background-color:#e5e7eb;padding:10px;display:flex;flex-direction:row',
  );
  function picture(src, placeholder) {
    const image = __CreateImage(0);
    __SetInlineStyles(
      image,
      'width:80px;height:60px;margin-right:10px;background-color:#334155;'
        + 'image-rendering:pixelated',
    );
    if (placeholder) __SetAttribute(image, 'placeholder', placeholder);
    if (src) __SetAttribute(image, 'src', src);
    __AppendElement(page, image);
  }
  picture('', HOLDING);
  picture(PENDING, HOLDING);
  picture(PUNCHED, HOLDING);
  picture(BROKEN, HOLDING);
};
";
    let images = picture_store();
    let (mut view, mut painter) = booted_with_images(image_page(BODY), &images, 370.0, 80.0).await;
    settle_drawing(
        &mut view,
        &mut painter,
        &images,
        &[HOLDING_SOURCE, PUNCHED_SOURCE],
    );

    screenshots().assert_matches(&["image-placeholder"], &captured(&mut painter));
}

/// Requirement: rewriting `src` to a source that has no pixels gives the
/// placeholder the box back.
///
/// Both boxes load the landscape and then rewrite `src` to a source the host
/// never answers. The left one has a placeholder and shows it; the right one
/// has none and shows its own background. Neither shows the landscape, which
/// is the whole point: the source it drew is gone.
///
/// The rewrite rides the element's own `load`, which is the one moment at
/// which the first source is known to have pixels — and, by the epilogue's
/// order, one that still commits inside the turn that settled it.
#[tokio::test]
async fn rewriting_src_to_a_pending_source_shows_the_placeholder_again() {
    const BODY: &str = r"
globalThis.renderPage = function renderPage() {
  const page = __CreatePage('card', 0);
  __SetInlineStyles(
    page,
    'background-color:#e5e7eb;padding:10px;display:flex;flex-direction:row',
  );
  globalThis.runWorklet = (value, params) => value.body(params[0]);
  function picture(placeholder) {
    const image = __CreateImage(0);
    __SetInlineStyles(
      image,
      'width:80px;height:60px;margin-right:10px;background-color:#334155;'
        + 'image-rendering:pixelated',
    );
    if (placeholder) __SetAttribute(image, 'placeholder', placeholder);
    __SetAttribute(image, 'src', LANDSCAPE);
    __AddEvent(image, 'bindEvent', 'load', {
      type: 'worklet',
      value: { body: () => __SetAttribute(image, 'src', PENDING) },
    });
    __AppendElement(page, image);
  }
  picture(HOLDING);
  picture('');
};
";
    let images = picture_store();
    let (mut view, mut painter) = booted_with_images(image_page(BODY), &images, 190.0, 80.0).await;
    settle_drawing(&mut view, &mut painter, &images, &[HOLDING_SOURCE]);

    screenshots().assert_matches(&["image-src-swap"], &captured(&mut painter));
}

/// Requirement: `auto-size` makes the box an ordinary replaced flex item, and
/// the six parents of `auto_size_sizes_the_box_from_its_bitmap` — web-core's
/// own `basic-element-image-auto-size` fixture — size it the way both
/// references do.
///
/// Every inner view is blue and every outer one pink, so the box the image
/// ended up with is whatever the bitmap covers. Top block, a column outer:
/// an inner constraining both axes stretches the image across it (100x80), an
/// inner 40 wide derives the height through the ratio (40x20), and an inner
/// 50 tall derives the width (100x50) inside a view stretched to the outer's
/// own 200. Middle block, a row outer: the same three inners, with the middle
/// one now stretched to the outer's full height (40x120). Bottom row: the
/// literal `auto-size="false"` is off, so containment stands and the box is
/// zero on its free axis and draws nothing; an authored width under
/// `auto-size` keeps that width and takes the height from the ratio (60x30).
#[tokio::test]
async fn auto_size_sizes_an_image_box_from_its_bitmap() {
    const BODY: &str = r"
globalThis.renderPage = function renderPage() {
  const page = __CreatePage('card', 0);
  __SetInlineStyles(page, 'background-color:#e5e7eb;padding:10px');
  function block(styles) {
    const view = __CreateView(0);
    __SetInlineStyles(view, 'display:flex;background-color:#fca5a5;' + styles);
    __AppendElement(page, view);
    return view;
  }
  function inner(parent, styles) {
    const view = __CreateView(0);
    __SetInlineStyles(view, 'display:flex;background-color:#93c5fd;' + styles);
    __AppendElement(parent, view);
    return view;
  }
  function picture(parent, autoSize, styles) {
    const image = __CreateImage(0);
    __SetInlineStyles(image, 'image-rendering:pixelated;' + styles);
    __SetAttribute(image, 'auto-size', autoSize);
    __SetAttribute(image, 'src', LANDSCAPE);
    __AppendElement(parent, image);
  }
  const column = block('flex-direction:column;width:200px;height:150px');
  picture(inner(column, 'width:100px;height:80px'), '', '');
  picture(inner(column, 'width:40px'), '', '');
  picture(inner(column, 'height:50px'), '', '');
  const row = block('flex-direction:row;width:260px;height:120px;margin-top:10px');
  picture(inner(row, 'width:100px;height:80px'), '', '');
  picture(inner(row, 'width:40px'), '', '');
  picture(inner(row, 'height:50px'), '', '');
  const last = block('flex-direction:row;background-color:#e5e7eb;margin-top:10px');
  picture(inner(last, 'width:100px;height:60px;margin-right:10px'), 'false', '');
  picture(inner(last, 'width:100px;height:60px'), '', 'width:60px');
};
";
    let images = picture_store();
    let (mut view, mut painter) = booted_with_images(image_page(BODY), &images, 280.0, 370.0).await;
    settle_drawing(&mut view, &mut painter, &images, &[LANDSCAPE_SOURCE]);

    screenshots().assert_matches(&["image-auto-size"], &captured(&mut painter));
}

/// Requirement: the box `auto-size` derives is a content box, so `padding`
/// and `box-sizing` move it exactly as they move any other replaced element's.
///
/// web-core's `basic-element-image-auto-size-with-padding` fixture, at this
/// suite's scale and over this suite's bitmap: an asymmetric
/// `5px 10px 15px 30px` padding under both box-sizings, with and without
/// `auto-size`. Every image is blue, so the padding is the blue margin around
/// the bitmap and the bitmap itself is the content box; every pink holder is
/// large enough that no box shrinks to fit it.
///
/// Top row, border-box: `auto-size` reads a 40-tall content box out of a
/// 60-tall border box and derives 80 of content width from the ratio, which
/// is a 120x60 box — pixel for pixel the box beside it, where the author
/// wrote `width: 120px` instead. Bottom row, content-box: the same `height:
/// 60px` is now 60 of *content*, so the derived width is 120 and the box is
/// 160x80 — again exactly the authored box beside it. The padding is
/// asymmetric so which side the content box sits on is readable: 30 of it is
/// on the left and 10 on the right, 5 on top and 15 below.
#[tokio::test]
async fn auto_size_derives_a_content_box_under_both_box_sizings() {
    const BODY: &str = r"
globalThis.renderPage = function renderPage() {
  const page = __CreatePage('card', 0);
  __SetInlineStyles(page, 'background-color:#e5e7eb;padding:10px');
  function row() {
    const view = __CreateView(0);
    __SetInlineStyles(view, 'display:flex;flex-direction:row');
    __AppendElement(page, view);
    return view;
  }
  function picture(parent, autoSize, styles) {
    const holder = __CreateView(0);
    __SetInlineStyles(
      holder,
      'display:flex;width:170px;height:90px;margin-right:10px;background-color:#fca5a5',
    );
    __AppendElement(parent, holder);
    const image = __CreateImage(0);
    __SetInlineStyles(
      image,
      'background-color:#93c5fd;image-rendering:pixelated;'
        + 'padding:5px 10px 15px 30px;height:60px;' + styles,
    );
    if (autoSize) __SetAttribute(image, 'auto-size', '');
    __SetAttribute(image, 'src', LANDSCAPE);
    __AppendElement(holder, image);
  }
  const first = row();
  picture(first, true, '');
  picture(first, false, 'width:120px');
  const second = row();
  __SetInlineStyles(second, 'display:flex;flex-direction:row;margin-top:10px');
  picture(second, true, 'box-sizing:content-box');
  picture(second, false, 'box-sizing:content-box;width:120px');
};
";
    let images = picture_store();
    let (mut view, mut painter) = booted_with_images(image_page(BODY), &images, 380.0, 210.0).await;
    settle_drawing(&mut view, &mut painter, &images, &[LANDSCAPE_SOURCE]);

    screenshots().assert_matches(&["image-auto-size-padding"], &captured(&mut painter));
}

/// Requirement: under `auto-size` the natural size is a layout input, and the
/// bitmap it names is whichever one the element draws — so a placeholder
/// sizes the box until the source has pixels, and the box then resizes.
///
/// Two captures of one view. In the first the source is still pending, so the
/// 2:1 placeholder gives the box its 40x20. The test then publishes the 1:2
/// source, and the second capture is 20x40 of the source's own palette. That
/// a placeholder can size a box at all follows iOS and web-core; Android
/// sizes from `src` alone.
#[tokio::test]
async fn an_auto_size_box_resizes_when_its_source_replaces_its_placeholder() {
    const BODY: &str = r"
globalThis.renderPage = function renderPage() {
  const page = __CreatePage('card', 0);
  __SetInlineStyles(page, 'background-color:#e5e7eb;padding:10px');
  const holder = __CreateView(0);
  __SetInlineStyles(
    holder,
    'display:flex;flex-direction:row;align-items:flex-start;'
      + 'width:120px;height:120px;background-color:#fca5a5',
  );
  __AppendElement(page, holder);
  const image = __CreateImage(0);
  __SetInlineStyles(image, 'image-rendering:pixelated');
  __SetAttribute(image, 'auto-size', '');
  __SetAttribute(image, 'placeholder', HOLDING);
  __SetAttribute(image, 'src', PORTRAIT);
  __AppendElement(holder, image);
};
";
    let images = picture_store();
    let (mut view, mut painter) = booted_with_images(image_page(BODY), &images, 140.0, 140.0).await;
    settle_drawing(&mut view, &mut painter, &images, &[HOLDING_SOURCE]);
    screenshots().assert_matches(
        &["image-auto-size-placeholder-before"],
        &captured(&mut painter),
    );

    let (width, height, rgba) = portrait_bitmap();
    images.insert_rgba8(PORTRAIT_SOURCE, width, height, rgba);
    settle_drawing(&mut view, &mut painter, &images, &[PORTRAIT_SOURCE]);

    screenshots().assert_matches(
        &["image-auto-size-placeholder-after"],
        &captured(&mut painter),
    );
}

/// Requirement: `load` and `error` reach a card's own handlers, carry
/// web-core's details, fire for the element's own `src` alone, and do not
/// bubble — all of it read off the pixels the handlers painted.
///
/// The top row is the two images: a `src` that loads, and a `src` that fails
/// behind a placeholder that loads. The bottom row is five markers, grey
/// until a handler repaints one, left to right:
///
/// 1. the image's own `bindEvent` `load` — green, and **40x20 rather than the 20x20 it started
///    at**, because the handler sized it from `event.detail.width` and `event.detail.height`.
/// 2. a `bindEvent` `load` on the page — still grey: `load` does not bubble, so the bind pass runs
///    on the target alone.
/// 3. a `capture-bind` `load` on the images' own parent view — blue: a non-bubbling event still
///    captures down the whole path.
/// 4. the failing image's `bindEvent` `error` — red.
/// 5. a `bindEvent` `load` on that same failing image — still grey. Its placeholder loaded, and a
///    placeholder's own ending is nobody's event.
///
/// The two path registrations are on different elements on purpose: within
/// one kind, `__AddEvent` keys on the event name alone, so a `capture-bind`
/// filed over a `bindEvent` of the same name on one node would replace it and
/// the grey marker would prove nothing.
#[tokio::test]
async fn image_load_and_error_reach_their_handlers_without_bubbling() {
    const BODY: &str = r"
globalThis.renderPage = function renderPage() {
  const page = __CreatePage('card', 0);
  __SetInlineStyles(page, 'background-color:#e5e7eb;padding:10px');
  globalThis.runWorklet = (value, params) => value.body(params[0]);
  const worklet = (body) => ({ type: 'worklet', value: { body } });
  const pictures = __CreateView(0);
  __SetInlineStyles(pictures, 'display:flex;flex-direction:row');
  __AppendElement(page, pictures);
  const markers = __CreateView(0);
  __SetInlineStyles(
    markers,
    'display:flex;flex-direction:row;align-items:flex-start;margin-top:10px',
  );
  __AppendElement(page, markers);
  function marker() {
    const view = __CreateView(0);
    __SetInlineStyles(view, 'width:20px;height:20px;margin-right:10px;background-color:#9ca3af');
    __AppendElement(markers, view);
    return view;
  }
  function paint(view, width, height, color) {
    __SetInlineStyles(
      view,
      'width:' + width + 'px;height:' + height + 'px;margin-right:10px;background-color:' + color,
    );
  }
  function picture(src, placeholder) {
    const image = __CreateImage(0);
    __SetInlineStyles(
      image,
      'width:80px;height:60px;margin-right:10px;background-color:#334155;'
        + 'image-rendering:pixelated',
    );
    if (placeholder) __SetAttribute(image, 'placeholder', placeholder);
    __SetAttribute(image, 'src', src);
    __AppendElement(pictures, image);
    return image;
  }
  const loaded = marker();
  const bubbled = marker();
  const captured = marker();
  const failed = marker();
  const placeheld = marker();
  const first = picture(LANDSCAPE, '');
  const second = picture(BROKEN, HOLDING);
  __AddEvent(first, 'bindEvent', 'load', worklet((event) => {
    paint(loaded, event.detail.width, event.detail.height, '#16a34a');
  }));
  __AddEvent(page, 'bindEvent', 'load', worklet(() => paint(bubbled, 20, 20, '#16a34a')));
  __AddEvent(pictures, 'capture-bind', 'load', worklet(() => paint(captured, 20, 20, '#2563eb')));
  __AddEvent(second, 'bindEvent', 'error', worklet(() => paint(failed, 20, 20, '#dc2626')));
  __AddEvent(second, 'bindEvent', 'load', worklet(() => paint(placeheld, 20, 20, '#16a34a')));
};
";
    let images = picture_store();
    let (mut view, mut painter) = booted_with_images(image_page(BODY), &images, 190.0, 110.0).await;
    settle_drawing(
        &mut view,
        &mut painter,
        &images,
        &[LANDSCAPE_SOURCE, HOLDING_SOURCE],
    );

    screenshots().assert_matches(&["image-events"], &captured(&mut painter));
}

/// A card built the only way script can: two `.card` views, each holding a
/// `.badge` and a `<text>` run, with the second one opting out of the filter.
const BLUR_CARD_SCRIPT: &str = r"
globalThis.renderPage = function renderPage() {
  const page = __CreatePage('card', 0);
  __SetClasses(page, 'page');
  function card(classes, label) {
    const node = __CreateView(0);
    __SetClasses(node, classes);
    const badge = __CreateView(0);
    __SetClasses(badge, 'badge');
    __AppendElement(node, badge);
    const text = __CreateText(0);
    __SetClasses(text, 'label');
    __AppendElement(text, __CreateRawText(label));
    __AppendElement(node, text);
    __AppendElement(page, node);
  }
  card('card', 'Blurred card');
  card('card plain', 'Crisp card');
};
";

/// Requirement: `filter: blur()` survives the whole embedder path — an author
/// sheet the host pre-parsed, a commit on `bobcat-main`, and the painter's own
/// `compose_and_render`, which bakes the group offscreen before it composes.
///
/// This is the one golden that covers that pre-step end to end. `dom`'s own
/// blur screenshots drive the headless renderer directly and would pass
/// unchanged if `Painter` never called `prepare_filters` at all; here the
/// blurred card would simply come out crisp.
///
/// The second card is the same card with `filter: none`, so the golden shows
/// a relationship rather than a picture: background, 4 px border, 18 px radius,
/// padding, a badge and a Roboto run are all inside the blurred group, and all
/// of them have a crisp twin 40 px below to be compared against.
#[tokio::test]
async fn a_blurred_card_reaches_the_offscreen_draw_target() {
    const ROBOTO: &[u8] = include_bytes!("../../hughie/tests/fixtures/Roboto-Regular.ttf");

    let (_view, mut painter) = booted_with_sheet_sources(
        BLUR_CARD_SCRIPT.as_bytes(),
        PreparsedStyleSheet {
            rules: vec![
                PreparsedRule::Style {
                    selectors: ".page".to_owned(),
                    declarations: vec![
                        declaration("display", "flex"),
                        declaration("flex-direction", "column"),
                        declaration("padding", "36px"),
                        declaration("background-color", "#e5e7eb"),
                        declaration("font-family", "Roboto"),
                    ],
                },
                PreparsedRule::Style {
                    selectors: ".card".to_owned(),
                    declarations: vec![
                        declaration("display", "flex"),
                        declaration("flex-direction", "column"),
                        declaration("width", "260px"),
                        declaration("height", "150px"),
                        declaration("padding", "18px"),
                        declaration("box-sizing", "border-box"),
                        declaration("margin-bottom", "40px"),
                        declaration("background-color", "#ffffff"),
                        declaration("border", "4px solid #2563eb"),
                        declaration("border-radius", "18px"),
                        declaration("filter", "blur(3px)"),
                    ],
                },
                // Source order decides: `.plain` is declared after `.card`, so
                // the second card keeps every other declaration and drops only
                // the filter.
                PreparsedRule::Style {
                    selectors: ".plain".to_owned(),
                    declarations: vec![declaration("filter", "none")],
                },
                PreparsedRule::Style {
                    selectors: ".badge".to_owned(),
                    declarations: vec![
                        declaration("display", "flex"),
                        declaration("width", "84px"),
                        declaration("height", "34px"),
                        declaration("background-color", "#f59e0b"),
                        declaration("border-radius", "8px"),
                    ],
                },
                PreparsedRule::Style {
                    selectors: ".label".to_owned(),
                    declarations: vec![
                        declaration("margin-top", "16px"),
                        declaration("font-size", "22px"),
                        declaration("color", "#111827"),
                    ],
                },
            ],
        },
        393.0,
        727.0,
        ViewSources {
            style_sheets: vec!["app:///author.css".to_owned()],
            fonts: vec![FontBlob::from_static(ROBOTO)],
            default_font_family: Some("Roboto".to_owned()),
            ..ViewSources::new("app:///", SCRIPT_URL, SCREEN)
        },
    )
    .await;

    let shot = painter.capture().expect("capture the blurred page");
    let image = Image::from_rgba8(shot.size.width, shot.size.height, shot.pixels)
        .expect("captured RGBA image");
    screenshots().assert_matches(&["filter-blur-card"], &image);
}

/// One `display: flex` rule over `properties` — the backdrop golden's whole
/// sheet is boxes, and naming `display` in every one of them is noise.
fn block(selectors: &str, properties: &[(&str, &str)]) -> PreparsedRule {
    PreparsedRule::Style {
        selectors: selectors.to_owned(),
        declarations: std::iter::once(declaration("display", "flex"))
            .chain(
                properties
                    .iter()
                    .map(|(name, value)| declaration(name, value)),
            )
            .collect(),
    }
}

/// The backdrop board, built the only way script can: a crimson left half,
/// two amber bars, one Roboto run, and two `.card` views over them — the
/// first with `backdrop-filter`, the second opting out.
const BACKDROP_CARD_SCRIPT: &str = r"
globalThis.renderPage = function renderPage() {
  const page = __CreatePage('card', 0);
  __SetClasses(page, 'page');
  const board = __CreateView(0);
  __SetClasses(board, 'board');
  function part(classes) {
    const node = __CreateView(0);
    __SetClasses(node, classes);
    __AppendElement(board, node);
  }
  part('half');
  part('bar');
  part('bar lower');
  const text = __CreateText(0);
  __SetClasses(text, 'label');
  __AppendElement(text, __CreateRawText('Frosted over text'));
  __AppendElement(board, text);
  part('card');
  part('card plain');
  __AppendElement(page, board);
};
";

/// Requirement: `backdrop-filter` survives the whole embedder path — an author
/// sheet the host pre-parsed, a commit on `bobcat-main`, and the painter's own
/// `compose_and_render`, whose bake pre-step now produces backdrops beside
/// blur groups.
///
/// The two cards are the same translucent rounded box over the same crimson /
/// white seam and the same amber bar, one above the other, and the lower one
/// carries `backdrop-filter: none`. So the golden shows a relationship rather
/// than a picture: everything behind the upper card is filtered and everything
/// behind the lower one is not.
#[tokio::test]
async fn a_backdrop_filtered_card_reaches_the_offscreen_draw_target() {
    const ROBOTO: &[u8] = include_bytes!("../../hughie/tests/fixtures/Roboto-Regular.ttf");

    let (_view, mut painter) = booted_with_sheet_sources(
        BACKDROP_CARD_SCRIPT.as_bytes(),
        PreparsedStyleSheet {
            rules: vec![
                PreparsedRule::Style {
                    selectors: ".page".to_owned(),
                    declarations: vec![
                        declaration("display", "flex"),
                        declaration("padding", "20px"),
                        declaration("background-color", "#e5e7eb"),
                        declaration("font-family", "Roboto"),
                    ],
                },
                block(
                    ".board",
                    &[
                        ("position", "relative"),
                        ("width", "353px"),
                        ("height", "320px"),
                        ("background-color", "#ffffff"),
                    ],
                ),
                block(
                    ".half",
                    &[
                        ("position", "absolute"),
                        ("left", "0px"),
                        ("top", "0px"),
                        ("width", "176px"),
                        ("height", "320px"),
                        ("background-color", "#dc2626"),
                    ],
                ),
                block(
                    ".bar",
                    &[
                        ("position", "absolute"),
                        ("left", "0px"),
                        ("top", "56px"),
                        ("width", "353px"),
                        ("height", "24px"),
                        ("background-color", "#f59e0b"),
                    ],
                ),
                PreparsedRule::Style {
                    selectors: ".lower".to_owned(),
                    declarations: vec![declaration("top", "216px")],
                },
                PreparsedRule::Style {
                    selectors: ".label".to_owned(),
                    declarations: vec![
                        declaration("position", "absolute"),
                        declaration("left", "24px"),
                        declaration("top", "96px"),
                        declaration("width", "310px"),
                        declaration("font-size", "24px"),
                        declaration("color", "#111827"),
                    ],
                },
                block(
                    ".card",
                    &[
                        ("position", "absolute"),
                        ("left", "36px"),
                        ("top", "16px"),
                        ("width", "280px"),
                        ("height", "130px"),
                        ("border-radius", "18px"),
                        ("border", "2px solid rgb(255 255 255 / 70%)"),
                        ("box-sizing", "border-box"),
                        ("background-color", "rgb(255 255 255 / 35%)"),
                        ("backdrop-filter", "blur(7px)"),
                    ],
                ),
                // Source order decides: `.plain` is declared after `.card`, so
                // the second card keeps every other declaration, drops to the
                // lower band, and gives up only the filter.
                PreparsedRule::Style {
                    selectors: ".plain".to_owned(),
                    declarations: vec![
                        declaration("top", "176px"),
                        declaration("backdrop-filter", "none"),
                    ],
                },
            ],
        },
        393.0,
        360.0,
        ViewSources {
            style_sheets: vec!["app:///author.css".to_owned()],
            fonts: vec![FontBlob::from_static(ROBOTO)],
            default_font_family: Some("Roboto".to_owned()),
            ..ViewSources::new("app:///", SCRIPT_URL, SCREEN)
        },
    )
    .await;

    let shot = painter.capture().expect("capture the backdrop page");
    let image = Image::from_rgba8(shot.size.width, shot.size.height, shot.pixels)
        .expect("captured RGBA image");
    screenshots().assert_matches(&["backdrop-filter-card"], &image);
}

/// The Lynx `<blur-view>`, written the only way script can: `__CreateElement`
/// mints the tag and `__SetAttribute` names the radius. The two cards are the
/// same translucent rounded box with the same `<text>` child over the same
/// crimson / white seam and amber bar — one is a `blur-view` carrying
/// `blur-radius="25"`, the other an `x-blur-view` carrying no attribute at all,
/// which is web-core's own `x-blur-view/basic.html` fixture reduced to blocks.
const BLUR_VIEW_SCRIPT: &str = r"
globalThis.renderPage = function renderPage() {
  const page = __CreatePage('card', 0);
  __SetClasses(page, 'page');
  const board = __CreateView(0);
  __SetClasses(board, 'board');
  function part(classes) {
    const node = __CreateView(0);
    __SetClasses(node, classes);
    __AppendElement(board, node);
  }
  part('half');
  part('bar');
  part('bar lower');
  function frost(tag, classes, radius, label) {
    const node = __CreateElement(tag, 0);
    __SetClasses(node, classes);
    if (radius !== null) __SetAttribute(node, 'blur-radius', radius);
    const text = __CreateText(0);
    __SetClasses(text, 'label');
    __AppendElement(text, __CreateRawText(label));
    __AppendElement(node, text);
    __AppendElement(board, node);
  }
  frost('blur-view', 'frost', '25', 'blur-radius 25');
  frost('x-blur-view', 'frost plain', null, 'no blur-radius');
  __AppendElement(page, board);
};
";

/// Requirement: the `blur-radius` attribute reaches the same painter path
/// author `backdrop-filter` reaches, through the whole embedder stack — the
/// component's presentational hint, a commit on `bobcat-main`, and the
/// painter's backdrop bake.
///
/// The golden shows a relationship rather than a picture: the two cards differ
/// only in the attribute, so everything behind the upper one is filtered and
/// everything behind the lower one is not. The two tag names are split between
/// them on purpose — `blur-view` above, `x-blur-view` below — so the golden
/// would also catch one name losing its component.
#[tokio::test]
async fn a_blur_view_reaches_the_offscreen_draw_target() {
    const ROBOTO: &[u8] = include_bytes!("../../hughie/tests/fixtures/Roboto-Regular.ttf");

    let (_view, mut painter) = booted_with_sheet_sources(
        BLUR_VIEW_SCRIPT.as_bytes(),
        PreparsedStyleSheet {
            rules: vec![
                PreparsedRule::Style {
                    selectors: ".page".to_owned(),
                    declarations: vec![
                        declaration("display", "flex"),
                        declaration("padding", "20px"),
                        declaration("background-color", "#e5e7eb"),
                        declaration("font-family", "Roboto"),
                    ],
                },
                block(
                    ".board",
                    &[
                        ("position", "relative"),
                        ("width", "353px"),
                        ("height", "460px"),
                        ("background-color", "#ffffff"),
                    ],
                ),
                block(
                    ".half",
                    &[
                        ("position", "absolute"),
                        ("left", "0px"),
                        ("top", "0px"),
                        ("width", "176px"),
                        ("height", "460px"),
                        ("background-color", "#dc2626"),
                    ],
                ),
                block(
                    ".bar",
                    &[
                        ("position", "absolute"),
                        ("left", "0px"),
                        ("top", "140px"),
                        ("width", "353px"),
                        ("height", "26px"),
                        ("background-color", "#f59e0b"),
                    ],
                ),
                PreparsedRule::Style {
                    selectors: ".lower".to_owned(),
                    declarations: vec![declaration("top", "340px")],
                },
                // No `display` here: the box has to keep the UA sheet's own
                // container mode, which is what the golden is checking on the
                // cascade side.
                PreparsedRule::Style {
                    selectors: ".frost".to_owned(),
                    declarations: vec![
                        declaration("position", "absolute"),
                        declaration("left", "20px"),
                        declaration("top", "110px"),
                        declaration("width", "313px"),
                        declaration("height", "100px"),
                        declaration("padding", "18px"),
                        declaration("box-sizing", "border-box"),
                        declaration("border-radius", "18px"),
                        declaration("border", "2px solid rgb(255 255 255 / 70%)"),
                        declaration("background-color", "rgb(255 255 255 / 25%)"),
                    ],
                },
                // Source order decides: `.plain` follows `.frost`, so the
                // second card keeps every other declaration and only drops to
                // the lower band.
                PreparsedRule::Style {
                    selectors: ".plain".to_owned(),
                    declarations: vec![declaration("top", "310px")],
                },
                PreparsedRule::Style {
                    selectors: ".label".to_owned(),
                    declarations: vec![
                        declaration("font-size", "22px"),
                        declaration("color", "#111827"),
                    ],
                },
            ],
        },
        393.0,
        520.0,
        ViewSources {
            style_sheets: vec!["app:///author.css".to_owned()],
            fonts: vec![FontBlob::from_static(ROBOTO)],
            default_font_family: Some("Roboto".to_owned()),
            ..ViewSources::new("app:///", SCRIPT_URL, SCREEN)
        },
    )
    .await;

    let shot = painter.capture().expect("capture the blur-view page");
    let image = Image::from_rgba8(shot.size.width, shot.size.height, shot.pixels)
        .expect("captured RGBA image");
    screenshots().assert_matches(&["blur-view-card"], &image);
}
