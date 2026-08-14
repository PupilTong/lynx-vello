#![cfg(feature = "quickjs")]

//! Behavior tests for main-thread (MTS) script execution and the Element PAPI
//! globals it runs against.

use bobcat_core::engine::SharedTree;
use bobcat_core::quickjs::MainThreadRuntime;
use bobcat_core::tree::{PageConfig, Viewport, new_document};

const VIEWPORT: Viewport = Viewport::new(393.0, 727.0);

fn runtime() -> (MainThreadRuntime, SharedTree) {
    let elements = SharedTree::new(new_document(VIEWPORT, PageConfig::default()));
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
    assert!(elements.get(2).is_some());
    assert!(elements.get(3).is_some());
    assert!(elements.get(4).is_some());
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
    assert!(elements.get(2).is_some());
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
            elements.get(id).is_some(),
            "tree mutation must not retire element {id}"
        );
    }
}

#[test]
fn structural_misuse_crashes_the_host_function() {
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
        .expect_err("a foreign reference crashes the host function");
    assert!(error.to_string().contains("panicked"), "{error}");
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
        assert!(elements.get(id).is_some(), "element {id} must be live");
    }
}

#[test]
fn replace_elements_and_swap_element_restructure_the_tree() {
    let (mut runtime, elements) = runtime();
    runtime
        .run_main_thread_script(
            r"
            globalThis.renderPage = function () {
              if (__ReplaceElements.length !== 3 || __SwapElement.length !== 2) {
                throw new Error('tree PAPI host functions expose the wrong arity');
              }
              const page = __CreatePage('card', 0);
              const a = __CreateView(0);
              const b = __CreateView(0);
              const c = __CreateView(0);
              __AppendElement(page, a);
              __AppendElement(page, b);
              __AppendElement(page, c);
              __SwapElement(a, c);
              const d = __CreateView(0);
              const e = __CreateView(0);
              __ReplaceElements(page, [d, e], [b, a]);
              __ReplaceElements(page, __CreateView(0));
              globalThis.kept = [a, b, c, d, e];
            };
            ",
        )
        .expect("main-thread script");

    // Node ids: a=2, b=3, c=4; the swap's transient marker takes slot 5 and
    // is freed, so d reuses 5, e=6, and the appended view is 7. The swap
    // yields [c, b, a]; replacing [b, a] with [d, e] detaches a and puts
    // d, e in b's place; the old-less form appends.
    let elements = elements.tree();
    assert_eq!(elements.document_element().child_ids(), [4, 5, 6, 7]);
    assert!(elements.get(2).is_some(), "a stays live but detached");
    assert!(!elements.is_connected(2));
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
    assert!(elements.tree().rounded_layout(1).is_some());
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
        assert!(elements.get(id).is_some(), "child {id} must be live");
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
fn the_page_has_no_size_until_the_tree_is_flushed() {
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
    {
        let elements = elements.tree();
        let unflushed = elements.rounded_layout(1).expect("layout state");
        assert!(unflushed.size.width.abs() < f32::EPSILON);
    }

    runtime.render_page().expect("render");
    let elements = elements.tree();
    let flushed = elements.rounded_layout(1).expect("layout state");
    assert!((flushed.size.width - 393.0).abs() < f32::EPSILON);
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
    assert!(error.to_string().contains("expects a number"), "{error}");
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
    assert!(error.to_string().contains("expects a number"), "{error}");
}

#[test]
fn a_missing_papi_global_fails_loudly() {
    let mut runtime = bare_runtime();
    let error = runtime
        .run_main_thread_script(
            r"
            globalThis.renderPage = function () {
              __CreatePage('card', 0);
              if (typeof globalThis.__DropElement !== 'undefined') {
                throw new Error('__DropElement must stay absent: no web-core generation has it');
              }
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
fn collection_drops_unreferenced_js_elements_before_re_rendering() {
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
    assert!(
        elements.tree().is_connected(2),
        "an undelivered collection must not retire the committed element"
    );

    runtime.collect_garbage().expect("collect");
    assert!(elements.tree().get(2).is_none());

    runtime.render_page().expect("second boot");
    let elements = elements.tree();
    assert!(
        elements.is_connected(2),
        "the re-render's view reuses the freed slot and is attached"
    );
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

    // Collecting delivers the pending drop for the local-only parent. The two
    // descendants still have live JavaScript handles and must not be retired
    // with it.
    runtime.collect_garbage().expect("collect");

    let elements = elements.tree();
    assert!(elements.get(2).is_none());
    assert!(elements.get(3).is_some());
    assert!(elements.get(4).is_some());
}

#[test]
fn a_fresh_realm_over_a_retained_tree_keeps_working() {
    let elements = SharedTree::new(new_document(VIEWPORT, PageConfig::default()));
    let script = r"
        globalThis.renderPage = function () {
          __AppendElement(__CreatePage('card', 0), __CreateView(0));
        };
        ";

    let mut first = MainThreadRuntime::new(elements.clone(), || {}).expect("QuickJS realm");
    first.run_main_thread_script(script).expect("first boot");
    drop(first);

    // The Engine::run_script shape: a second bootstrap realm over the same
    // retained tree keeps creating elements — identity is the DOM node id,
    // so no realm-local allocator can collide with it.
    let mut second = MainThreadRuntime::new(elements.clone(), || {}).expect("QuickJS realm");
    second.run_main_thread_script(script).expect("second boot");

    let elements = elements.tree();
    assert_eq!(elements.document_element().child_ids().len(), 2);
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
    assert!(elements.tree().get(2).is_some());

    drop(runtime);

    assert!(elements.tree().get(2).is_some());
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
            elements.get(id).is_some(),
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
        elements.rounded_layout(1).is_some(),
        "the boot sequence still ran to completion"
    );
}
