use tokio::sync::mpsc;

use super::*;
use crate::background::{WorkerCommand, WorkerEvent};
use crate::link::{DetachedView, ViewObserver, detached_outbox};
use crate::main::tree::{PageConfig, Viewport};
use crate::main::workers::WorkerFactory;
use crate::view::NoWakeup;

/// The handle a packed id names. A handle carries a generation as well as
/// an arena key, so a test spells one the way script sees it — and for a
/// document that has freed nothing the generation is zero, which is why
/// these read as the small integers the PAPI hands out.
fn node_id(bits: u64) -> dom::NodeId {
    dom::NodeId::from_bits(bits).expect("a well-formed packed handle")
}

/// The name and detail a dispatch carries, spelled as the painting side
/// already owns them.
fn tap() -> Arc<str> {
    Arc::from("tap")
}

fn no_detail() -> Arc<str> {
    Arc::from("")
}

/// The ingredients a test stages when the document is not what it is about.
fn ingredients() -> DocumentIngredients {
    DocumentIngredients::for_test(Viewport::new(393.0, 727.0), PageConfig::default())
}

fn runtime() -> (ScriptRuntime, MainThreadRuntime, DocumentProbe) {
    runtime_over(ingredients())
}

/// The same runtime over a document that can shape text: Ahem's solid em
/// squares make a run's box its glyph count times its font size.
///
/// The fonts are staged rather than registered, because the document does
/// not exist until the boot module creates it — and staging is what a view
/// with fonts does now.
fn text_runtime() -> (ScriptRuntime, MainThreadRuntime, DocumentProbe) {
    const AHEM: &[u8] = include_bytes!("../../../../hughie/tests/fixtures/Ahem.ttf");

    let mut text = dom::TextContext::new();
    assert_eq!(text.register_fonts(dom::FontBlob::from_static(AHEM)), 1);
    runtime_over(DocumentIngredients {
        text_context: Some(text),
        ..ingredients()
    })
}

fn runtime_over(
    ingredients: DocumentIngredients,
) -> (ScriptRuntime, MainThreadRuntime, DocumentProbe) {
    let (js_runtime, runtime, elements, _) = runtime_over_watching_names(ingredients);
    (js_runtime, runtime, elements)
}

/// A same-thread window onto the realm's document, so a test can observe
/// what script built without going through the runtime's own methods.
struct DocumentProbe {
    slot: Rc<RefCell<DocumentSlot>>,
    // These tests exercise only MTS. Keep the far ends open so the realm's
    // sends succeed; worker_tests executes both sides against a real worker
    // runtime.
    _workers: mpsc::UnboundedReceiver<WorkerCommand>,
    _worker_events: mpsc::UnboundedReceiver<WorkerEvent>,
}

impl DocumentProbe {
    /// The document the realm's boot module created. Booting is what makes
    /// one exist, so a test that asks before it has booted is asking about
    /// something that is not there yet.
    fn tree(&self) -> RefMut<'_, LynxDocument> {
        RefMut::filter_map(self.slot.borrow_mut(), |slot| slot.document.as_mut())
            .ok()
            .expect("the realm has created its document")
    }
}

/// The painting side's view of the realm's name set, driven by hand: a test
/// resyncs it where a routing pass would and then asks what the realm has
/// published.
struct PublishedNames(DetachedView);

impl PublishedNames {
    fn contains(&mut self, name: &str) -> bool {
        self.0.published.sync();
        self.0.published.has_listener(name)
    }
}

/// The same runtime, plus the painting end of its link — so a test can
/// ask what the realm published.
fn runtime_over_watching_names(
    ingredients: DocumentIngredients,
) -> (
    ScriptRuntime,
    MainThreadRuntime,
    DocumentProbe,
    PublishedNames,
) {
    let (outbox, far_end) = detached_outbox(Arc::new(NoWakeup));
    let mut js_runtime = ScriptRuntime::new().expect("the test runtime starts");
    install_shared_modules(&mut js_runtime).expect("the shared modules register");
    let (workers, inbox) = mpsc::unbounded_channel();
    let (runtime, worker_events) = MainThreadRuntime::new(
        &mut js_runtime,
        ingredients,
        outbox,
        &WorkerFactory::new(workers),
        "app:///main.js",
        None,
        PageData::default(),
    )
    .expect("main-thread runtime");
    let probe = DocumentProbe {
        slot: Rc::clone(&runtime.slot),
        _workers: inbox,
        _worker_events: worker_events,
    };
    (js_runtime, runtime, probe, PublishedNames(far_end))
}

/// Two views' realms on one group's `QuickJS` runtime, each over its own
/// document — what a `LynxGroup` holds once a second view joins it.
fn two_view_group() -> (
    ScriptRuntime,
    MainThreadRuntime,
    MainThreadRuntime,
    GroupFarEnds,
) {
    two_view_group_with([PageData::default(), PageData::default()])
}

/// The same group, each view given its own page data.
fn two_view_group_with(
    pages: [PageData; 2],
) -> (
    ScriptRuntime,
    MainThreadRuntime,
    MainThreadRuntime,
    GroupFarEnds,
) {
    let mut js_runtime = ScriptRuntime::new().expect("the test runtime starts");
    install_shared_modules(&mut js_runtime).expect("the shared modules register");
    let mut views = Vec::new();
    let mut ends = GroupFarEnds::default();
    let (workers, inbox) = mpsc::unbounded_channel();
    ends.workers = Some(inbox);
    let workers = WorkerFactory::new(workers);
    for page_data in pages {
        let (outbox, far_end) = detached_outbox(Arc::new(NoWakeup));
        let (runtime, worker_events) = MainThreadRuntime::new(
            &mut js_runtime,
            ingredients(),
            outbox,
            &workers,
            "app:///main.js",
            None,
            page_data,
        )
        .expect("main-thread runtime");
        ends.views.push(far_end);
        ends.worker_events.push(worker_events);
        views.push(runtime);
    }
    let second = views.pop().expect("the second view");
    let first = views.pop().expect("the first view");
    (js_runtime, first, second, ends)
}

/// The far ends of a group's channels, held so the realms' sends succeed.
#[derive(Default)]
struct GroupFarEnds {
    views: Vec<DetachedView>,
    workers: Option<mpsc::UnboundedReceiver<WorkerCommand>>,
    worker_events: Vec<mpsc::UnboundedReceiver<WorkerEvent>>,
}

/// The host's page data reaches the realm it was given to as plain strings,
/// and `bobcat:runtime` parses them there: the entry sees the global props as
/// it loads, and `processData` gets the init data. A view given none sees
/// `{}` for both, never its sibling's.
#[test]
fn page_data_is_parsed_by_the_realm_it_was_given_to() {
    let (mut js, mut first, mut second, _workers) = two_view_group_with([
        PageData {
            init_data: Some(r#"{"count": 2, "text": "中文 🦀"}"#.to_owned()),
            global_props: Some(r#"{"theme": "dark"}"#.to_owned()),
        },
        PageData::default(),
    ]);
    first
        .run_main_thread_script(
            &mut js,
            r"
            if (__globalProps.theme !== 'dark' || lynx.__globalProps !== __globalProps) {
              throw new Error('global props: ' + JSON.stringify(__globalProps));
            }
            globalThis.processData = function (data) {
              if (data.count !== 2 || data.text !== '中文 🦀') {
                throw new Error('init data: ' + JSON.stringify(data));
              }
              return data.count;
            };
            globalThis.renderPage = function (processed) {
              if (processed !== 2) throw new Error('renderPage got ' + processed);
            };
            ",
            "app:///first.js",
        )
        .expect("the first view boots over its page data");
    second
        .run_main_thread_script(
            &mut js,
            r"
            if (JSON.stringify(__globalProps) !== '{}' || lynx.__globalProps !== __globalProps) {
              throw new Error('global props: ' + JSON.stringify(__globalProps));
            }
            globalThis.renderPage = function (data) {
              if (JSON.stringify(data) !== '{}') throw new Error('init data: ' + JSON.stringify(data));
            };
            ",
            "app:///second.js",
        )
        .expect("a view given no page data boots over empty objects");
}

/// Nothing native reads page data, so text that is not JSON is first met in
/// the realm, as `bobcat:runtime` evaluates: that view's boot fails naming the
/// input, before the entry loads.
#[test]
fn malformed_page_data_fails_boot_before_the_entry_runs() {
    let (mut js, first, second, _workers) = two_view_group_with([
        PageData {
            init_data: Some("{".to_owned()),
            global_props: None,
        },
        PageData {
            init_data: None,
            global_props: Some("[1,".to_owned()),
        },
    ]);
    for (mut runtime, named) in [
        (first, "initData is not valid JSON"),
        (second, "globalProps is not valid JSON"),
    ] {
        let failure = runtime
            .run_main_thread_script(&mut js, "globalThis.entered = true;", "app:///main.js")
            .expect_err("page data that is not JSON fails boot");
        assert!(failure.to_string().contains(named), "{failure}");
        runtime
            .evaluate_module(
                &mut js,
                "if (globalThis.entered) throw new Error('the entry ran');",
                "app:///verify.js",
                "verifying",
            )
            .expect("the entry never loaded");
    }
}

/// One view's entry failing must not fail the view beside it.
///
/// The two realms share one `QuickJS` runtime, and therefore one promise-job
/// queue and one unhandled-rejection queue. Boot loads the entry with
/// `await import(...)`, so an entry that throws rejects *through* that queue,
/// and what it leaves there outlasts the failure its own caller was handed.
/// Left runtime-wide, the next realm to enter — this one's boot, its event
/// dispatch, its timer — would be handed a failure it could not have caused.
#[test]
fn a_failed_boot_leaves_the_group_s_other_view_alone() {
    let (mut js_runtime, mut first, mut second, _workers) = two_view_group();

    let failure = first
        .run_main_thread_script(
            &mut js_runtime,
            r"
                Promise.reject(new Error('first view: floating'));
                throw new Error('first view: boot');
                ",
            "app:///first.js",
        )
        .expect_err("the first view's entry throws");
    assert!(
        failure.to_string().contains("first view"),
        "the first view hears its own failure: {failure}"
    );

    second
        .run_main_thread_script(
            &mut js_runtime,
            r"
                globalThis.seen = [];
                globalThis.renderPage = function () {
                  const page = __CreatePage('card', 0);
                  globalThis.held = [page];
                  __AddEventListener(page, 'tap', () => seen.push('tap'), {});
                  setTimeout(() => seen.push('timer'), 0);
                };
                ",
            "app:///second.js",
        )
        .expect("the second view boots on a runtime the first one failed on");

    assert!(
        second
            .dispatch_event(&mut js_runtime, node_id(2), &tap(), &no_detail())
            .expect("the second view's dispatch is not the first view's failure"),
        "the listener ran"
    );
    assert!(
        second.run_due_timers(&mut js_runtime).is_empty(),
        "the second view's timer callback is not the first view's failure"
    );

    second
        .evaluate_module(
            &mut js_runtime,
            "if (seen.join('|') !== 'tap|timer') throw new Error(seen.join('|'));",
            "app:///verify.js",
            "verifying",
        )
        .expect("both entry points ran, and ran the second view's own code");
}

#[test]
fn element_papi_boot_builds_the_private_tree() {
    let (mut js_runtime, mut runtime, elements) = runtime();
    runtime
        .run_main_thread_script(
            &mut js_runtime,
            r"
                globalThis.renderPage = function () {
                  const page = __CreatePage('card', 0);
                  __AppendElement(page, __CreateView(0));
                };
                ",
            "app:///main.js",
        )
        .expect("boot");

    assert!(elements.tree().get(node_id(3)).is_some());
}

#[test]
fn boot_dispatches_render_page_when_the_entry_has_no_global_function() {
    let (mut js_runtime, mut runtime, elements) = runtime();
    runtime
        .run_main_thread_script(
            &mut js_runtime,
            r"
                const engine = lynx.getEngine();
                const page = __CreatePage('card', 0);
                globalThis.processData = function () {
                  return 42;
                };
                engine.addEventListener('__RenderPage', function (event) {
                  if (this !== engine || event.type !== '__RenderPage' || event.data !== 42) {
                    throw new Error('the engine render event lost its target or processed data');
                  }
                  __AppendElement(page, __CreateView(0));
                });
                ",
            "app:///engine-render.js",
        )
        .expect("engine render-event boot");

    assert!(elements.tree().get(node_id(3)).is_some());
}

#[test]
fn boot_allows_an_entry_with_neither_render_path() {
    let (mut js_runtime, mut runtime, elements) = runtime();
    runtime
        .run_main_thread_script(
            &mut js_runtime,
            "if ('renderPage' in globalThis) throw new Error('unexpected global');",
            "app:///no-render.js",
        )
        .expect("an entry is not required to assign renderPage or register a listener");

    assert!(
        elements.tree().document_element().child_ids().is_empty(),
        "an unhandled render event must leave the permanent page empty"
    );
}

#[test]
fn boot_awaits_the_esm_entry_before_rendering_once() {
    let (mut js_runtime, mut runtime, elements) = runtime();
    runtime
        .run_main_thread_script(
            &mut js_runtime,
            r"
                import { __CreateView as createView } from 'bobcat:element';
                await Promise.resolve();
                if (typeof globalThis.__CreateView !== 'undefined') {
                  throw new Error('Element PAPI must be ESM-only');
                }
                lynx.getEngine().addEventListener('__RenderPage', function () {
                  throw new Error('the fallback event must not accompany a global renderPage');
                });
                let renderCount = 0;
                globalThis.renderPage = function () {
                  renderCount += 1;
                  if (renderCount !== 1) {
                    throw new Error('renderPage ran more than once');
                  }
                  const page = __CreatePage('card', 0);
                  __AppendElement(page, createView(0));
                };
                ",
            "app:///async-entry.mjs",
        )
        .expect("top-level-await entry boot");

    assert!(elements.tree().get(node_id(3)).is_some());
}

#[test]
fn imported_runtime_bindings_supply_bridges_without_globals() {
    let (mut js_runtime, mut runtime, _elements) = runtime();
    runtime
        .run_main_thread_script(
            &mut js_runtime,
            r"
                if (lynx.SystemInfo !== SystemInfo || !Object.isFrozen(SystemInfo)) {
                  throw new Error('SystemInfo must be one frozen shared snapshot');
                }
                if (lynx.__globalProps !== __globalProps) {
                  throw new Error('the bare and lynx global props must share identity');
                }
                if (lynx.__initData === null || typeof lynx.__initData !== 'object') {
                  throw new Error('init data must start as an empty object');
                }
                if (NativeModules !== undefined) {
                  throw new Error('the imported native-module sentinel must be undefined');
                }
                for (const name of [
                  'lynx', 'SystemInfo', '__globalProps', 'NativeModules',
                  '_AddEventListener', '_ReportError', '_SetSourceMapRelease',
                  '__OnLifecycleEvent', 'bobcat'
                ]) {
                  if (name in globalThis) {
                    throw new Error(name + ' must be supplied only by the injected import');
                  }
                }
                if (typeof lynxCoreInject !== 'undefined') {
                  throw new Error('the background-thread injection must not leak into this realm');
                }
                if (typeof globDynamicComponentEntry !== 'undefined') {
                  throw new Error('the dynamic-chunk entry must not leak into the card realm');
                }
                if (typeof __SetCSSId !== 'function' ||
                    __SetCSSId([], 0, 'entry') !== undefined) {
                  throw new Error('the scoped-style PAPI must accept its call and record nothing');
                }

                const core = lynx.getCoreContext();
                const js = lynx.getJSContext();
                const native = lynx.getNative();
                if (core === js || core === native || js === native) {
                  throw new Error('the three context directions must remain distinct');
                }
                for (const [name, context, again] of [
                  ['core', core, lynx.getCoreContext()],
                  ['js', js, lynx.getJSContext()],
                  ['native', native, lynx.getNative()]
                ]) {
                  if (context !== again) {
                    throw new Error(name + ' context identity must be stable');
                  }
                  context.postMessage({});
                  context.addEventListener('ignored', function () {});
                  context.removeEventListener('ignored', function () {});
                  if (context.dispatchEvent({ type: 'ignored', data: {} }) !== (name === 'js' ? 0 : 3)) {
                    throw new Error(name + ' context returned the wrong delivery result');
                  }
                }

                const emitter = lynx.getJSModule('GlobalEventEmitter');
                if (emitter !== lynx.getJSModule('GlobalEventEmitter') ||
                    lynx.getJSModule('missing') !== undefined) {
                  throw new Error('only the stable empty global-event module is exposed');
                }
                for (const method of [
                  'addListener', 'removeListener', 'removeAllListeners',
                  'emit', 'trigger', 'toggle'
                ]) {
                  emitter[method]('ignored', function () {});
                }

                if (lynx.performance.isProfileRecording() !== false ||
                    lynx.performance.profileFlowId() !== 0 ||
                    lynx.performance._generatePipelineOptions() !== undefined) {
                  throw new Error('the performance shell must stay inert');
                }
                _AddEventListener('ignored', function () {});
                _ReportError(new Error('ignored'));
                _SetSourceMapRelease({ release: 'ignored' });
                __OnLifecycleEvent(['ignored', {}]);

                globalThis.renderPage = function () {
                  __CreatePage('card', 0);
                };
                ",
            "app:///runtime-imports.mjs",
        )
        .expect("imported runtime bindings");
}

#[test]
fn get_engine_returns_one_event_target_with_standard_listener_identity() {
    let (mut js_runtime, mut runtime, _elements) = runtime();
    runtime
        .run_main_thread_script(
            &mut js_runtime,
            r"
                const engine = lynx.getEngine();
                if (engine !== lynx.getEngine() ||
                    Object.prototype.toString.call(engine) !== '[object EventTarget]') {
                  throw new Error('getEngine must return one stable EventTarget');
                }
                const probe = { type: 'probe', data: 7 };
                const calls = [];
                function listener(event) {
                  if (this !== engine || event !== probe) {
                    throw new Error('function listeners need EventTarget receiver semantics');
                  }
                  calls.push('function');
                }
                const objectListener = {
                  handleEvent(event) {
                    if (this !== objectListener || event !== probe) {
                      throw new Error('listener objects need handleEvent receiver semantics');
                    }
                    calls.push('object');
                  }
                };
                engine.addEventListener('probe', listener);
                engine.addEventListener('probe', listener);
                engine.addEventListener('probe', listener, { capture: true, once: true });
                engine.addEventListener('probe', objectListener, { once: true });
                if (engine.dispatchEvent(probe) !== true ||
                    calls.join(',') !== 'function,function,object') {
                  throw new Error('engine listener identity or first dispatch is wrong: ' + calls);
                }
                calls.length = 0;
                engine.dispatchEvent(probe);
                if (calls.join(',') !== 'function') {
                  throw new Error('once listeners must leave only the persistent listener');
                }
                engine.removeEventListener('probe', listener);
                calls.length = 0;
                engine.dispatchEvent(probe);
                if (calls.length !== 0) {
                  throw new Error('removeEventListener must remove the matching listener');
                }
                ",
            "app:///engine-event-target.mjs",
        )
        .expect("engine EventTarget behavior");
}

#[test]
fn bundle_url_reaches_script_error_location() {
    let (mut js_runtime, mut runtime, _elements) = runtime();
    let error = runtime
        .run_main_thread_script(&mut js_runtime, "const = 1", "app:///broken.js")
        .expect_err("syntax error");

    assert!(
        error
            .source
            .location
            .as_ref()
            .and_then(|location| location.source.as_deref())
            .is_some_and(|source| source == "app:///broken.js")
    );
}

#[test]
fn stale_element_ids_become_script_errors_without_losing_the_tree() {
    let (mut js_runtime, mut runtime, elements) = runtime();
    let error = runtime
        .run_main_thread_script(
            &mut js_runtime,
            r"
                import { removeElement } from 'bobcat-internal:host';
                globalThis.renderPage = function () {
                  removeElement(999999);
                };
                ",
            "app:///invalid-tree-operation.js",
        )
        .expect_err("a stale id must be refused");

    assert!(error.source.message.contains("stale element id"));
    assert!(
        elements.tree().get(node_id(2)).is_some(),
        "a rejected callback leaves the document usable"
    );
}

/// The number script holds *is* the DOM's `NodeId` and the element's
/// Lynx `unique_id` — one identity, issued by native — and dropping an
/// element retires it. The element built afterwards reuses the freed
/// node's storage but reports a different `unique_id`, so a handle that
/// outlived its element can only ever name nothing.
#[test]
fn a_collected_element_retires_its_unique_id_instead_of_lending_it_out() {
    let (mut js_runtime, mut runtime, elements) = runtime();
    runtime
        .run_main_thread_script(
            &mut js_runtime,
            r"
                globalThis.renderPage = function () {
                  const page = __CreatePage('card', 0);
                  let doomed = __CreateView(0);
                  __AppendElement(page, doomed);
                  if (__GetElementUniqueID(doomed) !== 3) {
                    throw new Error(
                      'the first element is node 3, got ' + __GetElementUniqueID(doomed),
                    );
                  }
                  __RemoveElement(page, doomed);
                  doomed = undefined;
                };
                ",
            "app:///collected.js",
        )
        .expect("main-thread script");
    assert!(
        elements.tree().get(node_id(3)).is_some(),
        "the detached element is still allocated while script could reach it"
    );

    runtime
        .collect_garbage(&mut js_runtime)
        .expect("collection");
    assert!(
        elements.tree().get(node_id(3)).is_none(),
        "a swept handle drops its element through the finalization registry"
    );

    // A second module rather than a second boot: a realm boots once, because
    // its boot module is what creates its one document.
    runtime
        .evaluate_module(
            &mut js_runtime,
            r"
                import { __AppendElement, __CreatePage, __CreateView, __GetElementUniqueID }
                  from 'bobcat:element';
                const page = __CreatePage('card', 0);
                const replacement = __CreateView(0);
                __AppendElement(page, replacement);
                if (__GetElementUniqueID(replacement) === 3) {
                  throw new Error('a retired unique id was handed to a new element');
                }
                ",
            "app:///replacement.mjs",
            "replacing",
        )
        .expect("the replacement module runs");
    assert!(
        elements.tree().get(node_id(3)).is_none(),
        "and the retired id keeps naming nothing"
    );
}

#[test]
fn classes_attributes_and_identity_queries_reach_the_private_document() {
    let (mut js_runtime, mut runtime, elements) = runtime();
    runtime
        .run_main_thread_script(
            &mut js_runtime,
            r"
                globalThis.renderPage = function () {
                  const page = __CreatePage('card', 0);
                  const view = __CreateView(0);
                  __AppendElement(page, view);
                  __SetClasses(view, 'row bold');
                  __SetID(view, 'header');
                  __SetAttribute(view, 'flex-grow', 1);
                  if (__GetID(view) !== 'header') {
                    throw new Error('__GetID must read the id back, got ' + __GetID(view));
                  }
                  if (__GetTag(view) !== 'view' || __GetTag(page) !== 'page') {
                    throw new Error('__GetTag must report the Lynx tag');
                  }
                  if (__GetElementUniqueID(page) !== 2) {
                    throw new Error('the page is node 2, got ' + __GetElementUniqueID(page));
                  }
                };
                ",
            "app:///properties.js",
        )
        .expect("main-thread script");

    let elements = elements.tree();
    let view = elements.get(node_id(3)).expect("the view is live");
    assert_eq!(view.classes().collect::<Vec<_>>(), ["row", "bold"]);
    assert_eq!(view.id_attribute(), Some("header"));
    assert_eq!(view.attribute("flex-grow"), Some("1"));
    assert_eq!(view.tag_name(), Some("view"));
}

#[test]
fn clearing_a_class_id_or_attribute_removes_it_from_the_private_document() {
    let (mut js_runtime, mut runtime, elements) = runtime();
    runtime
        .run_main_thread_script(
            &mut js_runtime,
            r"
                globalThis.renderPage = function () {
                  const page = __CreatePage('card', 0);
                  const view = __CreateView(0);
                  __AppendElement(page, view);
                  __SetClasses(view, 'row');
                  __SetID(view, 'header');
                  __SetAttribute(view, 'text', 'hello');
                  __SetClasses(view, '');
                  __SetID(view, null);
                  __SetAttribute(view, 'text', undefined);
                  if (__GetID(view) !== null) {
                    throw new Error('__GetID must report null once the id is cleared');
                  }
                };
                ",
            "app:///clear-properties.js",
        )
        .expect("main-thread script");

    let elements = elements.tree();
    let view = elements.get(node_id(3)).expect("the view is live");
    assert_eq!(view.classes().len(), 0);
    assert_eq!(view.id_attribute(), None);
    assert_eq!(view.attribute("text"), None);
}

#[test]
fn inline_styles_reach_computed_style_and_layout() {
    let (mut js_runtime, mut runtime, elements) = runtime();
    runtime
        .run_main_thread_script(
            &mut js_runtime,
            r"
                globalThis.renderPage = function () {
                  const page = __CreatePage('card', 0);
                  const fromString = __CreateView(0);
                  const fromRecord = __CreateView(0);
                  __AppendElement(page, fromString);
                  __AppendElement(page, fromRecord);
                  __SetInlineStyles(fromString, 'width:10px;height:10px');
                  __SetInlineStyles(fromRecord, { width: '20px', height: '20px' });
                };
                ",
            "app:///inline-styles.js",
        )
        .expect("main-thread script");

    let elements = elements.tree();
    for (id, expected) in [(node_id(3), 10.0_f32), (node_id(4), 20.0_f32)] {
        let layout = elements
            .rounded_layout(id)
            .expect("the styled view is laid out");
        assert!(
            (layout.size.width - expected).abs() < f32::EPSILON,
            "node {id} width {} should be {expected}",
            layout.size.width
        );
    }
}

#[test]
fn adding_inline_styles_preserves_other_properties_and_uses_cssom_validation() {
    let (mut js_runtime, mut runtime, elements) = runtime();
    runtime
        .run_main_thread_script(
            &mut js_runtime,
            r"
                const page = __CreatePage();
                const view = __CreateView();
                __AppendElement(page, view);
                __SetInlineStyles(view, 'width:10px;height:14px;padding:9px');
                __AddInlineStyle(view, 'width', '20px');
                __AddInlineStyle(view, 'width', '30px; height:99px');
                __AddInlineStyle(view, 'height', 'not-a-length');
                __AddInlineStyle(view, 'padding', null);
                __AddInlineStyle(view, '--accent', 'green');
                __AddInlineStyle(view, 'unknown-property', 'yes');
                let rejected = false;
                try { __AddInlineStyle(view, 1, '30px'); }
                catch (error) { rejected = error instanceof TypeError; }
                if (!rejected) throw Error('numeric native CSS IDs must be rejected');
                __FlushElementTree();
            ",
            "app:///add-inline-style.js",
        )
        .expect("main-thread script");
    let elements = elements.tree();
    let view = elements.get(node_id(3)).expect("the view is live");
    let style = view.attribute("style").expect("inline style remains");
    assert!(style.contains("--accent: green"), "{style}");
    assert!(!style.contains("padding"), "{style}");
    assert!(!style.contains("unknown-property"), "{style}");
    let layout = elements
        .rounded_layout(node_id(3))
        .expect("view is laid out");
    assert!(
        (layout.size.width - 20.0).abs() < f32::EPSILON,
        "{layout:?}"
    );
    assert!(
        (layout.size.height - 14.0).abs() < f32::EPSILON,
        "{layout:?}"
    );
}

#[test]
fn record_inline_styles_are_resolved_by_name_before_reaching_stylo() {
    let (mut js_runtime, mut runtime, elements) = runtime();
    runtime
        .run_main_thread_script(
            &mut js_runtime,
            r"
                globalThis.renderPage = function () {
                  const page = __CreatePage('card', 0);
                  const view = __CreateView(0);
                  __AppendElement(page, view);
                  __SetInlineStyles(view, {
                    paddingLeft: '4px',
                    '--accentColor': 'tomato',
                    color: null,
                    width: undefined,
                    definitelyNotAProperty: 'value',
                    height: 'not-a-length',
                  });
                };
                ",
            "app:///record-style.js",
        )
        .expect("main-thread script");
    let elements = elements.tree();
    let style = elements
        .get(node_id(3))
        .expect("the view is live")
        .attribute("style")
        .expect("valid single-property updates create an inline style");
    assert!(style.contains("padding-left: 4px"), "{style}");
    assert!(style.contains("--accentColor: tomato"), "{style}");
    assert!(!style.contains("definitely"), "{style}");
    assert!(!style.contains("height"), "{style}");
    assert!(
        !style
            .split(';')
            .any(|declaration| declaration.trim_start().starts_with("color:")),
        "{style}"
    );
}

#[test]
fn a_style_record_value_carries_delimiters_and_non_bmp_text_intact() {
    let (mut js_runtime, mut runtime, elements) = runtime();
    runtime
        .run_main_thread_script(
            &mut js_runtime,
            r"
                globalThis.renderPage = function () {
                  const page = __CreatePage('card', 0);
                  const view = __CreateView(0);
                  __AppendElement(page, view);
                  __SetInlineStyles(view, {
                    '--separators': 'a:b 3:x 11:y',
                    '--astral': '\u{1F980}',
                    width: '7px',
                  });
                };
                ",
            "app:///delimiter-style.js",
        )
        .expect("main-thread script");

    let elements = elements.tree();
    let style = elements
        .get(node_id(3))
        .expect("the view is live")
        .attribute("style")
        .expect("the record produced an inline style");
    assert!(style.contains("--separators: a:b 3:x 11:y"), "{style}");
    assert!(style.contains("--astral: \u{1F980}"), "{style}");
    assert!(style.contains("width: 7px"), "{style}");
}

/// A value the per-property setter would reject must stay rejected: a
/// batch that concatenated the record into style-attribute text would let
/// a `;` start a second declaration instead.
#[test]
fn a_style_record_value_cannot_inject_a_second_declaration() {
    let (mut js_runtime, mut runtime, elements) = runtime();
    runtime
        .run_main_thread_script(
            &mut js_runtime,
            r"
                globalThis.renderPage = function () {
                  const page = __CreatePage('card', 0);
                  const view = __CreateView(0);
                  __AppendElement(page, view);
                  __SetInlineStyles(view, { width: '5px; height: 9px' });
                };
                ",
            "app:///injection-style.js",
        )
        .expect("main-thread script");

    let elements = elements.tree();
    let view = elements.get(node_id(3)).expect("the view is live");
    let style = view
        .attribute("style")
        .expect("an empty block is still set");
    assert!(!style.contains("height"), "{style}");
    assert!(!style.contains("width"), "{style}");
}

#[test]
fn a_malformed_style_record_is_a_boundary_error_rather_than_a_guess() {
    for payload in ["4:ab", "notalength:x0:", "3:ab", "2:ab", "1:\u{1F980}x0:"] {
        assert!(
            split_style_record("bobcat.setInlineStyles", payload).is_err(),
            "{payload:?}"
        );
    }
}

#[test]
fn a_style_record_splits_on_lengths_rather_than_delimiters() {
    let payload = "5:width4:10px11:font-family9:a;b:c 3:x";
    assert_eq!(
        split_style_record("bobcat.setInlineStyles", payload).expect("well-formed")[..],
        [("width", "10px"), ("font-family", "a;b:c 3:x")]
    );
    assert!(
        split_style_record("bobcat.setInlineStyles", "")
            .expect("an empty record")
            .is_empty()
    );
    assert_eq!(
        split_style_record("bobcat.setInlineStyles", "7:--empty0:").expect("empty value")[..],
        [("--empty", "")]
    );
}

#[test]
fn a_later_inline_style_record_replaces_the_complete_declaration_block() {
    let (mut js_runtime, mut runtime, elements) = runtime();
    runtime
        .run_main_thread_script(
            &mut js_runtime,
            r"
                globalThis.renderPage = function () {
                  const page = __CreatePage('card', 0);
                  const view = __CreateView(0);
                  __AppendElement(page, view);
                  __SetInlineStyles(view, { width: '10px', height: '20px' });
                  __SetInlineStyles(view, { height: '30px' });
                };
                ",
            "app:///replace-record-style.js",
        )
        .expect("main-thread script");

    let elements = elements.tree();
    let view = elements.get(node_id(3)).expect("the view is live");
    let style = view.attribute("style").expect("height remains inline");
    assert!(!style.contains("width"), "{style}");
    assert!(style.contains("height: 30px"), "{style}");
    let layout = elements
        .rounded_layout(node_id(3))
        .expect("the view is laid out");
    assert!((layout.size.width - 393.0).abs() < f32::EPSILON);
    assert!((layout.size.height - 30.0).abs() < f32::EPSILON);
}

#[test]
fn clearing_inline_styles_removes_the_attribute_and_layout_effect() {
    let (mut js_runtime, mut runtime, elements) = runtime();
    runtime
        .run_main_thread_script(
            &mut js_runtime,
            r"
                globalThis.renderPage = function () {
                  const page = __CreatePage('card', 0);
                  const view = __CreateView(0);
                  __AppendElement(page, view);
                  __SetInlineStyles(view, 'width:10px');
                  __SetInlineStyles(view, undefined);
                };
                ",
            "app:///clear-style.js",
        )
        .expect("main-thread script");

    let elements = elements.tree();
    let view = elements.get(node_id(3)).expect("the view is live");
    assert_eq!(view.attribute("style"), None);
    let layout = elements
        .rounded_layout(node_id(3))
        .expect("the view is laid out");
    assert!(
        (layout.size.width - 393.0).abs() < f32::EPSILON,
        "the cleared width falls back to the page's, got {}",
        layout.size.width
    );
}

/// The two indexes are one fact written twice, so every mutation has to
/// leave them agreeing — and the painting side has to hear exactly the
/// global edges of the name set, no more and no fewer.
#[test]
fn the_listener_indexes_and_the_published_edges_stay_in_step() {
    let (outbox, mut far_end) = detached_outbox(Arc::new(NoWakeup));
    let state = EventState::new(outbox);
    let (a, b) = (node_id(3), node_id(4));
    // One call at a time, so what crossed and what did not is asserted per
    // registration rather than inferred from the set afterwards. The names
    // come back sorted because a set has no order of its own.
    let published = |observer: &mut ViewObserver| {
        observer.sync();
        let mut names: Vec<String> = observer
            .listener_names()
            .iter()
            .map(std::string::ToString::to_string)
            .collect();
        names.sort();
        names
    };
    let edge = |observer: &mut ViewObserver| observer.take_published_edge();

    state.enable(a, "tap", false);
    assert!(
        edge(&mut far_end.published),
        "a name's first registration crosses"
    );
    state.enable(a, "tap", true);
    assert!(
        !edge(&mut far_end.published),
        "its second anywhere does not"
    );
    state.enable(a, "scroll", false);
    assert!(edge(&mut far_end.published));
    state.enable(b, "tap", false);
    assert!(!edge(&mut far_end.published));
    assert_eq!(published(&mut far_end.published), ["scroll", "tap"]);
    assert_eq!(state.by_node.borrow()[&a].len(), 3);
    assert_eq!(state.by_node.borrow()[&b].len(), 1);

    // A repeat registration is not a second one — neither index moves,
    // so no edge is published either.
    state.enable(a, "tap", false);
    assert_eq!(state.by_node.borrow()[&a].len(), 3);
    assert!(
        !edge(&mut far_end.published),
        "a repeat registration publishes nothing"
    );

    state.disable(a, "scroll", false);
    assert!(
        edge(&mut far_end.published),
        "the last removal closes the name"
    );
    assert_eq!(published(&mut far_end.published), ["tap"]);
    assert_eq!(state.by_node.borrow()[&a].len(), 2);

    // Dropping an element takes its own registrations and only those.
    state.forget_node(a);
    assert!(!state.by_node.borrow().contains_key(&a));
    assert!(
        !edge(&mut far_end.published),
        "the sibling registration still holds the name open"
    );
    assert_eq!(
        state.listeners.borrow()["tap"]
            .iter()
            .copied()
            .collect::<Vec<_>>(),
        vec![(b, false)]
    );

    state.forget_node(b);
    assert!(state.listeners.borrow().is_empty());
    assert!(state.by_node.borrow().is_empty());
    assert!(
        edge(&mut far_end.published),
        "the last listener unpublishes its name"
    );
    assert!(published(&mut far_end.published).is_empty());
}

/// The replica is what the painting side filters against, so a
/// registration has to reach it as the realm makes it.
#[test]
fn registering_a_listener_publishes_its_name_to_the_painting_side() {
    let (mut js_runtime, mut runtime, _elements, mut names) =
        runtime_over_watching_names(ingredients());
    runtime
        .run_main_thread_script(
            &mut js_runtime,
            r"
                globalThis.renderPage = function () {
                  const page = __CreatePage('card', 0);
                  const view = __CreateView(0);
                  __AppendElement(page, view);
                  globalThis.held = [page, view];
                  __AddEventListener(view, 'tap', () => {}, {});
                };
                ",
            "app:///publish.js",
        )
        .expect("main-thread script");
    assert!(names.contains("tap"));
    assert!(!names.contains("scroll"));

    // A second module rather than a second entry: the point is a later
    // unregistration, not a second boot.
    runtime
        .evaluate_module(
            &mut js_runtime,
            r"
                import { __GetElementUniqueID } from 'bobcat:element';
                import { disableEventListener } from 'bobcat-internal:host';
                disableEventListener(__GetElementUniqueID(globalThis.held[1]), 0, 'tap');
                ",
            "app:///unpublish.mjs",
            "unpublishing",
        )
        .expect("unregistration");
    assert!(
        !names.contains("tap"),
        "the last listener for a name unpublishes it"
    );
}

#[test]
fn a_dispatch_reaches_only_the_nodes_that_registered_a_listener() {
    let (mut js_runtime, mut runtime, _elements) = runtime();
    runtime
        .run_main_thread_script(
            &mut js_runtime,
            r"
                globalThis.seen = [];
                globalThis.renderPage = function () {
                  const page = __CreatePage('card', 0);
                  const outer = __CreateView(0);
                  const inner = __CreateView(0);
                  __AppendElement(page, outer);
                  __AppendElement(outer, inner);
                  // A registration is weak by its handle, so an app that wants
                  // its listeners to survive holds its elements. ReactLynx's
                  // snapshot instances do; this stands in for them.
                  globalThis.held = [page, outer, inner];
                  const note = (label) => (event) =>
                    seen.push(label + ':' + event.currentTarget.uid + ':' + event.eventPhase);
                  __AddEventListener(page, 'tap', note('page-capture'), { capture: true });
                  __AddEventListener(inner, 'tap', note('inner'), {});
                  // `outer` registers nothing, so the walk must skip it.
                };
                ",
            "app:///listeners.js",
        )
        .expect("main-thread script");

    let target = 4;
    let delivered = runtime
        .dispatch_event(
            &mut js_runtime,
            node_id(target),
            &tap(),
            &Arc::from("{\"x\":1}"),
        )
        .expect("dispatch");
    assert!(delivered);

    runtime
        .evaluate_module(
            &mut js_runtime,
            r"
                if (seen.join('|') !== 'page-capture:2:1|inner:4:2') {
                  throw new Error('unexpected deliveries: ' + seen.join('|'));
                }
                ",
            "app:///verify.js",
            "verifying",
        )
        .expect("verification");
}

#[test]
fn add_event_registers_against_the_real_index_and_a_catch_form_ends_the_walk() {
    let (mut js_runtime, mut runtime, _elements) = runtime();
    runtime
        .run_main_thread_script(
            &mut js_runtime,
            r"
                globalThis.seen = [];
                // A card's own worklet runtime installs this; `__AddEvent`
                // reaches for it per delivery, since a worklet is the only
                // handler kind that runs in this realm.
                globalThis.runWorklet = (value, params) => value.body(params[0]);
                globalThis.renderPage = function () {
                  const page = __CreatePage('card', 0);
                  const outer = __CreateView(0);
                  const inner = __CreateView(0);
                  __AppendElement(page, outer);
                  __AppendElement(outer, inner);
                  globalThis.held = [page, outer, inner];
                  const note = (label) => ({
                    type: 'worklet',
                    value: {
                      body: (event) =>
                        seen.push(label + ':' + event.currentTarget.uid),
                    },
                  });
                  // A catch form on the target, a plain bind on its ancestor:
                  // the second must never be reached, and only the host can
                  // decide that, from the `stopPropagation` the catch causes.
                  __AddEvent(inner, 'catchEvent', 'tap', note('inner-catch'));
                  __AddEvent(outer, 'bindEvent', 'tap', note('outer-bind'));
                  // The same node, same name, other pass: a separate index
                  // entry, and one the bubble walk must not reach.
                  __AddEvent(page, 'capture-bind', 'tap', note('page-capture'));
                };
                ",
            "app:///handlers.js",
        )
        .expect("main-thread script");

    assert!(
        runtime
            .dispatch_event(&mut js_runtime, node_id(4), &tap(), &no_detail())
            .expect("dispatch")
    );

    runtime
        .evaluate_module(
            &mut js_runtime,
            r"
                if (seen.join('|') !== 'page-capture:2|inner-catch:4') {
                  throw new Error('unexpected deliveries: ' + seen.join('|'));
                }
                ",
            "app:///verify.js",
            "verifying",
        )
        .expect("verification");
}

#[test]
fn a_replaced_add_event_handler_moves_its_node_between_passes() {
    let (mut js_runtime, mut runtime, _elements) = runtime();
    runtime
        .run_main_thread_script(
            &mut js_runtime,
            r"
                globalThis.seen = [];
                globalThis.runWorklet = (value, params) => value.body(params[0]);
                globalThis.renderPage = function () {
                  const page = __CreatePage('card', 0);
                  const inner = __CreateView(0);
                  __AppendElement(page, inner);
                  globalThis.held = [page, inner];
                  const note = (label) => ({
                    type: 'worklet',
                    value: { body: () => seen.push(label) },
                  });
                  // One name, one entry: the second call replaces the first
                  // outright, which also moves the node's index entry from the
                  // bubble pass to the capture one.
                  __AddEvent(inner, 'bindEvent', 'tap', note('bubble'));
                  __AddEvent(inner, 'capture-bind', 'tap', note('capture'));
                };
                ",
            "app:///handlers.js",
        )
        .expect("main-thread script");

    assert!(
        runtime
            .dispatch_event(&mut js_runtime, node_id(3), &tap(), &no_detail())
            .expect("dispatch")
    );

    runtime
        .evaluate_module(
            &mut js_runtime,
            r"
                import { __AddEvent, __GetEvent } from 'bobcat:element';
                if (seen.join('|') !== 'capture') {
                  throw new Error('unexpected deliveries: ' + seen.join('|'));
                }
                if (__GetEvent(held[1], 'tap', 'bindEvent') !== undefined) {
                  throw new Error('the replaced form must not still answer');
                }
                // Removing it leaves the node out of the index entirely, so a
                // further dispatch reaches nobody at all.
                __AddEvent(held[1], 'capture-bind', 'tap', undefined);
                ",
            "app:///verify.js",
            "verifying",
        )
        .expect("verification");

    assert!(
        !runtime
            .dispatch_event(&mut js_runtime, node_id(3), &tap(), &no_detail())
            .expect("dispatch")
    );
}

/// The id and the last-call flag are the two things a delivery carries
/// beyond the path itself, and both are observable from the realm: the id
/// is what makes one walk hold one event object, and the flag is what ends
/// the dispatch — which the standard makes visible by resetting
/// `eventPhase` and `currentTarget` on an event a listener kept.
#[test]
fn one_id_names_a_whole_walk_and_only_its_last_delivery_is_flagged() {
    let (mut js_runtime, mut runtime, _elements) = runtime();
    runtime
        .run_main_thread_script(
            &mut js_runtime,
            r"
                globalThis.seen = [];
                globalThis.renderPage = function () {
                  const page = __CreatePage('card', 0);
                  const outer = __CreateView(0);
                  const inner = __CreateView(0);
                  __AppendElement(page, outer);
                  __AppendElement(outer, inner);
                  globalThis.held = [page, outer, inner];
                  const record = (where) => (event) => {
                    seen.push({ where, event, phase: event.eventPhase });
                  };
                  __AddEventListener(page, 'tap', record('page'), { capture: true });
                  __AddEventListener(inner, 'tap', record('inner'), {});
                };
                ",
            "app:///listeners.js",
        )
        .expect("main-thread script");

    for _ in 0..2 {
        assert!(
            runtime
                .dispatch_event(&mut js_runtime, node_id(4), &tap(), &no_detail())
                .expect("dispatch")
        );
    }

    runtime
        .evaluate_module(
            &mut js_runtime,
            r"
                // Two deliveries per walk: `outer` registered nothing, so it
                // is on the path but never reached.
                const order = seen.map((step) => step.where).join('|');
                if (order !== 'page|inner|page|inner') {
                  throw new Error('deliveries: ' + order);
                }
                // One id, one event object — which is what lets a property one
                // listener writes reach the next.
                if (seen[0].event !== seen[1].event || seen[2].event !== seen[3].event) {
                  throw new Error('a walk minted more than one event');
                }
                if (seen[0].event === seen[2].event) {
                  throw new Error('two walks shared one event');
                }
                // Read while the dispatch was live: capturing at the ancestor,
                // at-target on the target itself.
                const phases = seen.map((step) => step.phase).join('|');
                if (phases !== '1|2|1|2') {
                  throw new Error('phases: ' + phases);
                }
                // The last delivery of each walk was flagged, so the realm
                // ended the dispatch rather than leaving the kept event still
                // naming whichever node it stopped on.
                for (const { event } of seen) {
                  if (event.eventPhase !== 0 || event.currentTarget !== null) {
                    throw new Error('a dispatch outlived its walk');
                  }
                }
                ",
            "app:///verify.js",
            "verifying",
        )
        .expect("verification");
}

#[test]
fn a_listener_may_mutate_the_tree_it_was_dispatched_on() {
    let (mut js_runtime, mut runtime, elements) = runtime();
    runtime
        .run_main_thread_script(
            &mut js_runtime,
            r"
                globalThis.renderPage = function () {
                  const page = __CreatePage('card', 0);
                  const view = __CreateView(0);
                  __AppendElement(page, view);
                  globalThis.held = [page, view];
                  __AddEventListener(view, 'tap', () => {
                    // The document is back in its slot while this runs, which
                    // is the whole reason the path is computed up front.
                    __SetAttribute(view, 'tapped', 'yes');
                  }, {});
                };
                ",
            "app:///mutate.js",
        )
        .expect("main-thread script");

    runtime
        .dispatch_event(&mut js_runtime, node_id(3), &tap(), &no_detail())
        .expect("dispatch");

    assert_eq!(
        elements
            .tree()
            .get(node_id(3))
            .expect("the view is live")
            .attribute("tapped"),
        Some("yes")
    );
}

#[test]
fn an_unrelated_element_being_collected_does_not_truncate_the_walk() {
    let (mut js_runtime, mut runtime, _elements) = runtime();
    runtime
        .run_main_thread_script(
            &mut js_runtime,
            r"
                globalThis.seen = [];
                globalThis.renderPage = function () {
                  const page = __CreatePage('card', 0);
                  const view = __CreateView(0);
                  __AppendElement(page, view);
                  // Detached and let go of: this is the element a sweep
                  // collects. Attached, the page's handle would keep it.
                  const doomed = __CreateView(0);
                  __AppendElement(page, doomed);
                  __RemoveElement(page, doomed);
                  globalThis.doomed = __GetElementUniqueID(doomed);
                  globalThis.held = [page, view];
                  __AddEventListener(page, 'tap', () => seen.push('page'), { capture: true });
                  __AddEventListener(view, 'tap', () => seen.push('view'), {});
                };
                ",
            "app:///collect.js",
        )
        .expect("main-thread script");

    // Collect the unrelated handle between building the path and running
    // the walk. The real finalizer performs the one `dropElement` call;
    // invoking it manually here would leave that finalizer armed and make
    // its later cleanup a duplicate stale-id call.
    runtime.collect_garbage(&mut js_runtime).expect("sweep");

    runtime
        .dispatch_event(&mut js_runtime, node_id(3), &tap(), &no_detail())
        .expect("dispatch");

    // A collected handle is routine — a ReactLynx re-render drops them
    // constantly — so it must not silently cost the rest of the walk.
    runtime
        .evaluate_module(
            &mut js_runtime,
            "if (seen.join('|') !== 'page|view') throw new Error('truncated: ' + seen.join('|'));",
            "app:///verify.js",
            "verifying",
        )
        .expect("verification");
}

#[test]
fn stopping_propagation_ends_the_walk() {
    let (mut js_runtime, mut runtime, _elements) = runtime();
    runtime
        .run_main_thread_script(
            &mut js_runtime,
            r"
                globalThis.seen = [];
                globalThis.renderPage = function () {
                  const page = __CreatePage('card', 0);
                  const view = __CreateView(0);
                  __AppendElement(page, view);
                  globalThis.held = [page, view];
                  __AddEventListener(page, 'tap', (event) => {
                    seen.push('page');
                    __StopPropagation(event);
                  }, { capture: true });
                  __AddEventListener(view, 'tap', () => seen.push('view'), {});
                };
                ",
            "app:///stop.js",
        )
        .expect("main-thread script");

    runtime
        .dispatch_event(&mut js_runtime, node_id(3), &tap(), &no_detail())
        .expect("dispatch");

    runtime
        .evaluate_module(
            &mut js_runtime,
            "if (seen.join('|') !== 'page') throw new Error('got ' + seen.join('|'));",
            "app:///verify.js",
            "verifying",
        )
        .expect("verification");
}

#[test]
fn a_document_whose_script_registered_nothing_never_enters_the_realm() {
    let (mut js_runtime, mut runtime, _elements) = runtime();
    runtime
        .run_main_thread_script(
            &mut js_runtime,
            r"
                globalThis.renderPage = function () {
                  const page = __CreatePage('card', 0);
                  __AppendElement(page, __CreateView(0));
                };
                ",
            "app:///quiet.js",
        )
        .expect("main-thread script");

    assert!(
        !runtime
            .dispatch_event(&mut js_runtime, node_id(3), &tap(), &no_detail())
            .expect("dispatch"),
        "with an empty listener index the walk crosses the boundary zero times"
    );
}

#[test]
fn a_raw_text_reaches_the_private_document_as_a_laid_out_run() {
    let (mut js_runtime, mut runtime, elements) = text_runtime();
    runtime
        .run_main_thread_script(
            &mut js_runtime,
            r"
                globalThis.renderPage = function () {
                  const page = __CreatePage('card', 0);
                  const text = __CreateText(0);
                  __SetInlineStyles(text, 'font-family:Ahem;font-size:20px');
                  __AppendElement(text, __CreateRawText('hello'));
                  __AppendElement(page, text);
                };
                ",
            "app:///raw-text.js",
        )
        .expect("main-thread script");

    let tree = elements.tree();
    let carrier = tree.get(node_id(4)).expect("the raw-text is live");
    assert_eq!(carrier.tag_name(), Some("raw-text"));
    assert_eq!(carrier.attribute("text"), Some("hello"));
    assert!(
        carrier.child_ids().is_empty(),
        "generated text is absent from the DOM"
    );

    // The run is content of the paragraph its `text` element owns, so the
    // measured size lives on the element (node 3), not on the text node.
    let measured = tree
        .text_block_size(node_id(3))
        .expect("the text element established a paragraph");
    assert!(
        (measured.width - 100.0).abs() < f32::EPSILON
            && (measured.height - 20.0).abs() < f32::EPSILON,
        "five Ahem em squares at 20px, got {measured:?}"
    );
    assert!(
        tree.rounded_layout(node_id(3))
            .is_some_and(|text| (text.size.height - 20.0).abs() < f32::EPSILON),
        "and the text element is sized by the run it contains"
    );
}

#[test]
#[expect(
    clippy::float_cmp,
    reason = "explicit line heights have exact pixel metrics"
)]
fn text_maxline_from_element_papi_limits_the_paragraph_and_its_box() {
    let (mut js_runtime, mut runtime, elements) = text_runtime();
    runtime
        .run_main_thread_script(
            &mut js_runtime,
            r"
                globalThis.renderPage = function () {
                  const page = __CreatePage('card', 0);
                  for (const limited of [false, true]) {
                    const text = __CreateText(0);
                    __SetID(text, limited ? 'limited' : 'unlimited');
                    __SetInlineStyles(text, 'font-family:Ahem;font-size:20px;line-height:21px');
                    __AppendElement(text, __CreateRawText('ab\ncd'));
                    if (limited) __SetAttribute(text, 'text-maxline', '1');
                    __AppendElement(page, text);
                  }
                };
                ",
            "app:///text-maxline.js",
        )
        .expect("main-thread script");

    let tree = elements.tree();
    for (name, height) in [("unlimited", 42.0), ("limited", 21.0)] {
        let id = tree
            .document_element()
            .children()
            .find(|node| node.attribute("id") == Some(name))
            .expect("the text element")
            .id();
        assert_eq!(
            tree.text_block_size(id)
                .expect("committed paragraph")
                .height,
            height,
            "{name}: the paragraph must measure only visible lines"
        );
        assert_eq!(
            tree.rounded_layout(id).expect("committed box").size.height,
            height,
            "{name}: the box must use the truncated paragraph's height"
        );
    }
}

#[test]
#[expect(
    clippy::float_cmp,
    reason = "explicit line heights have exact pixel metrics"
)]
fn text_limits_from_papi_relayout_after_an_earlier_flush() {
    for (attribute, width) in [("text-maxline", 40.0), ("text-maxlength", 20.0)] {
        let (mut js_runtime, mut runtime, elements) = text_runtime();
        runtime
            .run_main_thread_script(
                &mut js_runtime,
                &format!(
                    r"
                    globalThis.renderPage = function () {{
                      const page = __CreatePage('card', 0);
                      const text = __CreateText(0);
                      __SetInlineStyles(text, 'font-family:Ahem;font-size:20px;line-height:21px');
                      __AppendElement(text, __CreateRawText('ab\ncd'));
                      __AppendElement(page, text);
                      __FlushElementTree();
                      __SetAttribute(text, '{attribute}', '1');
                    }};
                    "
                ),
                "app:///text-limit-update.js",
            )
            .expect("main-thread script");
        let tree = elements.tree();
        let text = tree.document_element().first_child().unwrap().id();
        let paragraph = tree.text_block_size(text).expect("paragraph");
        assert_eq!(paragraph.height, 21.0, "{attribute}");
        assert_eq!(paragraph.width, width, "{attribute}");
        assert_eq!(tree.rounded_layout(text).unwrap().size.height, 21.0);
    }
}

#[test]
#[expect(
    clippy::float_cmp,
    reason = "explicit line heights have exact pixel metrics"
)]
fn replacing_inline_styles_preserves_attribute_text_limits() {
    for (attribute, width) in [("text-maxline", 40.0), ("text-maxlength", 20.0)] {
        let (mut js_runtime, mut runtime, elements) = text_runtime();
        runtime
            .run_main_thread_script(
                &mut js_runtime,
                &format!(
                    r"
                    globalThis.renderPage = function () {{
                      const page = __CreatePage('card', 0);
                      for (const flush of [false, true]) {{
                        for (const style of [
                          'font-family:Ahem;font-size:20px;line-height:21px',
                          {{fontFamily:'Ahem',fontSize:'20px',lineHeight:'21px'}},
                        ]) {{
                          const text = __CreateText(0);
                          __SetInlineStyles(text, 'font-family:Ahem;font-size:20px');
                          __SetAttribute(text, '{attribute}', '1');
                          __AppendElement(text, __CreateRawText('ab\ncd'));
                          __AppendElement(page, text);
                          if (flush) __FlushElementTree();
                          __SetInlineStyles(text, style);
                        }}
                      }}
                    }};
                    "
                ),
                "app:///text-limit-style-replacement.js",
            )
            .expect("main-thread script");
        let tree = elements.tree();
        for text in tree.document_element().children() {
            assert_eq!(text.attribute(attribute), Some("1"));
            assert!(!text.attribute("style").unwrap().contains("--lynx-text-"));
            let paragraph = tree.text_block_size(text.id()).expect("paragraph");
            assert_eq!(paragraph.height, 21.0, "{attribute}");
            assert_eq!(paragraph.width, width, "{attribute}");
            assert_eq!(tree.rounded_layout(text.id()).unwrap().size.height, 21.0);
        }
    }
}

#[test]
fn rewriting_the_text_attribute_updates_generated_content() {
    let (mut js_runtime, mut runtime, elements) = text_runtime();
    runtime
        .run_main_thread_script(
            &mut js_runtime,
            r"
                globalThis.renderPage = function () {
                  const page = __CreatePage('card', 0);
                  const text = __CreateText(0);
                  __SetInlineStyles(text, 'font-family:Ahem;font-size:20px');
                  const raw = __CreateRawText('hello');
                  __AppendElement(text, raw);
                  __AppendElement(page, text);
                  __SetAttribute(raw, 'text', 'hi');
                };
                ",
            "app:///update-raw-text.js",
        )
        .expect("main-thread script");

    let tree = elements.tree();
    let carrier = tree.get(node_id(4)).expect("carrier");
    assert_eq!(carrier.attribute("text"), Some("hi"));
    assert!(carrier.child_ids().is_empty());
    assert!(
        tree.text_block_size(node_id(3))
            .is_some_and(|size| (size.width - 40.0).abs() < f32::EPSILON),
        "the shorter run is re-measured, not left at its old width"
    );
}

#[test]
fn a_collected_raw_text_releases_its_attribute_content() {
    let (mut js_runtime, mut runtime, elements) = runtime();
    runtime
        .run_main_thread_script(
            &mut js_runtime,
            r"
                globalThis.renderPage = function () {
                  const page = __CreatePage('card', 0);
                  const text = __CreateText(0);
                  __AppendElement(page, text);
                  let raw = __CreateRawText('hello');
                  __AppendElement(text, raw);
                  __RemoveElement(text, raw);
                  raw = undefined;
                };
                ",
            "app:///collected-raw-text.js",
        )
        .expect("main-thread script");
    assert_eq!(
        elements.tree().get(node_id(4)).unwrap().attribute("text"),
        Some("hello")
    );
    assert!(
        elements
            .tree()
            .get(node_id(4))
            .unwrap()
            .child_ids()
            .is_empty()
    );

    runtime
        .collect_garbage(&mut js_runtime)
        .expect("collection");

    let tree = elements.tree();
    assert!(tree.get(node_id(4)).is_none(), "the carrier is freed");
    assert!(
        tree.get(node_id(5)).is_none(),
        "generated content never allocated a fifth node"
    );
}

/// A handle is what keeps an element alive, and while the element is
/// attached its handle is kept by its parent's — up to the permanent page
/// handle. So a `ReactLynx` list handing a recycled cell's elements
/// between snapshot instances, and deleting the old `__elements` array,
/// takes nothing away: the elements are on screen and their handles are
/// reachable from the page. What ends a subtree is detaching it and then
/// letting go.
#[test]
fn an_attached_element_s_handle_is_kept_by_its_parent_s() {
    let (mut js_runtime, mut runtime, elements) = runtime();
    runtime
        .run_main_thread_script(
            &mut js_runtime,
            r"
                globalThis.renderPage = function () {
                  const page = __CreatePage('card', 0);
                  const wrapper = __CreateView(0);
                  __AppendElement(page, wrapper);
                  let child = __CreateView(0);
                  __AppendElement(wrapper, child);
                  // The snapshot instance that created it lets go; the
                  // wrapper's handle is the one that holds it now.
                  child = undefined;
                  globalThis.wrapper = wrapper;
                };
                ",
            "app:///attached.js",
        )
        .expect("main-thread script");

    runtime
        .collect_garbage(&mut js_runtime)
        .expect("collection");
    assert_eq!(
        elements
            .tree()
            .get(node_id(4))
            .and_then(dom::Node::parent_id),
        Some(node_id(3)),
        "the element script let go of is still attached under its parent"
    );

    runtime
        .evaluate_module(
            &mut js_runtime,
            "import { __CreatePage, __RemoveElement } from 'bobcat:element';
                 __RemoveElement(__CreatePage('card', 0), globalThis.wrapper);",
            "app:///detach.js",
            "detaching",
        )
        .expect("detach");
    let tree = elements.tree();
    assert!(
        tree.get(node_id(4)).is_some(),
        "a removal frees nothing: the wrapper's handle still names both"
    );
    drop(tree);

    runtime
        .evaluate_module(
            &mut js_runtime,
            "globalThis.wrapper = undefined;",
            "app:///let-go.js",
            "letting go",
        )
        .expect("let go");
    runtime
        .collect_garbage(&mut js_runtime)
        .expect("collection");
    let tree = elements.tree();
    assert!(
        tree.get(node_id(3)).is_none(),
        "the detached wrapper goes once its handle does"
    );
    assert!(
        tree.get(node_id(4)).is_none(),
        "and the child with it: the wrapper's handle held the only \
             reference left to the child's"
    );
}

/// The whole ownership graph, through every mutation that changes a
/// parent. Script keeps no reference of its own to anything, so the only
/// thing that can survive a collection is what the page's permanent
/// handle holds through the chain of child sets — which must be exactly
/// the connected elements, and nothing more.
#[test]
fn every_connected_element_survives_a_collection_script_holds_nothing_through() {
    let (mut js_runtime, mut runtime, elements) = runtime();
    runtime
        .run_main_thread_script(
            &mut js_runtime,
            r"
                globalThis.renderPage = function () {
                  const page = __CreatePage('card', 0);
                  const a = __CreateView(0);
                  __AppendElement(page, a);
                  const b = __CreateView(0);
                  __InsertElementBefore(page, b, a);
                  // A move: b leaves the page's set for a's.
                  __InsertElementBefore(a, b, null);
                  const c = __CreateView(0);
                  __ReplaceElement(c, b);
                  const d = __CreateView(0);
                  const e = __CreateView(0);
                  __ReplaceElements(a, [d, e], [c]);
                  // Within one parent, then across two.
                  __SwapElement(d, e);
                  const f = __CreateView(0);
                  __AppendElement(page, f);
                  __SwapElement(d, f);
                  const g = __CreateView(0);
                  __AppendElement(page, g);
                  __RemoveElement(page, g);
                };
                ",
            "app:///ownership.js",
        )
        .expect("main-thread script");

    runtime
        .collect_garbage(&mut js_runtime)
        .expect("collection");
    let tree = elements.tree();
    // page 2, a 3, b 4, c 5, d 6, e 7, f 8, g 9.
    for (id, parent) in [(3, 2), (6, 2), (7, 3), (8, 3)] {
        assert_eq!(
            tree.get(node_id(id)).and_then(dom::Node::parent_id),
            Some(node_id(parent)),
            "node {id} is connected, so its handle is reachable from the page's"
        );
    }
    for id in [4, 5, 9] {
        assert!(
            tree.get(node_id(id)).is_none(),
            "node {id} was left detached and unreferenced, so its handle went"
        );
    }
}

/// The invariant, checked rather than argued: a connected element's
/// handle is held by its parent's, so a drop can never name one. If it
/// does, the realm's ownership graph has diverged from the tree, and the
/// element must not quietly disappear from the screen.
#[test]
fn dropping_a_connected_element_is_refused() {
    let (mut js_runtime, mut runtime, elements) = runtime();
    let error = runtime
        .run_main_thread_script(
            &mut js_runtime,
            r"
                import { dropElement } from 'bobcat-internal:host';
                globalThis.renderPage = function () {
                  const page = __CreatePage('card', 0);
                  const view = __CreateView(0);
                  __AppendElement(page, view);
                  dropElement(__GetElementUniqueID(view));
                };
                ",
            "app:///connected-drop.js",
        )
        .expect_err("a connected element cannot be dropped");
    assert!(error.to_string().contains("ownership graph"), "{error}");
    assert!(
        elements.tree().get(node_id(3)).is_some(),
        "and the element is still there"
    );
}

/// A handle that script has let go of reads as gone at once — `QuickJS`
/// answers a `WeakRef` from the refcount — while its element stays
/// allocated and stays a parent until the collection that finalizes it.
/// So the ownership graph must never be what decides which native
/// operation runs: a child of a let-go parent is still attached, and
/// treating it as detached turns this swap into a silent deletion of the
/// element it was swapped with.
#[test]
fn a_child_of_a_let_go_parent_is_still_attached_for_the_host() {
    let (mut js_runtime, mut runtime, elements) = runtime();
    runtime
        .run_main_thread_script(
            &mut js_runtime,
            r"
                globalThis.renderPage = function () {
                  const page = __CreatePage('card', 0);
                  const visible = __CreateView(0);
                  __AppendElement(page, visible);
                  globalThis.cell = (function () {
                    const wrapper = __CreateWrapperElement(0);
                    const cell = __CreateView(0);
                    __AppendElement(wrapper, cell);
                    // The wrapper's handle is unreachable from here on, and
                    // no collection has run: its element is still `cell`'s
                    // parent.
                    return cell;
                  })();
                  __SwapElement(globalThis.cell, visible);
                };
                ",
            "app:///let-go-parent.js",
        )
        .expect("main-thread script");

    // page 2, visible 3, wrapper 4, cell 5.
    let tree = elements.tree();
    assert_eq!(
        tree.get(node_id(5)).and_then(dom::Node::parent_id),
        Some(node_id(2)),
        "the swap moved the cell under the page"
    );
    assert_eq!(
        tree.get(node_id(3)).and_then(dom::Node::parent_id),
        Some(node_id(4)),
        "and moved the visible element under the wrapper, rather than \
             deleting it as a replace would have"
    );
}

/// A drop frees one node. A descendant script still names is unlinked
/// from the freed ancestor and goes on as a detached root it can attach
/// somewhere else — the ancestor's handle dying does not take it.
#[test]
fn dropping_a_detached_ancestor_leaves_a_still_named_descendant_a_root() {
    let (mut js_runtime, mut runtime, elements) = runtime();
    runtime
        .run_main_thread_script(
            &mut js_runtime,
            r"
                globalThis.renderPage = function () {
                  const page = __CreatePage('card', 0);
                  let outer = __CreateView(0);
                  const inner = __CreateView(0);
                  __AppendElement(page, outer);
                  __AppendElement(outer, inner);
                  __RemoveElement(page, outer);
                  outer = undefined;
                  globalThis.inner = inner;
                };
                ",
            "app:///ancestor.js",
        )
        .expect("main-thread script");

    runtime
        .collect_garbage(&mut js_runtime)
        .expect("collection");
    let tree = elements.tree();
    assert!(
        tree.get(node_id(3)).is_none(),
        "the detached ancestor is freed with its handle"
    );
    let inner = tree
        .get(node_id(4))
        .expect("the descendant script still names stays allocated");
    assert_eq!(inner.parent_id(), None, "as a detached root of its own");
    drop(tree);

    runtime
        .evaluate_module(
            &mut js_runtime,
            "import { __AppendElement, __CreatePage } from 'bobcat:element';
                 __AppendElement(__CreatePage('card', 0), globalThis.inner);",
            "app:///reattach.js",
            "re-attaching",
        )
        .expect("the surviving handle still works");
    assert_eq!(
        elements
            .tree()
            .get(node_id(4))
            .and_then(dom::Node::parent_id),
        Some(node_id(2))
    );
}

/// `ReactLynx`'s unmount: `__RemoveElement` on the snapshot's root, then
/// every handle of the subtree is let go at once. Whatever order the
/// finalizer delivers those in, the whole subtree is gone after one
/// collection and the ids are retired.
#[test]
fn an_unmounted_subtree_is_freed_by_the_collection_that_takes_its_handles() {
    let (mut js_runtime, mut runtime, elements) = runtime();
    runtime
        .run_main_thread_script(
            &mut js_runtime,
            r"
                globalThis.renderPage = function () {
                  const page = __CreatePage('card', 0);
                  const root = __CreateView(0);
                  const middle = __CreateText(0);
                  const leaf = __CreateRawText('leaf');
                  __AppendElement(page, root);
                  __AppendElement(root, middle);
                  __AppendElement(middle, leaf);
                  __RemoveElement(page, root);
                  // `__elements` of the unmounted snapshot instance, deleted.
                };
                ",
            "app:///unmount.js",
        )
        .expect("main-thread script");
    assert_eq!(
        elements
            .tree()
            .get(node_id(5))
            .and_then(dom::Node::parent_id),
        Some(node_id(4)),
        "before collection the detached subtree is intact"
    );

    runtime
        .collect_garbage(&mut js_runtime)
        .expect("collection");
    let tree = elements.tree();
    for id in 3..=5 {
        assert!(
            tree.get(node_id(id)).is_none(),
            "node {id} of the unmounted subtree is freed"
        );
    }
}

/// `__ReplaceElement` detaches what it replaces, and the detached
/// element is kept by the handle script still holds — together with the
/// subtree under it, whose handles that one holds in turn. Both go when
/// script lets go.
#[test]
fn a_replaced_element_lives_as_long_as_the_handle_that_names_it() {
    let (mut js_runtime, mut runtime, elements) = runtime();
    runtime
        .run_main_thread_script(
            &mut js_runtime,
            r"
                globalThis.renderPage = function () {
                  const page = __CreatePage('card', 0);
                  const holder = __CreateView(0);
                  let inner = __CreateView(0);
                  __AppendElement(page, holder);
                  __AppendElement(holder, inner);
                  inner = undefined;
                  globalThis.holder = holder;
                };
                ",
            "app:///removal.js",
        )
        .expect("main-thread script");
    runtime
        .collect_garbage(&mut js_runtime)
        .expect("collection");
    assert!(elements.tree().get(node_id(4)).is_some());

    runtime
        .evaluate_module(
            &mut js_runtime,
            "import { __CreateView, __ReplaceElement } from 'bobcat:element';
                 __ReplaceElement(__CreateView(0), globalThis.holder);",
            "app:///replace.js",
            "replacing",
        )
        .expect("replace");
    let tree = elements.tree();
    assert_eq!(
        tree.get(node_id(3)).and_then(dom::Node::parent_id),
        None,
        "the replaced holder is detached, and live: its handle names it"
    );
    assert!(
        tree.get(node_id(4)).is_some(),
        "and it holds the handle of the child under it"
    );
    drop(tree);

    runtime
        .evaluate_module(
            &mut js_runtime,
            "import { __RemoveElement } from 'bobcat:element';
                 __RemoveElement(null, globalThis.holder);",
            "app:///noop.js",
            "no-op",
        )
        .expect("removing a detached element is a no-op");
    runtime
        .evaluate_module(
            &mut js_runtime,
            "globalThis.holder = undefined;",
            "app:///let-go.js",
            "letting go",
        )
        .expect("let go");
    runtime
        .collect_garbage(&mut js_runtime)
        .expect("collection");
    let tree = elements.tree();
    assert!(tree.get(node_id(3)).is_none() && tree.get(node_id(4)).is_none());
}

/// A drop is immediate and final: the element is gone the moment the
/// finalizer's call lands, and the id it used names nothing afterwards.
#[test]
fn a_drop_frees_the_element_at_once_and_retires_its_id() {
    let (mut js_runtime, mut runtime, elements) = runtime();
    runtime
        .run_main_thread_script(
            &mut js_runtime,
            r"
                import { dropElement, tagName } from 'bobcat-internal:host';
                globalThis.renderPage = function () {
                  const page = __CreatePage('card', 0);
                  const gone = __CreateView(0);
                  __AppendElement(page, gone);
                  __RemoveElement(page, gone);
                  globalThis.goneId = __GetElementUniqueID(gone);
                  if (tagName(goneId) !== 'view') {
                    throw new Error('the detached element is gone before its drop');
                  }
                  // Called directly, where a finalizer would.
                  dropElement(goneId);
                };
                ",
            "app:///drop.js",
        )
        .expect("main-thread script");
    assert!(elements.tree().get(node_id(3)).is_none());
    runtime
        .evaluate_module(
            &mut js_runtime,
            "import { tagName } from 'bobcat-internal:host';
                 tagName(globalThis.goneId);",
            "app:///after.js",
            "reading a freed id",
        )
        .expect_err("a freed id names nothing");
}

/// A listener that captures its own element must not keep it alive: the
/// closure is reachable only from the handle it captures, so the cycle
/// has no root once script lets go. This is exactly what a per-handle
/// store in a `WeakMap` would break under `QuickJS`, whose `WeakMap` marks
/// its values unconditionally.
#[test]
fn a_listener_capturing_its_own_element_does_not_keep_it_alive() {
    for (label, registration) in [
        (
            "listener closure",
            "{ const self = view; __AddEventListener(view, 'tap', () => self, {}); }",
        ),
        (
            "worklet handler",
            "__AddEvent(view, 'bindEvent', 'tap', { type: 'worklet', value: { ref: view } });",
        ),
        (
            "list callbacks",
            "{ const self = view; __UpdateListCallbacks(view, () => self, () => self, () => self); }",
        ),
    ] {
        let (mut js_runtime, mut runtime, elements) = runtime();
        runtime
            .run_main_thread_script(
                &mut js_runtime,
                &format!(
                    r"
                    globalThis.renderPage = function () {{
                      const page = __CreatePage('card', 0);
                      let view = __CreateView(0);
                      __AppendElement(page, view);
                      {registration}
                      __RemoveElement(page, view);
                      view = undefined;
                    }};
                    "
                ),
                "app:///self-capture.js",
            )
            .expect("main-thread script");
        runtime
            .collect_garbage(&mut js_runtime)
            .expect("collection");
        runtime
            .collect_garbage(&mut js_runtime)
            .expect("collection");
        let tree = elements.tree();
        assert!(
            tree.get(node_id(3)).is_none(),
            "{label}: the element whose handle only its own registration reached is freed"
        );
    }
}

/// Removals pace collection: once enough subtrees have been removed,
/// the batch that crosses the count ends with a collection, so the
/// handles those subtrees left behind are finalized and the subtrees freed
/// without any allocation pressure or explicit collection.
#[test]
fn enough_removals_end_a_batch_with_a_collection() {
    let (mut js_runtime, mut runtime, elements) = runtime();
    runtime
        .run_main_thread_script(
            &mut js_runtime,
            r"
                globalThis.renderPage = function () {
                  const page = __CreatePage('card', 0);
                  globalThis.page = page;
                  globalThis.churn = function (count) {
                    for (let i = 0; i < count; i += 1) {
                      const cell = __CreateView(0);
                      __AppendElement(page, cell);
                      __RemoveElement(page, cell);
                    }
                  };
                };
                ",
            "app:///paced.js",
        )
        .expect("main-thread script");

    let below = REMOVALS_PER_COLLECTION - 1;
    runtime
        .evaluate_module(
            &mut js_runtime,
            &format!("globalThis.churn({below});"),
            "app:///below.js",
            "churning",
        )
        .expect("churn");
    // The count, not the tree, is the witness: QuickJS may collect on its
    // own allocation pressure at any point, which frees cells too, but
    // only the paced collection resets the count.
    assert_eq!(
        runtime.slot.borrow().removals,
        below,
        "below the count, no paced collection has run"
    );

    runtime
        .evaluate_module(
            &mut js_runtime,
            "globalThis.churn(1);",
            "app:///cross.js",
            "churning",
        )
        .expect("churn");
    assert_eq!(
        runtime.slot.borrow().removals,
        0,
        "crossing the count ran the collection and reset it"
    );
    let tree = elements.tree();
    for id in 3..3 + u64::from(REMOVALS_PER_COLLECTION) {
        assert!(
            tree.get(node_id(id)).is_none(),
            "cell {id}: the batch that crossed the count collected and freed it"
        );
    }
}

/// Every element on an event path carries a handle — a connected one is
/// held by its parent's, up to the permanent page handle — so a target
/// always resolves to one. A target that does not is the ownership graph
/// and the tree disagreeing, and the realm says so instead of inventing
/// an `Event` that cannot name what it happened to.
///
/// Routing cannot produce one today: it targets elements, and a hit on a
/// text run maps to its element in `hit.rs`. This builds the path by hand
/// against the run itself, the one node no handle ever names. The case
/// that *will* produce one is a UA component with hit-testable shadow
/// chrome — `first_element_at` answers with the flat-tree element it
/// hits, shadow tree included, and script names no shadow node — so the
/// first such component owes the event path a retarget to its host, the
/// same one `event_path` already performs for every step outside the
/// tree. Generated text has no DOM identity and cannot become such a target.
#[test]
fn an_event_target_no_handle_names_is_an_error_not_a_silent_drop() {
    let (mut js_runtime, mut runtime, elements) = runtime();
    runtime
        .run_main_thread_script(
            &mut js_runtime,
            r"
                globalThis.renderPage = function () {
                  const page = __CreatePage('card', 0);
                  const text = __CreateText(0);
                  __AppendElement(page, text);
                  __AppendElement(text, __CreateRawText('hello'));
                  __AddEventListener(text, 'tap', () => {}, {});
                };
                ",
            "app:///run-target.js",
        )
        .expect("main-thread script");
    // Explicitly construct a host-owned text node the realm has no handle for.
    let run = {
        let mut tree = elements.tree();
        let run = tree.create_text_node("host-owned", ());
        tree.append_child(node_id(3), run);
        run
    };

    let error = runtime
        .dispatch_event(&mut js_runtime, run, &tap(), &no_detail())
        .expect_err("a target no handle names cannot be delivered");
    assert!(error.to_string().contains("ownership graph"), "{error}");
}

#[test]
fn update_list_info_is_refused_instead_of_becoming_an_attribute() {
    let (mut js_runtime, mut runtime, _elements) = runtime();
    let error = runtime
        .run_main_thread_script(
            &mut js_runtime,
            r"
                globalThis.renderPage = function () {
                  const page = __CreatePage('card', 0);
                  const list = __CreateList(0, function () {}, function () {});
                  __AppendElement(page, list);
                  __SetAttribute(list, 'update-list-info', { insertAction: [], removeAction: [] });
                };
                ",
            "app:///list.js",
        )
        .expect_err("the unimplemented list surface");

    assert!(error.to_string().contains("update-list-info"), "{error}");
}

/// The realm's timers, from the four globals a card calls to the schedule a
/// page's epilogue runs. A zero delay is due the moment it is armed, so a
/// test spends a turn by asking the runtime to run what is due — which is
/// exactly what the first step of every epilogue does.
#[test]
fn a_timeout_runs_once_with_the_arguments_it_was_given() {
    let (mut js_runtime, mut runtime, _elements) = runtime();
    runtime
        .run_main_thread_script(
            &mut js_runtime,
            r"
                globalThis.fired = [];
                globalThis.renderPage = function () {
                  __CreatePage('card', 0);
                  const handle = setTimeout((a, b) => fired.push(a + b), 0, 'x', 'y');
                  if (!handle) throw new Error('a timer id must survive a truth test');
                };
                ",
            "app:///timeout.js",
        )
        .expect("main-thread script");

    assert!(
        runtime.run_due_timers(&mut js_runtime).is_empty(),
        "the callback returned"
    );
    // Nothing is armed any more, so a second pass finds nothing to run.
    assert!(runtime.run_due_timers(&mut js_runtime).is_empty());
    assert_eq!(runtime.next_timer_deadline(), None);

    runtime
        .evaluate_module(
            &mut js_runtime,
            "if (fired.join('|') !== 'xy') throw new Error(fired.join('|'));",
            "app:///verify.js",
            "verifying",
        )
        .expect("verification");
}

#[test]
fn a_cleared_timeout_never_runs() {
    let (mut js_runtime, mut runtime, _elements) = runtime();
    runtime
        .run_main_thread_script(
            &mut js_runtime,
            r"
                globalThis.fired = [];
                globalThis.renderPage = function () {
                  __CreatePage('card', 0);
                  clearTimeout(setTimeout(() => fired.push('no'), 0));
                };
                ",
            "app:///cleared.js",
        )
        .expect("main-thread script");

    assert_eq!(runtime.next_timer_deadline(), None, "nothing stays armed");
    assert!(runtime.run_due_timers(&mut js_runtime).is_empty());

    runtime
        .evaluate_module(
            &mut js_runtime,
            "if (fired.length !== 0) throw new Error(fired.join('|'));",
            "app:///verify.js",
            "verifying",
        )
        .expect("verification");
}

#[test]
fn an_interval_runs_every_round_until_its_own_callback_clears_it() {
    let (mut js_runtime, mut runtime, _elements) = runtime();
    runtime
        .run_main_thread_script(
            &mut js_runtime,
            r"
                globalThis.ticks = 0;
                globalThis.renderPage = function () {
                  __CreatePage('card', 0);
                  const handle = setInterval(() => {
                    ticks += 1;
                    if (ticks === 3) {
                      clearInterval(handle);
                    }
                  }, 0);
                };
                ",
            "app:///interval.js",
        )
        .expect("main-thread script");

    for _ in 0..6 {
        assert!(runtime.run_due_timers(&mut js_runtime).is_empty());
    }

    // Three passes ran it and the third disarmed it, so the last three found
    // nothing — a repeat neither runs twice in one pass nor outlives its own
    // `clearInterval`.
    runtime
        .evaluate_module(
            &mut js_runtime,
            "if (ticks !== 3) throw new Error(String(ticks));",
            "app:///verify.js",
            "verifying",
        )
        .expect("verification");
    assert_eq!(runtime.next_timer_deadline(), None);
}

#[test]
fn a_timer_cleared_by_an_earlier_one_in_the_same_round_does_not_run() {
    let (mut js_runtime, mut runtime, _elements) = runtime();
    runtime
        .run_main_thread_script(
            &mut js_runtime,
            r"
                globalThis.fired = [];
                globalThis.victim = 0;
                globalThis.renderPage = function () {
                  __CreatePage('card', 0);
                  // Armed first, so it runs first: ids are handed out in
                  // arming order and that is the order they come due in.
                  setTimeout(() => clearTimeout(victim), 0);
                  victim = setTimeout(() => fired.push('victim'), 0);
                };
                ",
            "app:///same-round.js",
        )
        .expect("main-thread script");

    assert!(runtime.run_due_timers(&mut js_runtime).is_empty());

    runtime
        .evaluate_module(
            &mut js_runtime,
            "if (fired.length !== 0) throw new Error(fired.join('|'));",
            "app:///verify.js",
            "verifying",
        )
        .expect("verification");
}

#[test]
fn a_timer_that_throws_is_reported_and_the_next_one_still_runs() {
    let (mut js_runtime, mut runtime, _elements) = runtime();
    runtime
        .run_main_thread_script(
            &mut js_runtime,
            r"
                globalThis.fired = [];
                globalThis.renderPage = function () {
                  __CreatePage('card', 0);
                  setTimeout(() => { throw new Error('boom'); }, 0);
                  setTimeout(() => fired.push('after'), 0);
                };
                ",
            "app:///throwing.js",
        )
        .expect("main-thread script");

    let failures = runtime.run_due_timers(&mut js_runtime);
    assert_eq!(failures.len(), 1, "one callback threw");
    assert!(failures[0].to_string().contains("boom"), "{}", failures[0]);

    runtime
        .evaluate_module(
            &mut js_runtime,
            "if (fired.join('|') !== 'after') throw new Error(fired.join('|'));",
            "app:///verify.js",
            "verifying",
        )
        .expect("verification");
}

#[test]
fn a_timer_callback_mutates_the_document_the_realm_shares() {
    let (mut js_runtime, mut runtime, elements) = runtime();
    runtime
        .run_main_thread_script(
            &mut js_runtime,
            r"
                globalThis.renderPage = function () {
                  const page = __CreatePage('card', 0);
                  const view = __CreateView(0);
                  __AppendElement(page, view);
                  globalThis.held = [page, view];
                  setTimeout(() => __SetAttribute(view, 'ticked', 'yes'), 0);
                };
                ",
            "app:///mutating-timer.js",
        )
        .expect("main-thread script");

    assert!(runtime.run_due_timers(&mut js_runtime).is_empty());

    assert_eq!(
        elements
            .tree()
            .get(node_id(3))
            .expect("the view is live")
            .attribute("ticked"),
        Some("yes")
    );
}

/// A chain of zero-delay timers is exactly what the standard's nesting clamp
/// exists for: it runs unclamped to the fifth link and waits from there on,
/// which is what keeps such a chain from spinning `bobcat-main`.
#[test]
fn a_chain_of_zero_delay_timers_starts_waiting_once_it_nests_deeply() {
    let (mut js_runtime, mut runtime, _elements) = runtime();
    runtime
        .run_main_thread_script(
            &mut js_runtime,
            r"
                globalThis.depth = 0;
                globalThis.renderPage = function () {
                  __CreatePage('card', 0);
                  const tick = () => {
                    depth += 1;
                    setTimeout(tick, 0);
                  };
                  setTimeout(tick, 0);
                };
                ",
            "app:///nested.js",
        )
        .expect("main-thread script");

    for level in 1..=5 {
        assert!(runtime.run_due_timers(&mut js_runtime).is_empty());
        let armed = ClockInstant::now();
        let deadline = runtime.next_timer_deadline().expect("the chain goes on");
        assert!(deadline <= armed, "level {level} still asks for no delay");
    }

    let before = ClockInstant::now();
    assert!(runtime.run_due_timers(&mut js_runtime).is_empty());
    let deadline = runtime.next_timer_deadline().expect("the chain goes on");
    assert!(deadline > before, "the sixth link waits");
}
