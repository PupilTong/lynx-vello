#![cfg(feature = "quickjs")]

//! Public-facade coverage for the offscreen render path.
//!
//! CSS, document, image-store, and tree mutation are deliberately absent from
//! this integration-test boundary. Element construction happens only inside
//! the fetched Element-PAPI script.

mod support;

use std::sync::Arc;
use std::time::{Duration, Instant};

use bobcat_core::resource::ResourceFetcher;
use bobcat_core::{
    EngineEvent, LynxViewError, OffscreenLynxView, PageConfig, ScriptRunError,
    quickjs_engine_factory,
};
use support::FetcherDouble;

const SCRIPT_URL: &str = "app:///main.js";
const MAIN_THREAD_SCRIPT: &str = r"
globalThis.renderPage = function renderPage() {
  const page = __CreatePage('card', 0);
  __AppendElement(page, __CreateView(0));
};
";

fn view(source: &[u8]) -> OffscreenLynxView {
    let resources: Arc<dyn ResourceFetcher> =
        Arc::new(FetcherDouble::new(source.to_vec()).resolving_to(SCRIPT_URL));
    OffscreenLynxView::new(
        PageConfig::default(),
        resources,
        quickjs_engine_factory(),
        393.0,
        727.0,
        1.0,
    )
    .expect("view")
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
