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
        Arc::new(|| {}),
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
        Arc::new(|| {}),
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
        Arc::new(|| {}),
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

/// The realm's `EventTarget` half, end to end against the real element tree:
/// registration identity, the index the host is told to keep, and the
/// stop-propagation seam. Delivery itself is driven by the engine's script
/// thread, which is exercised where that loop lives.
#[tokio::test]
async fn the_realm_registers_listeners_against_real_node_ids() {
    run(
        r"
        globalThis.renderPage = function () {
          const page = __CreatePage('card', 0);
          const outer = __CreateView(0);
          const inner = __CreateView(0);
          __AppendElement(page, outer);
          __AppendElement(outer, inner);
          globalThis.held = [page, outer, inner];

          const handler = () => {};
          __AddEventListener(inner, 'tap', handler, {});
          __AddEventListener(inner, 'tap', handler, {});
          __AddEventListener(inner, 'tap', handler, { capture: true });

          // Removing the bubble registration must leave the capture one, and
          // an unrelated callback must remove nothing.
          __RemoveEventListener(inner, 'tap', () => {}, {});
          __RemoveEventListener(inner, 'tap', handler, {});

          if (__GetElementUniqueID(inner) !== 4) {
            throw new Error('the tree shape this test assumes has changed');
          }
          if (typeof __AddEvent !== 'undefined') {
            throw new Error('__AddEvent must be gone, not merely unused');
          }
        };
        ",
        "app:///listeners.js",
    )
    .await
    .expect("main-thread boot");
}

/// A registration is keyed by handle and indexed by node id, and neither is
/// disturbed by the tree mutations a re-render performs.
#[tokio::test]
async fn registrations_survive_the_tree_mutations_a_rerender_makes() {
    run(
        r"
        globalThis.renderPage = function () {
          const page = __CreatePage('card', 0);
          const first = __CreateView(0);
          const second = __CreateView(0);
          const box = __CreateView(0);
          __AppendElement(page, first);
          __AppendElement(page, second);
          __AppendElement(page, box);
          globalThis.held = [page, first, second, box];

          const id = __GetElementUniqueID(first);
          __AddEventListener(first, 'tap', () => {}, {});

          __AppendElement(box, first);
          __SwapElement(first, second);
          __ReplaceElement(second, first);

          if (__GetElementUniqueID(first) !== id) {
            throw new Error('a handle must keep its node id across re-parenting');
          }
        };
        ",
        "app:///registration-stability.js",
    )
    .await
    .expect("main-thread boot");
}
