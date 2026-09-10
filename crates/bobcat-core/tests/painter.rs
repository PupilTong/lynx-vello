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
use support::{FetcherDouble, wait_for_script};

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

/// A scroller over two 100px rows in a 100x100 viewport — the shape that
/// makes a frame carry a composite plan, so its rows are baked into retained
/// plane textures instead of drawn straight into the target.
fn scroller(top: &str, bottom: &str) -> Vec<u8> {
    format!(
        r"
globalThis.renderPage = function renderPage() {{
  const page = __CreatePage('card', 0);
  const view = __CreateView(0);
  const first = __CreateView(0);
  const second = __CreateView(0);
  __AppendElement(page, view);
  __AppendElement(view, first);
  __AppendElement(view, second);
  __SetInlineStyles(view,
    'display:flex;flex-direction:column;overflow:scroll;width:100px;height:100px');
  __SetInlineStyles(first,
    'flex-shrink:0;width:100px;height:100px;background-color:{top}');
  __SetInlineStyles(second,
    'flex-shrink:0;width:100px;height:100px;background-color:{bottom}');
}};
"
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
            |_reports| Rc::new(FetcherDouble::new(page("#ff0000")).resolving_to(SCRIPT_URL)),
            ViewSources::new(SCRIPT_URL),
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
            |_reports| Rc::new(FetcherDouble::new(page("#ff0000")).resolving_to(SCRIPT_URL)),
            ViewSources::new(SCRIPT_URL),
        )
        .expect("the view is built");
    let mut first = offscreen_painter().await;
    first.attach(&view).expect("a fresh view takes a painter");
    wait_for_script(&mut view).expect("the entry module boots");
    first.tick(true).expect("the first frame");
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
    second.tick(true).expect("the same page, drawn again");
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
            |_reports| Rc::new(FetcherDouble::new(page("#ff0000")).resolving_to(SCRIPT_URL)),
            ViewSources::new(SCRIPT_URL),
        )
        .expect("the view is built");
    let mut painter = offscreen_painter().await;
    painter.attach(&view).expect("a fresh view takes a painter");
    wait_for_script(&mut view).expect("the entry module boots");
    painter.tick(true).expect("the frame the view committed");
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
        painter
            .tick(true)
            .expect("the retained frame still renders"),
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
                    FetcherDouble::new(image_page())
                        .resolving_to(SCRIPT_URL)
                        .with_images(Rc::clone(&images))
                        .serving(sink),
                )
            },
            ViewSources::new(SCRIPT_URL),
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
        let _ = painter.tick(true);
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
        painter
            .tick(true)
            .expect("the retained frame still renders"),
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
            |_reports| Rc::new(FetcherDouble::new(page("#ff0000")).resolving_to(SCRIPT_URL)),
            ViewSources::new(SCRIPT_URL),
        )
        .expect("the first view is built");
    painter.attach(&first).expect("the first page takes it");
    wait_for_script(&mut first).expect("the first entry boots");
    painter.tick(true).expect("the first page's frame");
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
            |_reports| Rc::new(FetcherDouble::new(page("#0000ff")).resolving_to(SCRIPT_URL)),
            ViewSources::new(SCRIPT_URL),
        )
        .expect("the second view is built");
    painter.attach(&second).expect("the second page takes it");
    assert!(
        painter.capture().is_err(),
        "nothing of the new page has been rendered yet"
    );
    wait_for_script(&mut second).expect("the second entry boots");
    assert!(
        painter.tick(false).expect("the second page's frame"),
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
            |_reports| Rc::new(FetcherDouble::new(page("#ff0000")).resolving_to(SCRIPT_URL)),
            ViewSources::new(SCRIPT_URL),
        )
        .expect("the first view is built");
    painter.attach(&first).expect("the first page takes it");
    wait_for_script(&mut first).expect("the first entry boots");
    painter.tick(true).expect("the first page's frame");
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
            |_reports| Rc::new(FetcherDouble::new(page("#0000ff")).resolving_to(SCRIPT_URL)),
            ViewSources::new(SCRIPT_URL),
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
    painter.tick(true).expect("the second page's frame");
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
            |_reports| Rc::new(FetcherDouble::new(page("#ff0000")).resolving_to(SCRIPT_URL)),
            ViewSources::new(SCRIPT_URL),
        )
        .expect("the first view is built");
    painter.attach(&first).expect("the first page takes it");
    wait_for_script(&mut first).expect("the first entry boots");
    painter.tick(true).expect("the first page's frame");
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
            |_reports| Rc::new(FetcherDouble::new(page("#0000ff")).resolving_to(SCRIPT_URL)),
            ViewSources::new(SCRIPT_URL),
        )
        .expect("the second view is built");
    painter
        .attach(&second)
        .expect("a painter that already noticed takes the next page");
    wait_for_script(&mut second).expect("the second entry boots");
    assert!(
        painter.tick(false).expect("the second page's frame"),
        "the attach dropped the first page's compose key"
    );
    assert_eq!(
        top_left(&mut painter),
        BLUE,
        "and the frame it rendered is the second page's"
    );
}

/// A page whose frame is composed out of retained plane textures rather than
/// drawn straight into the target, which is the other half of what a change of
/// page has to reset.
///
/// The two pages have the identical plan and therefore the identical commit
/// id; only the colours baked into the planes differ. A bank still holding the
/// first page's id would answer the second page's first composite with the
/// first page's planes, and the swap is what makes that visible.
#[tokio::test]
async fn a_re_attached_painter_bakes_the_new_pages_planes() {
    let mut painter = Painter::new(DrawTarget::Offscreen, 100.0, 100.0, 1.0)
        .await
        .expect("an offscreen painter is built");

    let first_group = group().await;
    let mut first = first_group
        .create_lynx_view(
            100.0,
            100.0,
            1.0,
            |_reports| {
                Rc::new(FetcherDouble::new(scroller("#ff0000", "#0000ff")).resolving_to(SCRIPT_URL))
            },
            ViewSources::new(SCRIPT_URL),
        )
        .expect("the first view is built");
    painter.attach(&first).expect("the first page takes it");
    wait_for_script(&mut first).expect("the first entry boots");
    painter.tick(true).expect("the first page's frame");
    assert_eq!(top_left(&mut painter), RED, "the first page's top row");

    painter.detach();
    drop(first);
    drop(first_group);

    let second_group = group().await;
    let mut second = second_group
        .create_lynx_view(
            100.0,
            100.0,
            1.0,
            |_reports| {
                Rc::new(FetcherDouble::new(scroller("#0000ff", "#ff0000")).resolving_to(SCRIPT_URL))
            },
            ViewSources::new(SCRIPT_URL),
        )
        .expect("the second view is built");
    painter.attach(&second).expect("the second page takes it");
    wait_for_script(&mut second).expect("the second entry boots");
    painter.tick(true).expect("the second page's frame");
    assert_eq!(
        top_left(&mut painter),
        BLUE,
        "the planes were baked from the page the painter is now attached to"
    );
}
