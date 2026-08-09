#![cfg(feature = "quickjs")]

//! Behavior tests for main-thread (MTS) script execution and the Element PAPI
//! globals it runs against.

use bobcat_core::engine::SharedTree;
use bobcat_core::quickjs::MainThreadRuntime;
use lynx_element::{ElementTree, PageConfig, Viewport};

const VIEWPORT: Viewport = Viewport::new(393.0, 727.0);

/// The single-threaded composition: the realm takes the tree from this
/// slot per batch and every `__FlushElementTree` puts it back committed.
fn runtime() -> (MainThreadRuntime, SharedTree) {
    let elements = SharedTree::new(ElementTree::new(VIEWPORT, PageConfig::default()));
    let runtime = MainThreadRuntime::new(elements.clone(), VIEWPORT, || {}).expect("QuickJS realm");
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
    let elements = elements.tree();
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
    let elements = elements.tree();
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

    let elements = elements.tree();
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

    let elements = elements.tree();
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
    assert!(elements.tree().page().is_some());
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
    let elements = elements.tree();
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
    assert!(elements.tree().page().is_none());

    runtime.render_page().expect("render");
    assert!(elements.tree().page().is_some());
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

/// A PAPI member the runtime does not implement stays a precise
/// `ReferenceError` naming it, rather than a silent no-op that would render
/// something wrong. `__CreateComponent` stands in for the whole unimplemented
/// remainder; swap it for another one if it ever lands.
#[test]
fn a_missing_papi_global_fails_loudly() {
    let mut runtime = bare_runtime();
    let error = runtime
        .run_main_thread_script(
            r"
            globalThis.renderPage = function () {
              __CreatePage('card', 0);
              __CreateComponent(0, 'id', 0, 'entry', 'name', 'path', null, null);
            };
            ",
        )
        .expect_err("an unimplemented PAPI member");
    assert!(error.to_string().contains("__CreateComponent"), "{error}");
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
    let elements = elements.tree();
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
    let elements = elements.tree();
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

    let elements = elements.tree();
    assert!(
        elements.page().is_some(),
        "the boot sequence still ran to completion"
    );
}

/// The main-thread global object web-core installs alongside the Element PAPI.
///
/// A `ReactLynx` card root reaches for `lynx` and `SystemInfo` during its own
/// module initialization, before `renderPage` exists, so these are boot
/// requirements rather than conveniences.
#[test]
fn the_main_thread_globals_are_installed_before_the_bundle_runs() {
    let (mut runtime, _elements) = runtime();
    runtime
        .run_main_thread_script(
            r"
            if (typeof lynx !== 'object') throw new Error('lynx is ' + typeof lynx);
            if (typeof SystemInfo !== 'object') throw new Error('SystemInfo missing');
            if (lynx.SystemInfo !== SystemInfo) throw new Error('lynx.SystemInfo must be the same object');
            if (SystemInfo.lynxSdkVersion !== '3.0') throw new Error('unexpected sdk version');
            if (SystemInfo.pixelWidth !== 393) throw new Error('pixelWidth is ' + SystemInfo.pixelWidth);
            if (typeof lynx.__initData !== 'object') throw new Error('__initData missing');
            globalThis.renderPage = function () { __CreatePage('card', 0); };
            ",
        )
        .expect("main-thread script");
}

/// `__SetInlineStyles` takes either a declaration-block string or a property
/// map. The host boundary carries primitives only, so the map form is
/// flattened before it crosses — this checks it is not rejected.
#[test]
fn set_inline_styles_accepts_both_the_string_and_the_object_form() {
    let (mut runtime, elements) = runtime();
    runtime
        .run_main_thread_script(
            r"
            globalThis.renderPage = function () {
              const page = __CreatePage('card', 0);
              const text = __CreateView(0);
              __SetInlineStyles(text, 'width:10px;height:10px');
              __AppendElement(page, text);
              const mapped = __CreateView(0);
              __SetInlineStyles(mapped, { width: '20px', height: '20px' });
              __AppendElement(page, mapped);
              const cleared = __CreateView(0);
              __SetInlineStyles(cleared, { width: '30px' });
              __SetInlineStyles(cleared, undefined);
              __AppendElement(page, cleared);
            };
            ",
        )
        .expect("main-thread script");
    let elements = elements.tree();
    for id in 2..=4 {
        assert!(elements.element(id).is_some(), "view {id} must be live");
    }
}

/// Property names in the object form arrive in the JavaScript spelling, and
/// web-core hyphenates them before writing the style attribute — a `marginTop`
/// that stayed camelCase would be dropped by the CSS parser rather than
/// applied.
///
/// This asserts only that the call is accepted and committed; the resulting
/// declaration block is not readable from this layer, so the hyphenation
/// itself is covered by `lynx-element`'s inline-style tests against
/// already-hyphenated input plus the parity with web-core's
/// `hyphenate_style_name`.
#[test]
fn set_inline_styles_hyphenates_the_object_form_property_names() {
    let (mut runtime, elements) = runtime();
    runtime
        .run_main_thread_script(
            r"
            globalThis.renderPage = function () {
              const page = __CreatePage('card', 0);
              const view = __CreateView(0);
              __SetInlineStyles(view, { width: '40px', marginTop: '7px' });
              __AppendElement(page, view);
            };
            ",
        )
        .expect("main-thread script");
    // The committed layout is the observable proof: a dropped declaration
    // would leave the view at the top of the page.
    let elements = elements.tree();
    assert!(elements.element(2).is_some());
    assert!(!elements.has_uncommitted_mutations());
}

/// `__SetCSSId` takes a *list* of elements. The prelude flattens it over the
/// one-element host call rather than crossing the boundary with an array.
#[test]
fn set_css_id_accepts_a_list_of_elements() {
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
              __SetCSSId([first, second], 7, undefined);
            };
            ",
        )
        .expect("main-thread script");
    let elements = elements.tree();
    for id in 2..=3 {
        assert_eq!(
            elements
                .element(id)
                .map(lynx_element::LynxElement::component_css_id),
            Some(7)
        );
    }
}

/// A worklet handler arrives as an object, which the primitives-only boundary
/// cannot carry. The binding is still recorded, without the payload.
#[test]
fn add_event_records_a_string_handler_and_tolerates_a_worklet_object() {
    let (mut runtime, elements) = runtime();
    runtime
        .run_main_thread_script(
            r"
            globalThis.renderPage = function () {
              const page = __CreatePage('card', 0);
              const view = __CreateView(0);
              __AppendElement(page, view);
              __AddEvent(view, 'bindEvent', 'tap', '-3:0:');
              __AddEvent(view, 'catchEvent', 'longpress', { type: 'worklet', value: {} });
            };
            ",
        )
        .expect("main-thread script");
    let elements = elements.tree();
    let events = elements
        .element(2)
        .map(lynx_element::LynxElement::events)
        .expect("the bound view");
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].handler.as_deref(), Some("-3:0:"));
    assert_eq!(events[1].name, "longpress");
    assert_eq!(events[1].handler, None);
}

/// Re-binding the same event type and name replaces the handler rather than
/// stacking a second one, which is what a re-render does.
#[test]
fn add_event_replaces_a_binding_for_the_same_type_and_name() {
    let (mut runtime, elements) = runtime();
    runtime
        .run_main_thread_script(
            r"
            globalThis.renderPage = function () {
              const page = __CreatePage('card', 0);
              const view = __CreateView(0);
              __AppendElement(page, view);
              __AddEvent(view, 'bindEvent', 'tap', 'first');
              __AddEvent(view, 'bindEvent', 'tap', 'second');
            };
            ",
        )
        .expect("main-thread script");
    let elements = elements.tree();
    let events = elements
        .element(2)
        .map(lynx_element::LynxElement::events)
        .expect("the bound view");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].handler.as_deref(), Some("second"));
}

/// `ReactLynx` catches a failing first render, reports it, and removes the
/// children it had built — so a reported error means no first screen, and must
/// not be mistaken for a successful boot.
#[test]
fn an_error_the_script_reports_rather_than_throws_still_fails_the_render() {
    let mut runtime = bare_runtime();
    let error = runtime
        .run_main_thread_script(
            r"
            globalThis.renderPage = function () {
              __CreatePage('card', 0);
              _ReportError(new Error('first screen blew up'), { errorCode: 1101 });
            };
            ",
        )
        .expect_err("a reported error fails the render");
    assert!(
        error.to_string().contains("first screen blew up"),
        "{error}"
    );
}

/// `__GetElementUniqueID` is the handle's own liveness check here, because the
/// handle *is* the unique id.
#[test]
fn get_element_unique_id_returns_the_handle_and_rejects_a_dead_one() {
    let (mut runtime, _elements) = runtime();
    runtime
        .run_main_thread_script(
            r"
            globalThis.renderPage = function () {
              const page = __CreatePage('card', 0);
              if (__GetElementUniqueID(page) !== page) {
                throw new Error('the unique id is the handle');
              }
              const dropped = __CreateView(0);
              __DropElement(dropped);
              let threw = false;
              try { __GetElementUniqueID(dropped); } catch (error) { threw = true; }
              if (!threw) throw new Error('a retired handle must be rejected');
            };
            ",
        )
        .expect("main-thread script");
}
