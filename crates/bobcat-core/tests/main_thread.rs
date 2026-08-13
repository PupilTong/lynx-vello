#![cfg(feature = "quickjs")]

//! Behavior tests for main-thread (MTS) script execution and the Element PAPI
//! globals it runs against.

use bobcat_core::engine::SharedTree;
use bobcat_core::quickjs::MainThreadRuntime;
use lynx_element::{ElementTree, PageConfig, Viewport};

const VIEWPORT: Viewport = Viewport::new(393.0, 727.0);

fn runtime() -> (MainThreadRuntime, SharedTree) {
    let elements = SharedTree::new(ElementTree::new(VIEWPORT, PageConfig::default()));
    let runtime = MainThreadRuntime::new(elements.clone(), || {}).expect("QuickJS realm");
    (runtime, elements)
}

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
              if (typeof view !== 'object') {
                throw new Error('__CreateView must return an object, got ' + typeof view);
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
fn all_four_tree_mutation_papis_are_host_functions_with_native_return_values() {
    let (mut runtime, elements) = runtime();
    runtime
        .run_main_thread_script(
            r"
            globalThis.renderPage = function () {
              const page = __CreatePage('card', 0);
              const first = __CreateView(0);
              const second = __CreateView(0);
              const third = __CreateView(0);
              const replacement = __CreateView(0);

              if (__AppendElement.length !== 2
                  || __InsertElementBefore.length !== 3
                  || __RemoveElement.length !== 2
                  || __ReplaceElement.length !== 2) {
                throw new Error('tree PAPI host functions expose the wrong arity');
              }
              if (__AppendElement(page, first) !== first) {
                throw new Error('__AppendElement must return the child');
              }
              if (__InsertElementBefore(page, second, first) !== second) {
                throw new Error('__InsertElementBefore must return the child');
              }
              if (__InsertElementBefore(page, third) !== third) {
                throw new Error('__InsertElementBefore must append for an omitted ref');
              }
              if (__RemoveElement(page, first) !== first) {
                throw new Error('__RemoveElement must return the child');
              }
              __InsertElementBefore(page, first, null);
              if (__ReplaceElement(replacement, second) !== undefined) {
                throw new Error('__ReplaceElement must return undefined');
              }

              globalThis.keptTreeElements = [first, second, third, replacement];
            };
            ",
        )
        .expect("main-thread script");

    let elements = elements.tree();
    for id in 2..=5 {
        assert!(
            elements.element(id).is_some(),
            "tree mutation must not retire element {id}"
        );
    }
}

#[test]
fn tree_mutation_validation_surfaces_as_a_javascript_exception() {
    let mut runtime = bare_runtime();
    let error = runtime
        .run_main_thread_script(
            r"
            globalThis.renderPage = function () {
              const page = __CreatePage('card', 0);
              const otherParent = __CreateView(0);
              const reference = __CreateView(0);
              const child = __CreateView(0);
              __AppendElement(page, otherParent);
              __AppendElement(otherParent, reference);
              __InsertElementBefore(page, child, reference);
            };
            ",
        )
        .expect_err("a reference from another parent must be rejected");
    assert!(error.to_string().contains("not a child"), "{error}");
}

#[test]
fn every_reactlynx_create_function_except_frame_returns_an_element_handle() {
    let (mut runtime, elements) = runtime();
    runtime
        .run_main_thread_script(
            r"
            globalThis.renderPage = function () {
              const page = __CreatePage('card', 0);
              const created = [
                __CreateElement('custom-widget', 0),
                __CreateWrapperElement(0),
                __CreateText(0),
                __CreateImage(0),
                __CreateView(0),
                __CreateScrollView(0),
                __CreateRawText('Hello, Lynx'),
                __CreateList(
                  0,
                  function componentAtIndex() {},
                  function enqueueComponent() {},
                  {},
                  function componentAtIndexes() {}
                ),
              ];
              if (__CreateList.length !== 3) {
                throw new Error('__CreateList must expose the web-core arity');
              }
              if (typeof globalThis.__CreateListElementHost !== 'undefined') {
                throw new Error('the primitive list host must stay private');
              }
              for (const element of created) {
                if (typeof element !== 'object') {
                  throw new Error('every create function must return an object handle');
                }
                __AppendElement(page, element);
              }
              globalThis.created = created;
            };
            ",
        )
        .expect("main-thread script");

    let elements = elements.tree();
    for id in 2..=9 {
        assert!(elements.element(id).is_some(), "element {id} must be live");
    }
}

#[test]
fn create_frame_remains_an_explicitly_missing_global() {
    let mut runtime = bare_runtime();
    let error = runtime
        .run_main_thread_script(
            r"
            globalThis.renderPage = function () {
              __CreatePage('card', 0);
              __CreateFrame(0);
            };
            ",
        )
        .expect_err("__CreateFrame is deliberately not implemented");
    assert!(error.to_string().contains("__CreateFrame"), "{error}");
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
fn drop_element_retires_only_the_target_and_preserves_descendant_handles() {
    let (mut runtime, elements) = runtime();
    runtime
        .run_main_thread_script(
            r"
            globalThis.renderPage = function () {
              const page = __CreatePage('card', 0);
              const parent = __CreateView(0);
              const child = __CreateView(0);
              const grandchild = __CreateView(0);
              __AppendElement(page, parent);
              __AppendElement(parent, child);
              __AppendElement(child, grandchild);
              __DropElement(parent);
            };
            ",
        )
        .expect("main-thread script");

    let elements = elements.tree();
    assert!(elements.element(2).is_none());
    assert!(elements.element(3).is_some());
    assert!(elements.element(4).is_some());
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
              const retired = __CreateView(0);
              __DropElement(retired);
              __AppendElement(retired, __CreateView(0));
            };
            ",
        )
        .expect_err("an unknown handle");
    assert!(
        error.to_string().contains("unique id 2"),
        "the error should name the bad handle: {error}"
    );
}

#[test]
fn parent_component_ids_accept_the_full_u32_range() {
    let mut runtime = bare_runtime();
    let error = runtime
        .run_main_thread_script(
            r"
            globalThis.renderPage = function () {
              __CreateView(4294967295);
            };
            ",
        )
        .expect_err("an unknown u32::MAX handle");
    assert!(
        error
            .to_string()
            .contains("no element has the unique id 4294967295"),
        "the u32 component id should reach lynx-element validation: {error}"
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
    assert!(error.to_string().contains("weak reference"), "{error}");
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
fn the_next_realm_entry_drops_unreferenced_js_elements_before_re_rendering() {
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
    let elements = elements.tree();
    assert!(elements.element(2).is_none());
    assert!(elements.element(3).is_some());
}

#[test]
fn vm_drop_of_a_parent_preserves_descendants_with_live_handles() {
    let (mut runtime, elements) = runtime();
    runtime
        .run_main_thread_script(
            r"
            globalThis.renderPage = function () {
              const page = __CreatePage('card', 0);
              const parent = __CreateView(0);
              const child = __CreateView(0);
              const grandchild = __CreateView(0);
              __AppendElement(page, parent);
              __AppendElement(parent, child);
              __AppendElement(child, grandchild);
              globalThis.savedChild = child;
              globalThis.savedGrandchild = grandchild;
            };
            ",
        )
        .expect("first boot");

    // Entering the realm again delivers the pending finalizer for the local-only parent. The two
    // descendants still have live JavaScript handles and must not be retired with it.
    runtime
        .evaluate_main_thread_script("")
        .expect("deliver pending VM drops");

    let elements = elements.tree();
    assert!(elements.element(2).is_none());
    assert!(elements.element(3).is_some());
    assert!(elements.element(4).is_some());
}

#[test]
fn bootstrap_realm_teardown_preserves_the_last_committed_tree() {
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
    assert!(elements.tree().element(2).is_some());

    drop(runtime);

    assert!(elements.tree().element(2).is_some());
}

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

#[test]
fn a_run_that_exceeds_the_job_limit_finishes_before_the_next_one_starts() {
    let (mut runtime, elements) = runtime();
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

    let _ = over_budget;
    runtime.render_page().ok();

    let elements = elements.tree();
    assert!(
        elements.page().is_some(),
        "the boot sequence still ran to completion"
    );
}
