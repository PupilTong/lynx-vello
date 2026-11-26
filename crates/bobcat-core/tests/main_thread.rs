mod support;

use std::rc::Rc;
use std::sync::Arc;

use bobcat_core::{DrawTarget, LynxView, LynxViewError, NoWakeup, ViewSources};
use support::{FetcherDouble, solo_view, wait_for_script};

const SCREEN: bobcat_core::ScreenMetrics =
    bobcat_core::ScreenMetrics::for_viewport(32.0, 24.0, 1.0);

async fn view(
    fetcher: FetcherDouble,
    resolved_url: &str,
) -> Result<(LynxView<Rc<FetcherDouble>>, bobcat_core::Painter), LynxViewError> {
    let fetcher = Rc::new(fetcher.resolving_to(resolved_url));
    solo_view(
        Arc::new(NoWakeup),
        393.0,
        727.0,
        1.0,
        DrawTarget::Offscreen,
        |_reports| fetcher,
        ViewSources::new("app:///", "app:///main.js", SCREEN, "en-US"),
    )
    .await
}

async fn run(source: &str, resolved_url: &str) -> Result<(), LynxViewError> {
    let (mut view, _painter) = view(FetcherDouble::card(source), resolved_url).await?;
    wait_for_script(&mut view)?;
    Ok(())
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
        globalThis.processData = function () { return {value:'processed'}; };
        const engine = lynx.getEngine();
        engine.addEventListener('__RenderPage', function (event) {
          if (this !== engine || event.data[0].value !== 'processed') {
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
async fn a_card_that_constructs_a_second_document_fails_its_boot() {
    let error = run(
        r#"
        import { Document } from "bobcat:element";
        new Document({
          defaultDisplayLinear: true,
          defaultOverflowVisible: true,
          enableCssSelector: true,
          enableJSDataProcessor: false,
          enableCssInheritance: false,
        });
        "#,
        "app:///second-document.js",
    )
    .await
    .expect_err("the boot module already created this realm's document");
    let message = error.to_string();
    assert!(
        message.contains("the realm already created its document"),
        "{message}"
    );
}

#[tokio::test]
async fn the_requested_entry_url_is_preserved_in_errors() {
    let error = run("const = 1", "app:///broken.js")
        .await
        .expect_err("syntax error");
    let message = error.to_string();
    assert!(matches!(error, LynxViewError::Script(_)));
    assert!(message.contains("booting the MTS entry"), "{message}");
    assert!(message.contains("app:///main.js:1:"), "{message}");
}

fn failures_until_boot_settles(
    view: &mut LynxView<Rc<FetcherDouble>>,
) -> Vec<bobcat_core::EngineEvent> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    let mut failures = Vec::new();
    let mut settled = false;
    loop {
        for event in view.pump() {
            match event {
                bobcat_core::EngineEvent::ScriptFinished => settled = true,
                event @ (bobcat_core::EngineEvent::StartupFailed(_)
                | bobcat_core::EngineEvent::Panicked(_)) => {
                    failures.push(event);
                    settled = true;
                }
                event @ bobcat_core::EngineEvent::ScriptRunError(_) => failures.push(event),
                _ => {}
            }
        }
        if settled {
            return failures;
        }
        assert!(std::time::Instant::now() < deadline, "boot did not settle");
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
}

#[tokio::test]
async fn an_entry_that_imports_the_papi_itself_boots_as_it_was_answered() {
    let (mut view, _painter) = view(
        FetcherDouble::new(
            br"import { __AppendElement, __CreatePage, __CreateView } from 'bobcat:element';
if (typeof lynx !== 'undefined' || typeof __SetClasses !== 'undefined')
  throw Error('the engine put a binding in scope that this entry did not import');
globalThis.renderPage = function () {
  __AppendElement(__CreatePage('card', 0), __CreateView(0));
};
"
            .to_vec(),
        ),
        "app:///raw.js",
    )
    .await
    .expect("loading view");
    let failures = failures_until_boot_settles(&mut view);
    assert!(failures.is_empty(), "{failures:?}");
}

#[tokio::test]
async fn an_entry_that_names_the_papi_without_importing_it_fails_as_the_entry() {
    let (mut view, _painter) = view(
        FetcherDouble::new(
            b"__CreatePage('card', 0);
"
            .to_vec(),
        ),
        "app:///raw.js",
    )
    .await
    .expect("the view is constructed whatever its entry names");
    let failures = failures_until_boot_settles(&mut view);
    let [bobcat_core::EngineEvent::ScriptRunError(error)] = failures.as_slice() else {
        panic!("one entry failure and boot settled past it: {failures:?}");
    };
    assert!(
        error.message.contains("ReferenceError") && error.message.contains("__CreatePage"),
        "{error}"
    );
    let location = error.location.as_ref().expect("the throw has a location");
    assert_eq!(location.source.as_deref(), Some("app:///main.js"));
    assert_eq!(location.line, Some(1));
}

#[tokio::test]
async fn script_bytes_are_strict_utf8_at_the_view_boundary() {
    let (mut view, _painter) = view(FetcherDouble::new(vec![0xff, 0xfe]), "app:///invalid.js")
        .await
        .expect("loading view");
    let error = wait_for_script(&mut view).expect_err("invalid UTF-8 must not reach the VM");
    assert!(
        matches!(error, LynxViewError::InvalidScriptEncoding { .. }),
        "{error}"
    );
    let message = error.to_string();
    assert!(message.contains("app:///invalid.js"), "{message}");
    assert!(message.contains("UTF-8"), "{message}");
}

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

          // Generated content contributes no DOM children.
          const children = __GetChildren(text);
          if (children.length !== 1 || children[0] !== raw) {
            throw new Error('a text element has exactly its raw-text child');
          }
          if (__GetChildren(raw).length !== 0) {
            throw new Error('generated text is not a DOM child');
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
