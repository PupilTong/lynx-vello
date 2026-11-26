//! Attaching and detaching, from where an embedder stands.
//!
//! A painter is a standalone object over one draw target, and a view is a
//! running page. Which one is watching which is a fact an embedder changes at
//! runtime — a browser keeps one canvas painter across page loads — so what
//! this file pins is the pairing rule, what a change of view resets, and what
//! survives losing either half.

mod support;

use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use bobcat_core::{DrawTarget, LynxGroup, NoWakeup, Painter, StyleThreads, ViewSources};
use support::{FetcherDouble, tick, wait_for_script};

const SCREEN: bobcat_core::ScreenMetrics =
    bobcat_core::ScreenMetrics::for_viewport(32.0, 24.0, 1.0);

const SCRIPT_URL: &str = "app:///main.js";
const IMAGE_URL: &str = "https://example.test/pixel.png";

fn page(color: &str) -> Vec<u8> {
    format!(
        r"
globalThis.renderPage = function renderPage() {{
  const page = __CreatePage('card', 0);
  const box = __CreateView(0);
  __SetInlineStyles(box, 'width:32px;height:24px;background-color:{color}');
  __AppendElement(page, box);
}};
"
    )
    .into_bytes()
}

fn image_page() -> Vec<u8> {
    format!(
        r#"
globalThis.renderPage = function renderPage() {{
  const page = __CreatePage('card', 0);
  const box = __CreateView(0);
  __SetInlineStyles(box, 'width:32px;height:24px;background-image:url("{IMAGE_URL}")');
  __AppendElement(page, box);
}};
"#
    )
    .into_bytes()
}

const RED: [u8; 4] = [255, 0, 0, 255];
const BLUE: [u8; 4] = [0, 0, 255, 255];

fn top_left(painter: &mut Painter) -> [u8; 4] {
    let shot = painter.capture().expect("the committed frame");
    <[u8; 4]>::try_from(&shot.pixels[..4]).expect("an RGBA frame")
}

async fn offscreen_painter() -> Painter {
    Painter::new(DrawTarget::Offscreen, 32.0, 24.0, 1.0)
        .await
        .expect("an offscreen painter is built")
}

async fn group() -> LynxGroup {
    LynxGroup::new(Arc::new(NoWakeup), StyleThreads::Sequential)
        .await
        .expect("the group starts")
}

#[tokio::test]
async fn a_second_painter_on_one_view_is_refused() {
    let group = LynxGroup::new(Arc::new(NoWakeup), StyleThreads::Sequential)
        .await
        .expect("the group starts");
    let mut view = group
        .create_lynx_view(
            32.0,
            24.0,
            1.0,
            |_reports| Rc::new(FetcherDouble::card(page("#ff0000")).resolving_to(SCRIPT_URL)),
            Vec::new(),
            ViewSources::new("app:///", SCRIPT_URL, SCREEN, "en-US"),
        )
        .expect("the view is built");
    let mut first = offscreen_painter().await;
    first.attach(&view).expect("a fresh view takes a painter");
    wait_for_script(&mut view).expect("the entry module boots");

    let mut second = offscreen_painter().await;
    let error = second
        .attach(&view)
        .expect_err("a view already being painted refuses a second painter");
    assert!(
        matches!(error, bobcat_core::EngineError::PainterAttached),
        "{error}"
    );
    assert!(!second.is_attached(), "and the refusal attached nothing");
    assert!(first.is_attached(), "while the first still observes it");
}

#[tokio::test]
async fn a_detached_view_can_be_painted_by_another_painter() {
    let group = LynxGroup::new(Arc::new(NoWakeup), StyleThreads::Sequential)
        .await
        .expect("the group starts");
    let mut view = group
        .create_lynx_view(
            32.0,
            24.0,
            1.0,
            |_reports| Rc::new(FetcherDouble::card(page("#ff0000")).resolving_to(SCRIPT_URL)),
            Vec::new(),
            ViewSources::new("app:///", SCRIPT_URL, SCREEN, "en-US"),
        )
        .expect("the view is built");
    let mut first = offscreen_painter().await;
    first.attach(&view).expect("a fresh view takes a painter");
    wait_for_script(&mut view).expect("the entry module boots");
    tick(&mut first, true).expect("the first frame");
    assert_eq!(top_left(&mut first), RED);

    first.detach();
    assert!(!first.is_attached());
    assert_eq!(
        top_left(&mut first),
        RED,
        "detaching leaves what it drew on the target"
    );

    let mut second = offscreen_painter().await;
    second
        .attach(&view)
        .expect("a detached view takes a painter again");
    tick(&mut second, true).expect("the same page, drawn again");
    assert_eq!(
        top_left(&mut second),
        RED,
        "the page belongs to the view, not to whichever painter drew it"
    );
}

#[tokio::test]
async fn a_view_dropped_under_its_painter_leaves_the_last_frame_standing() {
    let group = LynxGroup::new(Arc::new(NoWakeup), StyleThreads::Sequential)
        .await
        .expect("the group starts");
    let mut view = group
        .create_lynx_view(
            32.0,
            24.0,
            1.0,
            |_reports| Rc::new(FetcherDouble::card(page("#ff0000")).resolving_to(SCRIPT_URL)),
            Vec::new(),
            ViewSources::new("app:///", SCRIPT_URL, SCREEN, "en-US"),
        )
        .expect("the view is built");
    let mut painter = offscreen_painter().await;
    painter.attach(&view).expect("a fresh view takes a painter");
    wait_for_script(&mut view).expect("the entry module boots");
    tick(&mut painter, true).expect("the frame the view committed");
    assert_eq!(top_left(&mut painter), RED);

    drop(view);
    painter
        .pump()
        .expect("a released view is not a draw failure");
    assert!(
        !painter.is_attached(),
        "the painter noticed the view had gone"
    );
    assert!(
        tick(&mut painter, true).expect("the retained frame still renders"),
        "the painter kept the frame it had adopted"
    );
    assert_eq!(
        top_left(&mut painter),
        RED,
        "and what it last drew is still there to read back"
    );
}

#[tokio::test]
async fn a_dropped_views_last_frame_keeps_the_image_pixels_it_read() {
    let images = Rc::new(prime_radiant::TestImages::new());
    images.insert_rgba8(IMAGE_URL, 1, 1, vec![0, 0, 255, 255]);
    let group = group().await;
    let mut view = group
        .create_lynx_view(
            32.0,
            24.0,
            1.0,
            |sink| {
                Rc::new(
                    FetcherDouble::card(image_page())
                        .resolving_to(SCRIPT_URL)
                        .with_images(Rc::clone(&images))
                        .serving(sink),
                )
            },
            Vec::new(),
            ViewSources::new("app:///", SCRIPT_URL, SCREEN, "en-US"),
        )
        .expect("the view is built");
    let mut painter = offscreen_painter().await;
    painter.attach(&view).expect("a fresh view takes a painter");
    wait_for_script(&mut view).expect("the entry module boots");

    let deadline = Instant::now() + Duration::from_secs(30);
    while !images.retained().iter().any(|set| !set.is_empty()) {
        let _ = view.pump();
        let _ = tick(&mut painter, true);
        assert!(
            Instant::now() < deadline,
            "no committed frame ever drew the image"
        );
    }
    assert_eq!(
        top_left(&mut painter),
        BLUE,
        "the image is what fills the box"
    );

    drop(view);
    painter
        .pump()
        .expect("a released view is not a draw failure");
    assert!(
        tick(&mut painter, true).expect("the retained frame still renders"),
        "the painter kept the frame it had adopted"
    );
    assert_eq!(
        top_left(&mut painter),
        BLUE,
        "and the pixels it had read went with it"
    );
}

fn restyled_image_page() -> Vec<u8> {
    format!(
        r#"
globalThis.renderPage = function renderPage() {{
  const page = __CreatePage('card', 0);
  const box = __CreateView(0);
  __SetInlineStyles(box, 'width:32px;height:24px;background-image:url("{IMAGE_URL}")');
  __AppendElement(page, box);
  globalThis.box = box;
}};
globalThis.updatePage = function updatePage(data) {{
  __SetInlineStyles(globalThis.box, data.style);
  __FlushElementTree();
}};
"#
    )
    .into_bytes()
}

async fn image_view(
    group: &LynxGroup,
    page: Vec<u8>,
    images: &Rc<prime_radiant::TestImages>,
) -> (bobcat_core::LynxView<Rc<FetcherDouble>>, Painter) {
    let mut view = group
        .create_lynx_view(
            32.0,
            24.0,
            1.0,
            |sink| {
                Rc::new(
                    FetcherDouble::card(page)
                        .resolving_to(SCRIPT_URL)
                        .with_images(Rc::clone(images))
                        .serving(sink),
                )
            },
            Vec::new(),
            ViewSources::new("app:///", SCRIPT_URL, SCREEN, "en-US"),
        )
        .expect("the view is built");
    let mut painter = offscreen_painter().await;
    painter.attach(&view).expect("a fresh view takes a painter");
    wait_for_script(&mut view).expect("the entry module boots");
    (view, painter)
}

fn turn_until(
    view: &mut bobcat_core::LynxView<Rc<FetcherDouble>>,
    painter: &mut Painter,
    what: &str,
    mut done: impl FnMut() -> bool,
) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while !done() {
        let _ = view.pump();
        let _ = tick(painter, true);
        assert!(Instant::now() < deadline, "{what}");
    }
}

fn image_reads(images: &prime_radiant::TestImages) -> usize {
    images
        .reads()
        .iter()
        .filter(|(source, _)| source == IMAGE_URL)
        .count()
}

fn restyle(
    view: &mut bobcat_core::LynxView<Rc<FetcherDouble>>,
    painter: &mut Painter,
    images: &prime_radiant::TestImages,
    style: &str,
) {
    let commits = images.retained().len();
    view.update_data(format!(r#"{{"style":"{style}"}}"#), String::new())
        .expect("a booted view accepts a data update");
    turn_until(view, painter, "the restyle never committed", || {
        images.retained().len() > commits
    });
}

#[tokio::test]
async fn a_bitmap_the_rasterizer_holds_is_released_and_drawn_from_its_upload() {
    let images = Rc::new(prime_radiant::TestImages::releasing());
    let published = prime_radiant::rgba8(1, 1, vec![0, 0, 255, 255]);
    let pixels = published.data.downgrade();
    images.insert(IMAGE_URL, published);
    let group = group().await;
    let (mut view, mut painter) = image_view(&group, restyled_image_page(), &images).await;

    turn_until(
        &mut view,
        &mut painter,
        "the image was never released",
        || !images.released().is_empty(),
    );
    assert_eq!(
        images.released(),
        [vec![Arc::<str>::from(IMAGE_URL)]],
        "released once, after the frame that uploaded it"
    );
    assert!(!images.holds_pixels(IMAGE_URL));
    assert!(
        pixels.upgrade().is_none(),
        "the store dropped the pixels, and nothing else held them"
    );
    assert_eq!(top_left(&mut painter), BLUE, "drawn from the upload");
    let reads = image_reads(&images);

    let described = images.described().len();
    restyle(
        &mut view,
        &mut painter,
        &images,
        &format!("width:31px;height:24px;background-image:url('{IMAGE_URL}')"),
    );
    assert!(
        images.described().len() > described,
        "the new commit described the image"
    );
    assert_eq!(
        image_reads(&images),
        reads,
        "and drew it from the leased upload without reading it"
    );
    assert_eq!(top_left(&mut painter), BLUE);
    assert!(!images.holds_pixels(IMAGE_URL));
}

#[tokio::test]
async fn a_draw_with_another_sampler_draws_the_released_image_from_its_upload() {
    let images = Rc::new(prime_radiant::TestImages::releasing());
    images.insert_rgba8(IMAGE_URL, 1, 1, vec![0, 0, 255, 255]);
    let group = group().await;
    let (mut view, mut painter) = image_view(&group, restyled_image_page(), &images).await;
    turn_until(
        &mut view,
        &mut painter,
        "the image was never released",
        || !images.released().is_empty(),
    );
    let reads = image_reads(&images);

    restyle(
        &mut view,
        &mut painter,
        &images,
        &format!(
            "width:32px;height:24px;background-image:url('{IMAGE_URL}');image-rendering:pixelated"
        ),
    );
    assert_eq!(top_left(&mut painter), BLUE, "the image draws");
    assert_eq!(image_reads(&images), reads, "from the upload, with no read");
    assert!(!images.holds_pixels(IMAGE_URL));
    assert_eq!(images.released().len(), 1, "released once");
}

fn two_sampler_page() -> Vec<u8> {
    format!(
        r#"
globalThis.renderPage = function renderPage() {{
  const page = __CreatePage('card', 0);
  const scroller = __CreateView(0);
  const first = __CreateView(0);
  const second = __CreateView(0);
  __SetInlineStyles(scroller,
    'display:flex;flex-direction:column;overflow:scroll;width:32px;height:24px');
  __SetInlineStyles(first,
    'flex-shrink:0;width:32px;height:30px;background-image:url("{IMAGE_URL}")');
  __SetInlineStyles(second,
    'flex-shrink:0;width:32px;height:30px;background-image:url("{IMAGE_URL}");image-rendering:pixelated');
  __AppendElement(page, scroller);
  __AppendElement(scroller, first);
  __AppendElement(scroller, second);
  globalThis.rows = [first, second];
}};
globalThis.updatePage = function updatePage(data) {{
  __SetInlineStyles(globalThis.rows[0],
    'flex-shrink:0;width:32px;height:' + data.rows + ';background-image:url("{IMAGE_URL}")');
  __SetInlineStyles(globalThis.rows[1],
    'flex-shrink:0;width:32px;height:' + data.rows + ';background-image:url("{IMAGE_URL}");image-rendering:pixelated');
  __FlushElementTree();
}};
"#
    )
    .into_bytes()
}

#[tokio::test]
async fn an_image_drawn_with_two_samplers_is_one_upload_released_once_it_exists() {
    let images = Rc::new(prime_radiant::TestImages::releasing());
    images.insert_rgba8(IMAGE_URL, 1, 1, vec![0, 0, 255, 255]);
    let group = group().await;
    let (mut view, mut painter) = image_view(&group, two_sampler_page(), &images).await;
    let deadline = Instant::now() + Duration::from_secs(30);
    while images.retained().is_empty() || top_left(&mut painter) != BLUE {
        let _ = view.pump();
        let _ = tick(&mut painter, true);
        assert!(Instant::now() < deadline, "the image was never drawn");
    }
    turn_until(
        &mut view,
        &mut painter,
        "the first row's upload never released the image",
        || !images.released().is_empty(),
    );
    assert!(!images.holds_pixels(IMAGE_URL));
    let reads = image_reads(&images);

    let commits = images.retained().len();
    view.update_data(r#"{"rows":"12px"}"#.to_owned(), String::new())
        .expect("a booted view accepts a data update");
    turn_until(
        &mut view,
        &mut painter,
        "the resized rows never committed",
        || images.retained().len() > commits,
    );
    assert!(!images.holds_pixels(IMAGE_URL));
    let shot = painter.capture().expect("the committed frame");
    let at = |y: usize| <[u8; 4]>::try_from(&shot.pixels[y * 32 * 4..y * 32 * 4 + 4]).unwrap();
    assert_eq!(
        [at(2), at(20)],
        [BLUE, BLUE],
        "both rows draw from the one upload"
    );
    assert_eq!(image_reads(&images), reads, "with no read");
    assert_eq!(images.released().len(), 1, "released once");
}

#[tokio::test]
async fn one_painter_re_attached_to_a_second_view_shows_the_second_page() {
    let mut painter = offscreen_painter().await;

    let first_group = LynxGroup::new(Arc::new(NoWakeup), StyleThreads::Sequential)
        .await
        .expect("the group starts");
    let mut first = first_group
        .create_lynx_view(
            32.0,
            24.0,
            1.0,
            |_reports| Rc::new(FetcherDouble::card(page("#ff0000")).resolving_to(SCRIPT_URL)),
            Vec::new(),
            ViewSources::new("app:///", SCRIPT_URL, SCREEN, "en-US"),
        )
        .expect("the first view is built");
    painter.attach(&first).expect("the first page takes it");
    wait_for_script(&mut first).expect("the first entry boots");
    tick(&mut painter, true).expect("the first page's frame");
    assert_eq!(top_left(&mut painter), RED);

    painter.detach();
    drop(first);
    drop(first_group);

    let second_group = LynxGroup::new(Arc::new(NoWakeup), StyleThreads::Sequential)
        .await
        .expect("the second group starts");
    let mut second = second_group
        .create_lynx_view(
            32.0,
            24.0,
            1.0,
            |_reports| Rc::new(FetcherDouble::card(page("#0000ff")).resolving_to(SCRIPT_URL)),
            Vec::new(),
            ViewSources::new("app:///", SCRIPT_URL, SCREEN, "en-US"),
        )
        .expect("the second view is built");
    painter.attach(&second).expect("the second page takes it");
    assert!(
        painter.capture().is_err(),
        "nothing of the new page has been rendered yet"
    );
    wait_for_script(&mut second).expect("the second entry boots");
    assert!(
        tick(&mut painter, false).expect("the second page's frame"),
        "the reset made the new page's first frame unrendered"
    );
    assert_eq!(
        top_left(&mut painter),
        BLUE,
        "the painter drew the page it is now attached to, not the one it kept"
    );
}

#[tokio::test]
async fn a_painter_whose_view_is_gone_attaches_to_the_next_one() {
    let mut painter = offscreen_painter().await;

    let first_group = group().await;
    let mut first = first_group
        .create_lynx_view(
            32.0,
            24.0,
            1.0,
            |_reports| Rc::new(FetcherDouble::card(page("#ff0000")).resolving_to(SCRIPT_URL)),
            Vec::new(),
            ViewSources::new("app:///", SCRIPT_URL, SCREEN, "en-US"),
        )
        .expect("the first view is built");
    painter.attach(&first).expect("the first page takes it");
    wait_for_script(&mut first).expect("the first entry boots");
    tick(&mut painter, true).expect("the first page's frame");
    assert_eq!(top_left(&mut painter), RED);

    drop(first);
    drop(first_group);

    let second_group = group().await;
    let mut second = second_group
        .create_lynx_view(
            32.0,
            24.0,
            1.0,
            |_reports| Rc::new(FetcherDouble::card(page("#0000ff")).resolving_to(SCRIPT_URL)),
            Vec::new(),
            ViewSources::new("app:///", SCRIPT_URL, SCREEN, "en-US"),
        )
        .expect("the second view is built");
    painter
        .attach(&second)
        .expect("a painter whose view has gone is not an attached one");
    match painter.capture() {
        Err(_) => {}
        Ok(shot) => assert_eq!(
            <[u8; 4]>::try_from(&shot.pixels[..4]).expect("an RGBA frame"),
            BLUE,
            "the painter kept the first page's frame"
        ),
    }
    wait_for_script(&mut second).expect("the second entry boots");
    tick(&mut painter, true).expect("the second page's frame");
    assert_eq!(
        top_left(&mut painter),
        BLUE,
        "and it drew the page it took, not the one it had kept"
    );
}

#[tokio::test]
async fn a_painter_re_attached_through_the_auto_detach_resets_what_it_kept() {
    let mut painter = offscreen_painter().await;

    let first_group = group().await;
    let mut first = first_group
        .create_lynx_view(
            32.0,
            24.0,
            1.0,
            |_reports| Rc::new(FetcherDouble::card(page("#ff0000")).resolving_to(SCRIPT_URL)),
            Vec::new(),
            ViewSources::new("app:///", SCRIPT_URL, SCREEN, "en-US"),
        )
        .expect("the first view is built");
    painter.attach(&first).expect("the first page takes it");
    wait_for_script(&mut first).expect("the first entry boots");
    tick(&mut painter, true).expect("the first page's frame");
    assert_eq!(top_left(&mut painter), RED);

    drop(first);
    drop(first_group);
    painter
        .pump()
        .expect("a released view is not a draw failure");
    assert!(
        !painter.is_attached(),
        "the turn is where the painter notices, and it took one"
    );

    let second_group = group().await;
    let mut second = second_group
        .create_lynx_view(
            32.0,
            24.0,
            1.0,
            |_reports| Rc::new(FetcherDouble::card(page("#0000ff")).resolving_to(SCRIPT_URL)),
            Vec::new(),
            ViewSources::new("app:///", SCRIPT_URL, SCREEN, "en-US"),
        )
        .expect("the second view is built");
    painter
        .attach(&second)
        .expect("a painter that already noticed takes the next page");
    wait_for_script(&mut second).expect("the second entry boots");
    assert!(
        tick(&mut painter, false).expect("the second page's frame"),
        "the attach dropped the first page's compose key"
    );
    assert_eq!(
        top_left(&mut painter),
        BLUE,
        "and the frame it rendered is the second page's"
    );
}

fn blurred_page() -> Vec<u8> {
    br"
globalThis.renderPage = function renderPage() {
  const page = __CreatePage('card', 0);
  __SetInlineStyles(page, 'width:64px;height:64px;background-color:#ffffff;position:relative');
  const box = __CreateView(0);
  __SetInlineStyles(
    box,
    'position:absolute;left:20px;top:20px;width:24px;height:24px;' +
      'background-color:#000000;filter:blur(4px)'
  );
  __AppendElement(page, box);
};
"
    .to_vec()
}

#[tokio::test]
async fn a_blurred_box_reaches_the_embedder_painter() {
    let group = group().await;
    let mut view = group
        .create_lynx_view(
            64.0,
            64.0,
            1.0,
            |_reports| Rc::new(FetcherDouble::card(blurred_page()).resolving_to(SCRIPT_URL)),
            Vec::new(),
            ViewSources::new("app:///", SCRIPT_URL, SCREEN, "en-US"),
        )
        .expect("the view is built");
    let mut painter = Painter::new(DrawTarget::Offscreen, 64.0, 64.0, 1.0)
        .await
        .expect("an offscreen painter is built");
    painter.attach(&view).expect("the page takes a painter");
    wait_for_script(&mut view).expect("the entry module boots");
    tick(&mut painter, true).expect("the blurred frame renders");

    let shot = painter.capture().expect("the committed frame");
    let luma = |x: usize, y: usize| i32::from(shot.pixels[(y * 64 + x) * 4]);

    assert!(luma(32, 32) < 8, "the box keeps its ink ({})", luma(32, 32));
    assert!(
        luma(50, 32) < 250,
        "and ink reaches 6 px past the border box ({})",
        luma(50, 32),
    );
    assert!(
        luma(61, 32) >= 253,
        "but effectively none past 4 sigma ({})",
        luma(61, 32),
    );
    for distance in 0..=9_usize {
        let inside = luma(43 - distance, 32);
        let outside = luma(44 + distance, 32);
        assert!(
            (inside + outside - 255).abs() <= 8,
            "the fall-off is symmetric about the border at {distance} px \
             ({inside} inside, {outside} outside)",
        );
    }
}

fn backdrop_page() -> Vec<u8> {
    br"
globalThis.renderPage = function renderPage() {
  const page = __CreatePage('card', 0);
  __SetInlineStyles(page, 'width:64px;height:64px;background-color:#ffffff;position:relative');
  const half = __CreateView(0);
  __SetInlineStyles(
    half,
    'position:absolute;left:0px;top:0px;width:32px;height:64px;background-color:#000000'
  );
  __AppendElement(page, half);
  const box = __CreateView(0);
  __SetInlineStyles(
    box,
    'position:absolute;left:16px;top:16px;width:32px;height:32px;' +
      'backdrop-filter:blur(5px)'
  );
  __AppendElement(page, box);
};
"
    .to_vec()
}

#[tokio::test]
async fn a_backdrop_filtered_box_reaches_the_embedder_painter() {
    let group = group().await;
    let mut view = group
        .create_lynx_view(
            64.0,
            64.0,
            1.0,
            |_reports| Rc::new(FetcherDouble::card(backdrop_page()).resolving_to(SCRIPT_URL)),
            Vec::new(),
            ViewSources::new("app:///", SCRIPT_URL, SCREEN, "en-US"),
        )
        .expect("the view is built");
    let mut painter = Painter::new(DrawTarget::Offscreen, 64.0, 64.0, 1.0)
        .await
        .expect("an offscreen painter is built");
    painter.attach(&view).expect("the page takes a painter");
    wait_for_script(&mut view).expect("the entry module boots");
    tick(&mut painter, true).expect("the filtered frame renders");

    let shot = painter.capture().expect("the committed frame");
    let luma = |x: usize, y: usize| i32::from(shot.pixels[(y * 64 + x) * 4]);

    let profile: Vec<i32> = (18..=46).map(|x| luma(x, 32)).collect();
    for pair in profile.windows(2) {
        assert!(
            pair[1] >= pair[0] - 1,
            "the filtered seam must rise monotonically: {profile:?}",
        );
    }
    assert!(
        profile[0] < 60 && profile[profile.len() - 1] > 180,
        "and span the seam ({} to {})",
        profile[0],
        profile[profile.len() - 1],
    );
    assert!(
        luma(30, 8) < 8 && luma(34, 8) >= 250,
        "while above the box it is still a step ({}, {})",
        luma(30, 8),
        luma(34, 8),
    );
}

fn blur_view_page() -> Vec<u8> {
    br"
globalThis.renderPage = function renderPage() {
  const page = __CreatePage('card', 0);
  __SetInlineStyles(page, 'width:128px;height:240px;position:relative');
  const half = __CreateView(0);
  __SetInlineStyles(
    half,
    'position:absolute;left:0px;top:0px;width:64px;height:240px;background-color:#000000'
  );
  __AppendElement(page, half);
  function frost(parent, style) {
    const view = __CreateElement('blur-view', 0);
    __SetAttribute(view, 'blur-radius', '5');
    __SetInlineStyles(view, 'position:absolute;left:24px;width:80px;height:56px;' + style);
    __AppendElement(parent, view);
  }
  frost(page, 'top:12px');
  frost(page, 'top:92px;transform:rotate(8deg)');
  const wrapper = __CreateView(0);
  __SetInlineStyles(
    wrapper,
    'position:absolute;left:0px;top:172px;width:128px;height:56px;transform:rotate(-8deg)'
  );
  __AppendElement(page, wrapper);
  frost(wrapper, 'top:0px');
};
"
    .to_vec()
}

#[tokio::test]
async fn a_blur_view_over_the_bare_canvas_blurs_an_edge_on_both_sides() {
    let group = group().await;
    let mut view = group
        .create_lynx_view(
            128.0,
            240.0,
            1.0,
            |_reports| Rc::new(FetcherDouble::card(blur_view_page()).resolving_to(SCRIPT_URL)),
            Vec::new(),
            ViewSources::new("app:///", SCRIPT_URL, SCREEN, "en-US"),
        )
        .expect("the view is built");
    let mut painter = Painter::new(DrawTarget::Offscreen, 128.0, 240.0, 1.0)
        .await
        .expect("an offscreen painter is built");
    painter.attach(&view).expect("the page takes a painter");
    wait_for_script(&mut view).expect("the entry module boots");
    tick(&mut painter, true).expect("the filtered frame renders");

    let shot = painter.capture().expect("the committed frame");
    let luma = |x: usize, y: usize| i32::from(shot.pixels[(y * 128 + x) * 4]);

    for (label, row) in [
        ("plain", 40),
        ("rotated", 120),
        ("in a rotated wrapper", 200),
    ] {
        for distance in 0..=9_usize {
            let dark = luma(63 - distance, row);
            let light = luma(64 + distance, row);
            assert!(
                (dark + light - 255).abs() <= 8,
                "{label}: the blurred edge is symmetric at {distance} px \
                 ({dark} dark, {light} light)",
            );
        }
        assert!(
            luma(61, row) > 40,
            "{label}: the black side of the edge is lightened, not left sharp ({})",
            luma(61, row),
        );
    }
    assert!(
        luma(60, 4) < 8 && luma(68, 4) >= 250,
        "while above the views the edge is still a step ({}, {})",
        luma(60, 4),
        luma(68, 4),
    );
}

fn flushing_page() -> Vec<u8> {
    br"
globalThis.renderPage = function renderPage() {
  const page = __CreatePage('card', 0);
  const box = __CreateView(0);
  __AppendElement(page, box);
  globalThis.box = box;
};
globalThis.updatePage = function updatePage(data) {
  __SetAttribute(globalThis.box, 'data-value', String(data.value));
  __FlushElementTree();
  console.log('flushed ' + data.value);
};
"
    .to_vec()
}

#[tokio::test]
async fn a_flush_after_the_painter_detached_does_not_park() {
    let group = group().await;
    let mut view = group
        .create_lynx_view(
            32.0,
            24.0,
            1.0,
            |_reports| Rc::new(FetcherDouble::card(flushing_page()).resolving_to(SCRIPT_URL)),
            Vec::new(),
            ViewSources::new("app:///", SCRIPT_URL, SCREEN, "en-US"),
        )
        .expect("the view is built");
    let mut painter = offscreen_painter().await;
    painter.attach(&view).expect("a fresh view takes a painter");
    wait_for_script(&mut view).expect("the entry module boots");

    painter.detach();
    assert!(!painter.is_attached());
    view.update_data(r#"{"value":7}"#.to_owned(), String::new())
        .expect("a booted view accepts a data update");

    let deadline = Instant::now() + Duration::from_secs(30);
    let mut flushed = None;
    while flushed.is_none() {
        for event in view.pump() {
            match event {
                bobcat_core::EngineEvent::ConsoleMessage { message, .. } => {
                    flushed = Some(message);
                }
                bobcat_core::EngineEvent::ScriptRunError(error)
                | bobcat_core::EngineEvent::Panicked(error) => {
                    panic!("the update failed: {}", error.message)
                }
                _ => {}
            }
        }
        assert!(
            Instant::now() < deadline,
            "the flush after the detach never returned"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(flushed.as_deref(), Some("flushed 7"));
}

const GREEN: [u8; 4] = [0, 255, 0, 255];

fn after_boot_page() -> Vec<u8> {
    br"
globalThis.renderPage = function renderPage() {
  const page = __CreatePage('card', 0);
  const box = __CreateView(0);
  __SetInlineStyles(box, 'width:32px;height:24px;background-color:#ff0000');
  __AppendElement(page, box);
  setTimeout(() => {
    setTimeout(() => {
      console.log('after boot');
      __SetInlineStyles(box, 'width:32px;height:24px;background-color:#00ff00');
      __FlushElementTree();
    }, 0);
  }, 0);
};
"
    .to_vec()
}

#[tokio::test]
async fn wait_for_script_answers_with_what_the_turn_that_ended_boot_reported_after_it() {
    let group = group().await;
    let mut view = group
        .create_lynx_view(
            32.0,
            24.0,
            1.0,
            |_reports| Rc::new(FetcherDouble::card(after_boot_page()).resolving_to(SCRIPT_URL)),
            Vec::new(),
            ViewSources::new("app:///", SCRIPT_URL, SCREEN, "en-US"),
        )
        .expect("the view is built");
    let mut painter = offscreen_painter().await;
    painter.attach(&view).expect("a fresh view takes a painter");

    let deadline = Instant::now() + Duration::from_secs(30);
    while !(tick(&mut painter, true).expect("a frame the view committed")
        && top_left(&mut painter) == GREEN)
    {
        assert!(Instant::now() < deadline, "the inner timer never ran");
        std::thread::sleep(Duration::from_millis(1));
    }

    let after_boot = wait_for_script(&mut view).expect("the entry module boots");
    let logged: Vec<String> = after_boot
        .into_iter()
        .filter_map(|event| match event {
            bobcat_core::EngineEvent::ConsoleMessage { message, .. } => Some(message),
            _ => None,
        })
        .collect();
    assert_eq!(logged, ["after boot"]);
}
