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

/// The screen these tests' views report, as a host with no screen to measure
/// names it. None of them reads `SystemInfo`.
const SCREEN: bobcat_core::ScreenMetrics =
    bobcat_core::ScreenMetrics::for_viewport(32.0, 24.0, 1.0);

const SCRIPT_URL: &str = "app:///main.js";
const IMAGE_URL: &str = "https://example.test/pixel.png";

/// A page filling the 32x24 viewport with one flat colour, so which document
/// a painter is showing is one pixel away.
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

/// The same page, filled by a 1x1 image the host serves rather than by a
/// colour: the top-left pixel then answers what the painter read out of that
/// view's store, not what the frame encoded by itself.
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

/// A view has at most one interactive painter. The second is refused rather
/// than silently taking the first's place, because two painters drawing one
/// document would each be routing input against a frame the other consumed.
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
            ViewSources::new("app:///", SCRIPT_URL, SCREEN),
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

/// Detaching releases the view, and a fresh painter then shows the same
/// committed frame: the document is the view's, and a painter is only ever
/// looking at it.
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
            ViewSources::new("app:///", SCRIPT_URL, SCREEN),
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

/// A view dropped under an attached painter is not an error anywhere. The
/// painter notices on its next turn, keeps what it drew, and goes on being
/// capturable — which is what lets a host tear a page down while its pixels
/// stay on screen.
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
            ViewSources::new("app:///", SCRIPT_URL, SCREEN),
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

/// The same law at the one place it costs something: a frame's pixels are the
/// view's, read out of its store, and the store goes when the view does. What
/// the painter adopted it adopted whole — frame and bitmaps together — so the
/// image is still there to draw once nothing can be read any more.
#[tokio::test]
async fn a_dropped_views_last_frame_keeps_the_image_pixels_it_read() {
    let images = Rc::new(flashbulb::TestImages::new());
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
            ViewSources::new("app:///", SCRIPT_URL, SCREEN),
        )
        .expect("the view is built");
    let mut painter = offscreen_painter().await;
    painter.attach(&view).expect("a fresh view takes a painter");
    wait_for_script(&mut view).expect("the entry module boots");

    // The store's retain log is written by the painter's own resolve pass, so
    // a non-empty working set is a committed frame that draws the image and a
    // painter that has read its pixels.
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

/// The browser's shape: one painter over one canvas, pointed at a second view
/// when the page changes.
///
/// The reset is what this pins. Commit ids restart at one per document, so a
/// painter that kept the previous page's retained key would answer the new
/// page's first frame with the old page's pixels.
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
            ViewSources::new("app:///", SCRIPT_URL, SCREEN),
        )
        .expect("the first view is built");
    painter.attach(&first).expect("the first page takes it");
    wait_for_script(&mut first).expect("the first entry boots");
    tick(&mut painter, true).expect("the first page's frame");
    assert_eq!(top_left(&mut painter), RED);

    // A page is a group of its own, exactly as the browser embedder builds
    // one: the entry module is registered on the group's script runtime, so a
    // second page cannot reuse the first's.
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
            ViewSources::new("app:///", SCRIPT_URL, SCREEN),
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

/// The same change of page without the `detach()` in the middle: a host that
/// drops a view and attaches the next one straight away.
///
/// A link whose view has gone is not an attachment, so this is accepted — and
/// what the painter kept of the old page is dropped by the attach itself,
/// which is the only thing on this path that clears it.
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
            ViewSources::new("app:///", SCRIPT_URL, SCREEN),
        )
        .expect("the first view is built");
    painter.attach(&first).expect("the first page takes it");
    wait_for_script(&mut first).expect("the first entry boots");
    tick(&mut painter, true).expect("the first page's frame");
    assert_eq!(top_left(&mut painter), RED);

    // No `detach`, and no turn in between either: the painter has not yet
    // taken one on which it could have noticed.
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
            ViewSources::new("app:///", SCRIPT_URL, SCREEN),
        )
        .expect("the second view is built");
    painter
        .attach(&second)
        .expect("a painter whose view has gone is not an attached one");
    assert!(
        painter.capture().is_err(),
        "nothing of the new page has been rendered yet"
    );
    wait_for_script(&mut second).expect("the second entry boots");
    tick(&mut painter, true).expect("the second page's frame");
    assert_eq!(
        top_left(&mut painter),
        BLUE,
        "and it drew the page it took, not the one it had kept"
    );
}

/// The same again with one turn in between, which is the path where the
/// auto-detach has already run.
///
/// That one releases the link and keeps everything composed from it, so
/// `attach`'s own reset is the only thing that drops the old page's compose
/// key — and without it the new page's first frame would look already drawn.
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
            ViewSources::new("app:///", SCRIPT_URL, SCREEN),
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
            ViewSources::new("app:///", SCRIPT_URL, SCREEN),
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

/// A 64x64 page holding one 24x24 blurred black box.
///
/// Sigma 4 against a 24 px box leaves the centre saturated (the box is six
/// sigma across), so the fall-off across a border is the clean one-dimensional
/// profile the assertions below read.
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

/// `filter: blur()` reaches the embedder's own painter: a blurred box's ink
/// leaves its border box, and the fall-off is symmetric about the border.
///
/// The bake is a pre-step of `compose_and_render`, so this is the one test
/// that proves an embedder gets it — `dom`'s own pixel tests drive the
/// headless renderer directly and would pass with the painter never calling
/// `prepare_filters` at all.
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
            ViewSources::new("app:///", SCRIPT_URL, SCREEN),
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

    // The box is (20, 20)-(44, 44); its centre is (32, 32) and its right
    // border is x = 44.
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
    // The border is at x = 44.0, so pixels 43 and 44 are the symmetric pair.
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

/// A 64x64 page: an opaque white ground, a black left half, and one 32x32
/// box over the seam carrying `backdrop-filter: blur(5px)`.
///
/// The page paints its ground itself; [`blur_view_page`] leaves it to the
/// painter's canvas.
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

/// `backdrop-filter` reaches the embedder's own painter: the black/white seam
/// is a gradient inside the element's border box and a step beside it.
///
/// Same argument as the blur test above — the bake is a pre-step of
/// `compose_and_render`, and `dom`'s own pixel tests drive the headless
/// renderer directly, so this is the one test that proves an embedder gets a
/// backdrop at all.
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
            ViewSources::new("app:///", SCRIPT_URL, SCREEN),
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

    // The box is (16, 16)-(48, 48) and the seam is x = 32. Inside the box the
    // seam rises monotonically; above the box it is one step.
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

/// A 128x240 page that paints no ground: a black left half over the
/// painter's bare canvas, and three 80x56 `<blur-view blur-radius="5">`s
/// centred on the seam — one plain, one rotated by its own `transform`, one
/// inside a wrapper that carries the rotation.
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

/// A `<blur-view>` over a coloured box's edge on the painter's bare canvas
/// blurs that edge on both sides, with or without a transform on the view or
/// on an ancestor of it.
///
/// The canvas is the colour the painter clears its target to, which no page
/// content draws; filter-effects-2 puts it in the document root's Backdrop
/// Root Image, so the view's bake has to render over it. Without it the
/// blurred copy of the black half fades to transparency toward the canvas
/// and the half's own sharp edge shows through, which reads here as a profile
/// that stays black up to the seam.
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
            ViewSources::new("app:///", SCRIPT_URL, SCREEN),
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

    // The seam is x = 64.0, so 63 and 64 are the symmetric pair, and each
    // view's centre row crosses it well inside the view.
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

/// A page that flushes and says so whenever the host updates its data, which
/// is how a test watches a flush return.
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

/// Only the *first* binding is waited for: once a painter has named this
/// view's metrics, detaching it does not make a later `__FlushElementTree`
/// park again.
///
/// Without that rule a view moved to the background would stop on its next
/// flush and, because a parked job holds the whole engine thread's queue,
/// take every other view in its group with it.
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
            ViewSources::new("app:///", SCRIPT_URL, SCREEN),
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

/// A red box that a timer armed from a timer the page armed logs `after
/// boot` for and turns green.
///
/// Nested because a realm runs the timers that are due before it reports the
/// boot it finished, so the outer one may run ahead of `ScriptFinished`; the
/// one it arms comes due in a later turn of the view's, after boot's end.
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

/// A host that took no turn while boot finished and the page went on gets
/// both in its next turn, and `wait_for_script` answers with what that turn
/// reported after boot's end rather than dropping it.
///
/// A test thread the OS leaves unscheduled under parallel load takes that
/// turn. Here the painter's turns stand in for the time it spent: they take
/// none of the view's, so the view's reports wait in its channel until the
/// green box says the inner timer has run, and only then does the wait take
/// the view's first turn.
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
            ViewSources::new("app:///", SCRIPT_URL, SCREEN),
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
