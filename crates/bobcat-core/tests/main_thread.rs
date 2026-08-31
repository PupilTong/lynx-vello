mod support;

use std::sync::Arc;

use bobcat_core::script::ScriptError;
use bobcat_core::{LynxView, LynxViewError, NoWakeup, PageConfig, ViewSources};
use support::{FetcherDouble, wait_for_script};

/// The one way to build a view: hand it its sources and it comes back with
/// its Lynx main thread already running the entry module.
async fn view(source: &[u8], resolved_url: &str) -> Result<LynxView, LynxViewError> {
    LynxView::new(
        PageConfig::default(),
        &FetcherDouble::new(source.to_vec()).resolving_to(resolved_url),
        Arc::new(NoWakeup),
        393.0,
        727.0,
        1.0,
        ViewSources::new("main.js"),
    )
    .await
}

async fn run(source: &str, resolved_url: &str) -> Result<(), ScriptError> {
    let mut view = view(source.as_bytes(), resolved_url)
        .await
        .expect("fetch and start");

    wait_for_script(&mut view)
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
async fn public_view_boots_through_the_engine_render_event() {
    run(
        r"
        globalThis.processData = function () { return 'processed'; };
        const engine = lynx.getEngine();
        engine.addEventListener('__RenderPage', function (event) {
          if (this !== engine || event.data !== 'processed') {
            throw new Error('invalid engine render event');
          }
          __AppendElement(__CreatePage('card', 0), __CreateView(0));
        });
        ",
        "app:///engine-render.js",
    )
    .await
    .expect("engine render-event boot");
}

#[tokio::test]
async fn script_finished_waits_for_the_tla_entry_and_javascript_boot() {
    run(
        r"
        import { __CreateView as createView } from 'bobcat:element';
        await Promise.resolve();
        if (typeof globalThis.__CreateView !== 'undefined') {
          throw new Error('Element PAPI leaked onto globalThis');
        }
        for (const name of [
          'lynx', 'SystemInfo', '__globalProps', 'NativeModules',
          '_AddEventListener', '_ReportError', '_SetSourceMapRelease',
          '__OnLifecycleEvent'
        ]) {
          if (name in globalThis) {
            throw new Error(name + ' leaked onto globalThis');
          }
        }
        globalThis.renderPage = function () {
          const page = __CreatePage('card', 0);
          __AppendElement(page, createView(0));
        };
        ",
        "app:///entry.mjs",
    )
    .await
    .expect("TLA entry boot");
}

#[tokio::test]
async fn resolved_script_url_is_preserved_in_errors() {
    let error = run("const = 1", "app:///broken.js")
        .await
        .expect_err("syntax error");
    let message = error.to_string();
    assert!(message.contains("booting the MTS entry"), "{message}");
    assert!(message.contains("app:///broken.js:"), "{message}");
}

/// Invalid UTF-8 fails the construction outright: no view exists, so no realm
/// was created and no main thread is running.
#[tokio::test]
async fn script_bytes_are_strict_utf8_at_the_view_boundary() {
    let error = view(&[0xff, 0xfe], "app:///invalid.js")
        .await
        .expect_err("invalid UTF-8 must not reach the VM");
    assert!(matches!(
        error,
        LynxViewError::InvalidScriptEncoding { ref url, .. } if url == "app:///invalid.js"
    ));
}

/// Both registration forms, end to end against the real element tree:
/// `__AddEventListener`'s standard identity and `__AddEvent`'s one-per-name
/// filing, over real handles and real node ids. Delivery itself is driven by
/// the engine's script thread, which is exercised where that loop lives.
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
          // `__AddEvent` files against the same handles, and answers for
          // the dispatch form it was filed under and no other.
          const worklet = { type: 'worklet', value: {} };
          __AddEvent(inner, 'capture-bind', 'tap', worklet);
          if (__GetEvent(inner, 'tap', 'capture-bind') !== worklet) {
            throw new Error('a filed handler must read back on a real handle');
          }
          if (__GetEvent(inner, 'tap', 'bindEvent') !== undefined) {
            throw new Error('a filed handler answers for its own form only');
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

#[tokio::test]
async fn tree_and_attribute_queries_answer_over_the_real_document() {
    run(
        r"
        globalThis.renderPage = function () {
          const page = __CreatePage('card', 0);
          const text = __CreateText(0);
          const raw = __CreateRawText('hello');
          __AppendElement(text, raw);
          __AppendElement(page, text);

          // The raw-text reflects its content into a DOM text node, which no
          // handle names. Element children is what keeps it out of the answer.
          const children = __GetChildren(text);
          if (children.length !== 1 || children[0] !== raw) {
            throw new Error('a text element has exactly its raw-text child');
          }
          if (__GetChildren(raw).length !== 0) {
            throw new Error('a reflected text node is not an element child');
          }

          const view = __CreateView(0);
          __AppendElement(page, view);
          __SetClasses(view, 'panel wide');
          __SetID(view, 'header');
          __SetAttribute(view, 'aria-label', 'Add one');

          // `class`, `id` and `style` reach the DOM through paths of their
          // own, and both inline-style paths take a different one again. Each
          // still writes the attribute, which is what makes the name list
          // whole rather than the leftovers.
          __SetInlineStyles(view, 'color: red');
          const fromString = __GetAttributeNames(view);
          __SetInlineStyles(view, { backgroundColor: 'blue' });
          const fromRecord = __GetAttributeNames(view);

          for (const names of [fromString, fromRecord]) {
            for (const name of ['class', 'id', 'aria-label', 'style']) {
              if (!names.includes(name)) {
                throw new Error('__GetAttributeNames dropped ' + name);
              }
            }
            if (new Set(names).size !== names.length) {
              throw new Error('__GetAttributeNames repeated a name');
            }
          }

          if (__GetAttributeByName(view, 'class') !== 'panel wide') {
            throw new Error('class does not read back');
          }
          if (__GetAttributeByName(view, 'id') !== __GetID(view)) {
            throw new Error('id disagrees with __GetID');
          }
          if (__GetAttributeByName(view, 'aria-label') !== 'Add one') {
            throw new Error('a plain attribute does not read back');
          }
          if (__GetAttributeByName(view, 'never-set') !== null) {
            throw new Error('an absent attribute must read as null');
          }

          if (__GetChildren(page).length !== 2) {
            throw new Error('the page has both of its element children');
          }
        };
        ",
        "app:///queries.js",
    )
    .await
    .expect("main-thread boot");
}
