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
use prime_radiant::{Image, Screenshots};
use support::{FetcherDouble, solo_view, tick, wait_for_script};

const SCREEN: bobcat_core::ScreenMetrics =
    bobcat_core::ScreenMetrics::for_viewport(32.0, 24.0, 1.0);

const SCRIPT_URL: &str = "app:///main.js";
const MAIN_THREAD_SCRIPT: &str = r"
globalThis.renderPage = function renderPage() {
  const page = __CreatePage('card', 0);
  __AppendElement(page, __CreateView(0));
};
";

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
        property: property.to_owned().into(),
        value: value.to_owned(),
        important: false,
    }
}

fn fetcher(source: &[u8]) -> impl FnOnce(bobcat_core::ImageReports) -> Rc<FetcherDouble> {
    let source = source.to_vec();
    move |_sink| Rc::new(FetcherDouble::card(source).resolving_to(SCRIPT_URL))
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
            ..ViewSources::new("app:///", SCRIPT_URL, SCREEN, "en-US")
        },
    )
    .await
}

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
                FetcherDouble::card(source)
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

fn settle(
    view: &mut LynxView<Rc<FetcherDouble>>,
    painter: &mut Painter,
    waiting_for: &str,
    ready: impl Fn() -> bool,
) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        let _ = view.pump();
        let _ = tick(painter, true);
        if ready() {
            return;
        }
        assert!(std::time::Instant::now() < deadline, "{waiting_for}");
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
}

fn settle_images(
    view: &mut LynxView<Rc<FetcherDouble>>,
    painter: &mut Painter,
    images: &prime_radiant::TestImages,
) {
    settle(
        view,
        painter,
        "no committed frame ever drew an image",
        || images.retained().iter().any(|set| !set.is_empty()),
    );
}

fn settle_drawing(
    view: &mut LynxView<Rc<FetcherDouble>>,
    painter: &mut Painter,
    images: &prime_radiant::TestImages,
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
    prime_radiant::screenshots_in(env!("CARGO_MANIFEST_DIR"))
}

fn captured(painter: &mut Painter) -> Image {
    let shot = painter.capture().expect("capture the committed page");
    Image::from_rgba8(shot.size.width, shot.size.height, shot.pixels).expect("captured RGBA image")
}

fn checker_store() -> Rc<prime_radiant::TestImages> {
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
    let images = Rc::new(prime_radiant::TestImages::new());
    images.insert_rgba8(IMAGE_URL, 4, 4, rgba);
    images
}

#[tokio::test]
async fn fetched_script_reaches_the_offscreen_draw_target() {
    let (_view, mut painter) = booted(
        fetcher(MAIN_THREAD_SCRIPT.as_bytes()),
        ViewSources::new("app:///", SCRIPT_URL, SCREEN, "en-US"),
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

#[tokio::test]
async fn an_embedder_image_store_reaches_the_private_painter() {
    let images = checker_store();
    let (mut view, mut painter) = booted(
        |sink| {
            Rc::new(
                FetcherDouble::card(IMAGE_SCRIPT)
                    .resolving_to(SCRIPT_URL)
                    .with_images(Rc::clone(&images))
                    .serving(sink),
            )
        },
        ViewSources::new("app:///", SCRIPT_URL, SCREEN, "en-US"),
    )
    .await;
    settle_images(&mut view, &mut painter, &images);

    screenshots().assert_matches(&["embedder-image-store"], &captured(&mut painter));
}

#[tokio::test]
async fn an_image_element_loads_and_paints_from_its_src() {
    let images = checker_store();
    let (mut view, mut painter) = booted(
        |sink| {
            Rc::new(
                FetcherDouble::card(IMAGE_ELEMENT_SCRIPT)
                    .resolving_to(SCRIPT_URL)
                    .with_images(Rc::clone(&images))
                    .serving(sink),
            )
        },
        ViewSources::new("app:///", SCRIPT_URL, SCREEN, "en-US"),
    )
    .await;
    settle_images(&mut view, &mut painter, &images);

    screenshots().assert_matches(&["image-element"], &captured(&mut painter));
}

#[tokio::test]
async fn raw_text_reaches_the_private_painter_as_glyphs() {
    const ROBOTO: &[u8] = include_bytes!("../../seldon/tests/fixtures/Roboto-Regular.ttf");

    let (_view, mut painter) = booted(
        fetcher(TEXT_SCRIPT.as_bytes()),
        ViewSources {
            fonts: vec![FontBlob::from_static(ROBOTO)],
            default_font_family: Some("Roboto".to_owned()),
            ..ViewSources::new("app:///", SCRIPT_URL, SCREEN, "en-US")
        },
    )
    .await;

    screenshots().assert_matches(&["raw-text-runs"], &captured(&mut painter));
}

#[tokio::test]
async fn markdown_renders_its_content_in_its_shadow_tree() {
    const ROBOTO: &[u8] = include_bytes!("../../seldon/tests/fixtures/Roboto-Regular.ttf");
    const SOURCE: &str = r"
globalThis.renderPage = function renderPage() {
  const page = __CreatePage('card', 0);
  __SetInlineStyles(
    page,
    'background-color:#ffffff;padding:16px;font-family:Roboto;color:#111827',
  );
  const markdown = __CreateElement('x-markdown', 0);
  __SetInlineStyles(markdown, 'font-size:14px');
  // The registered face for the code too: the template's monospace stack
  // would reach whichever system font the machine has.
  __SetAttribute(
    markdown,
    'markdown-style',
    JSON.stringify({
      inlineCode: { fontFamily: 'Roboto', color: 'b91c1c' },
      codeBlock: { fontFamily: 'Roboto' },
    }),
  );
  __SetAttribute(
    markdown,
    'content',
    '# Title\n\nthis is **bold** and *italic*, with a [link](https://lynxjs.org/).\n\n'
      + '- item 1\n- item 2\n\ninline code: `code`.\n\n```\nblock code\n```\n\n'
      + '| Name | Value |\n| - | :-: |\n| one | 1 |\n| two | 2 |\n',
  );
  __AppendElement(page, markdown);
};
";
    let (mut view, mut painter) = solo_view(
        Arc::new(NoWakeup),
        320.0,
        400.0,
        1.0,
        DrawTarget::Offscreen,
        fetcher(SOURCE.as_bytes()),
        ViewSources {
            fonts: vec![FontBlob::from_static(ROBOTO)],
            default_font_family: Some("Roboto".to_owned()),
            ..ViewSources::new("app:///", SCRIPT_URL, SCREEN, "en-US")
        },
    )
    .await
    .expect("view");
    wait_for_script(&mut view).expect("script execution");
    screenshots().assert_matches(&["markdown-card"], &captured(&mut painter));
}

#[tokio::test]
async fn text_maxline_halves_the_rendered_box_at_device_pixel_ratio_two() {
    const AHEM: &[u8] = include_bytes!("../../seldon/tests/fixtures/Ahem.ttf");
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
            ..ViewSources::new("app:///", SCRIPT_URL, SCREEN, "en-US")
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

#[tokio::test]
async fn text_fields_draw_their_placeholder_or_their_value() {
    const AHEM: &[u8] = include_bytes!("../../seldon/tests/fixtures/Ahem.ttf");
    const SOURCE: &str = r"
globalThis.renderPage = function () {
  const page = __CreatePage('card', 0);
  __SetInlineStyles(page, 'display:flex;flex-direction:column;align-items:flex-start;gap:8px;padding:8px;background-color:white');
  function field(tag, attributes) {
    const element = __CreateElement(tag, 0);
    __SetInlineStyles(element, 'width:180px;height:40px;padding:0 8px;font-size:10px;line-height:10px;background-color:#e5e7eb;color:#1d4ed8');
    for (const [name, value] of attributes) __SetAttribute(element, name, value);
    __AppendElement(page, element);
  }
  field('x-input', [['placeholder', 'say hi']]);
  field('x-input-ng', [['placeholder', 'red'], ['placeholder-color', 'red']]);
  field('x-input', [['value', 'hello world'], ['placeholder', 'unseen']]);
  field('x-textarea', [['value', 'one two three four five six seven']]);
};
";
    let (mut view, mut painter) = solo_view(
        Arc::new(NoWakeup),
        200.0,
        200.0,
        1.0,
        DrawTarget::Offscreen,
        fetcher(SOURCE.as_bytes()),
        ViewSources {
            fonts: vec![FontBlob::from_static(AHEM)],
            default_font_family: Some("Ahem".to_owned()),
            ..ViewSources::new("app:///", SCRIPT_URL, SCREEN, "en-US")
        },
    )
    .await
    .expect("view");
    wait_for_script(&mut view).expect("script execution");
    screenshots().assert_matches(&["text-fields"], &captured(&mut painter));
}

#[tokio::test]
async fn a_placeholder_rule_draws_the_placeholder_in_its_style() {
    const AHEM: &[u8] = include_bytes!("../../seldon/tests/fixtures/Ahem.ttf");
    const SOURCE: &str = r"
globalThis.renderPage = function () {
  const page = __CreatePage('card', 0);
  __SetInlineStyles(page, 'display:flex;flex-direction:column;align-items:flex-start;gap:8px;padding:8px;background-color:white');
  function field(tag, attributes) {
    const element = __CreateElement(tag, 0);
    __SetInlineStyles(element, 'width:180px;height:40px;padding:0 8px;font-size:10px;border:1px solid black;color:#1d4ed8');
    __SetClasses(element, 'target');
    for (const [name, value] of attributes) __SetAttribute(element, name, value);
    __AppendElement(page, element);
  }
  field('x-input', [['placeholder', 'foo']]);
  field('x-input', [['value', 'foo'], ['placeholder', 'unseen']]);
  field('x-textarea', [['placeholder', 'foo bar']]);
};
";
    let (_view, mut painter) = booted_with_sheet_sources(
        SOURCE.as_bytes(),
        PreparsedStyleSheet {
            rules: vec![PreparsedRule::Style {
                selectors: ".target::placeholder".to_owned(),
                declarations: vec![
                    declaration("color", "red"),
                    declaration("font-weight", "bold"),
                    declaration("font-size", "20px"),
                ],
            }],
        },
        200.0,
        200.0,
        ViewSources {
            fonts: vec![FontBlob::from_static(AHEM)],
            default_font_family: Some("Ahem".to_owned()),
            style_sheets: vec!["app:///author.css".to_owned()],
            ..ViewSources::new("app:///", SCRIPT_URL, SCREEN, "en-US")
        },
    )
    .await;
    screenshots().assert_matches(&["text-fields-placeholder-rule"], &captured(&mut painter));
}

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
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|&&pixel| pixel == wanted)
            .count()
    };
    assert_eq!(
        count([0, 255, 0, 255]),
        16 * 12,
        "the class swap must restyle against the author rules"
    );
    assert_eq!(count([255, 0, 0, 255]), 0, "no pre-swap pixels survive");
}

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

const LANDSCAPE_SOURCE: &str = "https://example.test/landscape.png";
const PORTRAIT_SOURCE: &str = "https://example.test/portrait.png";
const HOLDING_SOURCE: &str = "https://example.test/holding.png";
const PUNCHED_SOURCE: &str = "https://example.test/punched.png";
const PENDING_SOURCE: &str = "https://example.test/never-answered.png";
const BROKEN_SOURCE: &str = "https://example.test/broken.png";

const BITMAP_FRAME: u32 = 2;

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

fn picture_store() -> Rc<prime_radiant::TestImages> {
    let images = Rc::new(prime_radiant::TestImages::new());
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

async fn booted_with_images(
    source: String,
    images: &Rc<prime_radiant::TestImages>,
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
                FetcherDouble::card(source)
                    .resolving_to(SCRIPT_URL)
                    .with_images(images)
                    .serving(sink),
            )
        },
        ViewSources::new("app:///", SCRIPT_URL, SCREEN, "en-US"),
    )
    .await
    .expect("view");
    wait_for_script(&mut view).expect("script execution");
    (view, painter)
}

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

#[tokio::test]
async fn a_blurred_card_reaches_the_offscreen_draw_target() {
    const ROBOTO: &[u8] = include_bytes!("../../seldon/tests/fixtures/Roboto-Regular.ttf");

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
            ..ViewSources::new("app:///", SCRIPT_URL, SCREEN, "en-US")
        },
    )
    .await;

    let shot = painter.capture().expect("capture the blurred page");
    let image = Image::from_rgba8(shot.size.width, shot.size.height, shot.pixels)
        .expect("captured RGBA image");
    screenshots().assert_matches(&["filter-blur-card"], &image);
}

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

#[tokio::test]
async fn a_backdrop_filtered_card_reaches_the_offscreen_draw_target() {
    const ROBOTO: &[u8] = include_bytes!("../../seldon/tests/fixtures/Roboto-Regular.ttf");

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
            ..ViewSources::new("app:///", SCRIPT_URL, SCREEN, "en-US")
        },
    )
    .await;

    let shot = painter.capture().expect("capture the backdrop page");
    let image = Image::from_rgba8(shot.size.width, shot.size.height, shot.pixels)
        .expect("captured RGBA image");
    screenshots().assert_matches(&["backdrop-filter-card"], &image);
}

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

#[tokio::test]
async fn a_blur_view_reaches_the_offscreen_draw_target() {
    const ROBOTO: &[u8] = include_bytes!("../../seldon/tests/fixtures/Roboto-Regular.ttf");

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
            ..ViewSources::new("app:///", SCRIPT_URL, SCREEN, "en-US")
        },
    )
    .await;

    let shot = painter.capture().expect("capture the blur-view page");
    let image = Image::from_rgba8(shot.size.width, shot.size.height, shot.pixels)
        .expect("captured RGBA image");
    screenshots().assert_matches(&["blur-view-card"], &image);
}
