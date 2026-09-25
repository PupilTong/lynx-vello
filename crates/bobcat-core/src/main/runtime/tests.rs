use tokio::sync::mpsc;

use super::*;
use crate::background::{WorkerCommand, WorkerEvent};
use crate::esm::build_runtime;
use crate::jobs::JsThread;
use crate::link::{DetachedView, detached_outbox};
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

/// Where a test's event happened, when the position is not what it is about.
fn event_point() -> dom::Point2D<f32> {
    dom::Point2D::new(12.0, 30.0)
}

/// The ingredients a test stages when the document is not what it is about.
fn ingredients() -> DocumentIngredients {
    DocumentIngredients::for_test(Viewport::new(393.0, 727.0), PageConfig::default())
}

fn runtime() -> (ScriptRuntime, MainThreadRuntime, DocumentProbe) {
    runtime_over(ingredients())
}

#[test]
#[expect(clippy::float_cmp, reason = "explicit pixel widths are exact")]
fn boot_defers_flush_to_a_microtask_without_draining_between_hooks() {
    let (mut js, mut runtime, elements, mut far) = runtime_over_watching_names(ingredients());
    runtime
        .run_main_thread_script(
            &mut js,
            r"
        globalThis.processData = function() {
            if (this !== globalThis) throw Error('processor receiver');
            // An ordinary object rather than a Promise: the processed result
            // is posted to BTS, and the transport refuses a value structured
            // clone has no encoding for. `ready` still flips in a queued job,
            // so a drain between the hooks is what this detects.
            const result = { value: 42, ready: false };
            Promise.resolve().then(() => { result.ready = true; });
            globalThis.processed = result;
            return result;
        };
        globalThis.renderPage = function(data) {
            if (this !== globalThis || data !== processed || data.ready || data.value !== 42)
                throw Error('processor result was copied or drained before render');
            const page = __CreatePage();
            const view = __CreateView(0);
            __SetInlineStyles(view, 'width:10px;height:10px');
            __AppendElement(page, view);
            Promise.resolve().then(() => {
                __SetInlineStyles(view, 'width:20px;height:10px');
                Promise.resolve().then(() => {
                    __SetInlineStyles(view, 'width:30px;height:10px');
                });
            });
        };
    ",
            "app:///deferred-flush.js",
        )
        .unwrap();
    let tree = elements.tree();
    let view = tree
        .document_element()
        .first_child()
        .expect("rendered view");
    // The first render job precedes the queued flush; its nested job follows it.
    // This test drives the realm directly, before Page's dirty-commit epilogue.
    assert_eq!(tree.rounded_layout(view.id()).unwrap().size.width, 20.0);
    assert_eq!(view.attribute("style"), Some("width:30px;height:10px"));
    while let Ok(notice) = far.0.notices.try_recv() {
        assert!(
            !matches!(
                notice,
                ViewNotice::Engine(crate::EngineEvent::ScriptReported { .. })
            ),
            "boot reported an error"
        );
    }
}

#[test]
fn a_throwing_processor_reports_and_still_runs_render_and_flush() {
    let (mut js, mut runtime, elements, mut far) = runtime_over_watching_names(ingredients());
    runtime
        .run_main_thread_script(
            &mut js,
            r"
        const initial = lynx.__initData;
        let processorJobRan = false;
        globalThis.processData = () => {
            Promise.resolve().then(() => { processorJobRan = true; });
            throw Error('processor failed');
        };
        globalThis.renderPage = data => {
            if (data !== initial || processorJobRan)
                throw Error('processor failure changed the result or ran jobs before render');
            const page = __CreatePage();
            __AppendElement(page, __CreateView(0));
        };
    ",
            "app:///processor-error.js",
        )
        .unwrap();
    let tree = elements.tree();
    let view = tree
        .document_element()
        .first_child()
        .expect("rendered after processor failure");
    assert!(
        tree.rounded_layout(view.id()).is_some(),
        "boot still flushed"
    );
    let reports: Vec<_> = std::iter::from_fn(|| far.0.notices.try_recv().ok())
        .filter_map(|notice| match notice {
            ViewNotice::Engine(crate::EngineEvent::ScriptReported { message, .. }) => Some(message),
            _ => None,
        })
        .collect();
    assert_eq!(reports.len(), 1, "{reports:?}");
    assert!(reports[0].contains("processor failed"));
}

/// A named Lepus chunk is a script resource of its own, loaded through the
/// same synchronous host loader a `require` uses and run again on every
/// `__LoadLepusChunk` call, as native's `TemplateEntry` does.
///
/// What a chunk shares with the root is the bindings the entry preamble gives
/// the entry — the realm hands them to the chunk as the parameters of the
/// function body it was compiled as — and this realm's `globalThis`. Not the
/// root's lexical scope, and not the root's `var`s: a `var` at a chunk's top
/// level is local to that call.
//
// A plain test rather than a `tokio::test`, for the reason the `require`
// tests spell out: the load's wait is a `block_on` of the realm's engine
// thread, which tokio refuses from inside a runtime.
#[test]
fn lepus_chunks_load_per_call_and_defer_jobs_to_the_checkpoint() {
    let (mut js, mut runtime, _elements, far) = runtime_over_watching_names(ingredients());
    let mut notices = far.0.notices;
    let host = std::thread::spawn(move || {
        // Two calls, two requests, two evaluations: nothing caches a chunk.
        for call in 1..=2 {
            let (url, completion) = requested_module(&mut notices);
            assert_eq!(url, "app:///chunks.js/worklet.js", "call {call}");
            completion.complete(Ok(module_source(
                r"
                globalThis.executions = (globalThis.executions ?? 0) + 1;
                var chunkLocal = true;
                globalThis.chunkRuntime = lynx;
                globalThis.chunkPAPI = __CreateView;
                Promise.resolve().then(() => { globalThis.chunkJob = true; });
                ",
                "app:///chunks.js/worklet.js",
            )));
        }
        // A chunk this page does not carry is a load the host refuses.
        let (url, completion) = requested_module(&mut notices);
        assert_eq!(url, "app:///chunks.js/missing.js");
        completion.complete(Err(crate::resource::unanswered_source().into()));
        notices
    });
    runtime
        .run_main_thread_script(
            &mut js,
            r"
        if ((0, eval)('typeof lynx') !== 'undefined'
            || (0, eval)('typeof __CreateView') !== 'undefined')
            throw Error('runtime/PAPI imports were also installed as global bindings');
        if (globalThis.executions !== undefined)
            throw Error('a chunk ran before it was asked for');
        if (!__LoadLepusChunk('worklet', {}) || !__LoadLepusChunk('worklet', {}))
            throw Error('chunk not found');
        if (globalThis.executions !== 2)
            throw Error('a chunk runs on every call: ' + globalThis.executions);
        if (globalThis.chunkRuntime !== lynx || globalThis.chunkPAPI !== __CreateView)
            throw Error('the chunk was given other bindings than the entry');
        if (typeof chunkLocal !== 'undefined' || 'chunkLocal' in globalThis)
            throw Error('a chunk var leaked out of its own call');
        if (globalThis.chunkJob)
            throw Error('a job ran while the load had this job parked');
        if (__LoadLepusChunk('missing', {}))
            throw Error('a chunk this page does not carry was found');
        if (__LoadLepusChunk('worklet', {dynamicComponentEntry: 'app:///other.js'}))
            throw Error('a foreign entry was answered, or asked for');
        globalThis.renderPage = () => {
            if (!globalThis.chunkJob) throw Error('enclosing checkpoint lost chunk jobs');
        };
        ",
            "app:///chunks.js",
        )
        .unwrap();
    let mut notices = host.join().unwrap();
    while let Ok(notice) = notices.try_recv() {
        assert!(
            !matches!(
                notice,
                ViewNotice::Engine(crate::EngineEvent::ScriptReported { .. })
            ),
            "MTS execution reported an error"
        );
    }
}

#[test]
fn mts_imported_inputs_follow_global_props_updates() {
    let (mut js, mut runtime, _elements, mut far) = runtime_over_watching_names(ingredients());
    runtime
        .run_main_thread_script(
            &mut js,
            r"
        const state = {readProps: () => __globalProps};
        globalThis.scriptInputs = state;
        globalThis.updateGlobalProps = props => { state.updated = props; };
        if (state.readProps() !== __globalProps) throw Error('entry inputs');
    ",
            "app:///script-inputs.js",
        )
        .unwrap();
    runtime.evaluate_module(&mut js, &entry_module_source(r#"
        import {__BobcatUpdateGlobalProps} from 'bobcat:runtime';
        const oldProps = scriptInputs.readProps();
        __BobcatUpdateGlobalProps('{"next":2}');
        if (scriptInputs.readProps() !== __globalProps || __globalProps === oldProps || __globalProps.next !== 2)
            throw Error('Script props binding did not follow its module export');
        if (scriptInputs.updated !== __globalProps)
            throw Error('global-object lifecycle hook was not found');
    "#), "app:///update-script-inputs.js", "updating imported props").unwrap();
    while let Ok(notice) = far.0.notices.try_recv() {
        if let ViewNotice::Engine(crate::EngineEvent::ScriptReported { message, .. }) = notice {
            panic!("{message}");
        }
    }
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
    /// The engine thread `__AdoptStyleSheet` parks on. These tests call the
    /// runtime directly rather than queueing jobs, but a realm is opened with
    /// a live thread and a synchronous adoption needs one, so the probe holds
    /// it for as long as the realm lives.
    _thread: Rc<JsThread>,
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

    /// The whole published set, sorted, for a test about which names the
    /// realm has opened rather than about one of them.
    fn names(&mut self) -> Vec<String> {
        self.0.published.sync();
        let mut names: Vec<String> = self
            .0
            .published
            .listener_names()
            .iter()
            .map(std::string::ToString::to_string)
            .collect();
        names.sort();
        names
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
    let mut js_runtime = build_runtime().expect("the test runtime builds");
    let (workers, inbox) = mpsc::unbounded_channel();
    let thread = JsThread::new();
    let viewport = ingredients.viewport;
    let (runtime, worker_events) = MainThreadRuntime::new(
        &mut js_runtime,
        ingredients,
        bound_metrics(viewport),
        outbox,
        &WorkerFactory::new(workers, Arc::default()),
        thread.handle(),
        // No entry here: these tests evaluate their own scripts against the
        // realm afterwards.
        RealmStartup::default(),
    )
    .expect("main-thread runtime");
    let probe = DocumentProbe {
        slot: Rc::clone(&runtime.slot),
        _workers: inbox,
        _worker_events: worker_events,
        _thread: thread,
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
    two_view_group_with([RealmStartup::default(), RealmStartup::default()])
}

/// The same group, each view opened with its own startup.
fn two_view_group_with(
    pages: [RealmStartup; 2],
) -> (
    ScriptRuntime,
    MainThreadRuntime,
    MainThreadRuntime,
    GroupFarEnds,
) {
    let mut js_runtime = build_runtime().expect("the test runtime builds");
    let mut views = Vec::new();
    let mut ends = GroupFarEnds::default();
    let (workers, inbox) = mpsc::unbounded_channel();
    ends.workers = Some(inbox);
    let workers = WorkerFactory::new(workers, Arc::default());
    let thread = JsThread::new();
    ends.thread = Some(Rc::clone(&thread));
    for startup in pages {
        let (outbox, far_end) = detached_outbox(Arc::new(NoWakeup));
        let (runtime, worker_events) = MainThreadRuntime::new(
            &mut js_runtime,
            ingredients(),
            bound_metrics(Viewport::new(393.0, 727.0)),
            outbox,
            &workers,
            thread.handle(),
            startup,
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
    /// The engine thread both realms were opened with, held for their life.
    thread: Option<Rc<JsThread>>,
}

/// The host's page data reaches the realm it was given to as plain strings,
/// and `bobcat:runtime` parses them there: the entry sees the global props as
/// it loads, and `processData` gets the init data. A view given none sees
/// `{}` for both, never its sibling's.
#[test]
fn page_data_is_parsed_by_the_realm_it_was_given_to() {
    let (mut js, mut first, mut second, _workers) = two_view_group_with([
        RealmStartup {
            initial_processor: String::new(),
            init_data: Some(r#"{"count": 2, "text": "中文 🦀"}"#.to_owned()),
            global_props: Some(r#"{"theme": "dark"}"#.to_owned()),
            native_modules: String::new(),
            ..RealmStartup::default()
        },
        RealmStartup::default(),
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
              return { count: data.count };
            };
            globalThis.renderPage = function (processed) {
              if (processed.count !== 2) throw new Error('renderPage got ' + processed);
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
        RealmStartup {
            initial_processor: String::new(),
            init_data: Some("{".to_owned()),
            global_props: None,
            native_modules: String::new(),
            ..RealmStartup::default()
        },
        RealmStartup {
            initial_processor: String::new(),
            init_data: None,
            global_props: Some("[1,".to_owned()),
            native_modules: String::new(),
            ..RealmStartup::default()
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

#[test]
fn initial_values_reach_each_view_before_its_entry_and_render() {
    let (mut js, mut first, mut second, _workers) = two_view_group_with([
        RealmStartup {
            initial_processor: String::new(),
            init_data: Some(
                serde_json::json!({
                    "count": 42, "text": "中文", "items": [null, false, -1.25, "中文\0🦀", [], {}],
                    "__proto__": {"polluted": true}, "large": u64::MAX,
                })
                .to_string(),
            ),
            global_props: Some(serde_json::json!({"theme": "dark"}).to_string()),
            native_modules: String::new(),
            ..RealmStartup::default()
        },
        RealmStartup {
            initial_processor: String::new(),
            init_data: Some("null".to_owned()),
            global_props: None,
            native_modules: String::new(),
            ..RealmStartup::default()
        },
    ]);
    first.core.engine.collect_garbage(&mut js).unwrap();
    first.run_main_thread_script(&mut js, r"
        if (lynx.__initData.count !== 42 || lynx.__initData.text !== '中文') throw Error('entry data');
        const data = lynx.__initData;
        if (data.items[0] !== null || data.items[1] !== false || data.items[2] !== -1.25 ||
            data.items[3] !== '中文\0🦀' || !Array.isArray(data.items[4]) ||
            Object.keys(data.items[5]).length !== 0 || !Object.hasOwn(data, '__proto__') ||
            data.polluted !== undefined || typeof data.large !== 'number' ||
            data.large !== 18446744073709551615) throw Error('JSON conversion');
        if (lynx.__globalProps.theme !== 'dark' || __globalProps !== lynx.__globalProps) throw Error('entry props');
        globalThis.processData = data => {
            if (data !== lynx.__initData) throw Error('different render data');
            return {count: data.count + 1};
        };
        globalThis.renderPage = data => {
            if (data.count !== 43) throw Error('processed render data');
        };
    ", "app:///first.js").unwrap();
    second
        .run_main_thread_script(
            &mut js,
            r"
        if (lynx.__initData !== null) throw Error('null changed');
        if (Object.keys(lynx.__globalProps).length) throw Error('sibling props leaked');
        globalThis.renderPage = data => { if (data !== null) throw Error('null render data'); };
    ",
            "app:///second.js",
        )
        .unwrap();
}

#[test]
fn entry_initialization_cannot_replace_the_host_render_argument() {
    let (mut js, mut first, mut second, _workers) = two_view_group_with([
        RealmStartup {
            initial_processor: String::new(),
            init_data: Some(r#"{"showInitial":false}"#.to_owned()),
            global_props: None,
            native_modules: String::new(),
            ..RealmStartup::default()
        },
        RealmStartup {
            initial_processor: String::new(),
            init_data: Some("null".to_owned()),
            global_props: None,
            native_modules: String::new(),
            ..RealmStartup::default()
        },
    ]);
    first
        .run_main_thread_script(
            &mut js,
            r"
        const original = lynx.__initData;
        // React's MTS bootstrap writes this before installing renderPage.
        lynx.__initData = {};
        await Promise.resolve();
        globalThis.processData = data => {
            if (data !== original || data.showInitial !== false) throw Error('host data lost');
            return {processed: true};
        };
        globalThis.renderPage = data => {
            if (data.processed !== true) throw Error('processor result lost');
        };
    ",
            "app:///first.js",
        )
        .unwrap();
    second
        .run_main_thread_script(
            &mut js,
            r"
        lynx.__initData = {replacement:true};
        const engine = lynx.getEngine();
        engine.addEventListener('__RenderPage', event => {
            if (event.data[0] !== null) throw Error('host null replaced by entry');
        });
    ",
            "app:///second.js",
        )
        .unwrap();
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
            .dispatch_for_test(&mut js_runtime, node_id(2), &tap(), event_point())
            .expect("the second view's dispatch is not the first view's failure"),
        "the second view's realm published the dispatch export"
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
                  return {count:42};
                };
                engine.addEventListener('__RenderPage', function (event) {
                  if (this !== engine || event.type !== '__RenderPage' || event.data[0].count !== 42) {
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
                globalThis.renderPage = () => { throw Error('engine listener must take precedence'); };
                let renderCount = 0;
                lynx.getEngine().addEventListener('__RenderPage', function () {
                  renderCount += 1;
                  if (renderCount !== 1) {
                    throw new Error('renderPage ran more than once');
                  }
                  const page = __CreatePage('card', 0);
                  __AppendElement(page, createView(0));
                });
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
                if (JSON.stringify(lynx.__initData) !== '{}') {
                  throw new Error('omitted init data must be an empty object');
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
                removeElement(999999);
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

/// The page configuration reaches the document through the realm.
///
/// The boot module is written with the four switches as boolean literals and
/// hands them to `new Document(config)`, whose `createDocument` call passes
/// them back as four booleans, and the UA cascade is built from *those* — so
/// what these switches do to a `view`'s computed style is the boot module's
/// literals working. The document is built from what JavaScript handed back.
#[test]
fn the_page_config_written_into_the_boot_module_builds_the_ua_cascade() {
    use dom::stylo::values::computed::{Display, Overflow};

    for linear in [true, false] {
        let (mut js_runtime, mut runtime, elements, _names) =
            runtime_over_watching_names(DocumentIngredients::for_test(
                Viewport::new(393.0, 727.0),
                PageConfig {
                    default_display_linear: linear,
                    default_overflow_visible: !linear,
                    ..PageConfig::default()
                },
            ));
        runtime
            .run_main_thread_script(
                &mut js_runtime,
                r"
                globalThis.renderPage = function () {
                  __AppendElement(__CreatePage('card', 0), __CreateView(0));
                };
                ",
                "app:///config.js",
            )
            .expect("main-thread script");
        let tree = elements.tree();
        let view = tree.get(node_id(2)).expect("the card's one view");
        let style = view.computed_style().expect("a flushed element has style");
        assert_eq!(
            style.clone_display(),
            if linear {
                Display::Linear
            } else {
                Display::Flex
            },
            "`defaultDisplayLinear` reached the cascade: linear={linear}"
        );
        assert_eq!(
            style.clone_overflow_x() == Overflow::Visible,
            !linear,
            "`defaultOverflowVisible` reached it too: linear={linear}"
        );
    }
}

/// A page configuration switch that is not a boolean refuses the
/// construction, and the boot that asked for it fails.
///
/// Rust needs these four fields, so it reads each rather than forwarding
/// them; a card that reaches `createDocument` and hands it anything else gets
/// the message rather than a document built on defaults. The argument is read
/// before the second-document refusal, so this is the message the card sees.
#[test]
fn a_non_boolean_page_config_switch_refuses_the_document() {
    let (mut js_runtime, mut runtime, _elements, _names) =
        runtime_over_watching_names(ingredients());
    let error = runtime
        .run_main_thread_script(
            &mut js_runtime,
            r#"
            import { createDocument } from "bobcat-internal:host";
            createDocument(true, "yes", true, false);
            "#,
            "app:///bad-config.js",
        )
        .expect_err("a config whose second switch is a string");
    let message = error.to_string();
    assert!(
        message.contains("createDocument expects a boolean for argument 1"),
        "{message}"
    );
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

/// The painting side routes against the published name set, and the realm is
/// the only thing that can maintain it: every registration lives there, in
/// six places per element, and the host is told only the global edges — a
/// name's first registration anywhere, and the removal of its last.
#[test]
fn the_published_names_are_the_global_edges_of_the_realm_registrations() {
    let (mut js_runtime, mut runtime, _elements, mut names) =
        runtime_over_watching_names(ingredients());
    runtime
        .run_main_thread_script(
            &mut js_runtime,
            r"
                globalThis.renderPage = function () {
                  const page = __CreatePage('card', 0);
                  const outer = __CreateView(0);
                  const inner = __CreateView(0);
                  __AppendElement(page, outer);
                  __AppendElement(outer, inner);
                  globalThis.held = [page, outer, inner];
                  globalThis.listener = () => {};
                  // Two handles on one name, and on one of them a second
                  // registration of another kind entirely: still one name.
                  __AddEventListener(outer, 'tap', globalThis.listener, {});
                  __AddEventListener(inner, 'tap', globalThis.listener, { capture: true });
                  __AddEvent(inner, 'bindEvent', 'tap', '3:0:bindtap');
                  __AddEvent(outer, 'global-bindEvent', 'swipe', '3:0:swipe');
                };
                ",
            "app:///edges.js",
        )
        .expect("main-thread script");
    assert_eq!(names.names(), ["swipe", "tap"]);

    runtime
        .evaluate_module(
            &mut js_runtime,
            r"
                import { __AddEvent, __RemoveEventListener } from 'bobcat:element';
                const [, outer, inner] = globalThis.held;
                // `inner` still has its `__AddEvent` handler, and `outer`
                // still has its closure, so neither of these closes `tap`.
                __RemoveEventListener(inner, 'tap', globalThis.listener, { capture: true });
                __AddEvent(outer, 'global-bindEvent', 'swipe', null);
                ",
            "app:///partial.mjs",
            "unregistering",
        )
        .expect("unregistration");
    assert_eq!(names.names(), ["tap"]);

    runtime
        .evaluate_module(
            &mut js_runtime,
            r"
                import { __AddEvent, __RemoveEventListener } from 'bobcat:element';
                const [, outer, inner] = globalThis.held;
                __AddEvent(inner, 'bindEvent', 'tap', null);
                __RemoveEventListener(outer, 'tap', globalThis.listener, {});
                ",
            "app:///last.mjs",
            "unregistering",
        )
        .expect("unregistration");
    assert!(names.names().is_empty(), "the last registration closes it");
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
                  globalThis.listener = () => {};
                  __AddEventListener(view, 'tap', globalThis.listener, {});
                };
                ",
            "app:///publish.js",
        )
        .expect("main-thread script");
    assert!(names.contains("tap"));
    assert!(!names.contains("scroll"));

    // A second module rather than a second entry: the point is a later
    // unregistration, not a second boot. It goes through the PAPI, because
    // the host has no member that could take one — the realm decides when a
    // name's last registration anywhere is gone.
    runtime
        .evaluate_module(
            &mut js_runtime,
            r"
                import { __RemoveEventListener } from 'bobcat:element';
                __RemoveEventListener(globalThis.held[1], 'tap', globalThis.listener, {});
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

/// The host hands the realm the whole path and nothing else. Which steps
/// have a listener, and in which pass, is the realm's alone — so what is
/// observable here is which listeners ran, in which order, with which phase.
#[test]
fn a_dispatch_runs_the_path_listeners_and_skips_the_steps_with_none() {
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
                    seen.push(label + ':' + event.currentTarget.uid + ':' +
                              event.eventPhase + ':' + event.detail.x);
                  __AddEventListener(page, 'tap', note('page-capture'), { capture: true });
                  __AddEventListener(inner, 'tap', note('inner'), {});
                  // `outer` registers nothing, so the walk must pass over it.
                };
                ",
            "app:///listeners.js",
        )
        .expect("main-thread script");

    let target = 4;
    let delivered = runtime
        .dispatch_for_test(&mut js_runtime, node_id(target), &tap(), event_point())
        .expect("dispatch");
    // All the host learns: the realm published the export it called.
    assert!(delivered);

    runtime
        .evaluate_module(
            &mut js_runtime,
            r"
                // The position the host decided reaches every listener as the
                // event's own `detail`, built in the realm from two numbers.
                if (seen.join('|') !== 'page-capture:2:1:12|inner:4:2:12') {
                  throw new Error('unexpected deliveries: ' + seen.join('|'));
                }
                ",
            "app:///verify.js",
            "verifying",
        )
        .expect("verification");
}

#[test]
fn add_event_delivers_on_the_real_path_and_a_catch_form_ends_the_walk() {
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
                  // The same node, same name, other pass: the capture pass
                  // runs it, and the bubble pass must not reach it twice.
                  __AddEvent(page, 'capture-bind', 'tap', note('page-capture'));
                };
                ",
            "app:///handlers.js",
        )
        .expect("main-thread script");

    assert!(
        runtime
            .dispatch_for_test(&mut js_runtime, node_id(4), &tap(), event_point())
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

/// `global-bindEvent` is not on the path, so its delivery is neither
/// filtered by one nor ended by a `catch` on one. Both handler kinds run,
/// the string before the worklet, at the registered element.
#[test]
fn global_bind_handlers_run_after_the_path_even_when_a_catch_ended_it() {
    let (mut js_runtime, mut runtime, _elements) = runtime();
    runtime
        .run_main_thread_script(
            &mut js_runtime,
            r"
                globalThis.seen = [];
                globalThis.runWorklet = (value, params) => value.body(params[0]);
                globalThis.renderPage = function () {
                  const page = __CreatePage('card', 0);
                  const outer = __CreateView(0);
                  const inner = __CreateView(0);
                  // Off the path from `inner` entirely: a sibling, so only a
                  // delivery that ignores the path can reach it.
                  const aside = __CreateView(0);
                  __AppendElement(page, outer);
                  __AppendElement(outer, inner);
                  __AppendElement(page, aside);
                  globalThis.held = [page, outer, inner, aside];
                  const note = (label) => ({
                    type: 'worklet',
                    value: {
                      body: (event) =>
                        seen.push(
                          label + ':' + event.currentTarget.uid + ':' +
                          event.target.uid + ':' + event.eventPhase,
                        ),
                    },
                  });
                  // A catch on the target ends the walk over the path.
                  __AddEvent(inner, 'catchEvent', 'tap', note('inner-catch'));
                  __AddEvent(outer, 'bindEvent', 'tap', note('outer-bind'));
                  // Both kinds on the one global registration. The string is
                  // published to the background thread, which this realm has
                  // none of, so only the worklet records here — the point of
                  // the pair is that filing one did not clear the other.
                  __AddEvent(aside, 'global-bindEvent', 'tap', 'aside:global');
                  __AddEvent(aside, 'global-bindEvent', 'tap', note('aside-global'));
                };
                ",
            "app:///global-bind.js",
        )
        .expect("main-thread script");

    assert!(
        runtime
            .dispatch_for_test(&mut js_runtime, node_id(4), &tap(), event_point())
            .expect("dispatch")
    );

    runtime
        .evaluate_module(
            &mut js_runtime,
            r"
                import { __GetEvents } from 'bobcat:element';
                // `outer-bind` never ran: the catch ended the path. The
                // global one did, after it, with the event's own target and
                // no phase at all.
                if (seen.join('|') !== 'inner-catch:4:4:2|aside-global:5:4:0') {
                  throw new Error('unexpected deliveries: ' + seen.join('|'));
                }
                const filed = __GetEvents(held[3]);
                if (filed.length !== 2 || filed[0].function !== 'aside:global' ||
                    filed[1].function.type !== 'worklet') {
                  throw new Error('a kind displaced the other: ' + JSON.stringify(filed));
                }
                ",
            "app:///verify.js",
            "verifying",
        )
        .expect("verification");
}

/// A name no path registration holds open is still a name the painting side
/// has to route, or the event that would reach a global handler is dropped
/// before a path is ever built.
#[test]
fn a_global_only_registration_publishes_its_name_and_is_delivered() {
    let (mut js_runtime, mut runtime, _elements, mut names) =
        runtime_over_watching_names(ingredients());
    runtime
        .run_main_thread_script(
            &mut js_runtime,
            r"
                globalThis.seen = [];
                globalThis.runWorklet = (value, params) => value.body(params[0]);
                globalThis.renderPage = function () {
                  const page = __CreatePage('card', 0);
                  const view = __CreateView(0);
                  __AppendElement(page, view);
                  globalThis.held = [page, view];
                  __AddEvent(view, 'global-bindEvent', 'swipe', {
                    type: 'worklet',
                    value: { body: (event) => seen.push(event.currentTarget.uid) },
                  });
                };
                ",
            "app:///global-only.js",
        )
        .expect("main-thread script");
    assert!(names.contains("swipe"));

    assert!(
        runtime
            .dispatch_for_test(
                &mut js_runtime,
                node_id(2),
                &Arc::from("swipe"),
                event_point()
            )
            .expect("dispatch"),
        "a global registration alone is enough to deliver"
    );

    runtime
        .evaluate_module(
            &mut js_runtime,
            r"
                import { __AddEvent } from 'bobcat:element';
                if (seen.join('|') !== '3') {
                  throw new Error('unexpected deliveries: ' + seen.join('|'));
                }
                __AddEvent(held[1], 'global-bindEvent', 'swipe', null);
                ",
            "app:///verify.js",
            "verifying",
        )
        .expect("verification");
    assert!(
        !names.contains("swipe"),
        "the last global registration closes its name like any other"
    );

    // The painting side is what drops an event nobody wants; a dispatch that
    // reaches the realm anyway finds nothing registered and runs nothing.
    runtime
        .dispatch_for_test(
            &mut js_runtime,
            node_id(2),
            &Arc::from("swipe"),
            event_point(),
        )
        .expect("dispatch");
    runtime
        .evaluate_module(
            &mut js_runtime,
            r"
                if (seen.join('|') !== '3') {
                  throw new Error('a cleared registration was delivered to');
                }
                ",
            "app:///verify-cleared.js",
            "verifying",
        )
        .expect("verification");
}

/// `__QuerySelector`/`__QuerySelectorAll` are `Element.querySelector`'s
/// scope, which never answers the element they were asked on — unlike
/// `SelectorQuery`, which shares the same host primitive.
#[test]
fn the_query_selector_papi_never_answers_the_element_it_was_asked_on() {
    let (mut js_runtime, mut runtime, _elements) = runtime();
    runtime
        .run_main_thread_script(
            &mut js_runtime,
            r"
                globalThis.renderPage = function () {
                  const page = __CreatePage('card', 0);
                  const outer = __CreateView(0);
                  const inner = __CreateView(0);
                  __SetClasses(outer, 'row');
                  __SetClasses(inner, 'row');
                  __AppendElement(page, outer);
                  __AppendElement(outer, inner);
                  globalThis.held = [page, outer, inner];
                };
                ",
            "app:///query.js",
        )
        .expect("main-thread script");

    runtime
        .evaluate_module(
            &mut js_runtime,
            r"
                import {
                  __GetElementUniqueID,
                  __GetPageElement,
                  __QuerySelector,
                  __QuerySelectorAll,
                } from 'bobcat:element';
                const [page, outer, inner] = held;
                if (__GetPageElement() !== page) {
                  throw new Error('the page element is not the page handle');
                }
                // `outer` matches `.row` itself and is the root of this
                // query: the answer is its descendant, not itself.
                if (__QuerySelector(outer, '.row', {}) !== inner) {
                  throw new Error('the root answered its own selector');
                }
                const all = __QuerySelectorAll(outer, '.row', {});
                if (all.length !== 1 || all[0] !== inner) {
                  throw new Error(
                    'all: ' + all.map(__GetElementUniqueID).join('|'),
                  );
                }
                // From the page, both rows: the exclusion is of the root
                // alone, not of matching descendants.
                const fromPage = __QuerySelectorAll(page, '.row', {});
                if (fromPage.length !== 2 || fromPage[0] !== outer ||
                    fromPage[1] !== inner) {
                  throw new Error(
                    'fromPage: ' + fromPage.map(__GetElementUniqueID).join('|'),
                  );
                }
                if (__QuerySelector(inner, '.row', {}) !== undefined ||
                    __QuerySelectorAll(inner, '.row', {}).length !== 0) {
                  throw new Error('a leaf answered itself');
                }
                ",
            "app:///verify.js",
            "verifying",
        )
        .expect("verification");
}

/// The one UI method the engine dispatches: `boundingClientRect` answers the
/// border box of the last layout pass, and — as native does, where web-core
/// reports the id alone — the element's `id` attribute and `dataset` ride
/// along. `right` and `bottom` are sums the realm derives.
#[test]
fn invoke_answers_a_bounding_client_rect_carrying_the_elements_id_and_dataset() {
    let (mut js_runtime, mut runtime, _elements) = runtime();
    runtime
        .run_main_thread_script(
            &mut js_runtime,
            r"
                globalThis.renderPage = function () {
                  const page = __CreatePage('card', 0);
                  const view = __CreateView(0);
                  __SetInlineStyles(view, 'width:100px;height:50px;margin-left:20px');
                  __AppendElement(page, view);
                  globalThis.held = [page, view];
                };
                ",
            "app:///invoke.js",
        )
        .expect("main-thread script");

    runtime
        .evaluate_module(
            &mut js_runtime,
            r"
                import {
                  __FlushElementTree,
                  __InvokeUIMethod,
                  __SetDataset,
                  __SetID,
                } from 'bobcat:element';
                const [, view] = held;
                // The callback is synchronous and runs exactly once, which is
                // what a card that measures and then acts in one job needs.
                globalThis.measure = (element, method) => {
                  let answer;
                  let calls = 0;
                  __InvokeUIMethod(element, method, {}, result => {
                    answer = result;
                    calls += 1;
                  });
                  if (calls !== 1) throw new Error('the callback ran ' + calls + ' times');
                  return answer;
                };
                __FlushElementTree();
                const measured = measure(view, 'boundingClientRect');
                const expected = {
                  id: '', dataset: {}, left: 20, top: 0,
                  right: 120, bottom: 50, width: 100, height: 50,
                };
                if (measured.code !== 0) throw new Error(JSON.stringify(measured));
                for (const key of Object.keys(expected)) {
                  if (JSON.stringify(measured.data[key]) !== JSON.stringify(expected[key])) {
                    throw new Error(key + ': ' + JSON.stringify(measured.data));
                  }
                }
                __SetID(view, 'target');
                __SetDataset(view, {k: 1});
                const named = measure(view, 'boundingClientRect');
                if (named.data.id !== 'target' || named.data.dataset.k !== 1) {
                  throw new Error(JSON.stringify(named));
                }
                // Neither attribute moved the box, and neither did measuring.
                if (named.data.left !== 20 || named.data.width !== 100) {
                  throw new Error(JSON.stringify(named));
                }
                // Any other method is unknown to the engine: the shared
                // table's code 3, without a throw and without a `data`.
                const unsupported = measure(view, 'scrollIntoView');
                if (unsupported.code !== 3 || unsupported.data !== undefined) {
                  throw new Error(JSON.stringify(unsupported));
                }
                ",
            "app:///measure.js",
            "measuring",
        )
        .expect("measuring");
}

/// Measuring runs no pipeline step. A job that mutates and then measures
/// sees the box the last pass produced; the new one arrives only once the
/// realm flushes itself, or once the entry's epilogue commits for it.
#[test]
fn a_measurement_reports_the_last_pass_until_something_else_flushes() {
    let (mut js_runtime, mut runtime, _elements) = runtime();
    runtime
        .run_main_thread_script(
            &mut js_runtime,
            r"
                globalThis.renderPage = function () {
                  const page = __CreatePage('card', 0);
                  const view = __CreateView(0);
                  __SetInlineStyles(view, 'width:100px;height:50px;margin-left:20px');
                  __AppendElement(page, view);
                  globalThis.held = [page, view];
                };
                globalThis.left = element => {
                  let answer;
                  __InvokeUIMethod(element, 'boundingClientRect', {}, result => {
                    answer = result;
                  });
                  if (answer.code !== 0) throw new Error(JSON.stringify(answer));
                  if (answer.data.width !== 100) throw new Error(JSON.stringify(answer));
                  return answer.data.left;
                };
                ",
            "app:///no-flush.js",
        )
        .expect("main-thread script");

    runtime
        .evaluate_module(
            &mut js_runtime,
            r"
                import { __FlushElementTree, __SetInlineStyles } from 'bobcat:element';
                const [, view] = held;
                __SetInlineStyles(view, {width: '100px', height: '50px', marginLeft: '40px'});
                if (left(view) !== 20) throw new Error('measuring flushed: ' + left(view));
                __FlushElementTree();
                if (left(view) !== 40) throw new Error('the flush was not seen: ' + left(view));
                __SetInlineStyles(view, {width: '100px', height: '50px', marginLeft: '60px'});
                if (left(view) !== 40) throw new Error('measuring flushed: ' + left(view));
                ",
            "app:///mutate.js",
            "mutating",
        )
        .expect("mutating");

    // The epilogue every entry into the realm owes, which is why the
    // background thread's query path is always measuring a current tree.
    runtime.commit_if_dirty();

    runtime
        .evaluate_module(
            &mut js_runtime,
            r"
                const [, view] = held;
                if (left(view) !== 60) throw new Error('the epilogue did not commit: ' + left(view));
                ",
            "app:///next-entry.js",
            "next entry",
        )
        .expect("next entry");
}

/// Both readback members go through the same element validation as every
/// other tree primitive: a handle whose element has been freed is a script
/// error, not a zero rect or an empty style.
#[test]
fn measuring_or_reading_the_style_of_a_freed_element_fails_the_entry() {
    let (mut js_runtime, mut runtime, _elements) = runtime();
    runtime
        .run_main_thread_script(
            &mut js_runtime,
            r"
                import { dropElement } from 'bobcat-internal:host';
                globalThis.renderPage = function () {
                  const page = __CreatePage('card', 0);
                  const gone = __CreateView(0);
                  __AppendElement(page, gone);
                  __RemoveElement(page, gone);
                  // Called directly, where a finalizer would.
                  dropElement(__GetElementUniqueID(gone));
                  globalThis.gone = gone;
                };
                ",
            "app:///freed.js",
        )
        .expect("main-thread script");

    for (source, name) in [
        (
            "import { __InvokeUIMethod } from 'bobcat:element';
                 __InvokeUIMethod(globalThis.gone, 'boundingClientRect', {}, () => {});",
            "app:///measure-freed.js",
        ),
        (
            "import { __GetComputedStyleByKey } from 'bobcat:element';
                 __GetComputedStyleByKey(globalThis.gone, 'width');",
            "app:///style-freed.js",
        ),
    ] {
        let error = runtime
            .evaluate_module(&mut js_runtime, source, name, "reading a freed element")
            .expect_err("a freed element cannot be read");
        assert!(error.source.message.contains("stale element id"), "{error}");
    }
}

/// `__GetComputedStyleByKey` is CSSOM's `getPropertyValue`, so it takes CSS
/// property names and nothing else — no IDL spelling, no shorthand — and it
/// reports the resolved value without running a pass to produce one.
#[test]
fn computed_style_by_key_answers_css_names_off_the_last_flush() {
    let (mut js_runtime, mut runtime, _elements) = runtime();
    runtime
        .run_main_thread_script(
            &mut js_runtime,
            r"
                globalThis.renderPage = function () {
                  const page = __CreatePage('card', 0);
                  const view = __CreateView(0);
                  __SetInlineStyles(view, 'width:100px;height:50px;margin-left:20px');
                  __AppendElement(page, view);
                  globalThis.held = [page, view, __CreateView(0)];
                };
                ",
            "app:///computed.js",
        )
        .expect("main-thread script");

    runtime
        .evaluate_module(
            &mut js_runtime,
            r"
                import { __FlushElementTree, __GetComputedStyleByKey } from 'bobcat:element';
                const [, view, detached] = held;
                __FlushElementTree();
                const answers = {
                  'margin-top': __GetComputedStyleByKey(view, 'margin-top'),
                  'margin-left': __GetComputedStyleByKey(view, 'margin-left'),
                  // An IDL name is not a CSS name, a shorthand is not a
                  // longhand, and neither is a property at all.
                  'marginTop': __GetComputedStyleByKey(view, 'marginTop'),
                  'margin': __GetComputedStyleByKey(view, 'margin'),
                  'not-a-property': __GetComputedStyleByKey(view, 'not-a-property'),
                  // Live, but no pass has ever reached it.
                  'detached': __GetComputedStyleByKey(detached, 'width'),
                };
                const expected = {
                  'margin-top': '0px', 'margin-left': '20px', 'marginTop': '',
                  'margin': '', 'not-a-property': '', 'detached': '',
                };
                for (const key of Object.keys(expected)) {
                  if (answers[key] !== expected[key]) throw new Error(key + ': ' + JSON.stringify(answers));
                }
                ",
            "app:///read-style.js",
            "reading styles",
        )
        .expect("reading styles");
}

/// The Typed OM half: a whole-style snapshot of computed values, in which a
/// non-custom name that is not a property is a `TypeError` and an absent
/// custom property is simply missing.
#[test]
fn the_computed_style_map_snapshots_every_property_of_a_flushed_element() {
    let (mut js_runtime, mut runtime, _elements) = runtime();
    runtime
        .run_main_thread_script(
            &mut js_runtime,
            r"
                globalThis.renderPage = function () {
                  const page = __CreatePage('card', 0);
                  const view = __CreateView(0);
                  __SetInlineStyles(view, 'width:100px;height:50px;margin-left:20px');
                  __AppendElement(page, view);
                  globalThis.held = [page, view];
                };
                ",
            "app:///style-map.js",
        )
        .expect("main-thread script");

    runtime
        .evaluate_module(
            &mut js_runtime,
            r"
                import { __BobcatComputedStyleMap, __FlushElementTree } from 'bobcat:element';
                const [, view] = held;
                __FlushElementTree();
                const map = __BobcatComputedStyleMap(view);
                // Computed, not resolved: the declared width verbatim.
                if (String(map.get('width')) !== '100px') {
                  throw new Error('width: ' + map.get('width'));
                }
                if (map.has('--nope') !== false || map.get('--nope') !== undefined) {
                  throw new Error('an absent custom property is present');
                }
                if (!(map.size > 50)) throw new Error('size: ' + map.size);
                let thrown;
                try { map.get('bogus'); } catch (error) { thrown = error; }
                if (!(thrown instanceof TypeError)) throw new Error('bogus: ' + thrown);
                ",
            "app:///read-map.js",
            "reading the style map",
        )
        .expect("reading the style map");
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
            .dispatch_for_test(&mut js_runtime, node_id(3), &tap(), event_point())
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
                // Removing it leaves the node with nothing filed, so a
                // further dispatch reaches nobody at all.
                __AddEvent(held[1], 'capture-bind', 'tap', undefined);
                ",
            "app:///verify.js",
            "verifying",
        )
        .expect("verification");

    runtime
        .dispatch_for_test(&mut js_runtime, node_id(3), &tap(), event_point())
        .expect("dispatch");

    runtime
        .evaluate_module(
            &mut js_runtime,
            r"
                if (seen.join('|') !== 'capture') {
                  throw new Error('a cleared handler was still delivered to');
                }
                ",
            "app:///verify-cleared.js",
            "verifying",
        )
        .expect("verification");
}

/// One call is one dispatch, which is what makes one event object serve the
/// whole walk — a property one listener writes reaches the next — and what
/// makes the end of the walk a fact the realm owns: the standard's last
/// dispatch step resets `eventPhase` and `currentTarget` on an event a
/// listener kept.
#[test]
fn one_call_is_one_dispatch_with_one_event_object_that_is_reset_at_its_end() {
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
                .dispatch_for_test(&mut js_runtime, node_id(4), &tap(), event_point())
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
                // Each walk ended in the realm, rather than leaving the kept
                // event still naming whichever node it stopped on.
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
        .dispatch_for_test(&mut js_runtime, node_id(3), &tap(), event_point())
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
        .dispatch_for_test(&mut js_runtime, node_id(3), &tap(), event_point())
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
        .dispatch_for_test(&mut js_runtime, node_id(3), &tap(), event_point())
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

/// The host filters nothing: the painting side already refuses to route a
/// name no listener anywhere has registered, and a dispatch that arrives for
/// one anyway reaches a realm that finds nothing and runs nothing.
#[test]
fn a_dispatch_with_nothing_registered_reaches_the_realm_and_runs_nothing() {
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
        runtime
            .dispatch_for_test(&mut js_runtime, node_id(3), &tap(), event_point())
            .expect("dispatch"),
        "the realm published the export, which is all the answer means now"
    );
}

/// The source and the placeholder of the image tests below, and the pixel
/// count a load reports for the first of them.
const IMAGE_SOURCE: &str = "app:///a.png";
const IMAGE_PLACEHOLDER: &str = "app:///holding.png";
const IMAGE_PIXELS: (u32, u32) = (40, 20);

fn image_loaded(source: &str) -> dom::ImageEvent {
    dom::ImageEvent::Loaded {
        source: Arc::from(source),
        width: IMAGE_PIXELS.0,
        height: IMAGE_PIXELS.1,
    }
}

fn image_failed(source: &str) -> dom::ImageEvent {
    dom::ImageEvent::Failed {
        source: Arc::from(source),
    }
}

/// One image under the page, with a handler of every form that could see its
/// events: a worklet `bindload`/`binderror` on the image itself, the same
/// form on its parent, a `capture-bind` on the page, and a
/// `global-bindEvent` off the path. What runs is what the non-bubbling shape
/// allows, and `seen` is the record.
fn image_page(script_url: &str) -> (ScriptRuntime, MainThreadRuntime, DocumentProbe) {
    let (mut js_runtime, mut runtime, elements) = runtime();
    runtime
        .run_main_thread_script(
            &mut js_runtime,
            r"
                globalThis.seen = [];
                globalThis.runWorklet = (value, params) => value.body(params[0]);
                globalThis.renderPage = function () {
                  const page = __CreatePage('card', 0);
                  const outer = __CreateView(0);
                  const image = __CreateImage(0);
                  const aside = __CreateView(0);
                  __AppendElement(page, outer);
                  __AppendElement(outer, image);
                  __AppendElement(page, aside);
                  globalThis.held = [page, outer, image, aside];
                  const note = (label) => ({
                    type: 'worklet',
                    value: {
                      body: (event) =>
                        seen.push(
                          label + ':' + event.currentTarget.uid + ':' +
                          event.type + ':' + JSON.stringify(event.detail),
                        ),
                    },
                  });
                  for (const name of ['load', 'error']) {
                    __AddEvent(image, 'bindEvent', name, note('image'));
                    __AddEvent(outer, 'bindEvent', name, note('outer'));
                    __AddEvent(page, 'capture-bind', name, note('page-capture'));
                    __AddEvent(aside, 'global-bindEvent', name, note('aside-global'));
                  }
                };
                ",
            script_url,
        )
        .expect("main-thread script");
    (js_runtime, runtime, elements)
}

/// Asserts what the realm has recorded since the last check, and clears it.
///
/// The check runs in the realm, as every other event test's does: a
/// verification module throws, and the failure carries what was delivered.
fn expect_seen(js_runtime: &mut ScriptRuntime, runtime: &mut MainThreadRuntime, expected: &str) {
    runtime
        .evaluate_module(
            js_runtime,
            &format!(
                r"
                const actual = seen.join('|');
                seen.length = 0;
                if (actual !== {expected:?}) {{
                  throw new Error('unexpected deliveries: ' + actual);
                }}
                "
            ),
            "app:///verify-images.mjs",
            "verifying the deliveries",
        )
        .expect("the deliveries are what the dispatch owed");
}

/// An `<image>`'s `load` is web-core's: the intrinsic pixel size as the
/// detail, and non-bubbling, so the capture pass still runs the whole path,
/// the bind pass runs on the image alone, and there is no `global-bindEvent`
/// pass at all.
#[test]
fn an_image_load_carries_its_intrinsic_size_and_does_not_bubble() {
    let (mut js_runtime, mut runtime, _elements) = image_page("app:///image-load.js");
    js_set_src(&mut js_runtime, &mut runtime, IMAGE_SOURCE);
    // A source that has not loaded owes nothing.
    expect_seen(&mut js_runtime, &mut runtime, "");

    runtime.apply_image_events(&[image_loaded(IMAGE_SOURCE)]);
    assert!(runtime.dispatch_image_outcomes(&mut js_runtime).is_empty());

    expect_seen(
        &mut js_runtime,
        &mut runtime,
        r#"page-capture:2:load:{"width":40,"height":20}|image:4:load:{"width":40,"height":20}"#,
    );
}

/// `error` carries `{}`. Native's payload is richer — `errMsg`,
/// `error_code`, `lynx_categorized_code` — and web-core is what this follows;
/// see `docs/tracking/deviations.md`.
#[test]
fn an_image_error_carries_an_empty_detail() {
    let (mut js_runtime, mut runtime, _elements) = image_page("app:///image-error.js");
    js_set_src(&mut js_runtime, &mut runtime, IMAGE_SOURCE);

    runtime.apply_image_events(&[image_failed(IMAGE_SOURCE)]);
    assert!(runtime.dispatch_image_outcomes(&mut js_runtime).is_empty());

    expect_seen(
        &mut js_runtime,
        &mut runtime,
        "page-capture:2:error:{}|image:4:error:{}",
    );
}

/// The placeholder is a second source the page did not ask about, so neither
/// of its endings is an event — while the `src` behind it still reports its
/// own, whichever way it ends.
#[test]
fn a_placeholder_settling_is_nobody_s_event() {
    let (mut js_runtime, mut runtime, _elements) = image_page("app:///image-placeholder.js");
    runtime
        .evaluate_module(
            &mut js_runtime,
            r"
                import { __SetAttribute } from 'bobcat:element';
                __SetAttribute(held[2], 'placeholder', 'app:///holding.png');
                __SetAttribute(held[2], 'src', 'app:///a.png');
                ",
            "app:///sources.mjs",
            "writing both sources",
        )
        .expect("writing both sources");

    for event in [
        image_loaded(IMAGE_PLACEHOLDER),
        image_failed(IMAGE_PLACEHOLDER),
    ] {
        runtime.apply_image_events(&[event]);
        assert!(runtime.dispatch_image_outcomes(&mut js_runtime).is_empty());
        // A placeholder is an interim picture, not an answer to the page.
        expect_seen(&mut js_runtime, &mut runtime, "");
    }

    // The source behind it is still the element's own, and its failure is
    // still the element's `error` — once.
    runtime.apply_image_events(&[image_failed(IMAGE_SOURCE), image_failed(IMAGE_SOURCE)]);
    assert!(runtime.dispatch_image_outcomes(&mut js_runtime).is_empty());
    expect_seen(
        &mut js_runtime,
        &mut runtime,
        "page-capture:2:error:{}|image:4:error:{}",
    );
}

/// Binding is what asks the host for a URL, so a URL this document has
/// already settled answers at the bind — and no report will ever arrive for
/// it again. It is still delivered as its own turn's work rather than from
/// inside the `__SetAttribute` that wrote it, which is web-core's shape too:
/// an `<img>` load event is a task, cached URL or not.
#[test]
fn a_second_mount_of_a_settled_source_is_delivered_after_the_call_that_bound_it() {
    let (mut js_runtime, mut runtime, _elements) = image_page("app:///image-remount.js");
    runtime.apply_image_events(&[image_loaded(IMAGE_SOURCE)]);
    assert!(
        runtime.dispatch_image_outcomes(&mut js_runtime).is_empty(),
        "nothing holds the source yet, so the report settles the registry alone"
    );
    expect_seen(&mut js_runtime, &mut runtime, "");

    // The bind settled it, and dispatching from inside `__SetAttribute` would
    // re-enter the realm in the middle of that call.
    js_set_src(&mut js_runtime, &mut runtime, IMAGE_SOURCE);
    expect_seen(&mut js_runtime, &mut runtime, "");

    assert!(runtime.dispatch_image_outcomes(&mut js_runtime).is_empty());
    expect_seen(
        &mut js_runtime,
        &mut runtime,
        r#"page-capture:2:load:{"width":40,"height":20}|image:4:load:{"width":40,"height":20}"#,
    );

    // Exactly once: the queue was drained, and rewriting the value already
    // there binds nothing.
    assert!(runtime.dispatch_image_outcomes(&mut js_runtime).is_empty());
    js_set_src(&mut js_runtime, &mut runtime, IMAGE_SOURCE);
    assert!(runtime.dispatch_image_outcomes(&mut js_runtime).is_empty());
    expect_seen(&mut js_runtime, &mut runtime, "");
}

/// An element freed between the outcome forming and its delivery resolves to
/// nothing. A `NodeId` names one node for the life of the document, so the
/// lookup can never reach a stranger.
#[test]
fn an_element_collected_before_its_load_is_delivered_gets_nothing() {
    let (mut js_runtime, mut runtime, elements) = image_page("app:///image-collected.js");
    js_set_src(&mut js_runtime, &mut runtime, IMAGE_SOURCE);
    runtime.apply_image_events(&[image_loaded(IMAGE_SOURCE)]);

    runtime
        .evaluate_module(
            &mut js_runtime,
            r"
                import { __RemoveElement } from 'bobcat:element';
                __RemoveElement(held[1], held[2]);
                held[2] = undefined;
                ",
            "app:///drop.mjs",
            "dropping the image",
        )
        .expect("dropping the image");
    runtime
        .collect_garbage(&mut js_runtime)
        .expect("collection");
    assert!(
        elements.tree().get(node_id(4)).is_none(),
        "the image is freed before its `load` is delivered"
    );

    assert!(runtime.dispatch_image_outcomes(&mut js_runtime).is_empty());
    expect_seen(&mut js_runtime, &mut runtime, "");
}

/// Writes a `src` the way a card does, through the PAPI.
fn js_set_src(js_runtime: &mut ScriptRuntime, runtime: &mut MainThreadRuntime, source: &str) {
    runtime
        .evaluate_module(
            js_runtime,
            &format!(
                r"
                import {{ __SetAttribute }} from 'bobcat:element';
                __SetAttribute(held[2], 'src', '{source}');
                "
            ),
            "app:///src.mjs",
            "writing the source",
        )
        .expect("writing the source");
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

/// Native Lynx accepts `text-overflow` as an element attribute on `<text>`
/// as well as a CSS property (`text_element.cc:176-182`), and the Lynx UA
/// sheet's attribute selectors are how this engine honours that
/// (`crates/bobcat-core/src/main/tree/text.rs`). This drives the whole path a
/// compiled card drives — `__SetAttribute` on a text already flushed — in
/// both directions: `ellipsis` widens the clamped paragraph by the three-dot
/// marker, and `clip` takes it away again.
#[test]
#[expect(clippy::float_cmp, reason = "Ahem em squares have exact metrics")]
fn the_text_overflow_attribute_from_papi_reaches_the_paragraph_marker() {
    let (mut js_runtime, mut runtime, elements) = text_runtime();
    runtime
        .run_main_thread_script(
            &mut js_runtime,
            r"
                globalThis.renderPage = function () {
                  const page = __CreatePage('card', 0);
                  const texts = ['widened', 'restored'].map((name) => {
                    const text = __CreateText(0);
                    __SetID(text, name);
                    __SetInlineStyles(text, 'font-family:Ahem;font-size:20px;line-height:21px');
                    __SetAttribute(text, 'text-maxlength', '1');
                    __AppendElement(text, __CreateRawText('abc def'));
                    __AppendElement(page, text);
                    return text;
                  });
                  // The clamp is committed with no text-overflow at all first,
                  // so each later write is an update rather than a first pass.
                  __FlushElementTree();
                  for (const text of texts) __SetAttribute(text, 'text-overflow', 'ellipsis');
                  __FlushElementTree();
                  __SetAttribute(texts[1], 'text-overflow', 'clip');
                };
                ",
            "app:///text-overflow-attribute.js",
        )
        .expect("main-thread script");

    let tree = elements.tree();
    for (name, width, attribute) in [("widened", 80.0, "ellipsis"), ("restored", 20.0, "clip")] {
        let text = tree
            .document_element()
            .children()
            .find(|node| node.attribute("id") == Some(name))
            .expect("the text element");
        assert_eq!(
            text.attribute("text-overflow"),
            Some(attribute),
            "{name}: the attribute reaches the document verbatim"
        );
        let id = text.id();
        assert_eq!(
            tree.text_block_size(id).expect("paragraph").width,
            width,
            "{name}: one em square, plus the three-dot marker under `ellipsis`"
        );
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
                const page = __CreatePage('card', 0);
                const view = __CreateView(0);
                __AppendElement(page, view);
                dropElement(__GetElementUniqueID(view));
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
        .dispatch_for_test(&mut js_runtime, run, &tap(), event_point())
        .expect_err("a target no handle names cannot be delivered");
    assert!(error.to_string().contains("ownership graph"), "{error}");
}

/// `__SetAttribute(list, "update-list-info", …)` against the real document,
/// which is where the protocol's two primitives actually live: the child at
/// an index (`childElementIds`) and the tree edits decided from it.
///
/// The batch is serviced in a microtask, so the callbacks a card files
/// *after* writing the operations are the ones that serve it — which is the
/// order `ReactLynx`'s own `ListUpdateInfoRecording.flush` writes in. Boot ends
/// in a checkpoint, so by the time this returns the queue has drained; what
/// the microtasks found is written back onto the list as attributes, since
/// the document is the only thing this test can read.
#[test]
fn update_list_info_builds_and_retires_cells_through_the_filed_callbacks() {
    let (mut js_runtime, mut runtime, elements) = runtime();
    runtime
        .run_main_thread_script(
            &mut js_runtime,
            r"
                globalThis.renderPage = function () {
                  const page = __CreatePage('card', 0);
                  const list = __CreateList(0, null, null);
                  __SetID(list, 'cells');
                  __AppendElement(page, list);
                  const listId = __GetElementUniqueID(list);
                  const keys = new Map();
                  const cells = [0, 1, 2, 3].map(index => {
                    const cell = __CreateElement('list-item', 0);
                    __SetAttribute(cell, 'item-key', 'cell-' + index);
                    keys.set(__GetElementUniqueID(cell), 'cell-' + index);
                    return cell;
                  });
                  const retired = [];
                  const retire = (element, id, sign) =>
                    retired.push(element === list && id === listId
                      ? keys.get(sign) ?? sign
                      : 'wrong-list');

                  // First batch: four cells, appended in position order.
                  __SetAttribute(list, 'update-list-info', {
                    insertAction: [0, 1, 2, 3].map(position => ({ position })),
                    removeAction: [],
                    // Ignored, as web-core's accepted payload ignores it.
                    updateAction: [{ from: 0, to: 0, type: 'x', flush: false }],
                  });
                  // Filed after the write, exactly as the framework files
                  // them: a synchronous service would have missed them.
                  __UpdateListCallbacks(
                    list,
                    (element, id, index, operationID, reuse) => {
                      if (element !== list || id !== listId || operationID !== 0 || reuse !== false)
                        return undefined;
                      __AppendElement(element, cells[index]);
                      return __GetElementUniqueID(cells[index]);
                    },
                    retire,
                    () => { throw Error('componentAtIndexes is never called'); },
                  );
                  if (__GetChildren(list).length !== 0)
                    throw Error('the protocol ran before its microtask');

                  // Second batch, behind the first: old indices 1 and 3 go,
                  // and the cell that was at 2 is left between them.
                  Promise.resolve().then(() => {
                    __SetAttribute(list, 'data-first-batch',
                      __GetChildren(list).map(__GetElementUniqueID).length);
                    __SetAttribute(list, 'update-list-info', {
                      insertAction: [],
                      removeAction: [1, 3],
                    });
                    __UpdateListCallbacks(list, null, retire, null);
                    // Queued after the batch's own microtask, so it reads
                    // what the removals left.
                    Promise.resolve().then(() => {
                      __SetAttribute(list, 'data-retired', retired.join(','));
                    });
                  });
                };
                ",
            "app:///list.js",
        )
        .expect("the list protocol");

    let tree = elements.tree();
    let list = tree
        .document_element()
        .children()
        .find(|node| node.attribute("id") == Some("cells"))
        .expect("the list element");
    assert_eq!(
        list.attribute("data-first-batch"),
        Some("4"),
        "every insertAction position built its cell"
    );
    assert_eq!(
        list.attribute("data-retired"),
        Some("cell-1,cell-3"),
        "enqueueComponent is told the list and the sign of each cell removed"
    );
    let keys: Vec<_> = list
        .children()
        .map(|cell| (cell.tag_name(), cell.attribute("item-key")))
        .collect();
    assert_eq!(
        keys,
        vec![
            (Some("list-item"), Some("cell-0")),
            (Some("list-item"), Some("cell-2")),
        ],
        "the i-th removal takes the child at `position - i`"
    );
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

fn preload_url(far: &mut PublishedNames) -> String {
    loop {
        match far.0.notices.try_recv().unwrap() {
            ViewNotice::PreloadSource(crate::resource::SourceRequest::StyleSheet(url)) => {
                return url;
            }
            ViewNotice::RequestSource { .. } => panic!("preload must not request a response"),
            _ => {}
        }
    }
}

fn requested_module(
    notices: &mut mpsc::UnboundedReceiver<ViewNotice>,
) -> (String, crate::resource::SourceCompletion) {
    use crate::link::block_on_deadline;
    let deadline = ClockInstant::now() + std::time::Duration::from_secs(10);
    loop {
        if let ViewNotice::RequestSource {
            request,
            completion,
        } = block_on_deadline(notices.recv(), deadline)
            .flatten()
            .unwrap()
        {
            let crate::resource::SourceRequest::Module(url) = request else {
                panic!("expected a module");
            };
            return (url, completion);
        }
    }
}

/// One `require` answered, from whichever URL the host says it found it at.
fn module_source(source: &str, url: &str) -> crate::resource::LoadedSource {
    crate::resource::LoadedSource::Entry {
        source: source.to_owned(),
        url: url.to_owned(),
    }
}

// Plain tests rather than `tokio::test`s, for the reason the adoption's own
// pin below spells out: a `require`'s wait is a `block_on` of the realm's
// engine thread, which tokio refuses from inside a runtime.
#[test]
fn require_waits_for_its_own_request_and_resolves_against_the_response_url() {
    let (mut js, mut runtime, _elements, far) = runtime_over_watching_names(ingredients());
    let mut notices = far.0.notices;
    let host = std::thread::spawn(move || {
        for (expected, response, source) in [
            (
                "app:///lib/answer.cjs",
                "https://cdn.test/lib/answer.cjs",
                "exports.answer = require('./deep.cjs').answer + 1;\nexports.dir = __dirname;",
            ),
            (
                "https://cdn.test/lib/deep.cjs",
                "https://cdn.test/lib/deep.cjs",
                "exports.answer = 41;",
            ),
            (
                "app:///config.json",
                "app:///config.json",
                r#"{"name": "card"}"#,
            ),
        ] {
            let (url, completion) = requested_module(&mut notices);
            assert_eq!(url, expected);
            completion.complete(Ok(module_source(source, response)));
        }
    });
    runtime
        .evaluate_module(
            &mut js,
            r"
        import { createRequire } from 'bobcat:module';
        const require = createRequire(import.meta.url);
        let jobRan = false;
        Promise.resolve().then(() => { jobRan = true; });
        const lib = require('./lib/answer.cjs');
        if (lib.answer !== 42) throw Error('answer ' + lib.answer);
        if (lib.dir !== 'https://cdn.test/lib/') throw Error('dirname ' + lib.dir);
        if (jobRan) throw Error('require ran JS jobs while waiting');
        const config = require('./config.json');
        if (config.name !== 'card') throw Error('config ' + JSON.stringify(config));
        if (require.resolve('./config.json') !== 'app:///config.json') throw Error('resolve');
        if (require.cache['app:///config.json'].exports !== config) throw Error('cache');
    ",
            "app:///require.js",
            "requiring through the host",
        )
        .unwrap();
    host.join().unwrap();
}

#[test]
fn a_require_that_cannot_load_throws_and_leaves_the_realm_usable() {
    for outcome in ["failure", "dropped", "cancelled"] {
        let (mut js, mut runtime, _elements, far) = runtime_over_watching_names(ingredients());
        let token = far.0.token.clone();
        let mut notices = far.0.notices;
        let host = std::thread::spawn(move || {
            let (url, completion) = requested_module(&mut notices);
            assert_eq!(url, "app:///missing.cjs");
            match outcome {
                "failure" => completion.complete(Err(crate::resource::unanswered_source().into())),
                "dropped" => drop(completion),
                // The embedder's release, while this realm is parked on the
                // answer: the token is the other arm of that wait.
                "cancelled" => {
                    token.cancel();
                    return Some(completion);
                }
                _ => unreachable!(),
            }
            None
        });
        runtime
            .evaluate_module(
                &mut js,
                r"
            import { createRequire } from 'bobcat:module';
            const require = createRequire(import.meta.url);
            let message = '';
            try { require('./missing.cjs'); }
            catch (error) { message = String(error); }
            if (!message.includes('app:///missing.cjs')) throw Error('lost the URL: ' + message);
            globalThis.caught = message;
        ",
                "app:///missing.js",
                "a require nobody answers",
            )
            .unwrap();
        if let Some(completion) = host.join().unwrap() {
            assert!(completion.is_cancelled());
        }
        runtime
            .evaluate_module(
                &mut js,
                "if (typeof globalThis.caught !== 'string') throw Error('the realm is broken');",
                "app:///after.js",
                "the realm after a failed require",
            )
            .unwrap();
    }
}

/// A name under an engine prefix is answered from the runtime's built-ins and
/// this realm's host modules, and from nothing else. One that is neither
/// fails in the realm with a `ReferenceError`, through an `import` or a
/// `require`, and never reaches the host: `bobcat:worker` is registered, but
/// it imports `bobcat-internal:worker`, which an MTS realm does not declare.
/// `bobcat:lynx-modules`, `bobcat:selector-query` and
/// `bobcat:global-event-emitter` import nothing an MTS realm lacks, so they
/// load here as well.
#[test]
fn an_engine_name_nothing_answers_fails_in_the_realm_without_a_request() {
    let (mut js, mut runtime, _elements, mut far) = runtime_over_watching_names(ingredients());
    runtime
        .evaluate_module(
            &mut js,
            r"
        import { createRequire } from 'bobcat:module';
        const expected = [
            ['bobcat:nope', 'bobcat:nope'],
            ['bobcat-internal:nope', 'bobcat-internal:nope'],
            ['bobcat:worker', 'bobcat-internal:worker'],
        ];
        for (const [specifier, named] of expected) {
            let failure;
            try { await import(specifier); } catch (error) { failure = error; }
            if (!(failure instanceof ReferenceError) || !failure.message.includes(`'${named}'`))
                throw Error(`${specifier}: ${failure}`);
        }
        let failure;
        try { createRequire(import.meta.url)('bobcat:nope'); } catch (error) { failure = error; }
        if (!(failure instanceof ReferenceError) || !failure.message.includes(`'bobcat:nope'`))
            throw Error(`require: ${failure}`);
        await import('bobcat:lynx-modules');
        await import('bobcat:selector-query');
        await import('bobcat:global-event-emitter');
        globalThis.finished = true;
    ",
            "app:///engine-names.js",
            "importing engine names",
        )
        .unwrap();
    runtime
        .evaluate_module(
            &mut js,
            "if (globalThis.finished !== true) throw Error('the imports never settled');",
            "app:///after.js",
            "the realm after the imports",
        )
        .unwrap();
    assert_eq!(runtime.take_module_request(), None);
    while let Ok(notice) = far.0.notices.try_recv() {
        assert!(
            !matches!(notice, ViewNotice::RequestSource { .. }),
            "an engine name reached the host"
        );
    }
}

/// The members an MTS realm's `bobcat-internal:host` exports, which is what
/// decides the built-ins it can link. Written down so that a change to the
/// set is a change to this list. A namespace lists its exports sorted by
/// name; `testFuture` is the test build's own producer.
#[test]
fn an_mts_realm_declares_these_host_members() {
    let expected = [
        "adoptStyleSheet",
        "attributeNames",
        "callElementMethod",
        "childElementIds",
        "clearTimer",
        "createDocument",
        "createElement",
        "createPage",
        "createWorker",
        "dropElement",
        "fetchResource",
        "flushElementTree",
        "getAttribute",
        "getComputedStyleMap",
        "globalProps",
        "initData",
        "initialProcessor",
        "insertBefore",
        "listenerNameClosed",
        "listenerNameOpened",
        "loadModuleSync",
        "logScriptMessage",
        "nativeModuleTable",
        "parentNode",
        "preloadStyleSheet",
        "queryElementIds",
        "removeAttribute",
        "removeElement",
        "replaceElement",
        "reportScriptError",
        "requestScriptFrame",
        "resolveModuleUrl",
        "sendWorkerMessage",
        "setAttribute",
        "setInlineStyleProperty",
        "setInlineStyles",
        "setTimer",
        "settleFuture",
        "supportsStyleProperty",
        "swapElement",
        "tagName",
        "takeFuture",
        "terminateWorker",
        "testFuture",
        "waitFuture",
    ]
    .join(",");
    let (mut js, mut runtime, _elements) = runtime();
    runtime
        .evaluate_module(
            &mut js,
            &format!(
                r"
        const members = Object.keys(await import('bobcat-internal:host')).join(',');
        if (members !== '{expected}') throw Error(members);
        globalThis.finished = true;
    "
            ),
            "app:///members.js",
            "reading the host members",
        )
        .unwrap();
    runtime
        .evaluate_module(
            &mut js,
            "if (globalThis.finished !== true) throw Error('the import never settled');",
            "app:///after.js",
            "the realm after the import",
        )
        .unwrap();
}

fn requested_stylesheet(
    notices: &mut mpsc::UnboundedReceiver<ViewNotice>,
) -> (String, crate::resource::SourceCompletion) {
    use crate::link::block_on_deadline;
    let deadline = ClockInstant::now() + std::time::Duration::from_secs(10);
    loop {
        if let ViewNotice::RequestSource {
            request,
            completion,
        } = block_on_deadline(notices.recv(), deadline)
            .flatten()
            .unwrap()
        {
            let crate::resource::SourceRequest::StyleSheet(url) = request else {
                panic!("expected a stylesheet");
            };
            return (url, completion);
        }
    }
}

fn sheet_source(text: bool, width: &str) -> crate::resource::LoadedSource {
    use crate::resource::{LoadedSource, StyleSheetSource};
    LoadedSource::StyleSheet(if text {
        StyleSheetSource::Text(format!("\u{feff}.box{{width:{width}}}"))
    } else {
        StyleSheetSource::Preparsed(Arc::new(crate::PreparsedStyleSheet {
            rules: vec![crate::PreparsedRule::Style {
                selectors: ".box".into(),
                declarations: vec![crate::PreparsedDeclaration {
                    property: "width".into(),
                    value: width.into(),
                    important: false,
                }],
            }],
        }))
    })
}

#[test]
#[expect(clippy::float_cmp, reason = "rounded widths are exact CSS pixels")]
fn every_adoption_requests_its_url_and_mounts_the_fetchers_response() {
    use crate::resource::StyleSheetSource;
    for text in [true, false] {
        let (mut js, mut runtime, elements, mut far) = runtime_over_watching_names(ingredients());
        runtime
            .run_main_thread_script_over_sheets(
                &mut js,
                vec![(
                    "app:///index.css",
                    crate::resource::LoadedSource::StyleSheet(StyleSheetSource::Text(
                        ".box{width:20px;height:10px} #strong{width:90px} .important{width:95px!important}"
                            .into(),
                    )),
                )],
                r"
            const page = __CreatePage();
            for (let i = 0; i !== 3; ++i) {
                const element = __CreateView(0);
                __SetClasses(element, i === 2 ? 'box important' : 'box');
                if (i === 1) __SetID(element, 'strong');
                __AppendElement(page, element);
            }
            if (__Card__ !== 'app:///main.js') throw Error('entry URL');
            globalThis.a = __LoadStyleSheet('CSS', '__Card__');
            globalThis.b = __LoadStyleSheet('CSS', 'https://cdn.test/bundle');
        ",
                "app:///main.js",
            )
            .unwrap();
        let widths = || {
            let tree = elements.tree();
            [3, 4, 5].map(|id| tree.rounded_layout(node_id(id)).unwrap().size.width)
        };
        let url_a = preload_url(&mut far);
        let url_b = preload_url(&mut far);
        assert_eq!(url_a, "app:///main.js/index.css");
        assert_eq!(url_b, "https://cdn.test/bundle/index.css");
        assert_eq!(widths(), [20.0, 90.0, 95.0], "preloading does not adopt");
        let mut notices = far.0.notices;
        let host = std::thread::spawn(move || {
            // This fetcher ignores preload hints and answers each adoption.
            // Repeated A returns different CSS, proving core kept no result cache.
            for (expected, width) in [
                (&url_b, "80px"),
                (&url_a, "40px"),
                (&url_b, "80px"),
                (&url_a, "50px"),
            ] {
                let (url, completion) = requested_stylesheet(&mut notices);
                assert_eq!(&url, expected);
                completion.complete(Ok(sheet_source(text, width)));
            }
        });
        runtime
            .evaluate_module(
                &mut js,
                r"
            import {__AdoptStyleSheet} from 'bobcat:runtime';
            import {__FlushElementTree} from 'bobcat:element';
            __AdoptStyleSheet(b); __FlushElementTree();
        ",
                "app:///adopt-b.js",
                "adopting B before A",
            )
            .unwrap();
        assert_eq!(widths(), [80.0, 90.0, 95.0]);
        runtime
            .evaluate_module(
                &mut js,
                r"
            import {__AdoptStyleSheet} from 'bobcat:runtime';
            import {__FlushElementTree} from 'bobcat:element';
            __AdoptStyleSheet(a); __AdoptStyleSheet(b); __AdoptStyleSheet(a);
            __FlushElementTree(); delete globalThis.a; delete globalThis.b;
        ",
                "app:///adopt-aba.js",
                "adopting A, B, A",
            )
            .unwrap();
        host.join().unwrap();
        assert_eq!(
            widths(),
            [50.0, 90.0, 95.0],
            "each call uses the fetcher's current response"
        );
        runtime.collect_garbage(&mut js).unwrap();
        assert_eq!(
            widths(),
            [50.0, 90.0, 95.0],
            "mounted styles outlive JS handles"
        );
    }
}

// A plain test rather than a `tokio::test`: the adoption's wait is a
// `block_on` of the realm's own engine thread, and starting a runtime from
// inside another one is what tokio refuses. Production is the same shape — the
// wait happens in a job, which the top loop runs outside its `block_on`.
#[test]
#[expect(clippy::float_cmp, reason = "rounded widths are exact CSS pixels")]
fn adopt_waits_for_its_own_request_and_reports_errors_synchronously() {
    for outcome in ["success", "failure", "dropped", "cancelled"] {
        let (mut js, mut runtime, elements, mut far) = runtime_over_watching_names(ingredients());
        runtime
            .run_main_thread_script(
                &mut js,
                r"
            const page = __CreatePage();
            const box = __CreateView(0);
            __SetClasses(box, 'box'); __AppendElement(page, box);
            globalThis.sheet = __LoadStyleSheet('CSS', '__Card__');
        ",
                "app:///main.js",
            )
            .unwrap();
        assert_eq!(preload_url(&mut far), "app:///main.js/index.css");
        let token = far.0.token.clone();
        let mut notices = far.0.notices;
        let host = std::thread::spawn(move || {
            let (_, completion) = requested_stylesheet(&mut notices);
            match outcome {
                "success" => completion.complete(Ok(sheet_source(true, "40px"))),
                "failure" => completion.complete(Err(crate::resource::unanswered_source().into())),
                "dropped" => drop(completion),
                "cancelled" => {
                    token.cancel();
                    return Some(completion);
                }
                _ => unreachable!(),
            }
            None
        });
        let expected_failure = outcome != "success";
        runtime
            .evaluate_module(
                &mut js,
                &format!(
                    r"
            import {{__AdoptStyleSheet}} from 'bobcat:runtime';
            import {{__FlushElementTree}} from 'bobcat:element';
            let jobRan = false;
            Promise.resolve().then(() => {{ jobRan = true; }});
            let failed = false;
            try {{ __AdoptStyleSheet(sheet); }}
            catch (error) {{
                failed = true;
                if (!String(error).includes('app:///main.js/index.css')) throw error;
            }}
            if (failed !== {expected_failure}) throw Error('adopt failure was not synchronous');
            if (jobRan) throw Error('adopt ran JS jobs while waiting');
            __FlushElementTree();
        "
                ),
                "app:///adopt.js",
                "synchronous adoption",
            )
            .unwrap();
        if let Some(completion) = host.join().unwrap() {
            assert!(completion.is_cancelled());
        }
        if !expected_failure {
            assert_eq!(
                elements
                    .tree()
                    .rounded_layout(node_id(3))
                    .unwrap()
                    .size
                    .width,
                40.0
            );
        }
    }
}

#[test]
fn collecting_a_js_style_handle_sends_no_native_release_or_load_request() {
    let (mut js, mut runtime, _elements, mut far) = runtime_over_watching_names(ingredients());
    runtime
        .run_main_thread_script(
            &mut js,
            "globalThis.sheet = __LoadStyleSheet('CSS', '__Card__');",
            "app:///main.js",
        )
        .unwrap();
    assert_eq!(preload_url(&mut far), "app:///main.js/index.css");
    runtime
        .evaluate_module(
            &mut js,
            "delete globalThis.sheet;",
            "app:///release.js",
            "collect handle",
        )
        .unwrap();
    runtime.collect_garbage(&mut js).unwrap();
    while let Ok(notice) = far.0.notices.try_recv() {
        assert!(!matches!(
            notice,
            ViewNotice::PreloadSource(_) | ViewNotice::RequestSource { .. }
        ));
    }
}

/// One `Future` in a view's MTS realm, read both ways over the real
/// boundary: a real page, its own token, and the settle task its epilogue
/// spawns.
///
/// Nothing in production registers a future yet, so the operation is the
/// host's test-only producer — `testFuture(delayMs, value, rejects)`, which
/// is why this test is in the crate rather than beside it. `wait(20)` runs
/// out its deadline against a 200 ms operation, and the `await` that follows
/// is the *same* Future, which is what says the timeout cancelled nothing.
/// Past that conversion the Future is a Promise, and a third read of it is
/// refused.
#[test]
fn one_mts_future_times_out_then_settles_as_a_promise_and_refuses_a_later_wait() {
    let mut engine = crate::test_support::TestViewSpec::new(
        r"
        import { Future } from 'bobcat:future';
        import { testFuture } from 'bobcat-internal:host';

        const slow = new Future(testFuture(200, 'late', false));
        try {
          slow.wait(20);
          console.log('mts the wait answered');
        } catch (error) {
          console.log('mts wait ' + error.name);
        }
        console.log('mts then ' + await slow);
        try {
          slow.wait();
          console.log('mts the third read answered');
        } catch (error) {
          console.log('mts after ' + error.name);
        }
        try {
          await new Future(testFuture(1, 'why', true));
          console.log('mts the rejection resolved');
        } catch (error) {
          console.log('mts catch ' + (error instanceof Error) + ' ' + error.message);
        }
        console.log('mts now ' + new Future(testFuture(1, 'now', false)).wait());
        __CreatePage();
    ",
    )
    .create(Arc::new(NoWakeup));

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    let mut logged = Vec::new();
    while logged.len() < 5 {
        for event in engine.pump() {
            match event {
                crate::EngineEvent::ConsoleMessage { message, .. } => logged.push(message),
                crate::EngineEvent::StartupFailed(error) => panic!("boot failed: {error}"),
                crate::EngineEvent::ScriptRunError(error) => {
                    panic!("the realm failed: {}", error.message)
                }
                _ => {}
            }
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the realm read its Future both ways: {logged:?}"
        );
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    assert_eq!(
        logged,
        [
            "mts wait TimeoutError",
            "mts then late",
            "mts after TypeError",
            "mts catch true why",
            "mts now now",
        ]
    );
}
