#![cfg(feature = "quickjs")]

mod support;

use std::sync::Arc;
use std::time::{Duration, Instant};

use bobcat_core::resource::ResourceFetcher;
use bobcat_core::{
    EngineError, EngineEvent, LynxView, LynxViewError, NoWindow, PageConfig, ScriptRunError,
    quickjs_engine_factory,
};
use support::FetcherDouble;

async fn run(source: &str, resolved_url: &str) -> Result<(), ScriptRunError> {
    let resources: Arc<dyn ResourceFetcher> =
        Arc::new(FetcherDouble::new(source.as_bytes().to_vec()).resolving_to(resolved_url));
    let mut view = LynxView::<NoWindow>::new(
        PageConfig::default(),
        resources,
        quickjs_engine_factory(),
        393.0,
        727.0,
        1.0,
    )
    .expect("view");
    view.execute_script("main.js")
        .await
        .expect("fetch and start");

    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if let Some(result) = view.pump().into_iter().find_map(|event| match event {
            EngineEvent::ScriptFinished(result) => Some(result),
            _ => None,
        }) {
            return result;
        }
        assert!(Instant::now() < deadline, "script thread did not finish");
        std::thread::yield_now();
    }
}

#[tokio::test]
async fn public_view_boots_element_papi_without_exposing_the_tree() {
    run(
        r"
        globalThis.renderPage = function () {
          const page = __CreatePage('card', 0);
          const view = __CreateView(0);
          if (typeof view !== 'object' || __AppendElement(page, view) !== view) {
            throw new Error('Element PAPI contract failed');
          }
        };
        ",
        "app:///main.js",
    )
    .await
    .expect("main-thread boot");
}

#[tokio::test]
async fn resolved_script_url_is_preserved_in_errors() {
    let error = run("const = 1", "app:///broken.js")
        .await
        .expect_err("syntax error");
    let message = error.to_string();
    assert!(message.contains("app:///broken.js:"), "{message}");
}

#[tokio::test]
async fn script_bytes_are_strict_utf8_at_the_view_boundary() {
    let resources: Arc<dyn ResourceFetcher> =
        Arc::new(FetcherDouble::new(vec![0xff, 0xfe]).resolving_to("app:///invalid.js"));
    let mut view = LynxView::<NoWindow>::new(
        PageConfig::default(),
        resources,
        quickjs_engine_factory(),
        393.0,
        727.0,
        1.0,
    )
    .expect("view");

    let error = view
        .execute_script("invalid.js")
        .await
        .expect_err("invalid UTF-8 must not reach the VM");
    assert!(matches!(
        error,
        LynxViewError::InvalidScriptEncoding { ref url, .. } if url == "app:///invalid.js"
    ));
}

#[tokio::test]
async fn a_view_accepts_only_one_entry_script() {
    let resources: Arc<dyn ResourceFetcher> = Arc::new(
        FetcherDouble::new(
            b"globalThis.renderPage = function () { __CreatePage('card', 0); };".to_vec(),
        )
        .resolving_to("app:///main.js"),
    );
    let mut view = LynxView::<NoWindow>::new(
        PageConfig::default(),
        resources,
        quickjs_engine_factory(),
        393.0,
        727.0,
        1.0,
    )
    .expect("view");
    view.execute_script("main.js").await.expect("first script");

    let error = view
        .execute_script("main.js")
        .await
        .expect_err("a second entry script must be rejected");
    assert!(matches!(
        error,
        LynxViewError::Engine(EngineError::ScriptAlreadyStarted)
    ));
}
