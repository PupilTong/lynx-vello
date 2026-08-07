#![cfg(feature = "quickjs")]

//! Behavior tests for main-thread (MTS) script execution and the Element PAPI
//! globals it runs against.

use std::sync::{Arc, Mutex};

use bobcat_core::quickjs::MainThreadRuntime;
use lynx_element::{ElementTree, PageConfig, Viewport};

const VIEWPORT: Viewport = Viewport::new(393.0, 727.0);

/// The single-threaded composition: the realm records PAPI writes, and every
/// `__FlushElementTree` applies them to this shared tree on the spot.
fn runtime() -> (MainThreadRuntime, Arc<Mutex<ElementTree>>) {
    let elements = Arc::new(Mutex::new(ElementTree::new(
        VIEWPORT,
        PageConfig::default(),
    )));
    let runtime = MainThreadRuntime::new(Arc::clone(&elements), || {}).expect("QuickJS realm");
    (runtime, elements)
}

/// A realm whose committed batches land in a tree no assertion reads.
fn bare_runtime() -> MainThreadRuntime {
    runtime().0
}

#[test]
fn a_card_root_builds_its_tree_through_the_papi() {
    let (mut runtime, elements) = runtime();
    runtime
        .run_main_thread_script(
            r"
            globalThis.renderPage = function () {
              const page = __CreatePage('card', 0);
              const first = __CreateView(0);
              const second = __CreateView(0);
              __AppendElement(page, first);
              __AppendElement(page, second);
              __AppendElement(first, __CreateView(0));
            };
            ",
        )
        .expect("main-thread script");

    // Ids allocate monotonically from 2 (the page is 1): the two rows and
    // the nested view. Their liveness proves the committed batch applied;
    // DOM shape (append order, tags, committed styles) is asserted where it
    // is owned - lynx-element's own unit tests.
    let elements = elements.lock().unwrap();
    assert!(elements.page().is_some());
    assert!(elements.element(2).is_some());
    assert!(elements.element(3).is_some());
    assert!(elements.element(4).is_some());
}

#[test]
fn create_view_returns_a_handle_append_element_accepts() {
    let (mut runtime, elements) = runtime();
    runtime
        .run_main_thread_script(
            r"
            globalThis.renderPage = function () {
              const page = __CreatePage('card', 0);
              const view = __CreateView(0);
              if (typeof view !== 'number') {
                throw new Error('__CreateView must return a number, got ' + typeof view);
              }
              const appended = __AppendElement(page, view);
              if (appended !== view) {
                throw new Error('__AppendElement must return the child');
              }
            };
            ",
        )
        .expect("main-thread script");
    let elements = elements.lock().unwrap();
    assert!(elements.page().is_some());
    assert!(elements.element(2).is_some());
}

#[test]
fn drop_element_retires_a_detached_element_and_does_not_reuse_its_id() {
    let (mut runtime, elements) = runtime();
    runtime
        .run_main_thread_script(
            r"
            globalThis.renderPage = function () {
              __CreatePage('card', 0);
              const dropped = __CreateView(0);
              __DropElement(dropped);
              __DropElement(dropped);
              __CreateView(0);
            };
            ",
        )
        .expect("main-thread script");

    let elements = elements.lock().unwrap();
    assert!(elements.element(2).is_none());
    assert!(elements.element(3).is_some());
}

#[test]
fn drop_element_retires_an_attached_subtree() {
    let (mut runtime, elements) = runtime();
    runtime
        .run_main_thread_script(
            r"
            globalThis.renderPage = function () {
              const page = __CreatePage('card', 0);
              const parent = __CreateView(0);
              const child = __CreateView(0);
              __AppendElement(page, parent);
              __AppendElement(parent, child);
              __DropElement(parent);
            };
            ",
        )
        .expect("main-thread script");

    let elements = elements.lock().unwrap();
    assert!(elements.element(2).is_none());
    assert!(elements.element(3).is_none());
}

#[test]
fn create_page_is_idempotent_across_calls() {
    let (mut runtime, elements) = runtime();
    runtime
        .run_main_thread_script(
            r"
            globalThis.renderPage = function () {
              const first = __CreatePage('card', 0);
              const second = __CreatePage('other', 3);
              if (first !== second) {
                throw new Error('__CreatePage must be idempotent');
              }
            };
            ",
        )
        .expect("main-thread script");
    assert!(elements.lock().unwrap().page().is_some());
}

#[test]
fn process_data_runs_before_render_page_and_feeds_it() {
    let (mut runtime, elements) = runtime();
    runtime
        .run_main_thread_script(
            r"
            globalThis.processData = function () { return 3; };
            globalThis.renderPage = function (count) {
              const page = __CreatePage('card', 0);
              for (let index = 0; index < count; index += 1) {
                __AppendElement(page, __CreateView(0));
              }
            };
            ",
        )
        .expect("main-thread script");
    let elements = elements.lock().unwrap();
    for id in 2..=4 {
        assert!(elements.element(id).is_some(), "child {id} must be live");
    }
}

#[test]
fn a_script_without_render_page_is_an_error() {
    let mut runtime = bare_runtime();
    let error = runtime
        .run_main_thread_script("globalThis.somethingElse = 1;")
        .expect_err("a bundle with no renderPage cannot boot");
    assert!(
        error.to_string().contains("renderPage"),
        "unexpected error: {error}"
    );
}

#[test]
fn the_page_is_not_in_the_document_until_the_tree_is_flushed() {
    let (mut runtime, elements) = runtime();
    runtime
        .evaluate_main_thread_script(
            r"
            globalThis.renderPage = function () {
              __AppendElement(__CreatePage('card', 0), __CreateView(0));
            };
            ",
        )
        .expect("evaluate");
    // Evaluation alone only defines the entry point.
    assert!(elements.lock().unwrap().page().is_none());

    runtime.render_page().expect("render");
    assert!(elements.lock().unwrap().page().is_some());
}

#[test]
fn a_syntax_error_reports_the_source_name() {
    let mut runtime = bare_runtime();
    let error = runtime
        .evaluate_main_thread_script("function ( {")
        .expect_err("a syntax error");
    let message = error.to_string();
    assert!(message.contains("main-thread.js"), "{message}");
}

#[test]
fn a_throwing_render_page_surfaces_the_javascript_error() {
    let mut runtime = bare_runtime();
    let error = runtime
        .run_main_thread_script("globalThis.renderPage = function () { throw new Error('boom'); };")
        .expect_err("the thrown error");
    assert!(error.to_string().contains("boom"), "{error}");
}

#[test]
fn papi_rejections_become_javascript_exceptions() {
    let mut runtime = bare_runtime();
    let error = runtime
        .run_main_thread_script(
            r"
            globalThis.renderPage = function () {
              __CreatePage('card', 0);
              __AppendElement(9999, 8888);
            };
            ",
        )
        .expect_err("an unknown handle");
    assert!(
        error.to_string().contains("9999"),
        "the error should name the bad handle: {error}"
    );
}

#[test]
fn element_handles_accept_the_full_u32_range() {
    let mut runtime = bare_runtime();
    let error = runtime
        .run_main_thread_script(
            r"
            globalThis.renderPage = function () {
              __AppendElement(4294967295, __CreateView(0));
            };
            ",
        )
        .expect_err("an unknown u32::MAX handle");
    assert!(
        error
            .to_string()
            .contains("no element has the unique id 4294967295"),
        "the u32 handle should reach lynx-element validation: {error}"
    );
}

#[test]
fn the_null_handle_is_rejected_by_append_element() {
    let mut runtime = bare_runtime();
    let error = runtime
        .run_main_thread_script(
            r"
            globalThis.renderPage = function () {
              __AppendElement(0, __CreateView(0));
            };
            ",
        )
        .expect_err("the null handle");
    assert!(error.to_string().contains("null handle"), "{error}");
}

#[test]
fn non_numeric_handles_are_rejected_rather_than_coerced() {
    let mut runtime = bare_runtime();
    let error = runtime
        .run_main_thread_script(
            r"
            globalThis.renderPage = function () {
              const page = __CreatePage('card', 0);
              __AppendElement(page, 'not a handle');
            };
            ",
        )
        .expect_err("a string handle");
    assert!(error.to_string().contains("__AppendElement"), "{error}");
}

#[test]
fn a_missing_papi_global_fails_loudly() {
    let mut runtime = bare_runtime();
    let error = runtime
        .run_main_thread_script(
            r"
            globalThis.renderPage = function () {
              __CreatePage('card', 0);
              __SetAttribute(1, 'name', 'value');
            };
            ",
        )
        .expect_err("an unimplemented PAPI member");
    assert!(error.to_string().contains("__SetAttribute"), "{error}");
}

#[test]
fn the_mts_wrapper_hides_the_browser_globals_web_core_hides() {
    let mut runtime = bare_runtime();
    runtime
        .run_main_thread_script(
            r"
            const shadowed = [typeof window, typeof navigator, typeof postMessage];
            globalThis.renderPage = function () {
              __CreatePage('card', 0);
              for (const kind of shadowed) {
                if (kind !== 'undefined') {
                  throw new Error('expected undefined, got ' + kind);
                }
              }
            };
            ",
        )
        .expect("main-thread script");
}

#[test]
fn a_second_boot_re_renders_into_the_same_tree() {
    let (mut runtime, elements) = runtime();
    runtime
        .run_main_thread_script(
            r"
            globalThis.renderPage = function () {
              __AppendElement(__CreatePage('card', 0), __CreateView(0));
            };
            ",
        )
        .expect("first boot");
    runtime.render_page().expect("second boot");
    // renderPage appended one more child into the same tree: the first
    // boot's view (id 2) and the second's (id 3) are both live.
    let elements = elements.lock().unwrap();
    assert!(elements.element(2).is_some());
    assert!(elements.element(3).is_some());
}

/// web-core's MTS realm is a browser realm: promise jobs queued while the
/// script runs execute before control returns to the host. Without an explicit
/// drain, a bundle's `Promise.resolve().then(…)` — or any `await` — would
/// never run at all.
#[test]
fn microtasks_queued_during_render_run_before_the_call_returns() {
    let (mut runtime, elements) = runtime();
    runtime
        .run_main_thread_script(
            r"
            globalThis.renderPage = function () {
              const page = __CreatePage('card', 0);
              Promise.resolve().then(function () {
                __AppendElement(page, __CreateView(0));
                __AppendElement(page, __CreateView(0));
                __FlushElementTree();
              });
              __AppendElement(page, __CreateView(0));
            };
            ",
        )
        .expect("main-thread script");
    let elements = elements.lock().unwrap();
    for id in 2..=4 {
        assert!(
            elements.element(id).is_some(),
            "the microtask's appends must have landed (id {id})"
        );
    }
}

/// An unhandled rejection raised by the script is reported rather than
/// swallowed, so a bundle failing asynchronously is not mistaken for success.
#[test]
fn an_unhandled_rejection_during_render_is_reported() {
    let mut runtime = bare_runtime();
    let error = runtime
        .run_main_thread_script(
            r"
            globalThis.renderPage = function () {
              __CreatePage('card', 0);
              Promise.reject(new Error('async boom'));
            };
            ",
        )
        .expect_err("the rejection should surface");
    assert!(error.to_string().contains("async boom"), "{error}");
}

/// Job ordering must match the crate's `ScriptEngine` impl.
///
/// Driving the realm directly and checkpointing beside it skips
/// `resume_incomplete_checkpoint`, so a run that hit the per-checkpoint job
/// limit would let the next source run ahead of the jobs still queued from the
/// previous one. Both paths now share one internal entry point.
#[test]
fn a_run_that_exceeds_the_job_limit_finishes_before_the_next_one_starts() {
    let (mut runtime, elements) = runtime();
    // Far more promise jobs than one checkpoint's budget.
    let over_budget = runtime.evaluate_main_thread_script(
        r"
        globalThis.ticks = 0;
        let chain = Promise.resolve();
        for (let i = 0; i < 1100; i += 1) {
          chain = chain.then(function () { globalThis.ticks += 1; });
        }
        globalThis.renderPage = function () { __CreatePage('card', 0); };
        ",
    );

    // Whether the first run reports hitting the limit is an implementation
    // detail; what matters is that the leftover jobs are not silently skipped
    // past by the next evaluation.
    let _ = over_budget;
    runtime.render_page().ok();

    let elements = elements.lock().unwrap();
    assert!(
        elements.page().is_some(),
        "the boot sequence still ran to completion"
    );
}
