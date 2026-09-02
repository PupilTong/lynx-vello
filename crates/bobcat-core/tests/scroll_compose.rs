//! Scrolling from where an embedder stands: a wheel moves the pixels.
//!
//! Two laws, both invisible from outside without a GPU. Inside the encode
//! window a scroll is pure composition — the frame the boot committed is
//! recomposed at the new offset and nothing recommits. Past half the
//! window's headroom the engine asks the main thread for a refill commit,
//! and the offscreen tick — a synchronization point by design — waits for
//! it, so the capture after a deep scroll shows the recommitted frame at
//! its published offsets. Either way the same pixels move the same way;
//! which path produced them is the engine's business, asserted directly by
//! `bobcat_core::paint::event_loop_tests`.

mod support;

use std::sync::Arc;

use bobcat_core::{DrawTarget, LynxView, NoWakeup, ViewSources};
use dom::Point2D;
use dom::input::InputEvent;
use support::{FetcherDouble, wait_for_script};

const SCRIPT_URL: &str = "app:///main.js";

/// A scroller filling the 100x100 viewport over two 100px rows: red, then
/// blue. `max_offset` is 100, so the encode window is the whole range and a
/// 30px scroll stays inside half its headroom while a 70px one does not.
const TWO_ROW_SCRIPT: &str = r"
globalThis.renderPage = function renderPage() {
  const page = __CreatePage('card', 0);
  const view = __CreateView(0);
  const red = __CreateView(0);
  const blue = __CreateView(0);
  __AppendElement(page, view);
  __AppendElement(view, red);
  __AppendElement(view, blue);
  globalThis.held = [page, view, red, blue];
  __SetInlineStyles(view,
    'display:flex;flex-direction:column;overflow:scroll;width:100px;height:100px');
  __SetInlineStyles(red,
    'flex-shrink:0;width:100px;height:100px;background-color:#ff0000');
  __SetInlineStyles(blue,
    'flex-shrink:0;width:100px;height:100px;background-color:#0000ff');
  __FlushElementTree();
};
";

async fn booted() -> LynxView<Arc<FetcherDouble>> {
    let fetcher =
        Arc::new(FetcherDouble::new(TWO_ROW_SCRIPT.as_bytes().to_vec()).resolving_to(SCRIPT_URL));
    let mut view = LynxView::new(
        Arc::new(NoWakeup),
        100.0,
        100.0,
        1.0,
        DrawTarget::Offscreen,
        |_sink| fetcher,
        ViewSources::new(SCRIPT_URL),
    )
    .await
    .expect("view construction fetches and boots the entry script");
    wait_for_script(&mut view).expect("script execution");
    view
}

/// The captured pixel at `(x, y)`, as RGBA.
fn pixel_at(view: &mut LynxView<Arc<FetcherDouble>>, x: usize, y: usize) -> [u8; 4] {
    let shot = view.capture().expect("capture the frame");
    let width = usize::try_from(shot.size.width).expect("the frame is addressable");
    let start = (y * width + x) * 4;
    shot.pixels[start..start + 4]
        .try_into()
        .expect("a whole pixel")
}

const RED: [u8; 4] = [255, 0, 0, 255];
const BLUE: [u8; 4] = [0, 0, 255, 255];

fn wheel(view: &mut LynxView<Arc<FetcherDouble>>, delta_y: f32) {
    view.dispatch_input(InputEvent::wheel(
        Point2D::new(50.0, 50.0),
        dom::Vector2D::new(0.0, delta_y),
    ));
}

#[tokio::test]
async fn a_wheel_scroll_moves_the_pixels_through_both_compose_paths() {
    let mut view = booted().await;
    view.tick(true).expect("the boot frame renders");
    assert_eq!(pixel_at(&mut view, 50, 50), RED, "unscrolled, the red row");
    assert_eq!(pixel_at(&mut view, 50, 85), RED, "still the red row");

    // 30px: inside half the window's headroom, so this frame is the boot
    // commit recomposed at the intent offset — nothing recommitted.
    wheel(&mut view, 30.0);
    assert!(
        view.tick(false).expect("the scrolled frame renders"),
        "a scroll must change the compose key even with no new commit"
    );
    assert_eq!(
        pixel_at(&mut view, 50, 60),
        RED,
        "content y=90 is still the red row"
    );
    assert_eq!(
        pixel_at(&mut view, 50, 85),
        BLUE,
        "content y=115 is the blue row: composition applied the offset"
    );

    // 40px more reaches 70: past half the headroom, so a refill commit is
    // requested, and the synchronizing tick waits for the round that
    // applies it — this capture is the recommitted frame at its published
    // offsets.
    wheel(&mut view, 40.0);
    assert!(view.tick(false).expect("the refilled frame renders"));
    assert_eq!(
        pixel_at(&mut view, 50, 20),
        RED,
        "content y=90 is still the red row"
    );
    assert_eq!(
        pixel_at(&mut view, 50, 45),
        BLUE,
        "content y=115 is the blue row, now through the refill commit"
    );
}
