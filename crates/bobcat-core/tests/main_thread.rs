#![cfg(feature = "quickjs")]

//! Behavior tests for main-thread (MTS) script execution and the Element PAPI
//! globals it runs against.

use std::cell::Cell;
use std::rc::Rc;

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
            globalThis.owned = [];
            globalThis.renderPage = function () {
              const page = __CreatePage('card', 0);
              const first = __CreateView(0);
              const second = __CreateView(0);
              const nested = __CreateView(0);
              owned.push(first, second, nested);
              __AppendElement(page, first);
              __AppendElement(page, second);
              __AppendElement(first, nested);
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
              if (typeof view !== 'object' || view === null) {
                throw new Error('__CreateView must return an object, got ' + typeof view);
              }
              const appended = __AppendElement(page, view);
              if (appended !== view) {
                throw new Error('__AppendElement must return the child');
              }
              globalThis.ownedView = view;
            };
            ",
        )
        .expect("main-thread script");
    let elements = elements.tree();
    assert!(elements.page().is_some());
    assert!(elements.element(2).is_some());
}

#[test]
fn remove_element_detaches_and_returns_the_exact_live_child_wrapper() {
    let (mut runtime, elements) = runtime();
    runtime
        .run_main_thread_script(
            r"
            globalThis.renderPage = function () {
              const page = __CreatePage('card', 0);
              const parent = __CreateView(0);
              const child = __CreateView(0);
              const grandchild = __CreateView(0);
              globalThis.owned = [parent, child, grandchild];
              __AppendElement(page, parent);
              __AppendElement(parent, child);
              __AppendElement(child, grandchild);

              const removed = __RemoveElement(parent, child);
              if (removed !== child) {
                throw new Error('__RemoveElement must return the exact child wrapper');
              }

              // A removed child remains a live detached subtree and can be
              // inserted again with the same wrapper.
              __AppendElement(page, removed);
            };
            ",
        )
        .expect("main-thread script");

    let elements = elements.tree();
    assert!(elements.element(2).is_some());
    assert!(elements.element(3).is_some());
    assert!(elements.element(4).is_some());
}

#[test]
fn remove_element_rejects_a_parent_that_does_not_own_the_child() {
    let (mut runtime, elements) = runtime();
    runtime
        .run_main_thread_script(
            r"
            globalThis.renderPage = function () {
              const page = __CreatePage('card', 0);
              const actualParent = __CreateView(0);
              const wrongParent = __CreateView(0);
              const child = __CreateView(0);
              globalThis.owned = [actualParent, wrongParent, child];
              __AppendElement(page, actualParent);
              __AppendElement(page, wrongParent);
              __AppendElement(actualParent, child);

              let rejected = false;
              try {
                __RemoveElement(wrongParent, child);
              } catch (error) {
                rejected = true;
              }
              if (!rejected) {
                throw new Error('__RemoveElement must reject a mismatched parent');
              }

              // The failed remove did not mutate the edge.
              __RemoveElement(actualParent, child);
              __AppendElement(page, child);
            };
            ",
        )
        .expect("main-thread script");

    let elements = elements.tree();
    assert!(elements.element(2).is_some());
    assert!(elements.element(3).is_some());
    assert!(elements.element(4).is_some());
}

#[test]
fn explicit_drop_is_idempotent_and_its_later_finalizer_is_harmless() {
    let (mut runtime, elements) = runtime();
    runtime
        .run_main_thread_script(
            r"
            globalThis.renderPage = function () {
              __CreatePage('card', 0);
              const dropped = __CreateView(0);
              __DropElement(dropped);
              __DropElement(dropped);
              globalThis.kept = __CreateView(0);
            };
            ",
        )
        .expect("main-thread script");

    let elements = elements.tree();
    assert!(elements.element(2).is_none());
    assert!(elements.element(3).is_some());
}

#[test]
fn drop_element_retires_only_the_attached_target_and_preserves_its_child() {
    let (mut runtime, elements) = runtime();
    runtime
        .run_main_thread_script(
            r"
            globalThis.renderPage = function () {
              const page = __CreatePage('card', 0);
              const parent = __CreateView(0);
              const child = __CreateView(0);
              globalThis.child = child;
              __AppendElement(page, parent);
              __AppendElement(parent, child);
              __DropElement(parent);

              // Drop detached the child but did not retire it.
              __AppendElement(page, child);
            };
            ",
        )
        .expect("main-thread script");

    let elements = elements.tree();
    assert!(elements.element(2).is_none());
    assert!(elements.element(3).is_some());
}

#[test]
fn gc_retires_an_attached_element_after_its_last_js_owner_is_released() {
    let elements = SharedTree::new(ElementTree::new(VIEWPORT, PageConfig::default()));
    let flushes = Rc::new(Cell::new(0));
    let observed_flushes = Rc::clone(&flushes);
    let mut runtime = MainThreadRuntime::new(elements.clone(), move || {
        observed_flushes.set(observed_flushes.get() + 1);
    })
    .expect("QuickJS realm");

    runtime
        .run_main_thread_script(
            r"
            globalThis.renderPage = function () {
              const page = __CreatePage('card', 0);
              const view = __CreateView(0);
              // Force the cycle collector path rather than relying only on
              // QuickJS's immediate reference counting.
              view.self = view;
              globalThis.ownedView = view;
              __AppendElement(page, view);
            };
            ",
        )
        .expect("main-thread script");
    assert!(elements.tree().element(2).is_some());
    assert_eq!(flushes.get(), 1, "the boot has one explicit flush");

    runtime
        .evaluate_main_thread_script("globalThis.ownedView = undefined;")
        .expect("release the last external owner");
    assert!(
        elements.tree().element(2).is_some(),
        "the self-cycle remains until a tracing collection"
    );

    runtime.collect_garbage();
    let elements = elements.tree();
    assert!(elements.element(2).is_none());
    assert!(!elements.has_uncommitted_mutations());
    assert_eq!(
        flushes.get(),
        2,
        "a GC-only removal is committed and presented as its own batch"
    );
}

#[test]
fn a_gc_release_does_not_commit_an_abandoned_batch_from_an_earlier_evaluation() {
    let elements = SharedTree::new(ElementTree::new(VIEWPORT, PageConfig::default()));
    let flushes = Rc::new(Cell::new(0));
    let observed_flushes = Rc::clone(&flushes);
    let mut runtime = MainThreadRuntime::new(elements.clone(), move || {
        observed_flushes.set(observed_flushes.get() + 1);
    })
    .expect("QuickJS realm");

    runtime
        .run_main_thread_script(
            r"
            globalThis.renderPage = function () {
              const page = __CreatePage('card', 0);
              globalThis.victim = __CreateView(0);
              __AppendElement(page, victim);
            };
            ",
        )
        .expect("initial committed batch");
    assert_eq!(flushes.get(), 1);

    runtime
        .evaluate_main_thread_script(
            r"
            globalThis.abandoned = __CreateView(0);
            __AppendElement(__CreatePage('card', 0), abandoned);
            ",
        )
        .expect("leave a batch open without flushing");
    assert!(elements.tree().has_uncommitted_mutations());

    // Releasing this unrelated wrapper is a native GC mutation, but it must
    // join the already-dirty tree without presenting the abandoned append.
    runtime
        .evaluate_main_thread_script("globalThis.victim = undefined;")
        .expect("release an earlier wrapper");
    {
        let elements = elements.tree();
        assert!(elements.element(2).is_none());
        assert!(elements.element(3).is_some());
        assert!(elements.has_uncommitted_mutations());
    }
    assert_eq!(
        flushes.get(),
        1,
        "a GC-only release must not commit an earlier abandoned batch"
    );

    runtime
        .evaluate_main_thread_script("__FlushElementTree();")
        .expect("explicitly commit the abandoned batch");
    assert!(!elements.tree().has_uncommitted_mutations());
    assert_eq!(flushes.get(), 2);
}

#[test]
fn collecting_each_wrapper_retires_only_its_element_and_detaches_live_children() {
    let (mut runtime, elements) = runtime();
    runtime
        .run_main_thread_script(
            r"
            globalThis.renderPage = function () {
              const page = __CreatePage('card', 0);
              globalThis.parent = __CreateView(0);
              globalThis.child = __CreateView(0);
              globalThis.grandchild = __CreateView(0);
              parent.self = parent;
              child.self = child;
              grandchild.self = grandchild;
              __AppendElement(page, parent);
              __AppendElement(parent, child);
              __AppendElement(child, grandchild);
            };
            ",
        )
        .expect("main-thread script");

    runtime
        .evaluate_main_thread_script("globalThis.parent = undefined;")
        .expect("release parent");
    runtime.collect_garbage();
    {
        let elements = elements.tree();
        assert!(elements.element(2).is_none());
        assert!(elements.element(3).is_some());
        assert!(elements.element(4).is_some());
    }

    // The child's original parent edge is gone, so its surviving wrapper can
    // attach the whole child subtree elsewhere.
    runtime
        .evaluate_main_thread_script(
            "__AppendElement(__CreatePage('card', 0), globalThis.child); \
             __FlushElementTree();",
        )
        .expect("reattach the detached child");

    runtime
        .evaluate_main_thread_script("globalThis.child = undefined;")
        .expect("release child wrapper");
    runtime.collect_garbage();
    {
        let elements = elements.tree();
        assert!(elements.element(3).is_none());
        assert!(elements.element(4).is_some());
    }

    // Dropping the child detached, but did not destroy, its live grandchild.
    runtime
        .evaluate_main_thread_script(
            "__AppendElement(__CreatePage('card', 0), globalThis.grandchild); \
             __FlushElementTree();",
        )
        .expect("reattach the detached grandchild");

    runtime
        .evaluate_main_thread_script("globalThis.grandchild = undefined;")
        .expect("release grandchild wrapper");
    runtime.collect_garbage();
    assert!(elements.tree().element(4).is_none());
}

#[test]
fn an_unowned_inline_child_is_reclaimed_instead_of_being_rooted_by_the_tree() {
    let (mut runtime, elements) = runtime();
    runtime
        .run_main_thread_script(
            r"
            globalThis.renderPage = function () {
              __AppendElement(__CreatePage('card', 0), __CreateView(0));
            };
            ",
        )
        .expect("main-thread script");
    runtime.collect_garbage();
    assert!(
        elements.tree().element(2).is_none(),
        "native tree membership must not create a hidden JS lease"
    );
}

#[test]
fn realm_teardown_releases_every_remaining_element_wrapper() {
    let (mut runtime, elements) = runtime();
    runtime
        .run_main_thread_script(
            r"
            globalThis.renderPage = function () {
              const page = __CreatePage('card', 0);
              globalThis.owned = __CreateView(0);
              __AppendElement(page, owned);
            };
            ",
        )
        .expect("main-thread script");
    assert!(elements.tree().element(2).is_some());

    drop(runtime);
    let elements = elements.tree();
    assert!(elements.element(2).is_none());
    assert!(!elements.has_uncommitted_mutations());
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
            globalThis.owned = [];
            globalThis.processData = function () { return 3; };
            globalThis.renderPage = function (count) {
              const page = __CreatePage('card', 0);
              for (let index = 0; index < count; index += 1) {
                const child = __CreateView(0);
                owned.push(child);
                __AppendElement(page, child);
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
fn forged_objects_are_rejected_as_element_handles() {
    let mut runtime = bare_runtime();
    let error = runtime
        .run_main_thread_script(
            r"
            globalThis.renderPage = function () {
              __CreatePage('card', 0);
              __AppendElement({ ElementId: 9999 }, { ElementId: 8888 });
            };
            ",
        )
        .expect_err("a forged handle");
    assert!(
        error.to_string().contains("bridge-owned host objects"),
        "the error should reject ordinary JS objects: {error}"
    );
}

#[test]
fn numeric_ids_are_not_element_handles() {
    let mut runtime = bare_runtime();
    let error = runtime
        .run_main_thread_script(
            r"
            globalThis.renderPage = function () {
              __AppendElement(4294967295, __CreateView(0));
            };
            ",
        )
        .expect_err("a numeric handle");
    assert!(
        error.to_string().contains("element object"),
        "numbers must not cross as element handles: {error}"
    );
}

#[test]
fn null_is_rejected_by_append_element() {
    let mut runtime = bare_runtime();
    let error = runtime
        .run_main_thread_script(
            r"
            globalThis.renderPage = function () {
              __AppendElement(null, __CreateView(0));
            };
            ",
        )
        .expect_err("a null handle");
    assert!(error.to_string().contains("element object"), "{error}");
}

#[test]
fn non_element_values_are_rejected_rather_than_coerced() {
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
            globalThis.owned = [];
            globalThis.renderPage = function () {
              const child = __CreateView(0);
              owned.push(child);
              __AppendElement(__CreatePage('card', 0), child);
            };
            ",
        )
        .expect("first boot");
    runtime.render_page().expect("second boot");
    let elements = elements.tree();
    assert!(elements.element(2).is_some());
    assert!(elements.element(3).is_some());
}

#[test]
fn microtasks_queued_during_render_run_before_the_call_returns() {
    let (mut runtime, elements) = runtime();
    runtime
        .run_main_thread_script(
            r"
            globalThis.owned = [];
            globalThis.renderPage = function () {
              const page = __CreatePage('card', 0);
              Promise.resolve().then(function () {
                const first = __CreateView(0);
                const second = __CreateView(0);
                owned.push(first, second);
                __AppendElement(page, first);
                __AppendElement(page, second);
                __FlushElementTree();
              });
              const immediate = __CreateView(0);
              owned.push(immediate);
              __AppendElement(page, immediate);
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
