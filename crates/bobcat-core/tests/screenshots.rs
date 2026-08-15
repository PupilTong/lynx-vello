#![cfg(feature = "quickjs")]

//! Public-facade coverage for the offscreen render path.
//!
//! CSS, document, image-store, and tree mutation are deliberately absent from
//! this integration-test boundary. Element construction happens only inside
//! the fetched Element-PAPI script.

mod support;

use std::future::Future;
use std::sync::Arc;
use std::task::Poll;
use std::time::{Duration, Instant};

use bobcat_core::image::{AlphaType, DecodedImage, ImageFormat};
use bobcat_core::resource::{
    CancellationToken, ResourceErrorKind, ResourceErrorPhase, ResourceFetcher,
};
use bobcat_core::{
    EngineEvent, LynxViewError, OffscreenLynxView, PageConfig, ScriptRunError,
    quickjs_engine_factory,
};
use flashbulb::{Image, Screenshots};
use support::FetcherDouble;

const SCRIPT_URL: &str = "app:///main.js";
const MAIN_THREAD_SCRIPT: &str = r"
globalThis.renderPage = function renderPage() {
  const page = __CreatePage('card', 0);
  __AppendElement(page, __CreateView(0));
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

fn view(source: &[u8]) -> OffscreenLynxView {
    let resources: Arc<dyn ResourceFetcher> =
        Arc::new(FetcherDouble::new(source.to_vec()).resolving_to(SCRIPT_URL));
    OffscreenLynxView::new(
        PageConfig::default(),
        resources,
        quickjs_engine_factory(),
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

fn checker_image() -> DecodedImage {
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
    DecodedImage::from_rgba8(4, 4, AlphaType::Straight, rgba, ImageFormat::Png)
        .expect("valid checker image")
}

#[tokio::test]
async fn a_pre_cancelled_entry_request_returns_a_typed_error() {
    let mut view = view(MAIN_THREAD_SCRIPT.as_bytes());
    let cancellation = CancellationToken::new();
    cancellation.cancel();

    let error = view
        .execute_script_with_cancellation(SCRIPT_URL, cancellation)
        .await
        .expect_err("pre-cancelled request");
    let LynxViewError::Resource(error) = error else {
        panic!("expected resource cancellation, got {error:?}");
    };
    assert_eq!(error.kind, ResourceErrorKind::Cancelled);
    assert_eq!(error.phase, ResourceErrorPhase::Resolve);
    assert_eq!(error.locator.as_deref(), Some(SCRIPT_URL));
    assert!(error.request_id.is_some());
}

#[tokio::test]
async fn dropping_entry_request_cancels_the_token_seen_by_the_host() {
    let resources = Arc::new(
        FetcherDouble::new(MAIN_THREAD_SCRIPT.as_bytes().to_vec())
            .resolving_to(SCRIPT_URL)
            .with_hung_resolve(),
    );
    let mut view = OffscreenLynxView::new(
        PageConfig::default(),
        resources.clone(),
        quickjs_engine_factory(),
        Arc::new(|| {}),
        393.0,
        727.0,
        1.0,
    )
    .expect("view");
    let external = CancellationToken::new();
    let mut execution =
        Box::pin(view.execute_script_with_cancellation(SCRIPT_URL, external.clone()));
    std::future::poll_fn(|context| match execution.as_mut().poll(context) {
        Poll::Pending => Poll::Ready(()),
        Poll::Ready(result) => panic!("hung resolver unexpectedly completed: {result:?}"),
    })
    .await;
    let host_token = resources
        .request_cancellation()
        .expect("resource provider observed request context");

    drop(execution);

    assert!(external.is_cancelled());
    assert!(host_token.is_cancelled());
}

async fn wait_for_script(view: &mut OffscreenLynxView) -> Result<(), ScriptRunError> {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if let Some(result) = view.pump().into_iter().find_map(|event| match event {
            EngineEvent::ScriptFinished(result) => Some(result),
            _ => None,
        }) {
            return result;
        }
        assert!(Instant::now() < deadline, "script thread did not finish");
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
}

#[tokio::test]
async fn fetched_script_reaches_the_offscreen_draw_target() {
    let mut view = view(MAIN_THREAD_SCRIPT.as_bytes());
    view.attach_offscreen()
        .expect("GPU initialization for the offscreen target");
    view.execute_script(SCRIPT_URL)
        .await
        .expect("fetch and start script");
    wait_for_script(&mut view).await.expect("script execution");

    let shot = view.capture().expect("capture the committed page");
    assert_eq!(shot.size.width, 393);
    assert_eq!(shot.size.height, 727);
    assert_eq!(
        shot.pixels.len(),
        shot.size.width as usize * shot.size.height as usize * 4
    );
}

#[tokio::test]
async fn decoded_image_url_reaches_the_private_painter() {
    let mut view = view(IMAGE_SCRIPT.as_bytes());
    view.register_image_url(IMAGE_URL, &checker_image())
        .expect("available private image registry");
    view.attach_offscreen()
        .expect("GPU initialization for the offscreen target");
    view.execute_script(SCRIPT_URL)
        .await
        .expect("fetch and start script");
    wait_for_script(&mut view).await.expect("script execution");

    let shot = view.capture().expect("capture the committed image");
    let image = Image::from_rgba8(shot.size.width, shot.size.height, shot.pixels)
        .expect("captured RGBA image");
    screenshots().assert_matches(&["retained-image-store"], &image);
}

#[tokio::test]
async fn stylesheet_loading_is_a_closed_url_api_for_now() {
    let mut view = view(MAIN_THREAD_SCRIPT.as_bytes());
    let error = view
        .load_style_sheet("app:///author.css")
        .expect_err("author stylesheets are deliberately not implemented yet");
    assert!(matches!(
        error,
        LynxViewError::StyleSheetUnsupported { ref url } if url == "app:///author.css"
    ));
}
