use tokio::sync::mpsc;

use super::*;
use crate::background::{WorkerCommand, WorkerEvent, WorkerPayload};
use crate::esm::build_runtime;
use crate::jobs::JsThread;
use crate::link::{DetachedView, detached_outbox, detached_outbox_probing_style_sheets};
use crate::main::record::write_record_field;
use crate::main::tree::{PageConfig, Viewport};
use crate::main::workers::WorkerFactory;
use crate::view::{NoWakeup, ScriptSource, WorkerId};

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
/// What a chunk shares with the root is the bindings `MTS_CHUNK_PREAMBLE`
/// gives a card's root — the realm hands them to the chunk as the parameters
/// of the function body it was compiled as — and this realm's `globalThis`.
/// Not the root's lexical scope, and not the root's `var`s: a `var` at a
/// chunk's top level is local to that call.
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
    runtime.evaluate_module(&mut js, &card_entry(r#"
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
    runtime_over_watching_names_probing(ingredients, None)
}

/// [`runtime_over_watching_names`], over a view whose fetcher answers
/// `style_sheet_probe` with `probe`.
fn runtime_over_watching_names_probing(
    ingredients: DocumentIngredients,
    probe: Option<crate::resource::StyleSheetProbe>,
) -> (
    ScriptRuntime,
    MainThreadRuntime,
    DocumentProbe,
    PublishedNames,
) {
    let (outbox, far_end) = detached_outbox_probing_style_sheets(Arc::new(NoWakeup), probe);
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
    let workers = WorkerFactory::new(workers, Arc::clone(&ends.trapped));
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
    /// The flag `bobcat-workers` sets once it has trapped, which the group's
    /// `WorkerFactory` reads at every construction. No thread reads this
    /// group's inbox, so a test sets it by hand.
    trapped: Arc<std::sync::atomic::AtomicBool>,
    worker_events: Vec<mpsc::UnboundedReceiver<WorkerEvent>>,
    /// The engine thread both realms were opened with, held for their life.
    thread: Option<Rc<JsThread>>,
}

/// A realm starts every worker the same way, from the URL its specifier joins
/// to, and asks its host for nothing as it does: a `Start` carries neither a
/// script nor a source, and a worker's realm asks for its own script once it
/// boots. The source the realm records under each key is the id of that key
/// and the worker's URL, at every URL: `bobcat:bts` is recorded as any other
/// is. Boot's own BTS and the worker the entry constructs over `bobcat:bts`
/// have that URL in common and are told apart by their ids.
#[test]
fn constructing_a_worker_asks_the_host_for_nothing_and_records_its_id_and_url() {
    let (mut js, mut first, _second, mut ends) = two_view_group();
    first
        .run_main_thread_script(
            &mut js,
            r"
            import { Worker } from 'bobcat-internal';
            globalThis.workers = [
                new Worker('bobcat:bts'), new Worker('./w.js'), new Worker('bobcat:timers'),
            ];
            ",
            "app:///workers.js",
        )
        .expect("the entry constructs the three workers");
    let workers = ends.workers.as_mut().expect("the group's worker inbox");
    // The entry's three, in the order it constructed them, then boot's own
    // BTS, which boot constructs once the entry has run.
    let mut start = || {
        let Ok(WorkerCommand::Start(start)) = workers.try_recv() else {
            panic!("each `new Worker` sent one Start")
        };
        start
    };
    let (scripted, fetched, engine, background) = (start(), start(), start(), start());
    for (start, url) in [
        (&scripted, "bobcat:bts"),
        (&fetched, "app:///w.js"),
        (&engine, "bobcat:timers"),
        (&background, "bobcat:bts"),
    ] {
        assert_eq!(start.url, url);
        assert_eq!(
            first.workers.source_of(start.key),
            Some(ScriptSource::Worker {
                id: WorkerId::from(start.key),
                url: url.into(),
            }),
            "the worker at {url}"
        );
    }
    assert_ne!(
        first.workers.source_of(scripted.key),
        first.workers.source_of(background.key),
        "two workers over one URL have different ids"
    );
    // Nothing reads this group's `Start`s but the test, so no worker has
    // booted: whatever the host was asked for, the constructing realm asked.
    while let Ok(notice) = ends.views[0].notices.try_recv() {
        assert!(
            !matches!(notice, ViewNotice::RequestSource { .. }),
            "constructing a worker asks the host for nothing, whatever its URL"
        );
    }
}

/// A worker constructed once `bobcat-workers` has trapped is sent nowhere: no
/// `Start` reaches the thread's inbox, and the view hears no `WorkerCreated`
/// for it. The realm queues the worker's `Failed` on its own channel instead,
/// under the key the script holds, and still names the worker by its source.
/// Boot constructs its BTS the same way once the entry has run, so two
/// workers fail here, the entry's first.
#[test]
fn a_worker_constructed_after_its_thread_trapped_is_sent_no_start() {
    let (mut js, mut first, _second, mut ends) = two_view_group();
    ends.trapped
        .store(true, std::sync::atomic::Ordering::Release);
    first
        .run_main_thread_script(
            &mut js,
            r"
            import { Worker } from 'bobcat-internal';
            globalThis.worker = new Worker('./w.js');
            ",
            "app:///workers.js",
        )
        .expect("the entry constructs its worker");
    let events = &mut ends.worker_events[0];
    let failed: Vec<_> = std::iter::from_fn(|| events.try_recv().ok())
        .map(|event| {
            assert!(
                matches!(event.payload, WorkerPayload::Failed(_)),
                "a worker constructed on a trapped thread has failed"
            );
            (event.key, first.workers.source_of(event.key))
        })
        .collect();
    let [(entry_key, entry_named), (boot_key, boot_named)] = failed.as_slice() else {
        panic!("the entry's worker failed at its construction, then boot's BTS: {failed:?}");
    };
    assert_eq!(
        *entry_named,
        Some(ScriptSource::Worker {
            id: WorkerId::from(*entry_key),
            url: "app:///w.js".into(),
        })
    );
    assert_eq!(
        *boot_named,
        Some(ScriptSource::Worker {
            id: WorkerId::from(*boot_key),
            url: "bobcat:bts".into(),
        })
    );
    let workers = ends.workers.as_mut().expect("the group's worker inbox");
    assert!(
        matches!(workers.try_recv(), Err(mpsc::error::TryRecvError::Empty)),
        "a thread that has trapped is sent no `Start`"
    );
    while let Ok(notice) = ends.views[0].notices.try_recv() {
        assert!(
            !matches!(notice, ViewNotice::WorkerCreated { .. }),
            "the view is not told of a worker that was sent nowhere"
        );
    }
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
                const scoped = __CreateView(0);
                if (typeof __SetCSSId !== 'function' ||
                    __SetCSSId([scoped], 3, 'entry') !== undefined ||
                    __GetAttributeByName(scoped, 'l-css-id') !== '3' ||
                    __GetAttributeByName(scoped, 'l-e-name') !== null) {
                  throw new Error('the scoped-style PAPI must write l-css-id and no entry name');
                }
                __SetCSSId([scoped], 0);
                if (__GetAttributeByName(scoped, 'l-css-id') !== null) {
                  throw new Error('css id 0 must remove l-css-id');
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

/// A card's MTS body is its entry with `MTS_CHUNK_PREAMBLE` on the body's
/// own first line, so an error in the body's first line is reported at line
/// 1 of the entry's URL.
#[test]
fn bundle_url_reaches_script_error_location() {
    let (mut js_runtime, mut runtime, _elements) = runtime();
    let error = runtime
        .run_main_thread_script(&mut js_runtime, "const = 1", "app:///broken.js")
        .expect_err("syntax error");

    let location = error.source.location.expect("the error has a location");
    assert_eq!(location.source.as_deref(), Some("app:///broken.js"));
    assert_eq!(location.line, Some(1), "the body's first line is line 1");
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
/// The boot module is written with the five switches as boolean literals and
/// hands them to `new Document(config)`, whose `createDocument` call passes
/// them back as five booleans, and the UA cascade is built from *those* — so
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
            *style.get_display(),
            if linear {
                Display::Linear
            } else {
                Display::Flex
            },
            "`defaultDisplayLinear` reached the cascade: linear={linear}"
        );
        assert_eq!(
            *style.get_overflow_x() == Overflow::Visible,
            !linear,
            "`defaultOverflowVisible` reached it too: linear={linear}"
        );
    }
}

/// `enableCSSInheritance` takes the same path as the two switches above: the
/// boot module's literal, `new Document(config)`, the fifth boolean of
/// `createDocument`, and the UA rule that boolean adds. With it on, a `text`
/// inside a `view` takes the view's color; with it off the text block's reset
/// keeps the initial black, as before the switch existed.
#[test]
fn the_css_inheritance_switch_written_into_the_boot_module_reaches_the_ua_cascade() {
    use dom::stylo::color::AbsoluteColor;

    for inheritance in [true, false] {
        let (mut js_runtime, mut runtime, elements, _names) =
            runtime_over_watching_names(DocumentIngredients::for_test(
                Viewport::new(393.0, 727.0),
                PageConfig {
                    enable_css_inheritance: inheritance,
                    ..PageConfig::default()
                },
            ));
        runtime
            .run_main_thread_script(
                &mut js_runtime,
                r"
                globalThis.renderPage = function () {
                  const page = __CreatePage('card', 0);
                  const view = __CreateView(0);
                  __SetInlineStyles(view, 'color: rgb(1, 2, 3)');
                  __AppendElement(page, view);
                  __AppendElement(view, __CreateText(0));
                };
                ",
                "app:///css-inheritance.js",
            )
            .expect("main-thread script");
        let tree = elements.tree();
        // The page is node 2, so the view and the text created after it are
        // 3 and 4; the tag is asserted so a different numbering fails here.
        let color = |id, tag| {
            let element = tree.get(node_id(id)).expect("a live element");
            assert_eq!(element.tag_name(), Some(tag));
            element
                .computed_style()
                .expect("a flushed element has style")
                .get_color()
                .solid_color()
        };
        let view = color(3, "view");
        let text = color(4, "text");
        assert_ne!(view, AbsoluteColor::BLACK, "the view declared a color");
        assert_eq!(
            text == view,
            inheritance,
            "`enableCSSInheritance` reached the cascade: inheritance={inheritance}"
        );
        if !inheritance {
            assert_eq!(text, AbsoluteColor::BLACK);
        }
    }
}

/// A page configuration switch that is not a boolean refuses the
/// construction, and the boot that asked for it fails.
///
/// Rust needs these five fields, so it reads each rather than forwarding
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
            createDocument(true, "yes", true, false, false);
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
/// six places per element, and the host is told only when a name's
/// document-wide registration count crosses zero — its first registration
/// anywhere added, and its last removed.
#[test]
fn the_published_names_follow_the_realm_registration_counts_crossing_zero() {
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
                  __AddEventListener(outer, 'longpress', globalThis.listener, {});
                  __AddEventListener(inner, 'longpress', globalThis.listener, { capture: true });
                  __AddEvent(inner, 'bindEvent', 'longpress', '3:0:bindlongpress');
                  __AddEvent(outer, 'global-bindEvent', 'swipe', '3:0:swipe');
                };
                ",
            "app:///registration-counts.js",
        )
        .expect("main-thread script");
    // Nothing of the components': `slotchange` (the swiper's and the refresh
    // view's) and `tap` (the markdown component's) are routed only while an
    // element of theirs exists, and this page has none.
    assert_eq!(names.names(), ["longpress", "swipe"]);

    runtime
        .evaluate_module(
            &mut js_runtime,
            r"
                import { __AddEvent, __RemoveEventListener } from 'bobcat:element';
                const [, outer, inner] = globalThis.held;
                // `inner` still has its `__AddEvent` handler, and `outer`
                // still has its closure, so neither of these closes `longpress`.
                __RemoveEventListener(inner, 'longpress', globalThis.listener, { capture: true });
                __AddEvent(outer, 'global-bindEvent', 'swipe', null);
                ",
            "app:///partial.mjs",
            "unregistering",
        )
        .expect("unregistration");
    assert_eq!(names.names(), ["longpress"]);

    runtime
        .evaluate_module(
            &mut js_runtime,
            r"
                import { __AddEvent, __RemoveEventListener } from 'bobcat:element';
                const [, outer, inner] = globalThis.held;
                __AddEvent(inner, 'bindEvent', 'longpress', null);
                __RemoveEventListener(outer, 'longpress', globalThis.listener, {});
                ",
            "app:///last.mjs",
            "unregistering",
        )
        .expect("unregistration");
    assert!(
        names.names().is_empty(),
        "removing the last registration unpublishes it"
    );

    // A name a component observes, with no element of it alive, is the
    // realm's like any other: removing its last registration unpublishes it.
    runtime
        .evaluate_module(
            &mut js_runtime,
            r"
                import { __AddEventListener, __RemoveEventListener } from 'bobcat:element';
                const [, outer] = globalThis.held;
                __AddEventListener(outer, 'slotchange', globalThis.listener, {});
                __RemoveEventListener(outer, 'slotchange', globalThis.listener, {});
                ",
            "app:///component-name.mjs",
            "registering a component's name",
        )
        .expect("registration");
    runtime.commit_if_dirty();
    assert!(names.names().is_empty());
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
                  __AddEventListener(view, 'longpress', globalThis.listener, {});
                };
                ",
            "app:///publish.js",
        )
        .expect("main-thread script");
    assert!(names.contains("longpress"));
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
                __RemoveEventListener(globalThis.held[1], 'longpress', globalThis.listener, {});
                ",
            "app:///unpublish.mjs",
            "unpublishing",
        )
        .expect("unregistration");
    assert!(
        !names.contains("longpress"),
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

/// A key event crosses as its two strings, `repeat` and a modifier bitmask,
/// and reaches the page as a `KeyboardEvent`-shaped event: the members on
/// the event itself, the `detail` empty.
#[test]
fn a_key_event_reaches_the_page_with_its_keyboard_members() {
    let (mut js_runtime, mut runtime, _elements) = runtime();
    runtime
        .run_main_thread_script(
            &mut js_runtime,
            r"
                globalThis.seen = [];
                globalThis.renderPage = function () {
                  const page = __CreatePage('card', 0);
                  globalThis.held = [page];
                  __AddEventListener(page, 'keydown', (event) => {
                    seen.push([event.type, event.key, event.code, event.repeat,
                               event.altKey, event.ctrlKey, event.metaKey,
                               event.shiftKey, JSON.stringify(event.detail)].join(','));
                  }, {});
                };
                ",
            "app:///keys.js",
        )
        .expect("main-thread script");

    let payload = InputEventPayload {
        detail: dom::event::EventDetail::Key {
            key: Arc::from("ArrowLeft"),
            code: Arc::from("ArrowLeft"),
            repeat: true,
            modifiers: dom::input::Modifiers {
                alt: true,
                ..dom::input::Modifiers::default()
            },
        },
        timestamp: 0.0,
    };
    runtime
        .dispatch_input_event(
            &mut js_runtime,
            dom::NodeId::DOCUMENT_ELEMENT,
            "keydown",
            &payload,
            None,
        )
        .expect("dispatch");
    runtime
        .evaluate_module(
            &mut js_runtime,
            r"
                if (seen.join('|') !== 'keydown,ArrowLeft,ArrowLeft,true,true,false,false,false,{}') {
                  throw new Error('unexpected deliveries: ' + seen.join('|'));
                }
                ",
            "app:///verify.js",
            "verifying",
        )
        .expect("verification");
}

/// A `click` is dispatched at the nearest common ancestor of its two targets,
/// and a second one at the same target within 500 ms adds a `dblclick` after
/// it; a third starts over.
#[test]
fn a_click_targets_the_common_ancestor_and_two_make_a_dblclick() {
    let (mut js_runtime, mut runtime, _elements) = runtime();
    runtime
        .run_main_thread_script(
            &mut js_runtime,
            r"
                globalThis.seen = [];
                globalThis.renderPage = function () {
                  const page = __CreatePage('card', 0);
                  const outer = __CreateView(0);
                  const a = __CreateView(0);
                  const b = __CreateView(0);
                  __AppendElement(page, outer);
                  __AppendElement(outer, a);
                  __AppendElement(outer, b);
                  globalThis.held = [page, outer, a, b];
                  const note = (event) => seen.push(event.type + '@' + event.target.uid +
                    ':' + event.button + ':' + event.detail.x);
                  __AddEventListener(page, 'click', note, {});
                  __AddEventListener(page, 'dblclick', note, {});
                };
                ",
            "app:///clicks.js",
        )
        .expect("main-thread script");

    // `page` is 2, `outer` 3, `a` 4, `b` 5.
    let mut click = |down: u64, release: u64, at: f64| {
        let payload = InputEventPayload {
            detail: dom::event::EventDetail::Pointer {
                position: dom::Point2D::new(7.0, 8.0),
                id: 1,
                device: dom::input::PointerKind::Mouse,
                button: 0,
                buttons: 0,
            },
            timestamp: at,
        };
        runtime
            .dispatch_input_event(
                &mut js_runtime,
                node_id(down),
                "click",
                &payload,
                Some(node_id(release)),
            )
            .expect("dispatch");
    };
    click(4, 5, 0.0);
    click(5, 4, 300.0);
    click(4, 5, 400.0);
    click(4, 5, 1000.0);
    click(4, 4, 2000.0);
    click(5, 5, 2100.0);
    runtime
        .evaluate_module(
            &mut js_runtime,
            r"
                const expected = [
                  'click@3:0:7', 'click@3:0:7', 'dblclick@3:0:7', 'click@3:0:7',
                  'click@3:0:7', 'click@4:0:7', 'click@5:0:7',
                ].join('|');
                if (seen.join('|') !== expected) {
                  throw new Error('unexpected deliveries: ' + seen.join('|'));
                }
                ",
            "app:///verify.js",
            "verifying",
        )
        .expect("verification");
}

/// `scroll` and `scrollend` are queued only for a name something hears: an
/// adopted offset on a page with neither listener queues nothing, and one
/// with a `scroll` listener queues the `scroll` alone, once per adopt.
#[test]
fn adopted_offsets_queue_scroll_events_only_for_a_name_something_hears() {
    let (mut js_runtime, mut runtime, elements) = runtime();
    runtime
        .run_main_thread_script(
            &mut js_runtime,
            r"
                globalThis.renderPage = function () {
                  const page = __CreatePage('card', 0);
                  const scroller = __CreateView(0);
                  __AppendElement(page, scroller);
                  const card = __CreateView(0);
                  __AppendElement(scroller, card);
                  globalThis.held = [page, scroller, card];
                  __SetInlineStyles(scroller, 'display:flex;flex-direction:column;overflow:scroll;width:200px;height:200px');
                  __SetInlineStyles(card, 'flex-shrink:0;width:200px;height:1000px');
                  globalThis.listen = () =>
                    __AddEventListener(scroller, 'scroll', () => {}, {});
                  __FlushElementTree();
                };
                ",
            "app:///scroll-gate.js",
        )
        .expect("main-thread script");
    let scroller = node_id(3);
    let entry = |y: f32| {
        (
            scroller,
            crate::link::ScrollEntry {
                offset: dom::Vector2D::new(0.0, y),
                at: 0.0,
                rest: Some(dom::Vector2D::new(0.0, y)),
                request: None,
            },
        )
    };

    runtime.adopt_scroll_offsets([entry(40.0)].into_iter());
    assert_eq!(
        elements.tree().scroll_offset(scroller),
        dom::Vector2D::new(0.0, 40.0)
    );
    assert!(!runtime.has_component_events(), "nothing hears either name");

    runtime
        .evaluate_module(
            &mut js_runtime,
            "globalThis.listen();",
            "app:///listen.js",
            "listening",
        )
        .expect("listening");
    runtime.adopt_scroll_offsets([entry(40.0)].into_iter());
    assert!(
        !runtime.has_component_events(),
        "an offset that did not move fires nothing"
    );
    runtime.adopt_scroll_offsets([entry(60.0)].into_iter());
    let queued = runtime.slot.borrow().component_events.take();
    assert_eq!(
        queued,
        [ComponentEvent::Scroll {
            node: scroller,
            name: "scroll",
            delta: dom::Vector2D::new(0.0, 20.0),
        }],
        "one `scroll`, with the change its post made, and no `scrollend` with nothing to \
         hear it"
    );
}

/// The painter's name set follows the realm's registration counts crossing
/// zero with two adjustments: a `dblclick` registration routes `click` there
/// too (the `dblclick` is decided from the `click`s the painter sends) and
/// removing it stops routing `click` only when `click` is not listened for
/// itself; and a name an element alive observes (the page's `x-swiper`'s
/// `slotchange`) stays routed whatever the realm's registrations for it say,
/// where an unobserved name stops being routed with its last registration.
/// `tap`, which no element here observes, is not there at all.
#[test]
fn the_painter_names_pair_click_with_dblclick_and_keep_observed_names() {
    let (mut js_runtime, mut runtime, _elements, mut names) =
        runtime_over_watching_names(ingredients());
    runtime
        .run_main_thread_script(
            &mut js_runtime,
            r"
                globalThis.renderPage = function () {
                  const page = __CreatePage('card', 0);
                  __AppendElement(page, __CreateElement('x-swiper', 0));
                  globalThis.held = [page];
                };
                globalThis.listener = () => {};
                ",
            "app:///names.js",
        )
        .expect("main-thread script");
    let mut step = |source: &str| {
        let source = format!(
            "import {{ __AddEventListener, __RemoveEventListener }} from 'bobcat:element';
             const [page] = globalThis.held;
             {source}"
        );
        runtime
            .evaluate_module(&mut js_runtime, &source, "app:///step.mjs", "a step")
            .expect("the step runs");
    };

    step("__AddEventListener(page, 'dblclick', globalThis.listener, {});");
    assert_eq!(names.names(), ["click", "dblclick", "slotchange"]);
    step("__AddEventListener(page, 'click', globalThis.listener, {});");
    step("__RemoveEventListener(page, 'dblclick', globalThis.listener, {});");
    assert_eq!(
        names.names(),
        ["click", "slotchange"],
        "`click` is listened for itself"
    );
    step("__AddEventListener(page, 'dblclick', globalThis.listener, {});");
    step("__RemoveEventListener(page, 'click', globalThis.listener, {});");
    assert_eq!(
        names.names(),
        ["click", "dblclick", "slotchange"],
        "`dblclick` still needs the `click`s"
    );
    step("__RemoveEventListener(page, 'dblclick', globalThis.listener, {});");
    assert_eq!(names.names(), ["slotchange"]);

    step(
        "__AddEventListener(page, 'slotchange', globalThis.listener, {});
         __AddEventListener(page, 'swipe', globalThis.listener, {});",
    );
    assert_eq!(names.names(), ["slotchange", "swipe"]);
    step(
        "__RemoveEventListener(page, 'slotchange', globalThis.listener, {});
         __RemoveEventListener(page, 'swipe', globalThis.listener, {});",
    );
    assert_eq!(
        names.names(),
        ["slotchange"],
        "the last removal of an observed name keeps it routed; of another, it does not"
    );
}

/// The extras channel end to end: a drag's `scroll` reaches the realm as a
/// W3C event whose Lynx `detail` is the scroll metrics — the offset and the
/// scrolling area read at delivery, the delta its post moved the container
/// by — and its `scrollend` with a zero delta; an `<image>` `load` still
/// reports `{width, height}`, now as extras.
#[test]
fn scroll_metrics_and_a_load_size_reach_the_lynx_detail_as_extras() {
    let (mut js_runtime, mut runtime, _elements) = runtime();
    runtime
        .run_main_thread_script(
            &mut js_runtime,
            r"
                globalThis.seen = [];
                globalThis.renderPage = function () {
                  const page = __CreatePage('card', 0);
                  const scroller = __CreateView(0);
                  __AppendElement(page, scroller);
                  const card = __CreateView(0);
                  __AppendElement(scroller, card);
                  globalThis.held = [page, scroller, card];
                  __SetInlineStyles(scroller, 'display:flex;flex-direction:column;overflow:scroll;width:200px;height:200px');
                  __SetInlineStyles(card, 'flex-shrink:0;width:200px;height:1000px');
                  const note = (event) =>
                    seen.push(event.type + ' ' + JSON.stringify(event.detail));
                  __AddEventListener(scroller, 'scroll', note, {});
                  __AddEventListener(scroller, 'scrollend', note, {});
                  __AddEventListener(card, 'load', note, {});
                  __FlushElementTree();
                };
                ",
            "app:///extras.js",
        )
        .expect("main-thread script");
    let (scroller, card) = (node_id(3), node_id(4));
    let entry = |y: f32, rest: bool| {
        (
            scroller,
            crate::link::ScrollEntry {
                offset: dom::Vector2D::new(0.0, y),
                at: 0.0,
                rest: rest.then_some(dom::Vector2D::new(0.0, y)),
                request: None,
            },
        )
    };
    // Delivered between the two posts, as the page's posted entry is: the
    // offset is read at delivery.
    runtime.adopt_scroll_offsets([entry(40.0, false)].into_iter());
    assert!(
        runtime
            .dispatch_component_events(&mut js_runtime)
            .is_empty()
    );
    runtime.adopt_scroll_offsets([entry(65.0, true)].into_iter());
    runtime
        .slot
        .borrow()
        .component_events
        .queue_image(Some(dom::ImageOutcome::Loaded {
            node: card,
            width: 40,
            height: 20,
        }));
    let failures = runtime.dispatch_component_events(&mut js_runtime);
    assert!(failures.is_empty(), "no listener threw: {failures:?}");
    runtime
        .evaluate_module(
            &mut js_runtime,
            r#"
                const metrics = (top, delta) => JSON.stringify({
                  scrollTop: top, scrollLeft: 0, scrollHeight: 1000, scrollWidth: 200,
                  deltaX: 0, deltaY: delta, isDragging: false,
                });
                const expected = [
                  'scroll ' + metrics(40, 40),
                  'scroll ' + metrics(65, 25),
                  'scrollend ' + metrics(65, 0),
                  'load {"width":40,"height":20}',
                ].join('|');
                if (seen.join('|') !== expected) {
                  throw new Error('unexpected deliveries: ' + seen.join('|'));
                }
                "#,
            "app:///verify.js",
            "verifying",
        )
        .expect("verification");
}

/// An engine component on the path that observes the event's name.
/// Records its delivery, marks the target so script can tell it ran first,
/// and stops propagation — which must end only its own walk.
struct StoppingComponent {
    log: Rc<RefCell<Vec<String>>>,
}

impl dom::CustomElement<()> for StoppingComponent {
    fn observed_events(&self) -> &'static [&'static str] {
        &["tap"]
    }

    fn handle_event(
        &self,
        document: &mut LynxDocument,
        _element: dom::NodeId,
        event: &mut dom::event::ElementEvent<'_>,
    ) {
        let x = event.detail().position().map_or(-1.0, |point| point.x);
        self.log
            .borrow_mut()
            .push(format!("{}:{:?}:{x}", event.type_name(), event.phase()));
        document.set_attribute(event.target(), "data-rust", "seen");
        event.stop_propagation();
    }
}

/// The one walk calls the page realm at the path's top step, so the
/// realm's whole Lynx walk, bubble listeners included, runs before
/// the component on the path hears the capture step (web-core's order,
/// `docs/tracking/deviations.md`); the component's `stop_propagation` then
/// ends the walk, which has nothing after it here.
#[test]
fn the_page_realm_runs_at_the_paths_top_before_the_components_on_the_path() {
    let (mut js_runtime, mut runtime, elements) = runtime();
    runtime
        .run_main_thread_script(
            &mut js_runtime,
            r"
                globalThis.seen = [];
                globalThis.renderPage = function () {
                  const page = __CreatePage('card', 0);
                  // Built later, once the test has defined the tag: a
                  // definition never reaches an element created before it.
                  globalThis.build = function () {
                    const outer = __CreateElement('x-probe', 0);
                    const inner = __CreateView(0);
                    __AppendElement(page, outer);
                    __AppendElement(outer, inner);
                    globalThis.held = [page, outer, inner];
                    const note = (label) => () =>
                      seen.push(label + ':' + __GetAttributeByName(inner, 'data-rust'));
                    __AddEventListener(inner, 'tap', note('inner'), {});
                    __AddEventListener(outer, 'tap', note('outer'), {});
                  };
                };
                ",
            "app:///rust-walk.js",
        )
        .expect("main-thread script");
    let log = Rc::new(RefCell::new(Vec::new()));
    elements.tree().define(
        "x-probe",
        Box::new(StoppingComponent {
            log: Rc::clone(&log),
        }),
    );
    assert!(elements.tree().observes_event("tap"));
    runtime
        .evaluate_module(
            &mut js_runtime,
            "globalThis.build();",
            "app:///rust-walk-tree.js",
            "building",
        )
        .expect("the tree");

    // `page` is 2, the probe 3, the view 4.
    runtime
        .dispatch_for_test(&mut js_runtime, node_id(4), &tap(), event_point())
        .expect("dispatch");
    assert_eq!(
        *log.borrow(),
        ["tap:Capturing:12"],
        "the component hears the capture step with the dispatch's detail, and its stop ends \
         its own walk before the bubble step"
    );
    runtime
        .evaluate_module(
            &mut js_runtime,
            r"
                // Both script listeners ran, before the component marked the
                // target.
                if (seen.join('|') !== 'inner:null|outer:null') {
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

/// A name no path registration holds is still a name the painting side
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
        "removing the last global registration unpublishes its name like any other"
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

/// A dataset entry is on the node as its `data-<key>` attribute, so the
/// document's selector engine matches `[data-<key>]` and `[data-<key>="…"]`
/// for an entry either member wrote, as web-core's DOM does
/// (`web-core/ts/client/mainthread/elementAPIs/createElementAPI.ts:434-446`,
/// `web-core/src/main_thread/client/element_apis/dataset_apis.rs:12-55`).
/// The typed values `__GetDataset` answers are not changed by it.
#[test]
fn a_dataset_entry_is_matched_by_a_data_attribute_selector() {
    let (mut js_runtime, mut runtime, _elements) = runtime();
    runtime
        .run_main_thread_script(
            &mut js_runtime,
            r"
                globalThis.renderPage = function () {
                  const page = __CreatePage('card', 0);
                  const added = __CreateView(0);
                  const set = __CreateView(0);
                  const plain = __CreateView(0);
                  __AddDataset(added, 'test', 'x');
                  __SetDataset(set, {test: 'y', count: 7, item: {id: 3}});
                  __AppendElement(page, added);
                  __AppendElement(page, set);
                  __AppendElement(page, plain);
                  globalThis.held = [page, added, set, plain];
                };
                ",
            "app:///dataset-selector.js",
        )
        .expect("main-thread script");

    runtime
        .evaluate_module(
            &mut js_runtime,
            r#"
                import {
                  __AddDataset,
                  __GetDataset,
                  __QuerySelector,
                  __QuerySelectorAll,
                  __SetDataset,
                } from 'bobcat:element';
                const [page, added, set, plain] = held;
                const names = new Map([[added, 'added'], [set, 'set'], [plain, 'plain']]);
                const expectAll = (selector, ...expected) => {
                  const found = __QuerySelectorAll(page, selector, {});
                  const got = found.map(element => names.get(element)).join(',');
                  if (got !== expected.map(element => names.get(element)).join(',')) {
                    throw new Error(selector + ' matched [' + got + ']');
                  }
                };
                expectAll('[data-test]', added, set);
                expectAll('[data-test="x"]', added);
                expectAll('[data-test="y"]', set);
                if (__QuerySelector(page, '[data-test="y"]', {}) !== set) {
                  throw new Error('the first match of [data-test="y"]');
                }
                // A non-string value is matched by its string, an object by
                // its JSON text.
                expectAll('[data-count="7"]', set);
                expectAll('[data-item=\'{"id":3}\']', set);

                // An update is what the selector sees next.
                __AddDataset(added, 'test', 'z');
                __SetDataset(set, {test: 'x'});
                expectAll('[data-test="z"]', added);
                expectAll('[data-test="x"]', set);
                // `__SetDataset` merges: the keys it did not name are still
                // in the dataset, and so still on the node.
                expectAll('[data-count="7"]', set);

                // A null or undefined value has no attribute; the key stays
                // in the typed dataset.
                __AddDataset(added, 'test', undefined);
                __SetDataset(set, {test: null});
                expectAll('[data-test]');
                if (!('test' in __GetDataset(added)) || __GetDataset(set).test !== null) {
                  throw new Error('a removed attribute dropped its dataset key');
                }
                __AddDataset(added, 'test', 'again');
                expectAll('[data-test]', added);

                // An uppercase key is matched under either spelling, as an
                // HTML element's attribute is.
                __AddDataset(plain, 'testId', 'row');
                expectAll('[data-testId="row"]', plain);
                expectAll('[data-testid="row"]', plain);

                const typed = __GetDataset(set);
                if (typed.count !== 7 || typed.item.id !== 3 || typed.test !== null) {
                  throw new Error('typed dataset: ' + JSON.stringify(typed));
                }
                if (__GetDataset(plain).testId !== 'row' || 'testid' in __GetDataset(plain)) {
                  throw new Error('typed dataset key: ' + JSON.stringify(__GetDataset(plain)));
                }
                "#,
            "app:///verify.js",
            "verifying",
        )
        .expect("verification");
}

/// A stylesheet's attribute selector over a dataset entry applies, and a
/// dataset write restyles the element at the next flush: the attribute write
/// behind it records the invalidation any attribute write does.
#[test]
fn a_dataset_write_restyles_an_attribute_selector_rule() {
    use crate::resource::{LoadedSource, StyleSheetSource};
    let (mut js_runtime, mut runtime, elements) = runtime();
    runtime
        .run_main_thread_script_over_sheets(
            &mut js_runtime,
            vec![(
                "app:///index.css",
                LoadedSource::StyleSheet(StyleSheetSource::Text(
                    ".box{width:20px;height:10px} \
                     .box[data-state=\"on\"]{width:90px} \
                     .box[data-wide]{height:40px}"
                        .into(),
                )),
            )],
            r"
                const page = __CreatePage('card', 0);
                const first = __CreateView(0);
                const second = __CreateView(0);
                __SetClasses(first, 'box');
                __SetClasses(second, 'box');
                __AddDataset(first, 'state', 'on');
                __AppendElement(page, first);
                __AppendElement(page, second);
                globalThis.held = [first, second];
            ",
            "app:///main.js",
        )
        .expect("main-thread script");
    let sizes = || {
        let tree = elements.tree();
        [3, 4].map(|id| {
            let size = tree.rounded_layout(node_id(id)).unwrap().size;
            (size.width, size.height)
        })
    };
    assert_eq!(sizes(), [(90.0, 10.0), (20.0, 10.0)]);

    let mut step = |script: &str, name: &str| {
        let source = format!(
            "import {{__AddDataset, __SetDataset, __FlushElementTree}} from 'bobcat:element';
             const [first, second] = held;
             {script}
             __FlushElementTree();"
        );
        runtime
            .evaluate_module(&mut js_runtime, &source, name, "writing a dataset")
            .expect("the dataset write");
    };
    step(
        "__AddDataset(first, 'state', 'off'); __SetDataset(second, {state: 'on', wide: true});",
        "app:///swap.js",
    );
    assert_eq!(sizes(), [(20.0, 10.0), (90.0, 40.0)]);
    // `wide` is not named by this write and stays; `state` loses its
    // attribute with its value.
    step("__SetDataset(second, {state: null});", "app:///clear.js");
    assert_eq!(sizes(), [(20.0, 10.0), (20.0, 40.0)]);
    step("__AddDataset(second, 'wide', false);", "app:///narrow.js");
    assert_eq!(sizes(), [(20.0, 10.0), (20.0, 10.0)]);
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
                const unsupported = measure(view, 'requestUIInfo');
                if (unsupported.code !== 3 || unsupported.data !== undefined) {
                  throw new Error(JSON.stringify(unsupported));
                }
                ",
            "app:///measure.js",
            "measuring",
        )
        .expect("measuring");
}

/// `selectTab` on a pager, through both paths a card reaches a UI method by:
/// `__InvokeUIMethod`, and the selector-query `invoke` the background
/// thread's `lynx.createSelectorQuery()` sends (`__BobcatQueryNodes`). The
/// pager's element child number `index` is scrolled into view at its start
/// edge, which for four 200px pages is `index` times 200; an instant turn
/// moves the document at once, a smooth one — the default — only records
/// the request for the painter. Bad params, and an index naming no page,
/// are code 4 and move nothing; `selectTab` anywhere but a pager, and any
/// other method on a pager, is code 3; `boundingClientRect` is unchanged.
#[test]
#[expect(clippy::too_many_lines, reason = "one scenario over both invoke paths")]
fn select_tab_turns_a_viewpager_through_both_invoke_paths() {
    let (mut js_runtime, mut runtime, elements) = runtime();
    runtime
        .run_main_thread_script(
            &mut js_runtime,
            r"
                globalThis.renderPage = function () {
                  const page = __CreatePage('card', 0);
                  const pager = __CreateElement('x-viewpager-ng', 0);
                  __SetID(pager, 'pager');
                  __SetInlineStyles(pager, 'width:200px;height:100px');
                  __AppendElement(page, pager);
                  for (let i = 0; i < 4; i++) {
                    __AppendElement(pager, __CreateElement('x-viewpager-item-ng', 0));
                  }
                  const view = __CreateView(0);
                  __SetID(view, 'plain');
                  __AppendElement(page, view);
                  globalThis.held = [page, pager, view];
                };
                ",
            "app:///pager.js",
        )
        .expect("main-thread script");
    runtime
        .evaluate_module(
            &mut js_runtime,
            r"
                import {
                  __BobcatQueryNodes,
                  __FlushElementTree,
                  __InvokeUIMethod,
                } from 'bobcat:element';
                __FlushElementTree();
                globalThis.viaPapi = (element, method, params) => {
                  let answer;
                  let calls = 0;
                  __InvokeUIMethod(element, method, params, result => {
                    answer = result;
                    calls += 1;
                  });
                  if (calls !== 1) throw new Error('the callback ran ' + calls + ' times');
                  return answer;
                };
                globalThis.viaQuery = (selector, method, params) => __BobcatQueryNodes({
                  bobcat: 'runtime', method: 'nodeQuery', operation: 'invoke',
                  token: {type: 0, identifier: selector, component_id: '',
                          first_only: true, root_unique_id: undefined},
                  params: {method, params},
                });
                globalThis.expectCode = (answer, code) => {
                  if (answer.code !== code || answer.data !== undefined) {
                    throw new Error('expected ' + code + ', got ' + JSON.stringify(answer));
                  }
                };
                ",
            "app:///helpers.js",
            "helpers",
        )
        .expect("helpers");
    let pager = {
        let tree = elements.tree();
        let page = tree.document_element().id();
        tree.get(page).expect("the page").child_ids()[0]
    };
    let offset = || elements.tree().scroll_offset(pager);
    let request = || {
        let mut tree = elements.tree();
        let frame = tree.commit();
        let slot = frame
            .slot_of(pager)
            .expect("the pager is a scroll container");
        frame.scroll_slots()[slot as usize]
            .request
            .map(|request| (request.target, request.behavior))
    };
    // One module per step, each under a URL of its own.
    let steps = std::cell::Cell::new(0);
    let run = |runtime: &mut MainThreadRuntime, js_runtime: &mut ScriptRuntime, source: &str| {
        steps.set(steps.get() + 1);
        let name = format!("app:///step-{}.js", steps.get());
        runtime
            .evaluate_module(js_runtime, source, &name, "step")
            .expect("step");
    };

    run(
        &mut runtime,
        &mut js_runtime,
        "expectCode(viaPapi(held[1], 'selectTab', {index: 2, smooth: false}), 0);",
    );
    assert_eq!(offset(), dom::Vector2D::new(400.0, 0.0), "instant: at once");
    assert_eq!(
        request(),
        Some((
            dom::Vector2D::new(400.0, 0.0),
            dom::scroll::ScrollBehavior::Instant
        ))
    );

    run(
        &mut runtime,
        &mut js_runtime,
        "expectCode(viaQuery('#pager', 'selectTab', {index: 1, smooth: true}), 0);",
    );
    assert_eq!(
        offset(),
        dom::Vector2D::new(400.0, 0.0),
        "smooth: the painter's"
    );
    assert_eq!(
        request(),
        Some((
            dom::Vector2D::new(200.0, 0.0),
            dom::scroll::ScrollBehavior::Smooth
        ))
    );

    // Smooth is the default, on either path.
    for source in [
        "expectCode(viaPapi(held[1], 'selectTab', {index: 3}), 0);",
        "expectCode(viaQuery('#pager', 'selectTab', {index: 3}), 0);",
    ] {
        run(&mut runtime, &mut js_runtime, source);
        assert_eq!(offset(), dom::Vector2D::new(400.0, 0.0), "{source}");
        assert_eq!(
            request(),
            Some((
                dom::Vector2D::new(600.0, 0.0),
                dom::scroll::ScrollBehavior::Smooth
            )),
            "{source}"
        );
    }

    // No numeric index: refused, and nothing recorded or moved.
    let pending = elements.tree().pending_scroll_request(pager);
    run(
        &mut runtime,
        &mut js_runtime,
        r"
            for (const params of [{}, {index: NaN}, {index: Infinity}, {index: '1'},
                                  {smooth: false}, undefined, null]) {
              expectCode(viaPapi(held[1], 'selectTab', params), 4);
              expectCode(viaQuery('#pager', 'selectTab', params), 4);
            }
            ",
    );
    assert_eq!(offset(), dom::Vector2D::new(400.0, 0.0));
    assert_eq!(elements.tree().pending_scroll_request(pager), pending);

    // An index naming no page, past the end or below zero, is refused on
    // either path, and nothing is recorded or moved.
    run(
        &mut runtime,
        &mut js_runtime,
        r"
            for (const index of [4, 9, -1]) {
              expectCode(viaPapi(held[1], 'selectTab', {index, smooth: false}), 4);
              expectCode(viaQuery('#pager', 'selectTab', {index, smooth: false}), 4);
            }
            ",
    );
    assert_eq!(offset(), dom::Vector2D::new(400.0, 0.0));
    assert_eq!(elements.tree().pending_scroll_request(pager), pending);

    run(
        &mut runtime,
        &mut js_runtime,
        r"
            // `selectTab` belongs to the pager tags alone.
            expectCode(viaPapi(held[2], 'selectTab', {index: 1}), 3);
            expectCode(viaQuery('#plain', 'selectTab', {index: 1}), 3);
            // And the pager has no other method than the ones every element
            // has. `scrollTo` is the base set's CSSOM-View method, which reads
            // no `index`: it answers 0 and keeps the offset.
            expectCode(viaPapi(held[1], 'scrollTo', {index: 1}), 0);
            expectCode(viaQuery('#pager', 'selectPage', {index: 1}), 3);
            // `boundingClientRect` still answers its rect, params or none.
            for (const answer of [viaPapi(held[1], 'boundingClientRect', {index: 1}),
                                  viaQuery('#pager', 'boundingClientRect', undefined)]) {
              const {code, data} = answer;
              if (code !== 0 || data.id !== 'pager' || data.left !== 0 || data.top !== 0 ||
                  data.width !== 200 || data.height !== 100 || data.right !== 200 ||
                  data.bottom !== 100) {
                throw new Error(JSON.stringify(answer));
              }
            }
            ",
    );
    assert_eq!(offset(), dom::Vector2D::new(400.0, 0.0));
}

/// An `x-swiper` given `autoplay` through `__SetAttribute` keeps a timer the
/// page waits for with its realm's own: nothing is armed before the
/// attribute, the deadline is one `interval` away after it, and the turn
/// that comes due asks the painter for the next item as a request that
/// yields to a pointer, then waits for the one after. No script runs for it.
#[test]
fn an_autoplaying_swiper_arms_the_pages_deadline_and_turns_when_due() {
    let (mut js_runtime, mut runtime, elements) = runtime();
    runtime
        .run_main_thread_script(
            &mut js_runtime,
            r"
                globalThis.renderPage = function () {
                  const page = __CreatePage('card', 0);
                  const swiper = __CreateElement('x-swiper', 0);
                  __SetInlineStyles(swiper, 'width:200px;height:100px');
                  __AppendElement(page, swiper);
                  for (let i = 0; i < 3; i++) {
                    __AppendElement(swiper, __CreateElement('x-swiper-item', 0));
                  }
                  __FlushElementTree();
                  globalThis.swiper = swiper;
                };
                ",
            "app:///autoplay.js",
        )
        .expect("main-thread script");
    assert_eq!(runtime.next_deadline(), None, "no autoplay, nothing armed");

    runtime
        .evaluate_module(
            &mut js_runtime,
            r"
                import { __SetAttribute } from 'bobcat:element';
                __SetAttribute(swiper, 'interval', '20');
                __SetAttribute(swiper, 'autoplay', true);
                ",
            "app:///start.js",
            "starting autoplay",
        )
        .expect("starting autoplay");
    let armed = runtime
        .next_deadline()
        .expect("the swiper's timer is armed");
    let wait = armed.duration_since(ClockInstant::now());
    assert!(
        wait <= std::time::Duration::from_millis(20),
        "one interval away"
    );

    let content = {
        let tree = elements.tree();
        let page = tree.document_element().id();
        let swiper = tree.get(page).expect("the page").child_ids()[0];
        crate::main::tree::swiper::content(&tree, swiper).expect("the swiper's `#content`")
    };
    assert!(
        runtime.run_due_timers(&mut js_runtime).is_empty(),
        "a turn throws nothing"
    );
    assert_eq!(
        elements.tree().pending_scroll_request(content),
        None,
        "not due yet"
    );

    std::thread::sleep(wait + std::time::Duration::from_millis(5));
    assert!(runtime.run_due_timers(&mut js_runtime).is_empty());
    let frame = elements.tree().commit();
    let slot = frame.slot_of(content).expect("the scroller has a slot");
    let request = frame.scroll_slots()[slot as usize]
        .request
        .expect("the turn is a scroll request");
    assert_eq!(
        request.target,
        dom::Vector2D::new(200.0, 0.0),
        "the next item"
    );
    assert_eq!(request.behavior, dom::scroll::ScrollBehavior::Smooth);
    assert!(request.yields_to_pointer);
    assert!(
        runtime.next_deadline().expect("armed again") > armed,
        "the following turn"
    );
}

/// `scrollTo({index, smooth})` on a swiper, through both paths a card reaches
/// a UI method by: `__InvokeUIMethod`, and the selector-query `invoke` the
/// background thread's `lynx.createSelectorQuery()` sends
/// (`__BobcatQueryNodes`). The swiper's element child number `index` is
/// scrolled into view in its shadow `#content` at the item's snap alignment,
/// which for four 200px `normal` items is `index` times 200; an instant turn
/// moves the document at once, a smooth one — the default — only records the
/// request for the painter. Bad params, and an index naming no item, are
/// code 4 and move nothing; `scrollTo` on a plain view is the base set's
/// CSSOM-View method, which finds no scroll container there: code 0, and
/// nothing moves.
#[test]
#[expect(clippy::too_many_lines, reason = "one scenario over both invoke paths")]
fn scroll_to_turns_a_swiper_through_both_invoke_paths() {
    let (mut js_runtime, mut runtime, elements) = runtime();
    runtime
        .run_main_thread_script(
            &mut js_runtime,
            r"
                globalThis.renderPage = function () {
                  const page = __CreatePage('card', 0);
                  const swiper = __CreateElement('x-swiper', 0);
                  __SetID(swiper, 'swiper');
                  __SetInlineStyles(swiper, 'width:200px;height:100px');
                  __AppendElement(page, swiper);
                  for (let i = 0; i < 4; i++) {
                    __AppendElement(swiper, __CreateElement('x-swiper-item', 0));
                  }
                  const view = __CreateView(0);
                  __SetID(view, 'plain');
                  __AppendElement(page, view);
                  globalThis.held = [page, swiper, view];
                };
                ",
            "app:///swiper.js",
        )
        .expect("main-thread script");
    runtime
        .evaluate_module(
            &mut js_runtime,
            r"
                import {
                  __BobcatQueryNodes,
                  __FlushElementTree,
                  __InvokeUIMethod,
                } from 'bobcat:element';
                __FlushElementTree();
                globalThis.viaPapi = (element, method, params) => {
                  let answer;
                  let calls = 0;
                  __InvokeUIMethod(element, method, params, result => {
                    answer = result;
                    calls += 1;
                  });
                  if (calls !== 1) throw new Error('the callback ran ' + calls + ' times');
                  return answer;
                };
                globalThis.viaQuery = (selector, method, params) => __BobcatQueryNodes({
                  bobcat: 'runtime', method: 'nodeQuery', operation: 'invoke',
                  token: {type: 0, identifier: selector, component_id: '',
                          first_only: true, root_unique_id: undefined},
                  params: {method, params},
                });
                globalThis.expectCode = (answer, code) => {
                  if (answer.code !== code || answer.data !== undefined) {
                    throw new Error('expected ' + code + ', got ' + JSON.stringify(answer));
                  }
                };
                ",
            "app:///helpers.js",
            "helpers",
        )
        .expect("helpers");
    let scroller = {
        let tree = elements.tree();
        let page = tree.document_element().id();
        let swiper = tree.get(page).expect("the page").child_ids()[0];
        crate::main::tree::swiper::content(&tree, swiper).expect("the swiper's `#content`")
    };
    let offset = || elements.tree().scroll_offset(scroller);
    let request = || {
        let mut tree = elements.tree();
        let frame = tree.commit();
        let slot = frame
            .slot_of(scroller)
            .expect("`#content` is a scroll container");
        frame.scroll_slots()[slot as usize]
            .request
            .map(|request| (request.target, request.behavior))
    };
    // One module per step, each under a URL of its own.
    let steps = std::cell::Cell::new(0);
    let run = |runtime: &mut MainThreadRuntime, js_runtime: &mut ScriptRuntime, source: &str| {
        steps.set(steps.get() + 1);
        let name = format!("app:///step-{}.js", steps.get());
        runtime
            .evaluate_module(js_runtime, source, &name, "step")
            .expect("step");
    };

    run(
        &mut runtime,
        &mut js_runtime,
        "expectCode(viaPapi(held[1], 'scrollTo', {index: 2, smooth: false}), 0);",
    );
    assert_eq!(offset(), dom::Vector2D::new(400.0, 0.0), "instant: at once");
    assert_eq!(
        request(),
        Some((
            dom::Vector2D::new(400.0, 0.0),
            dom::scroll::ScrollBehavior::Instant
        ))
    );

    run(
        &mut runtime,
        &mut js_runtime,
        "expectCode(viaQuery('#swiper', 'scrollTo', {index: 1, smooth: true}), 0);",
    );
    assert_eq!(
        offset(),
        dom::Vector2D::new(400.0, 0.0),
        "smooth: the painter's"
    );
    assert_eq!(
        request(),
        Some((
            dom::Vector2D::new(200.0, 0.0),
            dom::scroll::ScrollBehavior::Smooth
        ))
    );

    // Smooth is the default, on either path.
    for source in [
        "expectCode(viaPapi(held[1], 'scrollTo', {index: 3}), 0);",
        "expectCode(viaQuery('#swiper', 'scrollTo', {index: 3}), 0);",
    ] {
        run(&mut runtime, &mut js_runtime, source);
        assert_eq!(offset(), dom::Vector2D::new(400.0, 0.0), "{source}");
        assert_eq!(
            request(),
            Some((
                dom::Vector2D::new(600.0, 0.0),
                dom::scroll::ScrollBehavior::Smooth
            )),
            "{source}"
        );
    }

    // No numeric index: refused, and nothing recorded or moved.
    let pending = elements.tree().pending_scroll_request(scroller);
    run(
        &mut runtime,
        &mut js_runtime,
        r"
            for (const params of [{}, {index: NaN}, {index: Infinity}, {index: '1'},
                                  {smooth: false}, undefined, null]) {
              expectCode(viaPapi(held[1], 'scrollTo', params), 4);
              expectCode(viaQuery('#swiper', 'scrollTo', params), 4);
            }
            ",
    );
    assert_eq!(offset(), dom::Vector2D::new(400.0, 0.0));
    assert_eq!(elements.tree().pending_scroll_request(scroller), pending);

    // An index naming no item, past the end or below zero, is refused on
    // either path, and nothing is recorded or moved.
    run(
        &mut runtime,
        &mut js_runtime,
        r"
            for (const index of [4, 9, -1]) {
              expectCode(viaPapi(held[1], 'scrollTo', {index, smooth: false}), 4);
              expectCode(viaQuery('#swiper', 'scrollTo', {index, smooth: false}), 4);
            }
            ",
    );
    assert_eq!(offset(), dom::Vector2D::new(400.0, 0.0));
    assert_eq!(elements.tree().pending_scroll_request(scroller), pending);

    run(
        &mut runtime,
        &mut js_runtime,
        r"
            // On a plain view `scrollTo` is the base set's CSSOM-View method,
            // which does nothing on a box that is no scroll container and
            // answers 0, as web-core's `HTMLElement.scrollTo` does.
            expectCode(viaPapi(held[2], 'scrollTo', {index: 1}), 0);
            expectCode(viaQuery('#plain', 'scrollTo', {index: 1}), 0);
            // And the swiper has no other method than the ones every element has.
            expectCode(viaPapi(held[1], 'selectTab', {index: 1}), 3);
            expectCode(viaQuery('#swiper', 'scrollToNext', undefined), 3);
            // `boundingClientRect` still answers the host's rect.
            for (const answer of [viaPapi(held[1], 'boundingClientRect', {index: 1}),
                                  viaQuery('#swiper', 'boundingClientRect', undefined)]) {
              const {code, data} = answer;
              if (code !== 0 || data.id !== 'swiper' || data.left !== 0 || data.top !== 0 ||
                  data.width !== 200 || data.height !== 100) {
                throw new Error(JSON.stringify(answer));
              }
            }
            ",
    );
    assert_eq!(offset(), dom::Vector2D::new(400.0, 0.0));
}

/// `autoStartRefresh`, `finishRefresh` and `finishLoadMore` on an
/// `x-refresh-view`, through `__InvokeUIMethod` and the selector-query
/// `invoke`. The view is 400px tall with a 50px header, a content `view` of
/// its full height and a 50px footer, so the 120px top placeholder puts the
/// header's start at 120 and `#content` at rest at 170. Each method answers
/// code 0 and records a request for the shadow `#container`:
/// `autoStartRefresh` an instant one to the header's start, which moves the
/// document to 120 at once, and the other two a smooth one back to
/// `#content`, which the painter carries out while the document stays where
/// it is. `selectTab` is no method of the view: code 3.
#[test]
#[expect(clippy::too_many_lines, reason = "one scenario over both invoke paths")]
fn a_refresh_view_answers_its_three_methods_through_both_invoke_paths() {
    let (mut js_runtime, mut runtime, elements) = runtime();
    runtime
        .run_main_thread_script(
            &mut js_runtime,
            r"
                globalThis.renderPage = function () {
                  const page = __CreatePage('card', 0);
                  const view = __CreateElement('x-refresh-view', 0);
                  __SetID(view, 'refresh');
                  __SetInlineStyles(view, 'width:200px;height:400px');
                  __AppendElement(page, view);
                  const header = __CreateElement('x-refresh-header', 0);
                  __SetInlineStyles(header, 'height:50px');
                  const content = __CreateView(0);
                  __SetInlineStyles(content, 'height:100%');
                  const footer = __CreateElement('x-refresh-footer', 0);
                  __SetInlineStyles(footer, 'height:50px');
                  __AppendElement(view, header);
                  __AppendElement(view, content);
                  __AppendElement(view, footer);
                  globalThis.held = [page, view];
                };
                ",
            "app:///refresh-view.js",
        )
        .expect("main-thread script");
    runtime
        .evaluate_module(
            &mut js_runtime,
            r"
                import {
                  __BobcatQueryNodes,
                  __FlushElementTree,
                  __InvokeUIMethod,
                } from 'bobcat:element';
                __FlushElementTree();
                globalThis.viaPapi = (method, params) => {
                  let answer;
                  let calls = 0;
                  __InvokeUIMethod(held[1], method, params, result => {
                    answer = result;
                    calls += 1;
                  });
                  if (calls !== 1) throw new Error('the callback ran ' + calls + ' times');
                  return answer;
                };
                globalThis.viaQuery = (method, params) => __BobcatQueryNodes({
                  bobcat: 'runtime', method: 'nodeQuery', operation: 'invoke',
                  token: {type: 0, identifier: '#refresh', component_id: '',
                          first_only: true, root_unique_id: undefined},
                  params: {method, params},
                });
                globalThis.expectCode = (answer, code) => {
                  if (answer.code !== code || answer.data !== undefined) {
                    throw new Error('expected ' + code + ', got ' + JSON.stringify(answer));
                  }
                };
                ",
            "app:///helpers.js",
            "helpers",
        )
        .expect("helpers");
    let container = {
        let tree = elements.tree();
        let page = tree.document_element().id();
        let view = tree.get(page).expect("the page").child_ids()[0];
        crate::main::tree::refresh_view::container(&tree, view)
            .expect("the refresh view's `#container`")
    };
    let offset = || elements.tree().scroll_offset(container);
    let request = || {
        let mut tree = elements.tree();
        let frame = tree.commit();
        let slot = frame
            .slot_of(container)
            .expect("`#container` is a scroll container");
        frame.scroll_slots()[slot as usize]
            .request
            .map(|request| (request.target, request.behavior))
    };
    let instant = Some((
        dom::Vector2D::new(0.0, 120.0),
        dom::scroll::ScrollBehavior::Instant,
    ));
    let smooth = Some((
        dom::Vector2D::new(0.0, 170.0),
        dom::scroll::ScrollBehavior::Smooth,
    ));
    assert_eq!(
        offset(),
        dom::Vector2D::new(0.0, 170.0),
        "opens on `#content`"
    );
    let steps = std::cell::Cell::new(0);
    let run = |runtime: &mut MainThreadRuntime, js_runtime: &mut ScriptRuntime, source: &str| {
        steps.set(steps.get() + 1);
        let name = format!("app:///step-{}.js", steps.get());
        runtime
            .evaluate_module(js_runtime, source, &name, "step")
            .expect("step");
    };

    // After the first `autoStartRefresh` the document rests at 120: a
    // smooth request leaves it there for the painter, and an instant one to
    // where it already is still replaces the pending request.
    for (source, carried) in [
        ("expectCode(viaPapi('autoStartRefresh', {}), 0);", instant),
        (
            "expectCode(viaQuery('finishRefresh', undefined), 0);",
            smooth,
        ),
        (
            "expectCode(viaQuery('autoStartRefresh', undefined), 0);",
            instant,
        ),
        ("expectCode(viaPapi('finishLoadMore', null), 0);", smooth),
        (
            "expectCode(viaPapi('autoStartRefresh', undefined), 0);",
            instant,
        ),
        ("expectCode(viaPapi('finishRefresh', {}), 0);", smooth),
        ("expectCode(viaQuery('finishLoadMore', {}), 0);", smooth),
    ] {
        run(&mut runtime, &mut js_runtime, source);
        assert_eq!(offset(), dom::Vector2D::new(0.0, 120.0), "{source}");
        assert_eq!(request(), carried, "{source}");
    }

    let pending = elements.tree().pending_scroll_request(container);
    run(
        &mut runtime,
        &mut js_runtime,
        r"
            expectCode(viaPapi('selectTab', {index: 1}), 3);
            expectCode(viaQuery('selectTab', {index: 1}), 3);
            expectCode(viaPapi('startRefresh', {}), 3);
            ",
    );
    assert_eq!(elements.tree().pending_scroll_request(container), pending);
    assert_eq!(offset(), dom::Vector2D::new(0.0, 120.0));
}

/// `setFoldExpanded` on an `x-foldview-ng`, through `__InvokeUIMethod` and
/// the selector-query `invoke`. The coordinator is 300px tall with a 20px
/// toolbar, a 100px header and a slot, a fold range of 80. Each instant
/// call moves the document at once and answers code 0; an offset past the
/// range clamps to it; a missing `offset` is code 4. `getScrollInfo` is
/// iOS-only and not built: code 3. `scrollTo` and `scrollBy` are the base
/// set's CSSOM-View methods, which read `top` and not `offset`.
#[test]
#[expect(clippy::too_many_lines, reason = "one scenario over both invoke paths")]
fn a_foldview_answers_set_fold_expanded_through_both_invoke_paths() {
    let (mut js_runtime, mut runtime, elements) = runtime();
    runtime
        .run_main_thread_script(
            &mut js_runtime,
            r"
                globalThis.renderPage = function () {
                  const page = __CreatePage('card', 0);
                  const fold = __CreateElement('x-foldview-ng', 0);
                  __SetID(fold, 'fold');
                  __SetInlineStyles(fold, 'width:200px;height:300px');
                  __AppendElement(page, fold);
                  const toolbar = __CreateElement('x-foldview-toolbar-ng', 0);
                  __SetInlineStyles(toolbar, 'height:20px');
                  const header = __CreateElement('x-foldview-header-ng', 0);
                  __SetInlineStyles(header, 'height:100px');
                  const slot = __CreateElement('x-foldview-slot-ng', 0);
                  __AppendElement(fold, toolbar);
                  __AppendElement(fold, header);
                  __AppendElement(fold, slot);
                  globalThis.held = [page, fold];
                };
                ",
            "app:///foldview.js",
        )
        .expect("main-thread script");
    runtime
        .evaluate_module(
            &mut js_runtime,
            r"
                import {
                  __BobcatQueryNodes,
                  __FlushElementTree,
                  __InvokeUIMethod,
                } from 'bobcat:element';
                __FlushElementTree();
                globalThis.viaPapi = (method, params) => {
                  let answer;
                  let calls = 0;
                  __InvokeUIMethod(held[1], method, params, result => {
                    answer = result;
                    calls += 1;
                  });
                  if (calls !== 1) throw new Error('the callback ran ' + calls + ' times');
                  return answer;
                };
                globalThis.viaQuery = (method, params) => __BobcatQueryNodes({
                  bobcat: 'runtime', method: 'nodeQuery', operation: 'invoke',
                  token: {type: 0, identifier: '#fold', component_id: '',
                          first_only: true, root_unique_id: undefined},
                  params: {method, params},
                });
                globalThis.expectCode = (answer, code) => {
                  if (answer.code !== code || answer.data !== undefined) {
                    throw new Error('expected ' + code + ', got ' + JSON.stringify(answer));
                  }
                };
                ",
            "app:///helpers.js",
            "helpers",
        )
        .expect("helpers");
    let fold = {
        let tree = elements.tree();
        let page = tree.document_element().id();
        tree.get(page).expect("the page").child_ids()[0]
    };
    assert_eq!(
        elements
            .tree()
            .scroll_box(fold)
            .map(|scroll_box| scroll_box.max_offset()),
        Some(dom::Vector2D::new(0.0, 80.0)),
        "the header height less the toolbar height"
    );
    let offset = || elements.tree().scroll_offset(fold).y;
    let steps = std::cell::Cell::new(0);
    let run = |runtime: &mut MainThreadRuntime, js_runtime: &mut ScriptRuntime, source: &str| {
        steps.set(steps.get() + 1);
        let name = format!("app:///step-{}.js", steps.get());
        runtime
            .evaluate_module(js_runtime, source, &name, "step")
            .expect("step");
    };
    for (source, expected) in [
        (
            "expectCode(viaPapi('setFoldExpanded', {offset: 50, smooth: false}), 0);",
            50.0,
        ),
        (
            "expectCode(viaQuery('setFoldExpanded', {offset: 500, smooth: false}), 0);",
            80.0,
        ),
        (
            "expectCode(viaQuery('setFoldExpanded', {offset: '10px', smooth: false}), 0);",
            10.0,
        ),
        ("expectCode(viaPapi('setFoldExpanded', {}), 4);", 10.0),
        (
            "expectCode(viaQuery('setFoldExpanded', undefined), 4);",
            10.0,
        ),
        ("expectCode(viaPapi('getScrollInfo', {}), 3);", 10.0),
        ("expectCode(viaQuery('getScrollInfo', {}), 3);", 10.0),
        // The base set's `scrollBy` reads `left`/`top`: `offset` moves nothing.
        ("expectCode(viaPapi('scrollBy', {offset: 30}), 0);", 10.0),
        ("expectCode(viaQuery('scrollTo', {top: 70}), 0);", 70.0),
        ("expectCode(viaPapi('scrollTo', {top: 10}), 0);", 10.0),
    ] {
        run(&mut runtime, &mut js_runtime, source);
        assert!((offset() - expected).abs() < f32::EPSILON, "{source}");
    }
}

/// An `<x-markdown>`'s UI methods through `__InvokeUIMethod`, each answer
/// built by `element-papi.ts` from the record the component writes:
/// `getContent`'s `{content}` with web-core's inclusive `end`, `getImages`'
/// and `getParseResult`'s typings shapes, `getTextBoundingRect`'s
/// `{boundingRect, boxes}` and `getCharIndexByPoint`'s `{index}` over the
/// rendered text, and code 4 for `getParseResult` without tags. A 10px Ahem
/// paragraph at the page's origin.
#[test]
fn a_markdown_answers_its_ui_methods_through_invoke() {
    let (mut js_runtime, mut runtime, _elements) = text_runtime();
    runtime
        .run_main_thread_script(
            &mut js_runtime,
            r"
                globalThis.renderPage = function () {
                  const page = __CreatePage('card', 0);
                  const markdown = __CreateElement('x-markdown', 0);
                  __SetInlineStyles(markdown, 'font-family:Ahem;font-size:10px;width:300px');
                  __SetAttribute(markdown, 'content-id', 'doc-1');
                  __SetAttribute(markdown, 'content', 'see [docs](https://a.test/) ![i](x.png)');
                  __AppendElement(page, markdown);
                  globalThis.held = [page, markdown];
                };
                ",
            "app:///markdown-methods.js",
        )
        .expect("main-thread script");
    runtime
        .evaluate_module(
            &mut js_runtime,
            r"
                import { __FlushElementTree, __InvokeUIMethod } from 'bobcat:element';
                __FlushElementTree();
                const call = (method, params) => {
                  let answer;
                  __InvokeUIMethod(held[1], method, params, result => { answer = result; });
                  return answer;
                };
                const expectAnswer = (answer, code, data) => {
                  if (answer.code !== code || JSON.stringify(answer.data) !== JSON.stringify(data)) {
                    throw new Error('expected ' + JSON.stringify({code, data}) + ', got ' +
                                    JSON.stringify(answer));
                  }
                };
                expectAnswer(call('getContent', {start: 4, end: 9}), 0, {content: '[docs]'});
                expectAnswer(call('getImages', {}), 0, {images: ['x.png']});
                expectAnswer(call('getParseResult', {tags: ['a', 'img']}), 0, {
                  id: 'doc-1',
                  result: {a: [{start: 4, end: 8}], img: [{start: 0, end: 0}]},
                });
                expectAnswer(call('getParseResult', {}), 4, undefined);
                // 'docs' is rendered 4..8, 40px into the line.
                const box = {left: 40, top: 0, right: 80, bottom: 14, width: 40, height: 14};
                expectAnswer(call('getTextBoundingRect', {start: 4, end: 8}), 0,
                             {boundingRect: box, boxes: [box]});
                expectAnswer(call('getCharIndexByPoint', {x: 45, y: 5}), 0, {index: 4});
                expectAnswer(call('getCharIndexByPoint', {x: 45, y: 5, indexType: 'source'}), 0,
                             {index: 5});
                expectAnswer(call('getSelectedText', {}), 3, undefined);
                ",
            "app:///markdown-method-calls.js",
            "markdown methods",
        )
        .expect("the markdown's methods answer as expected");
}

/// `getTextBoundingRect` on a `text`, through `__InvokeUIMethod` and the
/// selector-query `invoke`: a 10px Ahem paragraph at (20, 30) with 5px of
/// padding answers its range's box in viewport CSS px, from the content
/// box's origin, with `boundingRect` the union `element-papi.ts` builds. A
/// reversed range is code 4, a range past the text's end code 8, the
/// selection methods code 3, and the method on a `view` code 3.
#[test]
fn a_text_answers_get_text_bounding_rect_through_both_invoke_paths() {
    let (mut js_runtime, mut runtime, _elements) = text_runtime();
    runtime
        .run_main_thread_script(
            &mut js_runtime,
            r"
                globalThis.renderPage = function () {
                  const page = __CreatePage('card', 0);
                  const text = __CreateText(0);
                  __SetID(text, 'label');
                  __SetInlineStyles(text,
                    'position:absolute;left:20px;top:30px;padding:5px;' +
                    'font-family:Ahem;font-size:10px');
                  __AppendElement(text, __CreateRawText('abcdef'));
                  __AppendElement(page, text);
                  const view = __CreateView(0);
                  __AppendElement(page, view);
                  globalThis.held = [page, text, view];
                };
                ",
            "app:///text-methods.js",
        )
        .expect("main-thread script");
    runtime
        .evaluate_module(
            &mut js_runtime,
            r"
                import {
                  __BobcatQueryNodes,
                  __FlushElementTree,
                  __InvokeUIMethod,
                } from 'bobcat:element';
                __FlushElementTree();
                const viaPapi = (element, method, params) => {
                  let answer;
                  let calls = 0;
                  __InvokeUIMethod(element, method, params, result => {
                    answer = result;
                    calls += 1;
                  });
                  if (calls !== 1) throw new Error('the callback ran ' + calls + ' times');
                  return answer;
                };
                const viaQuery = (method, params) => __BobcatQueryNodes({
                  bobcat: 'runtime', method: 'nodeQuery', operation: 'invoke',
                  token: {type: 0, identifier: '#label', component_id: '',
                          first_only: true, root_unique_id: undefined},
                  params: {method, params},
                });
                const expectAnswer = (answer, code, data) => {
                  if (answer.code !== code || JSON.stringify(answer.data) !== JSON.stringify(data)) {
                    throw new Error('expected ' + JSON.stringify({code, data}) + ', got ' +
                                    JSON.stringify(answer));
                  }
                };
                // Units 1 and 2, b and c, at x 10..30 of the paragraph, whose
                // content box starts at (25, 35).
                const box = {left: 35, top: 35, right: 55, bottom: 45, width: 20, height: 10};
                const found = {boundingRect: box, boxes: [box]};
                expectAnswer(viaPapi(held[1], 'getTextBoundingRect', {start: 1, end: 3}), 0, found);
                expectAnswer(viaQuery('getTextBoundingRect', {start: 1, end: 3}), 0, found);
                expectAnswer(viaPapi(held[1], 'getTextBoundingRect', {start: 3, end: 1}), 4,
                             undefined);
                expectAnswer(viaQuery('getTextBoundingRect', {start: 3, end: 1}), 4, undefined);
                expectAnswer(viaPapi(held[1], 'getTextBoundingRect', {start: 0, end: 99}), 8,
                             undefined);
                expectAnswer(viaQuery('getTextBoundingRect', {start: 0, end: 99}), 8, undefined);
                expectAnswer(viaPapi(held[1], 'getSelectedText', {}), 3, undefined);
                expectAnswer(viaQuery('setTextSelection',
                                      {startX: 0, startY: 0, endX: 10, endY: 10}), 3, undefined);
                expectAnswer(viaPapi(held[2], 'getTextBoundingRect', {start: 0, end: 1}), 3,
                             undefined);
                ",
            "app:///text-method-calls.js",
            "text methods",
        )
        .expect("text methods");
}

/// A `scroll-view`'s `getScrollInfo`, `scrollBy` and `scrollTo` through
/// `__InvokeUIMethod`: the data methods' records come back as the objects
/// `element-papi.ts` builds from them, in CSS px, and every method moves the
/// document the realm reads back. The scroller is 100px tall over five 40px
/// children, a range of 100.
#[test]
fn a_scroll_view_answers_its_scroll_info_and_what_scroll_by_consumed() {
    let (mut js_runtime, mut runtime, elements) = runtime();
    runtime
        .run_main_thread_script(
            &mut js_runtime,
            r"
                globalThis.renderPage = function () {
                  const page = __CreatePage('card', 0);
                  const scroller = __CreateElement('scroll-view', 0);
                  __SetInlineStyles(scroller, 'width:100px;height:100px');
                  __AppendElement(page, scroller);
                  for (let i = 0; i < 5; i++) {
                    const item = __CreateView(0);
                    __SetInlineStyles(item, 'height:40px;flex-shrink:0');
                    __AppendElement(scroller, item);
                  }
                  globalThis.held = [page, scroller];
                };
                ",
            "app:///scroll-view.js",
        )
        .expect("main-thread script");
    runtime
        .evaluate_module(
            &mut js_runtime,
            r"
                import { __FlushElementTree, __InvokeUIMethod } from 'bobcat:element';
                __FlushElementTree();
                const viaPapi = (method, params) => {
                  let answer;
                  __InvokeUIMethod(held[1], method, params, result => { answer = result; });
                  return answer;
                };
                const expectAnswer = (answer, code, data) => {
                  if (answer.code !== code || JSON.stringify(answer.data) !== JSON.stringify(data)) {
                    throw new Error('expected ' + JSON.stringify({code, data}) + ', got ' +
                                    JSON.stringify(answer));
                  }
                };
                expectAnswer(viaPapi('getScrollInfo', undefined), 0,
                             {scrollX: 0, scrollY: 0, scrollRange: 100, maxScrollOffset: 100});
                expectAnswer(viaPapi('scrollBy', {offset: 70}), 0,
                             {consumedX: 0, consumedY: 70, unconsumedX: 70, unconsumedY: 0});
                // The second one reaches the end: 30 consumed, 40 left over.
                expectAnswer(viaPapi('scrollBy', {offset: 70}), 0,
                             {consumedX: 0, consumedY: 30, unconsumedX: 70, unconsumedY: 40});
                expectAnswer(viaPapi('getScrollInfo', {}), 0,
                             {scrollX: 0, scrollY: 100, scrollRange: 100, maxScrollOffset: 100});
                expectAnswer(viaPapi('scrollBy', {}), 4, undefined);
                expectAnswer(viaPapi('scrollTo', {index: 1, offset: '2.5px'}), 0, undefined);
                expectAnswer(viaPapi('scrollTo', {index: 5}), 4, undefined);
                expectAnswer(viaPapi('getScrollInfo', {}), 0,
                             {scrollX: 0, scrollY: 42.5, scrollRange: 100, maxScrollOffset: 100});
                // The painter moves it; the document's offset stays put.
                expectAnswer(viaPapi('autoScroll', {rate: 60, start: true}), 0, undefined);
                expectAnswer(viaPapi('autoScroll', {start: true}), 4, undefined);
                expectAnswer(viaPapi('autoScroll', {start: false}), 0, undefined);
                expectAnswer(viaPapi('takeContentScreenshot', {}), 3, undefined);
                ",
            "app:///scroll-view-methods.js",
            "scroll-view methods",
        )
        .expect("scroll-view methods");
    let scroller = {
        let tree = elements.tree();
        let page = tree.document_element().id();
        tree.get(page).expect("the page").child_ids()[0]
    };
    assert_eq!(
        elements.tree().scroll_offset(scroller),
        dom::Vector2D::new(0.0, 42.5)
    );
}

/// A `list`'s `scrollToPosition`, `scrollBy`, `getScrollContainerInfo` and
/// `getVisibleCells` through `__InvokeUIMethod`: the data methods' records
/// come back as the objects `element-papi.ts` builds from them, in CSS px,
/// and the list's missing `scrollTo` is the base set's CSSOM-View one. The
/// list is 100px tall over five 40px cells, a range of 100.
#[test]
fn a_list_answers_its_methods_and_the_base_set_answers_scroll_to() {
    let (mut js_runtime, mut runtime, elements) = runtime();
    runtime
        .run_main_thread_script(
            &mut js_runtime,
            r"
                globalThis.renderPage = function () {
                  const page = __CreatePage('card', 0);
                  const list = __CreateElement('list', 0);
                  __SetInlineStyles(list, 'width:100px;height:100px');
                  __AppendElement(page, list);
                  const cells = [];
                  for (let i = 0; i < 5; i++) {
                    const cell = __CreateElement('list-item', 0);
                    __SetInlineStyles(cell, 'height:40px');
                    __AppendElement(list, cell);
                    cells.push(cell);
                  }
                  __SetAttribute(cells[2], 'item-key', 'a,b');
                  __SetID(cells[4], 'last');
                  __SetAttribute(cells[4], 'item-key', 'k4');
                  globalThis.held = [page, list];
                };
                ",
            "app:///list.js",
        )
        .expect("main-thread script");
    runtime
        .evaluate_module(
            &mut js_runtime,
            r"
                import { __FlushElementTree, __InvokeUIMethod } from 'bobcat:element';
                __FlushElementTree();
                const viaPapi = (method, params) => {
                  let answer;
                  __InvokeUIMethod(held[1], method, params, result => { answer = result; });
                  return answer;
                };
                const expectAnswer = (answer, code, data) => {
                  if (answer.code !== code || JSON.stringify(answer.data) !== JSON.stringify(data)) {
                    throw new Error('expected ' + JSON.stringify({code, data}) + ', got ' +
                                    JSON.stringify(answer));
                  }
                };
                // Cell 2 (80..120) with its end at the 100px port's end: 20.
                expectAnswer(viaPapi('scrollToPosition', {position: 2, alignTo: 'bottom'}), 0,
                             undefined);
                expectAnswer(viaPapi('scrollBy', {offset: 50}), 0,
                             {consumedX: 0, consumedY: 50, unconsumedX: 50, unconsumedY: 0});
                expectAnswer(viaPapi('getScrollContainerInfo', undefined), 0,
                             {scrollTop: 70, scrollLeft: 0, scrollHeight: 200, scrollWidth: 100});
                expectAnswer(viaPapi('getVisibleCells', {}), 0, [
                  {id: '', itemKey: '', index: 1, left: 0, top: -30, right: 100, bottom: 10},
                  {id: '', itemKey: 'a,b', index: 2, left: 0, top: 10, right: 100, bottom: 50},
                  {id: '', itemKey: '', index: 3, left: 0, top: 50, right: 100, bottom: 90},
                  {id: 'last', itemKey: 'k4', index: 4, left: 0, top: 90, right: 100, bottom: 130},
                ]);
                expectAnswer(viaPapi('scrollToPosition', {position: 5}), 4, undefined);
                expectAnswer(viaPapi('scrollBy', {}), 4, undefined);
                expectAnswer(viaPapi('autoScroll', {rate: '60px', start: true, autoStop: false}),
                             0, undefined);
                expectAnswer(viaPapi('autoScroll', {rate: 0, start: true}), 4, undefined);
                expectAnswer(viaPapi('getScrollInfo', {}), 3, undefined);
                // The base set's `scrollTo`, which the list does not shadow.
                expectAnswer(viaPapi('scrollTo', {top: 50}), 0, undefined);
                expectAnswer(viaPapi('getScrollContainerInfo', {}), 0,
                             {scrollTop: 50, scrollLeft: 0, scrollHeight: 200, scrollWidth: 100});
                ",
            "app:///list-methods.js",
            "list methods",
        )
        .expect("list methods");
    let list = {
        let tree = elements.tree();
        let page = tree.document_element().id();
        tree.get(page).expect("the page").child_ids()[0]
    };
    assert_eq!(
        elements.tree().scroll_offset(list),
        dom::Vector2D::new(0.0, 50.0)
    );
}

/// `scrollIntoView` through `__InvokeUIMethod` is the base method every
/// element has: a child view of a `scroll-view` scrolls the scroller (code
/// 0, no data), and the page, with no scroll container above it, answers
/// code 8. The scroller is 100px tall over five 40px children, a range of
/// 100.
#[test]
fn every_element_answers_scroll_into_view_and_one_with_no_scroller_above_fails() {
    let (mut js_runtime, mut runtime, elements) = runtime();
    runtime
        .run_main_thread_script(
            &mut js_runtime,
            r"
                globalThis.renderPage = function () {
                  const page = __CreatePage('card', 0);
                  const scroller = __CreateElement('scroll-view', 0);
                  __SetInlineStyles(scroller, 'width:100px;height:100px');
                  __AppendElement(page, scroller);
                  const items = [];
                  for (let i = 0; i < 5; i++) {
                    const item = __CreateView(0);
                    __SetInlineStyles(item, 'height:40px;flex-shrink:0');
                    __AppendElement(scroller, item);
                    items.push(item);
                  }
                  globalThis.held = [page, scroller, items];
                };
                ",
            "app:///scroll-into-view.js",
        )
        .expect("main-thread script");
    runtime
        .evaluate_module(
            &mut js_runtime,
            r"
                import { __FlushElementTree, __InvokeUIMethod } from 'bobcat:element';
                __FlushElementTree();
                const viaPapi = (element, params) => {
                  let answer;
                  __InvokeUIMethod(element, 'scrollIntoView', params, result => { answer = result; });
                  return answer;
                };
                const expectAnswer = (answer, code) => {
                  if (answer.code !== code || answer.data !== undefined) {
                    throw new Error('expected ' + code + ', got ' + JSON.stringify(answer));
                  }
                };
                // The fourth child (120..160) at the end of the 100px port.
                expectAnswer(viaPapi(held[2][3], {scrollIntoViewOptions: {block: 'end'}}), 0);
                expectAnswer(viaPapi(held[2][3], {}), 4);
                expectAnswer(viaPapi(held[0], {scrollIntoViewOptions: {}}), 8);
                ",
            "app:///scroll-into-view-methods.js",
            "scrollIntoView",
        )
        .expect("scrollIntoView");
    let scroller = {
        let tree = elements.tree();
        let page = tree.document_element().id();
        tree.get(page).expect("the page").child_ids()[0]
    };
    assert_eq!(
        elements.tree().scroll_offset(scroller),
        dom::Vector2D::new(0.0, 60.0)
    );
}

/// The base set's `scrollTo` and `scrollBy`, CSSOM-View's `ScrollToOptions`
/// methods, through both paths a card reaches a UI method by:
/// `__InvokeUIMethod` and the selector-query `invoke` (`__BobcatQueryNodes`).
/// A 100px square `view` with `overflow: scroll` over a 300px child, a range
/// of 200, scrolls to `top` and by `top`; a `scroll-view` of the same size
/// answers its own `scrollTo`, which reads `index` and `offset` and no `top`
/// (as web-core's `ScrollView.scrollTo` override shadows the standard one),
/// so `{top: 50}` there scrolls to offset 0. An unknown `behavior` and
/// params that are no dictionary are code 4. `boundingClientRect` answers
/// the same object on either path.
#[test]
#[expect(clippy::too_many_lines, reason = "one scenario over both invoke paths")]
fn every_element_answers_scroll_to_and_scroll_by_unless_its_kind_has_its_own() {
    let (mut js_runtime, mut runtime, elements) = runtime();
    runtime
        .run_main_thread_script(
            &mut js_runtime,
            r"
                globalThis.renderPage = function () {
                  const page = __CreatePage('card', 0);
                  const box = __CreateView(0);
                  __SetID(box, 'box');
                  __SetInlineStyles(box,
                    'width:100px;height:100px;overflow:scroll;flex-shrink:0;margin-left:10px');
                  __AppendElement(page, box);
                  const tall = __CreateView(0);
                  __SetInlineStyles(tall, 'width:100px;height:300px;flex-shrink:0');
                  __AppendElement(box, tall);
                  const scroller = __CreateElement('scroll-view', 0);
                  __SetID(scroller, 'scroller');
                  __SetInlineStyles(scroller, 'width:100px;height:100px;flex-shrink:0');
                  __AppendElement(page, scroller);
                  for (let i = 0; i < 5; i++) {
                    const item = __CreateView(0);
                    __SetInlineStyles(item, 'height:40px;flex-shrink:0');
                    __AppendElement(scroller, item);
                  }
                  globalThis.held = [page, box, scroller];
                };
                ",
            "app:///scroll-to-options.js",
        )
        .expect("main-thread script");
    runtime
        .evaluate_module(
            &mut js_runtime,
            r"
                import {
                  __BobcatQueryNodes,
                  __FlushElementTree,
                  __InvokeUIMethod,
                } from 'bobcat:element';
                __FlushElementTree();
                globalThis.viaPapi = (element, method, params) => {
                  let answer;
                  __InvokeUIMethod(element, method, params, result => { answer = result; });
                  return answer;
                };
                globalThis.viaQuery = (selector, method, params) => __BobcatQueryNodes({
                  bobcat: 'runtime', method: 'nodeQuery', operation: 'invoke',
                  token: {type: 0, identifier: selector, component_id: '',
                          first_only: true, root_unique_id: undefined},
                  params: {method, params},
                });
                globalThis.expectCode = (answer, code) => {
                  if (answer.code !== code || answer.data !== undefined) {
                    throw new Error('expected ' + code + ', got ' + JSON.stringify(answer));
                  }
                };
                ",
            "app:///helpers.js",
            "helpers",
        )
        .expect("helpers");
    let (scroll_box, scroller) = {
        let tree = elements.tree();
        let page = tree.document_element().id();
        let children = tree.get(page).expect("the page").child_ids();
        (children[0], children[1])
    };
    let offset = |id| elements.tree().scroll_offset(id);
    let steps = std::cell::Cell::new(0);
    let run = |runtime: &mut MainThreadRuntime, js_runtime: &mut ScriptRuntime, source: &str| {
        steps.set(steps.get() + 1);
        let name = format!("app:///step-{}.js", steps.get());
        runtime
            .evaluate_module(js_runtime, source, &name, "step")
            .expect("step");
    };

    run(
        &mut runtime,
        &mut js_runtime,
        "expectCode(viaPapi(held[1], 'scrollTo', {top: 50}), 0);",
    );
    assert_eq!(offset(scroll_box), dom::Vector2D::new(0.0, 50.0));
    run(
        &mut runtime,
        &mut js_runtime,
        "expectCode(viaQuery('#box', 'scrollBy', {top: 30}), 0);
         expectCode(viaPapi(held[1], 'scrollBy', {top: 30}), 0);",
    );
    assert_eq!(offset(scroll_box), dom::Vector2D::new(0.0, 110.0));
    run(
        &mut runtime,
        &mut js_runtime,
        "expectCode(viaQuery('#box', 'scrollTo', {top: 1e9}), 0);",
    );
    assert_eq!(offset(scroll_box), dom::Vector2D::new(0.0, 200.0));
    // A `behavior` outside the enum, and params that are no dictionary, are
    // code 4 and move nothing, as web-core answers the `TypeError` Web IDL
    // throws; an array is a dictionary with no members.
    run(
        &mut runtime,
        &mut js_runtime,
        "expectCode(viaPapi(held[1], 'scrollTo', {top: 0, behavior: 'bogus'}), 4);
         expectCode(viaQuery('#box', 'scrollBy', {top: -10, behavior: 1}), 4);
         expectCode(viaPapi(held[1], 'scrollTo', 5), 4);
         expectCode(viaQuery('#box', 'scrollBy', 'top'), 4);
         expectCode(viaPapi(held[1], 'scrollTo', []), 0);",
    );
    assert_eq!(offset(scroll_box), dom::Vector2D::new(0.0, 200.0));

    // The scroll-view's own `scrollTo` answers: `{index: 1}` lands on the
    // second child, and `{top: 50}` is its offset-less call, which scrolls
    // to 0.
    run(
        &mut runtime,
        &mut js_runtime,
        "expectCode(viaPapi(held[2], 'scrollTo', {index: 1}), 0);",
    );
    assert_eq!(offset(scroller), dom::Vector2D::new(0.0, 40.0));
    run(
        &mut runtime,
        &mut js_runtime,
        "expectCode(viaQuery('#scroller', 'scrollTo', {top: 50}), 0);",
    );
    assert_eq!(offset(scroller), dom::Vector2D::zero());

    // `boundingClientRect` is the base set's on every element, its answer
    // the object `element-papi.ts` builds on either path.
    run(
        &mut runtime,
        &mut js_runtime,
        r"
            for (const answer of [viaPapi(held[1], 'boundingClientRect', {top: 1}),
                                  viaQuery('#box', 'boundingClientRect', undefined)]) {
              const {code, data} = answer;
              if (code !== 0 || data.id !== 'box' || data.left !== 10 || data.top !== 0 ||
                  data.width !== 100 || data.height !== 100 || data.right !== 110 ||
                  data.bottom !== 100 || typeof data.dataset !== 'object') {
                throw new Error(JSON.stringify(answer));
              }
            }
            ",
    );
}

/// Each component-method outcome maps to one status code, and `Data` to a
/// record whose first field is the code `0`, the component's fields after it
/// unchanged.
#[test]
#[expect(
    clippy::float_cmp,
    reason = "the status codes are small integers, exact in f64"
)]
fn a_method_outcome_answers_its_status_code_or_a_record() {
    let number = |outcome| match method_answer(outcome) {
        HostValue::Number(code) => code,
        other => panic!("a status code, not {other:?}"),
    };
    assert_eq!(number(MethodOutcome::NotFound), 3.0);
    assert_eq!(number(MethodOutcome::Done), 0.0);
    assert_eq!(
        number(MethodOutcome::Failed(MethodError::InvalidParams)),
        4.0
    );
    assert_eq!(
        number(MethodOutcome::Failed(MethodError::InvalidState)),
        7.0
    );
    assert_eq!(number(MethodOutcome::Failed(MethodError::Operation)), 8.0);

    let mut fields = String::new();
    write_record_field(&mut fields, "a,b");
    write_record_field(&mut fields, "\u{1F600}");
    match method_answer(MethodOutcome::Data(fields)) {
        HostValue::String(record) => assert_eq!(record, "1:03:a,b2:\u{1F600}"),
        other => panic!("a record, not {other:?}"),
    }
}

/// A `<dialog>`'s four methods through `__InvokeUIMethod` and the
/// selector-query `invoke` (`__BobcatQueryNodes`): `showModal` opens it in
/// the top layer, where `:modal` matches and `open` is an attribute script
/// reads back; `show` on a modal dialog is HTML's `InvalidStateError`, code
/// 7 (native's `INVALID_STATE_ERROR`); `close` delivers a `close` and `requestClose` a `cancel` and
/// then a `close`, each non-bubbling — the capture pass runs the path, the bind
/// pass the dialog alone — with a `{}` detail, from an entry of their own.
#[test]
#[expect(clippy::too_many_lines, reason = "one scenario over both invoke paths")]
fn a_dialog_opens_closes_and_fires_through_both_invoke_paths() {
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
                  const dialog = __CreateElement('dialog', 0);
                  __SetID(dialog, 'd');
                  __AppendElement(page, outer);
                  __AppendElement(outer, dialog);
                  globalThis.held = [page, outer, dialog];
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
                  for (const name of ['close', 'cancel']) {
                    __AddEvent(dialog, 'bindEvent', name, note('dialog'));
                    __AddEvent(outer, 'bindEvent', name, note('outer'));
                    __AddEvent(page, 'capture-bind', name, note('page-capture'));
                  }
                };
                ",
            "app:///dialog.js",
        )
        .expect("main-thread script");
    runtime
        .evaluate_module(
            &mut js_runtime,
            r"
                import {
                  __BobcatQueryNodes,
                  __FlushElementTree,
                  __GetAttributeByName,
                  __InvokeUIMethod,
                } from 'bobcat:element';
                __FlushElementTree();
                globalThis.openAttribute = element => __GetAttributeByName(element, 'open');
                globalThis.viaPapi = (element, method, params) => {
                  let answer;
                  __InvokeUIMethod(element, method, params, result => { answer = result; });
                  return answer;
                };
                globalThis.viaQuery = (selector, method, params) => __BobcatQueryNodes({
                  bobcat: 'runtime', method: 'nodeQuery', operation: 'invoke',
                  token: {type: 0, identifier: selector, component_id: '',
                          first_only: true, root_unique_id: undefined},
                  params: {method, params},
                });
                globalThis.expectCode = (answer, code) => {
                  if (answer.code !== code || answer.data !== undefined) {
                    throw new Error('expected ' + code + ', got ' + JSON.stringify(answer));
                  }
                };
                ",
            "app:///dialog-helpers.js",
            "helpers",
        )
        .expect("helpers");
    let dialog = {
        let tree = elements.tree();
        let page = tree.document_element().id();
        let outer = tree.get(page).expect("the page").child_ids()[0];
        tree.get(outer).expect("the outer view").child_ids()[0]
    };
    let steps = std::cell::Cell::new(0);
    let run = |runtime: &mut MainThreadRuntime, js_runtime: &mut ScriptRuntime, source: &str| {
        steps.set(steps.get() + 1);
        let name = format!("app:///dialog-step-{}.js", steps.get());
        runtime
            .evaluate_module(js_runtime, source, &name, "step")
            .expect("step");
    };
    let modal = || {
        let tree = elements.tree();
        (
            tree.blocks_document(dialog),
            tree.matches(dialog, ":modal").expect("a valid selector"),
            tree.matches(dialog, ":open").expect("a valid selector"),
        )
    };

    run(
        &mut runtime,
        &mut js_runtime,
        r"
            expectCode(viaPapi(held[2], 'showModal', {}), 0);
            if (openAttribute(held[2]) !== '') throw new Error('open: ' + openAttribute(held[2]));
            ",
    );
    assert_eq!(modal(), (true, true, true));
    run(
        &mut runtime,
        &mut js_runtime,
        r"
            expectCode(viaQuery('#d', 'showModal', undefined), 0);
            expectCode(viaPapi(held[2], 'show', {}), 7);
            expectCode(viaQuery('#d', 'show', {}), 7);
            ",
    );
    assert_eq!(
        modal(),
        (true, true, true),
        "a refused `show` changes nothing"
    );
    assert!(!runtime.has_component_events());

    // `close(returnValue)`: the value is dropped.
    run(
        &mut runtime,
        &mut js_runtime,
        r"
            expectCode(viaQuery('#d', 'close', {returnValue: 'kept nowhere'}), 0);
            if (openAttribute(held[2]) !== null) throw new Error('still open');
            ",
    );
    assert_eq!(modal(), (false, false, false));
    assert!(
        runtime.has_component_events(),
        "owed to an entry of its own"
    );
    expect_seen(&mut js_runtime, &mut runtime, "");
    assert!(
        runtime
            .dispatch_component_events(&mut js_runtime)
            .is_empty()
    );
    expect_seen(
        &mut js_runtime,
        &mut runtime,
        "page-capture:2:close:{}|dialog:4:close:{}",
    );

    // Closing a closed dialog succeeds and owes nothing.
    run(
        &mut runtime,
        &mut js_runtime,
        r"
            expectCode(viaPapi(held[2], 'close', {}), 0);
            expectCode(viaPapi(held[2], 'requestClose', {}), 0);
            ",
    );
    assert!(!runtime.has_component_events());

    run(
        &mut runtime,
        &mut js_runtime,
        r"
            expectCode(viaQuery('#d', 'show', {}), 0);
            expectCode(viaQuery('#d', 'showModal', {}), 7);
            expectCode(viaPapi(held[2], 'requestClose', {}), 0);
            ",
    );
    assert_eq!(modal(), (false, false, false));
    assert!(
        runtime
            .dispatch_component_events(&mut js_runtime)
            .is_empty()
    );
    expect_seen(
        &mut js_runtime,
        &mut runtime,
        "page-capture:2:cancel:{}|dialog:4:cancel:{}|page-capture:2:close:{}|dialog:4:close:{}",
    );

    // The methods belong to `dialog` alone, and `dialog` has no others.
    run(
        &mut runtime,
        &mut js_runtime,
        r"
            expectCode(viaPapi(held[1], 'showModal', {}), 3);
            expectCode(viaPapi(held[2], 'selectTab', {index: 0}), 3);
            ",
    );
}

/// An `<x-overlay-ng>` shown and hidden through `__SetAttribute(…,
/// 'visible', …)`: the boolean `true` opens its shadow dialog in the top
/// layer, blocking the document, and owes `showoverlay`; `false`
/// (stringified to `"false"`) closes it and owes `dismissoverlay`. Each is
/// delivered at the host from an entry of its own, non-bubbling — the capture
/// pass runs the path, the bind pass the overlay alone, so the outer view's
/// `bindEvent` never runs — with a `{}` detail. A rewrite that keeps it shown
/// owes nothing.
#[test]
#[expect(clippy::too_many_lines, reason = "one scenario: show, rewrite, hide")]
fn an_overlay_shows_and_dismisses_through_set_attribute() {
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
                  const overlay = __CreateElement('x-overlay-ng', 0);
                  const panel = __CreateView(0);
                  __AppendElement(page, outer);
                  __AppendElement(outer, overlay);
                  __AppendElement(overlay, panel);
                  globalThis.held = [page, outer, overlay, panel];
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
                  for (const name of ['showoverlay', 'dismissoverlay']) {
                    __AddEvent(overlay, 'bindEvent', name, note('overlay'));
                    __AddEvent(outer, 'bindEvent', name, note('outer'));
                    __AddEvent(page, 'capture-bind', name, note('page-capture'));
                  }
                };
                ",
            "app:///overlay.js",
        )
        .expect("main-thread script");
    runtime
        .evaluate_module(
            &mut js_runtime,
            r"
                import { __FlushElementTree, __SetAttribute } from 'bobcat:element';
                __FlushElementTree();
                globalThis.setVisible = value => __SetAttribute(held[2], 'visible', value);
                ",
            "app:///overlay-helpers.js",
            "helpers",
        )
        .expect("helpers");
    let dialog = {
        let tree = elements.tree();
        let page = tree.document_element().id();
        let outer = tree.get(page).expect("the page").child_ids()[0];
        let overlay = tree.get(outer).expect("the outer view").child_ids()[0];
        overlay_dialog(&tree, overlay)
    };
    let steps = std::cell::Cell::new(0);
    let run = |runtime: &mut MainThreadRuntime, js_runtime: &mut ScriptRuntime, source: &str| {
        steps.set(steps.get() + 1);
        let name = format!("app:///overlay-step-{}.js", steps.get());
        runtime
            .evaluate_module(js_runtime, source, &name, "step")
            .expect("step");
    };
    let shown = || {
        let tree = elements.tree();
        (tree.in_top_layer(dialog), tree.blocks_document(dialog))
    };
    assert_eq!(shown(), (false, false));
    assert!(!runtime.has_component_events());

    run(&mut runtime, &mut js_runtime, "setVisible(true);");
    assert_eq!(shown(), (true, true));
    assert!(
        runtime.has_component_events(),
        "owed to an entry of its own"
    );
    expect_seen(&mut js_runtime, &mut runtime, "");
    assert!(
        runtime
            .dispatch_component_events(&mut js_runtime)
            .is_empty()
    );
    expect_seen(
        &mut js_runtime,
        &mut runtime,
        "page-capture:2:showoverlay:{}|overlay:4:showoverlay:{}",
    );

    run(&mut runtime, &mut js_runtime, "setVisible('');");
    assert_eq!(shown(), (true, true));
    assert!(!runtime.has_component_events(), "still shown: nothing owed");

    run(&mut runtime, &mut js_runtime, "setVisible(false);");
    assert_eq!(shown(), (false, false));
    assert!(
        runtime
            .dispatch_component_events(&mut js_runtime)
            .is_empty()
    );
    expect_seen(
        &mut js_runtime,
        &mut runtime,
        "page-capture:2:dismissoverlay:{}|overlay:4:dismissoverlay:{}",
    );
}

/// A component event with a string detail, queued from inside a scope as a
/// component's own callback queues one, is delivered from an entry of its
/// own at the element, not bubbling and composed, with the detail built in the realm from
/// one string argument per key: `{id}`, `{url, content, contentId}` and
/// `{url, contentId}`, the shapes `<x-markdown>`'s `parseEnd`, `link` and
/// `imageTap` carry. A string the component had no value for is a key the
/// detail leaves out, so it does not reach the `JSON.stringify` below even
/// as `undefined`.
#[test]
#[expect(
    clippy::too_many_lines,
    reason = "one scenario: every shape, with and without its optional key"
)]
fn a_component_event_with_a_string_detail_reaches_script_built_per_shape() {
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
                  const markdown = __CreateView(0);
                  const aside = __CreateView(0);
                  __AppendElement(page, outer);
                  __AppendElement(outer, markdown);
                  __AppendElement(page, aside);
                  globalThis.held = [page, outer, markdown, aside];
                  const note = (label) => ({
                    type: 'worklet',
                    value: {
                      body: (event) =>
                        seen.push(
                          label + ':' + event.currentTarget.uid + ':' + event.type + ':' +
                          JSON.stringify(event.detail) + ':' +
                          Object.keys(event.detail).join(','),
                        ),
                    },
                  });
                  for (const name of ['parseEnd', 'link', 'imageTap']) {
                    __AddEvent(markdown, 'bindEvent', name, note('markdown'));
                    __AddEvent(outer, 'bindEvent', name, note('outer'));
                    __AddEvent(aside, 'global-bindEvent', name, note('aside-global'));
                  }
                };
                ",
            "app:///string-details.js",
        )
        .expect("main-thread script");
    let markdown = {
        let tree = elements.tree();
        let page = tree.document_element().id();
        let outer = tree.get(page).expect("the page").child_ids()[0];
        tree.get(outer).expect("the outer view").child_ids()[0]
    };
    let text = |value: &str| Some(value.to_owned());
    let image = "https://lynxjs.org/a.png".to_owned();
    for (name, detail) in [
        ("parseEnd", ComponentDetail::Id { id: text("doc-1") }),
        ("parseEnd", ComponentDetail::Id { id: None }),
        (
            "link",
            ComponentDetail::UrlContent {
                url: "https://lynxjs.org/".to_owned(),
                content: "Lynx \u{201c}docs\u{201d}".to_owned(),
                content_id: text("doc-1"),
            },
        ),
        (
            "link",
            ComponentDetail::UrlContent {
                url: String::new(),
                content: String::new(),
                content_id: None,
            },
        ),
        (
            "imageTap",
            ComponentDetail::Url {
                url: image.clone(),
                content_id: text(""),
            },
        ),
        (
            "imageTap",
            ComponentDetail::Url {
                url: image,
                content_id: None,
            },
        ),
    ] {
        runtime
            .slot
            .borrow()
            .component_events
            .queue_detailed(markdown, name, detail);
    }
    assert!(runtime.has_component_events());
    assert!(
        runtime
            .dispatch_component_events(&mut js_runtime)
            .is_empty()
    );
    assert!(!runtime.has_component_events());
    // Not bubbling (native's custom events): the bind pass runs at the
    // element alone, so the outer view's handlers never run, and there is no
    // `global-bindEvent` pass.
    expect_seen(
        &mut js_runtime,
        &mut runtime,
        &[
            r#"markdown:4:parseend:{"id":"doc-1"}:id"#,
            "markdown:4:parseend:{}:",
            "markdown:4:link:{\"url\":\"https://lynxjs.org/\",\"content\":\"Lynx \u{201c}docs\u{201d}\",\"contentId\":\"doc-1\"}:url,content,contentId",
            r#"markdown:4:link:{"url":"","content":""}:url,content"#,
            r#"markdown:4:imagetap:{"url":"https://lynxjs.org/a.png","contentId":""}:url,contentId"#,
            r#"markdown:4:imagetap:{"url":"https://lynxjs.org/a.png"}:url"#,
        ]
        .join("|"),
    );

    // A `link` as `<x-markdown>` queues it, and a `Plain` event of the same
    // name: neither reaches the outer view's `bindEvent` or the
    // `global-bindEvent` pass, the detailed one is heard first and carries its
    // strings.
    {
        let slot = runtime.slot.borrow();
        slot.component_events.queue_detailed(
            markdown,
            "link",
            ComponentDetail::UrlContent {
                url: "https://lynxjs.org/".to_owned(),
                content: "Lynx".to_owned(),
                content_id: None,
            },
        );
        slot.component_events.queue(markdown, "link");
    }
    assert!(
        runtime
            .dispatch_component_events(&mut js_runtime)
            .is_empty()
    );
    expect_seen(
        &mut js_runtime,
        &mut runtime,
        &[
            r#"markdown:4:link:{"url":"https://lynxjs.org/","content":"Lynx"}:url,content"#,
            "markdown:4:link:{}:",
        ]
        .join("|"),
    );
}

/// What [`TapProbe`] heard, one entry per delivery.
type TapLog = Rc<RefCell<Vec<String>>>;

/// A test component that builds `<view><text/></view>` in its shadow tree,
/// as a UA component builds its content, and records every `tap` it hears
/// as `phase:target-tag:x,y`. For a bubble-pass delivery whose target is its
/// own shadow content — the one the markdown component acts on — it also
/// writes the tapped tag to its own `heard` attribute, which script's `tap`
/// listener reads to see that the component heard the tap first.
struct TapProbe {
    log: TapLog,
}

impl dom::CustomElement<()> for TapProbe {
    fn observed_events(&self) -> &'static [&'static str] {
        &["tap"]
    }

    fn constructed(&self, document: &mut LynxDocument, element: dom::NodeId) {
        let shadow = document.attach_shadow(element, dom::ShadowRootMode::Open);
        let frame = document.create_element("view", ());
        document.append_child(shadow, frame);
        let text = document.create_element("text", ());
        document.append_child(frame, text);
    }

    fn handle_event(
        &self,
        document: &mut LynxDocument,
        element: dom::NodeId,
        event: &mut dom::event::ElementEvent<'_>,
    ) {
        assert_eq!(event.type_name(), "tap", "the only type it observes");
        let Some(position) = event.detail().position() else {
            panic!("a tap carries a position: {:?}", event.detail());
        };
        let (x, y) = (position.x, position.y);
        let phase = match event.phase() {
            dom::event::EventPhase::Capturing => "capture",
            dom::event::EventPhase::AtTarget => "at-target",
            dom::event::EventPhase::Bubbling => "bubble",
        };
        let tag = document
            .get(event.target())
            .and_then(dom::Node::tag_name)
            .unwrap_or("?")
            .to_owned();
        if event.phase() == dom::event::EventPhase::Bubbling
            && document.shadow_root_of(event.target()) == document.shadow_root(element)
        {
            document.set_attribute(element, "heard", &tag);
        }
        self.log.borrow_mut().push(format!("{phase}:{tag}:{x},{y}"));
    }
}

/// A `tap` the gesture path delivers at a component's UA shadow content is
/// heard first by that component, through `dom`'s walk, with the shadow node
/// it hit as the target and the gesture's position; script's `tap` follows,
/// starting at the host, retargeted, and sees what the component did; the
/// same dispatch's Rust walk hands the component that composed `tap` too, at
/// the host as itself. A tap at the host's light child fires no
/// non-composed `tap`: the component hears only the composed one, as an
/// ancestor of a target outside its shadow tree.
#[test]
fn a_tap_at_shadow_content_is_heard_by_its_component_before_script() {
    let (mut js_runtime, mut runtime, elements) = runtime();
    runtime
        .run_main_thread_script(
            &mut js_runtime,
            r"
                globalThis.seen = [];
                globalThis.renderPage = function () {
                  globalThis.held = [__CreatePage('card', 0)];
                };
                ",
            "app:///tap-probe.js",
        )
        .expect("main-thread script");
    let log = TapLog::default();
    runtime.with_document(|document| {
        document.define(
            "x-tap-probe",
            Box::new(TapProbe {
                log: Rc::clone(&log),
            }),
        );
    });
    runtime
        .evaluate_module(
            &mut js_runtime,
            r"
                import {
                  __AddEventListener, __AppendElement, __CreateElement, __CreateView,
                  __GetAttributeByName,
                } from 'bobcat:element';
                const [page] = held;
                const probe = __CreateElement('x-tap-probe', 0);
                const light = __CreateView(0);
                __AppendElement(page, probe);
                __AppendElement(probe, light);
                held.push(probe, light);
                __AddEventListener(probe, 'tap', (event) => {
                  seen.push(
                    'script:' + event.currentTarget.uid + ':' + event.target.uid + ':' +
                    __GetAttributeByName(probe, 'heard') + ':' + event.detail.x,
                  );
                }, {});
                ",
            "app:///tap-probe-tree.js",
            "tree",
        )
        .expect("the probe tree");
    let (probe, light, text) = {
        let tree = elements.tree();
        let page = tree.document_element().id();
        let probe = tree.get(page).expect("the page").child_ids()[0];
        let light = tree.get(probe).expect("the probe").child_ids()[0];
        let shadow = tree.shadow_root(probe).expect("the probe's shadow root");
        let frame = tree.get(shadow).expect("the shadow root").child_ids()[0];
        let text = tree.get(frame).expect("the shadow view").child_ids()[0];
        (probe, light, text)
    };

    runtime
        .dispatch_for_test(&mut js_runtime, text, &tap(), event_point())
        .expect("the tap at shadow content");
    assert_eq!(
        std::mem::take(&mut *log.borrow_mut()),
        [
            "capture:text:12,30",
            "bubble:text:12,30",
            "at-target:x-tap-probe:12,30"
        ],
        "the non-composed tap's shadow-root step, in both passes, with the inner \
         node; then the one dispatch's composed tap at the host as itself"
    );
    expect_seen(
        &mut js_runtime,
        &mut runtime,
        &format!("script:{0}:{0}:text:12", probe.to_bits()),
    );

    runtime
        .dispatch_for_test(&mut js_runtime, light, &tap(), event_point())
        .expect("the tap at light content");
    assert_eq!(
        std::mem::take(&mut *log.borrow_mut()),
        ["capture:view:12,30", "bubble:view:12,30"],
        "a light child's tap fires no non-composed tap; the composed one reaches \
         the host as an ancestor, its target outside the host's shadow tree"
    );
    // `heard` is still the earlier tap's.
    expect_seen(
        &mut js_runtime,
        &mut runtime,
        &format!("script:{}:{}:text:12", probe.to_bits(), light.to_bits()),
    );
}

/// An `<x-markdown>` a script creates renders its `content` in its UA shadow
/// tree, and a `tap` the gesture path delivers at the paragraph holding a
/// link — the node the hit lands on, a paragraph's runs having no box of
/// their own — reaches the script as `link`, with web-core's
/// `{url, content, contentId}` detail, from an entry of its own. The
/// component hears the tap with no `tap` listener in the realm.
#[test]
fn a_tap_on_markdown_link_text_reaches_script_as_link() {
    let (mut js_runtime, mut runtime, elements) = text_runtime();
    runtime
        .run_main_thread_script(
            &mut js_runtime,
            r"
                globalThis.seen = [];
                globalThis.runWorklet = (value, params) => value.body(params[0]);
                globalThis.renderPage = function () {
                  const page = __CreatePage('card', 0);
                  const markdown = __CreateElement('x-markdown', 0);
                  __SetInlineStyles(markdown, 'font-family:Ahem;font-size:10px;width:300px');
                  __SetAttribute(markdown, 'content-id', 'doc-1');
                  __SetAttribute(markdown, 'content', 'see [docs](https://lynxjs.org/) now');
                  __AppendElement(page, markdown);
                  __AddEvent(markdown, 'bindEvent', 'link', {
                    type: 'worklet',
                    value: {
                      body: (event) => seen.push(
                        event.type + ':' + event.currentTarget.uid + ':' +
                        JSON.stringify(event.detail),
                      ),
                    },
                  });
                  globalThis.held = [page, markdown];
                };
                ",
            "app:///markdown-link.js",
        )
        .expect("main-thread script");
    runtime
        .evaluate_module(
            &mut js_runtime,
            r"
                import { __FlushElementTree } from 'bobcat:element';
                __FlushElementTree();
                ",
            "app:///markdown-link-flush.js",
            "flush",
        )
        .expect("the flush");
    // The render's `parseEnd`, which nothing here listens for.
    assert!(
        runtime
            .dispatch_component_events(&mut js_runtime)
            .is_empty()
    );
    let (markdown, paragraph) = {
        let tree = elements.tree();
        let page = tree.document_element().id();
        let markdown = tree.get(page).expect("the page").child_ids()[0];
        let shadow = tree
            .shadow_root(markdown)
            .expect("the markdown's shadow root");
        let root = tree.get(shadow).expect("the shadow root").child_ids()[0];
        let paragraph = tree.get(root).expect("the markdown root").child_ids()[0];
        assert_eq!(tree.get(paragraph).and_then(dom::Node::tag_name), Some("p"));
        (markdown, paragraph)
    };

    // "see " is four 10px Ahem glyphs: the link's text starts at x = 40.
    runtime
        .dispatch_for_test(
            &mut js_runtime,
            paragraph,
            &tap(),
            dom::Point2D::new(45.0, 5.0),
        )
        .expect("the tap on the link text");
    expect_seen(&mut js_runtime, &mut runtime, "");
    assert!(
        runtime.has_component_events(),
        "owed to an entry of its own"
    );
    assert!(
        runtime
            .dispatch_component_events(&mut js_runtime)
            .is_empty()
    );
    expect_seen(
        &mut js_runtime,
        &mut runtime,
        &format!(
            r#"link:{}:{{"url":"https://lynxjs.org/","content":"docs","contentId":"doc-1"}}"#,
            markdown.to_bits()
        ),
    );

    // Plain text owes nothing.
    runtime
        .dispatch_for_test(
            &mut js_runtime,
            paragraph,
            &tap(),
            dom::Point2D::new(5.0, 5.0),
        )
        .expect("the tap on plain text");
    assert!(!runtime.has_component_events());
}

/// A name a component observes is in the painter's name set with no realm
/// listener anywhere once an element of it exists — `tap`, for a probe
/// component defined after the build, whose element's creation publishes
/// it at the next commit — so the painter sends the gesture's `tap` and the
/// component hears it. Before that element, the document's own markdown
/// definition publishes nothing. A script that listens for the name and
/// then stops does not unpublish what the element still observes.
#[test]
fn a_tap_a_component_observes_is_routed_with_no_script_listener() {
    let (mut js_runtime, mut runtime, elements, mut names) =
        runtime_over_watching_names(ingredients());
    runtime
        .run_main_thread_script(
            &mut js_runtime,
            r"
                globalThis.renderPage = function () {
                  globalThis.held = [__CreatePage('card', 0)];
                  globalThis.listener = () => {};
                };
                ",
            "app:///observed-tap.js",
        )
        .expect("main-thread script");
    assert!(
        !names.contains("tap"),
        "the markdown definition alone, with no element of it, routes nothing"
    );

    let log = TapLog::default();
    elements.tree().define(
        "x-tap-probe",
        Box::new(TapProbe {
            log: Rc::clone(&log),
        }),
    );
    assert!(elements.tree().observes_event("tap"));
    runtime.commit_if_dirty();
    assert!(!names.contains("tap"), "a definition publishes nothing");

    runtime
        .evaluate_module(
            &mut js_runtime,
            r"
                import { __AppendElement, __CreateElement } from 'bobcat:element';
                const [page] = held;
                const probe = __CreateElement('x-tap-probe', 0);
                __AppendElement(page, probe);
                held.push(probe);
                ",
            "app:///observed-tap-tree.js",
            "tree",
        )
        .expect("the probe tree");
    assert!(
        !names.contains("tap"),
        "an element's names cross where the entry's commit does"
    );
    runtime.commit_if_dirty();
    assert!(names.contains("tap"), "the probe element publishes `tap`");
    runtime
        .evaluate_module(
            &mut js_runtime,
            r"
                import { __AddEventListener, __RemoveEventListener } from 'bobcat:element';
                const [, probe] = held;
                __AddEventListener(probe, 'tap', globalThis.listener, {});
                __RemoveEventListener(probe, 'tap', globalThis.listener, {});
                ",
            "app:///observed-tap-listener.js",
            "listening",
        )
        .expect("the listener");
    assert!(
        names.contains("tap"),
        "the realm's last removal does not unpublish an element's name"
    );

    let text = {
        let tree = elements.tree();
        let page = tree.document_element().id();
        let probe = tree.get(page).expect("the page").child_ids()[0];
        let shadow = tree.shadow_root(probe).expect("the probe's shadow root");
        let frame = tree.get(shadow).expect("the shadow root").child_ids()[0];
        tree.get(frame).expect("the shadow view").child_ids()[0]
    };
    runtime
        .dispatch_for_test(&mut js_runtime, text, &tap(), event_point())
        .expect("a tap no script listens for is no error");
    assert_eq!(
        *log.borrow(),
        [
            "capture:text:12,30",
            "bubble:text:12,30",
            "at-target:x-tap-probe:12,30"
        ]
    );
}

/// `tap` is in the painter's name set while an `x-markdown` element exists,
/// not because the document defines the component: creating one opens it at
/// the next commit, a script `tap` listener added and removed meanwhile does
/// not close it, freeing one of two markdowns (by collection, the only
/// release path) closes nothing, and freeing the last closes it unless
/// script still listens — in which case the script's last removal does.
#[test]
fn a_markdown_element_routes_tap_while_it_exists() {
    let (mut js_runtime, mut runtime, elements, mut names) =
        runtime_over_watching_names(ingredients());
    runtime
        .run_main_thread_script(
            &mut js_runtime,
            r"
                globalThis.renderPage = function () {
                  globalThis.held = [__CreatePage('card', 0)];
                  globalThis.listener = () => {};
                };
                ",
            "app:///markdown-tap.js",
        )
        .expect("main-thread script");
    assert!(!names.contains("tap"), "no markdown, no `tap`");
    let mut step = |runtime: &mut MainThreadRuntime, source: &str| {
        let source = format!(
            "import {{
               __AddEventListener, __AppendElement, __CreateElement, __RemoveElement,
               __RemoveEventListener,
             }} from 'bobcat:element';
             // A function's scope, not the module's, which outlives the
             // evaluation and would keep every element it names.
             (() => {{
               const [page] = globalThis.held;
               {source}
             }})();"
        );
        runtime
            .evaluate_module(&mut js_runtime, &source, "app:///step.mjs", "a step")
            .expect("the step runs");
        runtime
            .collect_garbage(&mut js_runtime)
            .expect("collection");
        // What the entry's epilogue does before its commit crosses.
        runtime.commit_if_dirty();
    };

    step(
        &mut runtime,
        "const first = __CreateElement('x-markdown', 0);
         const second = __CreateElement('x-markdown', 0);
         __AppendElement(page, first);
         __AppendElement(page, second);
         globalThis.held.push(first, second);",
    );
    let (first, second) = {
        let tree = elements.tree();
        let page = tree.document_element().id();
        let children = tree.get(page).expect("the page").child_ids();
        (children[0], children[1])
    };
    let freed = |node: dom::NodeId| elements.tree().get(node).is_none();
    assert!(names.contains("tap"), "a markdown element publishes `tap`");

    step(
        &mut runtime,
        "__AddEventListener(page, 'tap', globalThis.listener, {});
         __RemoveEventListener(page, 'tap', globalThis.listener, {});",
    );
    assert!(
        names.contains("tap"),
        "the realm's last removal does not close what a markdown observes"
    );

    step(
        &mut runtime,
        "__RemoveElement(page, globalThis.held[1]);
         globalThis.held.splice(1, 1);",
    );
    assert!(freed(first), "the first markdown is collected");
    assert!(!freed(second));
    assert!(
        names.contains("tap"),
        "the second markdown still observes it"
    );

    step(
        &mut runtime,
        "__AddEventListener(page, 'tap', globalThis.listener, {});
         __RemoveElement(page, globalThis.held.pop());",
    );
    assert!(freed(second), "the second markdown is collected");
    assert!(names.contains("tap"), "script still listens");
    step(
        &mut runtime,
        "__RemoveEventListener(page, 'tap', globalThis.listener, {});",
    );
    assert!(!names.contains("tap"), "neither side wants it");

    step(
        &mut runtime,
        "globalThis.held.push(__CreateElement('x-markdown', 0));",
    );
    assert!(names.contains("tap"), "a new markdown publishes it again");
    step(&mut runtime, "globalThis.held.pop();");
    assert!(
        !names.contains("tap"),
        "freeing the last markdown closes it"
    );
}

/// An event at a UA component's shadow content — an `<x-swiper>`'s
/// shadow `#content`, which a touch between its items hits — reaches script
/// as a browser reports it to listeners outside the shadow tree: retargeted
/// to the host. A tap there captures down from the page and bubbles from
/// the swiper, its `target` at every step. A non-composed event at the same
/// node never leaves
/// the shadow tree, where script names nothing, so it reaches no listener
/// and is no error.
#[test]
fn an_event_at_shadow_content_reaches_script_retargeted_to_its_host() {
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
                  const swiper = __CreateElement('x-swiper', 0);
                  __AppendElement(page, outer);
                  __AppendElement(outer, swiper);
                  __AppendElement(swiper, __CreateElement('x-swiper-item', 0));
                  globalThis.held = [page, outer, swiper];
                  const note = (label) => ({
                    type: 'worklet',
                    value: {
                      body: (event) =>
                        seen.push(
                          label + ':' + event.currentTarget.uid + ':' +
                          event.target.uid + ':' + event.type,
                        ),
                    },
                  });
                  for (const name of ['tap', 'close']) {
                    __AddEvent(swiper, 'bindEvent', name, note('swiper'));
                    __AddEvent(outer, 'bindEvent', name, note('outer'));
                    __AddEvent(page, 'capture-bind', name, note('page-capture'));
                  }
                };
                ",
            "app:///shadow-target.js",
        )
        .expect("main-thread script");
    runtime
        .evaluate_module(
            &mut js_runtime,
            r"
                import { __FlushElementTree } from 'bobcat:element';
                __FlushElementTree();
                ",
            "app:///shadow-target-flush.js",
            "flush",
        )
        .expect("flush");
    let (swiper, content) = {
        let tree = elements.tree();
        let page = tree.document_element().id();
        let outer = tree.get(page).expect("the page").child_ids()[0];
        let swiper = tree.get(outer).expect("the outer view").child_ids()[0];
        let root = tree.shadow_root(swiper).expect("the swiper's shadow root");
        let content = tree.get(root).expect("the shadow root").child_ids()[0];
        (swiper, content)
    };
    let uid = packed_node_id(swiper);

    runtime
        .dispatch_for_test(&mut js_runtime, content, "tap", event_point())
        .expect("a hit on shadow content is delivered");
    expect_seen(
        &mut js_runtime,
        &mut runtime,
        &format!("page-capture:2:{uid}:tap|swiper:{uid}:{uid}:tap|outer:3:{uid}:tap"),
    );

    runtime
        .slot
        .borrow()
        .component_events
        .queue(content, "close");
    assert!(
        runtime
            .dispatch_component_events(&mut js_runtime)
            .is_empty()
    );
    expect_seen(&mut js_runtime, &mut runtime, "");
}

/// A tap on a shown, blocking `<x-overlay-ng>` outside its content hits the
/// overlay's shadow dialog, the box that fills the viewport, and reaches
/// script as a browser reports it: at the overlay, retargeted, through its
/// ancestors.
#[test]
fn a_tap_on_an_overlay_outside_its_content_reaches_script_at_the_overlay() {
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
                  const overlay = __CreateElement('x-overlay-ng', 0);
                  __AppendElement(page, outer);
                  __AppendElement(outer, overlay);
                  __AppendElement(overlay, __CreateView(0));
                  __SetAttribute(overlay, 'visible', true);
                  globalThis.held = [page, outer, overlay];
                  const note = (label) => ({
                    type: 'worklet',
                    value: {
                      body: (event) =>
                        seen.push(
                          label + ':' + event.currentTarget.uid + ':' +
                          event.target.uid + ':' + event.type,
                        ),
                    },
                  });
                  __AddEvent(overlay, 'bindEvent', 'tap', note('overlay'));
                  __AddEvent(outer, 'bindEvent', 'tap', note('outer'));
                  __AddEvent(page, 'capture-bind', 'tap', note('page-capture'));
                };
                ",
            "app:///overlay-tap.js",
        )
        .expect("main-thread script");
    runtime
        .evaluate_module(
            &mut js_runtime,
            r"
                import { __FlushElementTree } from 'bobcat:element';
                __FlushElementTree();
                ",
            "app:///overlay-tap-flush.js",
            "flush",
        )
        .expect("flush");
    let (overlay, dialog) = {
        let tree = elements.tree();
        let page = tree.document_element().id();
        let outer = tree.get(page).expect("the page").child_ids()[0];
        let overlay = tree.get(outer).expect("the outer view").child_ids()[0];
        let dialog = overlay_dialog(&tree, overlay);
        assert!(tree.blocks_document(dialog), "shown, modal");
        (overlay, dialog)
    };
    let uid = packed_node_id(overlay);
    runtime
        .dispatch_for_test(&mut js_runtime, dialog, "tap", event_point())
        .expect("a hit on the shadow dialog is delivered");
    expect_seen(
        &mut js_runtime,
        &mut runtime,
        &format!("page-capture:2:{uid}:tap|overlay:{uid}:{uid}:tap|outer:3:{uid}:tap"),
    );
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

/// The painting side already refuses to route a name no listener anywhere
/// has registered, and a dispatch that arrives for one anyway is answered by
/// the host's listener table: nothing hears the name, so no path is built and
/// no realm is called.
#[test]
fn a_dispatch_with_nothing_registered_calls_no_realm() {
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
            .dispatch_for_test(&mut js_runtime, node_id(3), &tap(), event_point())
            .expect("dispatch"),
        "no realm was called"
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

/// The `<dialog>` in an `<overlay>`'s shadow tree: the element `visible`
/// puts in the top layer.
fn overlay_dialog(tree: &LynxDocument, overlay: dom::NodeId) -> dom::NodeId {
    let root = tree
        .shadow_root(overlay)
        .expect("the overlay's shadow root");
    tree.get(root).expect("the shadow root").child_ids()[0]
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
    assert!(
        runtime
            .dispatch_component_events(&mut js_runtime)
            .is_empty()
    );

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
    assert!(
        runtime
            .dispatch_component_events(&mut js_runtime)
            .is_empty()
    );

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
        assert!(
            runtime
                .dispatch_component_events(&mut js_runtime)
                .is_empty()
        );
        // A placeholder is an interim picture, not an answer to the page.
        expect_seen(&mut js_runtime, &mut runtime, "");
    }

    // The source behind it is still the element's own, and its failure is
    // still the element's `error` — once.
    runtime.apply_image_events(&[image_failed(IMAGE_SOURCE), image_failed(IMAGE_SOURCE)]);
    assert!(
        runtime
            .dispatch_component_events(&mut js_runtime)
            .is_empty()
    );
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
        runtime
            .dispatch_component_events(&mut js_runtime)
            .is_empty(),
        "nothing holds the source yet, so the report settles the registry alone"
    );
    expect_seen(&mut js_runtime, &mut runtime, "");

    // The bind settled it, and dispatching from inside `__SetAttribute` would
    // re-enter the realm in the middle of that call.
    js_set_src(&mut js_runtime, &mut runtime, IMAGE_SOURCE);
    expect_seen(&mut js_runtime, &mut runtime, "");

    assert!(
        runtime
            .dispatch_component_events(&mut js_runtime)
            .is_empty()
    );
    expect_seen(
        &mut js_runtime,
        &mut runtime,
        r#"page-capture:2:load:{"width":40,"height":20}|image:4:load:{"width":40,"height":20}"#,
    );

    // Exactly once: the queue was drained, and rewriting the value already
    // there binds nothing.
    assert!(
        runtime
            .dispatch_component_events(&mut js_runtime)
            .is_empty()
    );
    js_set_src(&mut js_runtime, &mut runtime, IMAGE_SOURCE);
    assert!(
        runtime
            .dispatch_component_events(&mut js_runtime)
            .is_empty()
    );
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

    assert!(
        runtime
            .dispatch_component_events(&mut js_runtime)
            .is_empty()
    );
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
/// against the run itself, the one node no handle ever names. A hit on a UA
/// component's shadow chrome is not one: `first_element_at` answers with the
/// flat-tree element it hits, shadow tree included, and the dispatch drops
/// every shadow step, so the event starts at the host
/// (`an_event_at_shadow_content_reaches_script_retargeted_to_its_host`).
/// Generated text has no DOM identity and cannot become such a target.
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
    crate::resource::LoadedSource::Module {
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

/// A synchronous load's URL is resolved against the view's base URL — the
/// detached outbox's `app:///` — by URL rules before it is requested, not
/// against the module that made the load: a rooted and a relative lazy
/// container name reach the fetcher as one absolute URL.
#[test]
fn a_sync_load_resolves_against_the_views_base() {
    let (mut js, mut runtime, _elements, far) = runtime_over_watching_names(ingredients());
    let mut notices = far.0.notices;
    let host = std::thread::spawn(move || {
        for (expected, source) in [
            ("app:///lazy.bundle/x.js", "module.exports = 'x';"),
            ("app:///lazy.bundle/y.js", "module.exports = 'y';"),
        ] {
            let (url, completion) = requested_module(&mut notices);
            assert_eq!(url, expected);
            completion.complete(Ok(module_source(source, &url)));
        }
    });
    runtime
        .evaluate_module(
            &mut js,
            r"
        import { lynx } from 'bobcat:runtime';
        const rooted = lynx.loadScript('x', {bundleName: '/lazy.bundle'});
        const relative = lynx.loadScript('y', {bundleName: 'lazy.bundle'});
        if (rooted !== 'x' || relative !== 'y') throw Error(`${rooted} ${relative}`);
    ",
            "app:///nested/load.js",
            "loading sections through the host",
        )
        .unwrap();
    host.join().unwrap();
}

/// A synchronous load whose URL does not resolve against the view's base
/// throws in the realm, as a failed load does, and is never requested.
#[test]
fn a_sync_load_that_does_not_resolve_throws_without_a_request() {
    let (mut js, mut runtime, _elements, far) = runtime_over_watching_names(ingredients());
    let mut notices = far.0.notices;
    let (finished, mut finish) = tokio::sync::oneshot::channel::<()>();
    // Every source request that reaches this thread is recorded and its
    // completion dropped, which fails that load: a URL that was requested
    // fails the assertion below instead of leaving the realm waiting for an
    // answer. Dropping `finished` ends the loop.
    let host = std::thread::spawn(move || {
        let deadline = ClockInstant::now() + std::time::Duration::from_secs(10);
        let mut requested = Vec::new();
        loop {
            let notice = crate::link::block_on_deadline(
                async {
                    tokio::select! {
                        notice = notices.recv() => notice,
                        _ = &mut finish => None,
                    }
                },
                deadline,
            )
            .flatten();
            match notice {
                Some(ViewNotice::RequestSource { request, .. }) => requested.push(request),
                Some(_) => {}
                None => break requested,
            }
        }
    });
    runtime
        .evaluate_module(
            &mut js,
            r"
        import { lynx } from 'bobcat:runtime';
        let failure;
        try { lynx.loadScript('x', {bundleName: 'http://['}); } catch (error) { failure = error; }
        if (failure?.name !== 'Error' || !failure.message.includes('http://[/x.js'))
            throw Error(`${failure}`);
    ",
            "app:///nested/load.js",
            "loading a section whose URL does not resolve",
        )
        .unwrap();
    drop(finished);
    let requested = host.join().unwrap();
    assert!(
        requested.is_empty(),
        "an unresolvable URL reached the host: {requested:?}"
    );
}

/// A fetch is not resolved by the engine: the fetcher gets the string the
/// realm wrote and resolves it against its own base.
#[test]
fn a_fetch_reaches_the_fetcher_as_the_realm_wrote_it() {
    let (mut js, mut runtime, _elements, mut far) = runtime_over_watching_names(ingredients());
    runtime
        .evaluate_module(
            &mut js,
            r"
        import { lynx } from 'bobcat:runtime';
        globalThis.handle = lynx.fetchBundle('/lazy.bundle');
    ",
            "app:///nested/fetch.js",
            "fetching a container",
        )
        .unwrap();
    let url = loop {
        match far.0.notices.try_recv().expect("the fetch was requested") {
            ViewNotice::RequestSource {
                request: crate::resource::SourceRequest::Fetch { url },
                ..
            } => break url,
            ViewNotice::RequestSource { request, .. } => panic!("expected a fetch: {request:?}"),
            _ => {}
        }
    };
    assert_eq!(url, "/lazy.bundle");
}

/// A name under an engine prefix is answered from the runtime's built-ins and
/// this realm's host modules, and from nothing else. One that is neither
/// fails in the realm with a `ReferenceError`, through an `import` or a
/// `require`, and never reaches the host: `bobcat:worker` and `bobcat:bts` are
/// registered, but each imports `bobcat-internal:worker` — `bobcat:bts`
/// through `bobcat:worker` — which an MTS realm does not declare.
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
            ['bobcat:bts', 'bobcat-internal:worker'],
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

/// The members an MTS realm's `bobcat-internal:host` and
/// `bobcat-internal:native-modules` export, which is what decides the
/// built-ins it can link. Written down so that a change to either set is a
/// change to these lists. A namespace lists its exports sorted by name;
/// `testFuture` is the test build's own producer. The native module member
/// is the one a worker realm has too; the module table is a startup member
/// under `bobcat-internal:host`, which only this realm kind has.
#[test]
fn an_mts_realm_declares_these_host_members() {
    let expected = MTS_HOST_MEMBERS.join(",");
    let native_modules = "invokeNativeModule";
    let (mut js, mut runtime, _elements) = runtime();
    runtime
        .evaluate_module(
            &mut js,
            &format!(
                r"
        const members = Object.keys(await import('bobcat-internal:host')).join(',');
        if (members !== '{expected}') throw Error(members);
        const native = Object.keys(await import('bobcat-internal:native-modules')).join(',');
        if (native !== '{native_modules}') throw Error(native);
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

/// What a `ShadowRealm` child of the MTS realm exports under
/// `bobcat-internal:host`: every member of the page realm's, the three
/// `Worker` members included, and no `bobcat-internal:native-modules` at
/// all.
#[test]
fn a_frame_realm_declares_the_page_members() {
    let expected = MTS_HOST_MEMBERS.join(",");
    let (mut js, mut runtime, _elements) = runtime();
    runtime
        .run_main_thread_script(
            &mut js,
            &format!(
                r"
        const members = await new Promise(resolve => new ShadowRealm().evaluate(`report => {{
            import('bobcat-internal:host').then(H => report(Object.keys(H).join(',')));
        }}`)(resolve));
        if (members !== '{expected}') throw Error(members);
    "
            ),
            "app:///frame-members.js",
        )
        .unwrap();
}

/// The page realm's `bobcat-internal:host` exports, sorted as a namespace
/// lists them.
const MTS_HOST_MEMBERS: [&str; 51] = [
    "adoptStyleSheet",
    "attributeNames",
    "callElementMethod",
    "childElementIds",
    "clearTimer",
    "closeRealm",
    "createDocument",
    "createElement",
    "createPage",
    "createWorker",
    "dropElement",
    "fetchResource",
    "flushElementTree",
    "framePage",
    "getAttribute",
    "getComputedStyleMap",
    "globalProps",
    "hasStyleSheet",
    "initData",
    "initialProcessor",
    "insertBefore",
    "listenerClosed",
    "listenerOpened",
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
    "resetFrame",
    "resolveModuleUrl",
    "resolveUrl",
    "sendWorkerMessage",
    "setAttribute",
    "setInlineStyleProperty",
    "setInlineStyles",
    "setTimer",
    "settleFuture",
    "supportsStyleProperty",
    "swapElement",
    "switchExposure",
    "tagName",
    "takeFuture",
    "terminateWorker",
    "testFuture",
    "waitFuture",
];

/// The card's one element, the page element's first child: the element
/// every exposure test below registers.
fn first_card_element(elements: &DocumentProbe) -> dom::NodeId {
    elements
        .tree()
        .document_element()
        .first_child()
        .expect("the card rendered its element")
        .id()
}

/// The observer the realm's exposure registration of `node` owns, if it is
/// registered and detection runs.
fn exposure_observer(
    elements: &DocumentProbe,
    node: dom::NodeId,
) -> Option<dom::IntersectionObserverId> {
    elements.slot.borrow().exposure.observer_of(node)
}

/// `rpx` in an exposure margin is the viewport width over 750 — stylo's
/// rule — read from the document's viewport when the element registers: a
/// `20rpx` top screen margin on this realm's 393px-wide viewport is
/// `20 × 393 / 750` px of target margin, on the bottom, the side a positive
/// screen margin lands on. Written through `__SetAttribute`,
/// which reaches the registration through the host's `setAttribute`.
#[test]
fn an_rpx_exposure_margin_resolves_against_the_viewport_width() {
    let (mut js, mut runtime, elements) = runtime();
    runtime
        .run_main_thread_script(
            &mut js,
            r"
        globalThis.renderPage = function () {
          const view = __CreateView(0);
          __AppendElement(__CreatePage('card', 0), view);
          __SetAttribute(view, 'exposure-screen-margin-top', '20rpx');
          __SetAttribute(view, 'exposure-id', 'card');
          globalThis.view = view;
        };
        ",
            "app:///exposure-rpx.js",
        )
        .expect("main-thread script");
    let view = first_card_element(&elements);
    let observer =
        exposure_observer(&elements, view).expect("an `exposure-id` registers the element");
    assert_eq!(
        elements
            .tree()
            .intersection_observer(observer)
            .target_margin()
            .bottom,
        dom::MarginLength::Px(20.0 * 393.0 / 750.0),
    );

    // Removing the id through `__SetAttribute`'s `null` is the host's
    // `removeAttribute`, which lets the registration go.
    runtime
        .evaluate_module(
            &mut js,
            r"
        import { __SetAttribute } from 'bobcat:element';
        __SetAttribute(globalThis.view, 'exposure-id', null);
        ",
            "app:///exposure-remove.js",
            "removing the exposure id",
        )
        .expect("removing the attribute");
    assert_eq!(exposure_observer(&elements, view), None);
}

/// The members exposure follows, from script: `listenerOpened` for
/// `uiappear` registers an element that has no `exposure-id` and
/// `listenerClosed` lets it go again — the listener table is what exposure
/// reads — `switchExposure` stops and resumes detection, and each refuses an
/// argument that is not its shape: a flag is `0` or `1`, as every tree
/// member's is, and kinds are 1 to 7.
#[test]
fn the_listener_members_register_exposure_and_switch_exposure_switches_detection() {
    let (mut js, mut runtime, elements) = runtime();
    runtime
        .run_main_thread_script(
            &mut js,
            r"
        globalThis.renderPage = function () {
          globalThis.view = __CreateView(0);
          __AppendElement(__CreatePage('card', 0), globalThis.view);
        };
        ",
            "app:///exposure-members.js",
        )
        .expect("main-thread script");
    let view = first_card_element(&elements);
    let mut run = |source: &str| {
        runtime
            .evaluate_module(
                &mut js,
                &format!(
                    r"
            import {{ listenerClosed, listenerOpened, switchExposure }} from 'bobcat-internal:host';
            import {{ __GetElementUniqueID }} from 'bobcat:element';
            const view = __GetElementUniqueID(globalThis.view);
            {source}
            "
                ),
                "app:///exposure-call.js",
                "calling an exposure member",
            )
            .expect("the exposure member call");
    };
    assert_eq!(exposure_observer(&elements, view), None);

    run("listenerOpened(view, 'uiappear', 1);");
    let first = exposure_observer(&elements, view).expect("a listener registers the element");
    run("listenerOpened(view, 'uiappear', 5); listenerOpened(view, 'uidisappear', 2);");
    assert_eq!(
        exposure_observer(&elements, view),
        Some(first),
        "new kinds and the other name are no change of whether the element listens",
    );
    run("listenerClosed(view, 'uidisappear');");
    run("switchExposure(0, 1);");
    assert_eq!(
        exposure_observer(&elements, view),
        None,
        "stopping drops every observer"
    );
    run("switchExposure(1, 1);");
    let resumed = exposure_observer(&elements, view).expect("resuming observes it again");
    assert_ne!(resumed, first, "with an observer of its own");
    run("listenerClosed(view, 'uiappear');");
    assert_eq!(exposure_observer(&elements, view), None);

    run(r"
        for (const [call, expected] of [
          [() => listenerOpened(view, 'uiappear', 0), 'listenerOpened expects the kinds 1 to 7 for argument 2'],
          [() => listenerOpened(view, 'uiappear', 8), 'listenerOpened expects the kinds 1 to 7 for argument 2'],
          [() => switchExposure(1, 2), 'switchExposure expects 0 or 1 for argument 1'],
          [() => listenerOpened(-1, 'uiappear', 1), 'listenerOpened'],
          [() => listenerClosed(view, 3), 'listenerClosed'],
        ]) {
          let thrown;
          try { call(); } catch (error) { thrown = String(error?.message ?? error); }
          if (thrown === undefined || !thrown.includes(expected)) {
            throw Error(`expected a throw naming ${expected}, got ${thrown}`);
          }
        }
    ");
}

/// An element is registered for exposure out of the listener table: the
/// first `uiappear`/`uidisappear` handler `__AddEvent` files arms an observer
/// for an element that has no `exposure-id`; a second handler, of the other
/// name or the other kind, changes nothing — the observer is the one the
/// first armed, where a second arming would have made a fresh one — and
/// web-core's disable mark, a `null` handler, taking the last one away lets
/// the registration go.
#[test]
fn an_add_event_for_uiappear_arms_and_disarms_exposure() {
    let (mut js, mut runtime, elements) = runtime();
    runtime
        .run_main_thread_script(
            &mut js,
            r"
        globalThis.renderPage = function () {
          globalThis.view = __CreateView(0);
          __AppendElement(__CreatePage('card', 0), globalThis.view);
        };
        ",
            "app:///exposure-add-event.js",
        )
        .expect("main-thread script");
    let view = first_card_element(&elements);
    let mut run = |source: &str| {
        runtime
            .evaluate_module(
                &mut js,
                &format!(
                    r"
            import {{ __AddEvent }} from 'bobcat:element';
            const view = globalThis.view;
            {source}
            "
                ),
                "app:///exposure-add-event-call.js",
                "filing an exposure handler",
            )
            .expect("the handler call");
    };
    assert_eq!(exposure_observer(&elements, view), None);

    run("__AddEvent(view, 'bindEvent', 'uiappear', '3:0:appear');");
    let armed =
        exposure_observer(&elements, view).expect("a `uiappear` handler registers the element");
    run(r"
        __AddEvent(view, 'bindEvent', 'uidisappear', '3:0:disappear');
        __AddEvent(view, 'bindEvent', 'uiappear', { type: 'worklet', value: {} });
        __AddEvent(view, 'bindEvent', 'uiappear', null);
    ");
    assert_eq!(
        exposure_observer(&elements, view),
        Some(armed),
        "neither a second handler nor removing one of two changes whether the element listens",
    );
    run("__AddEvent(view, 'bindEvent', 'uidisappear', undefined);");
    assert_eq!(
        exposure_observer(&elements, view),
        None,
        "the last handler went, and the registration with it",
    );

    // A worklet alone, `main-thread:binduiappear`, registers it as well.
    run("__AddEvent(view, 'bindEvent', 'uiappear', { type: 'worklet', value: {} });");
    assert!(exposure_observer(&elements, view).is_some());
}

/// A `uiappear` reaches an `__AddEventListener` closure through the
/// intersection delivery, non-bubbling, with web-core's detail: the target's
/// unique id, and its `exposure-id` and `exposure-scene` under both of
/// web-core's spellings. The closure is itself what registers the element,
/// as any registration form does. The element carries an `exposure-id` too,
/// so the same delivery sends the `exposure` record list, which `bobcat:runtime`
/// now takes — the delivery reports no failure.
#[test]
fn a_uiappear_listener_hears_web_cores_exposure_detail() {
    let (mut js, mut runtime, elements) = runtime();
    runtime
        .run_main_thread_script(
            &mut js,
            r"
        globalThis.renderPage = function () {
          const page = __CreatePage('card', 0);
          const view = __CreateView(0);
          __SetInlineStyles(view, 'width:100px;height:100px');
          __AppendElement(page, view);
          __SetAttribute(view, 'exposure-id', 'card-7');
          __SetAttribute(view, 'exposure-scene', 'feed');
          globalThis.heard = [];
          for (const element of [view, page]) {
            __AddEventListener(element, 'uiappear', (event) => {
              heard.push([event.type, __GetElementUniqueID(element), event.detail]);
            }, {});
          }
          globalThis.view = view;
        };
        ",
            "app:///exposure-detail.js",
        )
        .expect("main-thread script");
    let view = first_card_element(&elements);
    assert!(exposure_observer(&elements, view).is_some());

    assert!(
        runtime.update_intersection_observations(),
        "the first update after an observe queues the element",
    );
    let failures = runtime.notify_intersection_observers(&mut js);
    assert!(
        failures.is_empty(),
        "{:?}",
        failures
            .into_iter()
            .map(|failure| failure.into_script_error().message)
            .collect::<Vec<_>>()
    );
    runtime
        .evaluate_module(
            &mut js,
            r"
        import { __GetElementUniqueID, __GetPageElement } from 'bobcat:element';
        const uid = __GetElementUniqueID(globalThis.view);
        const detail = {
          'unique-id': uid,
          exposureID: 'card-7',
          exposureScene: 'feed',
          'exposure-id': 'card-7',
          'exposure-scene': 'feed',
        };
        // The page is visible too and registered by its own closure, but it
        // has no `exposure-id`: it hears its own `uiappear`, never the view's,
        // because the event does not bubble.
        const page = __GetElementUniqueID(__GetPageElement());
        const pageDetail = {
          'unique-id': page,
          exposureID: null,
          exposureScene: '',
          'exposure-id': null,
          'exposure-scene': '',
        };
        const expected = JSON.stringify([
          ['uiappear', page, pageDetail],
          ['uiappear', uid, detail],
        ]);
        const sorted = globalThis.heard.slice().sort((a, b) => a[1] - b[1]);
        if (JSON.stringify(sorted) !== expected) {
          throw Error(JSON.stringify(globalThis.heard));
        }
        ",
            "app:///exposure-detail-check.js",
            "reading what the listeners heard",
        )
        .expect("the listeners heard web-core's detail");
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
        assert_eq!(url_a, "app:///main.js/custom_section/CSS.css");
        assert_eq!(url_b, "https://cdn.test/bundle/custom_section/CSS.css");
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
        assert_eq!(
            preload_url(&mut far),
            "app:///main.js/custom_section/CSS.css"
        );
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
                if (!String(error).includes('app:///main.js/custom_section/CSS.css')) throw error;
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
    assert_eq!(
        preload_url(&mut far),
        "app:///main.js/custom_section/CSS.css"
    );
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

/// A stylesheet the view's fetcher does not hold is `null`, asked of the
/// fetcher's probe with the composed URL and nothing else: no preload hint,
/// no request. A held one is still the `{url}` handle, preloaded. What a card
/// such as `rLynxPrepareLazyBundleMTS` does with `null` is adopt nothing,
/// which is what native's `undefined` for a missing section makes it do.
#[test]
fn load_style_sheet_answers_null_for_a_sheet_the_fetcher_does_not_hold() {
    let asked = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let probe: crate::resource::StyleSheetProbe = {
        let asked = Arc::clone(&asked);
        Arc::new(move |url: &str| {
            asked.lock().unwrap().push(url.to_owned());
            url.starts_with("https://cdn.test/with-css.bundle/")
        })
    };
    let (mut js, mut runtime, _elements, mut far) =
        runtime_over_watching_names_probing(ingredients(), Some(probe));
    runtime
        .run_main_thread_script(
            &mut js,
            r"
            const missing = __LoadStyleSheet('CSS', 'https://cdn.test/no-css.bundle');
            if (missing !== null) throw Error('a missing sheet is null, got ' + JSON.stringify(missing));
            const held = __LoadStyleSheet('CSS', 'https://cdn.test/with-css.bundle');
            if (held === null || held.url !== 'https://cdn.test/with-css.bundle/custom_section/CSS.css')
                throw Error('a held sheet is its handle, got ' + JSON.stringify(held));
        ",
            "app:///main.js",
        )
        .unwrap();
    assert_eq!(
        *asked.lock().unwrap(),
        [
            "https://cdn.test/no-css.bundle/custom_section/CSS.css",
            "https://cdn.test/with-css.bundle/custom_section/CSS.css",
        ]
    );
    // Only the held sheet was hinted, and nothing was requested at all.
    let mut preloads = Vec::new();
    while let Ok(notice) = far.0.notices.try_recv() {
        match notice {
            ViewNotice::PreloadSource(crate::resource::SourceRequest::StyleSheet(url)) => {
                preloads.push(url);
            }
            ViewNotice::RequestSource { request, .. } => {
                panic!("no stylesheet may be requested: {request:?}")
            }
            _ => {}
        }
    }
    assert_eq!(
        preloads,
        ["https://cdn.test/with-css.bundle/custom_section/CSS.css"]
    );
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
                crate::EngineEvent::ScriptRunError(error) | crate::EngineEvent::Panicked(error) => {
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

/// `ShadowRealm` is a global of the MTS realm, like the W3C `Worker` a
/// bucket-1 capability: a child evaluates in a global of its own, answers
/// primitives, refuses objects with a `TypeError`, and wraps functions both
/// ways.
#[test]
fn the_main_thread_realm_has_shadow_realms() {
    let (mut js, mut runtime, _elements) = runtime();
    runtime
        .run_main_thread_script(
            &mut js,
            r"
        if (typeof ShadowRealm !== 'function') throw Error('no ShadowRealm: ' + typeof ShadowRealm);
        const realm = new ShadowRealm();
        if (realm.evaluate('1 + 1') !== 2) throw Error('evaluate');
        globalThis.marker = 'parent';
        realm.evaluate('globalThis.marker = \'child\'');
        if (globalThis.marker !== 'parent') throw Error('the child shares the parent global');
        let refused;
        try { realm.evaluate('({})'); } catch (error) { refused = error; }
        if (!(refused instanceof TypeError)) throw Error('an object crossed: ' + refused);
        if (realm.evaluate('f => f(21)')(x => x * 2) !== 42) throw Error('wrapped callback');
        if (typeof realm.evaluate('typeof ShadowRealm') !== 'string'
            || realm.evaluate('typeof ShadowRealm') !== 'function') throw Error('no nested ShadowRealm');
    ",
            "app:///shadow-realm.js",
        )
        .unwrap();
    assert_eq!(runtime.child_realms(), 1);
}

/// A function the page realm's tests define to make a `ShadowRealm` child
/// with `bobcat:element` and `bobcat-internal:host` loaded in it, as `E` and
/// `H` on the child's own global: what `bobcat:frame-boot` imports, reached
/// here through `evaluate`.
///
/// `bobcat:runtime` first, as the boot module imports it: it and
/// `bobcat:element` import each other, and the order is what lets
/// `bobcat:element` evaluate ahead of the module that reads its classes.
const FRAME_REALM: &str = r"
async function frameRealm() {
    const realm = new ShadowRealm();
    await new Promise((resolve, reject) => realm.evaluate(`(resolve, reject) => {
        import('bobcat:runtime').then(() => Promise.all([
            import('bobcat:element'),
            import('bobcat-internal:host'),
        ])).then(
            ([element, host]) => { globalThis.E = element; globalThis.H = host; resolve(); },
            error => reject(String(error && error.message || error)),
        );
    }`)(resolve, reject));
    return realm;
}
";

/// A child of the MTS realm has the core members and the document members
/// the page realm has, over the same document and as the same code:
/// `bobcat:timers` and `bobcat:element` both link in it, and its
/// `createPage` is the document's `page`, as the page realm's is. It has
/// the three `Worker` members too, over a side of its own, so
/// `bobcat-internal` links in it. Its startup strings answer `undefined`;
/// `bobcat-internal:native-modules` is the page realm's alone.
#[test]
fn a_shadow_realm_links_the_document_built_ins() {
    let (mut js, mut runtime, _elements) = runtime();
    runtime
        .run_main_thread_script(
            &mut js,
            &format!(
                r"{FRAME_REALM}
        import {{ createPage }} from 'bobcat-internal:host';
        const realm = await frameRealm();
        const run = await realm.importValue('bobcat:timers', '__BobcatRunTimer');
        if (typeof run !== 'function') throw Error('bobcat:timers did not link: ' + typeof run);
        if (realm.evaluate('typeof setTimeout') !== 'function') throw Error('no child setTimeout');
        if (realm.evaluate('H.createPage()') !== createPage()) throw Error('two createPages');
        for (const name of ['initData', 'globalProps', 'initialProcessor', 'nativeModuleTable']) {{
            if (realm.evaluate(`H.${{name}}()`) !== undefined) throw Error(name + ' answered');
        }}
        for (const name of ['createWorker', 'sendWorkerMessage', 'terminateWorker', 'resolveUrl']) {{
            if (realm.evaluate(`typeof H.${{name}}`) !== 'function') throw Error(name + ' missing');
        }}
        const worker = await realm.importValue('bobcat-internal', '__BobcatDispatchWorkerEvent');
        if (typeof worker !== 'function') throw Error('bobcat-internal did not link: ' + typeof worker);
        let failed;
        try {{
            await realm.importValue('bobcat:native-modules', 'callNativeModule');
        }} catch (error) {{
            failed = error;
        }}
        if (!(failed instanceof TypeError)) throw Error('native modules linked: ' + failed);
    "
            ),
            "app:///shadow-realm-imports.js",
        )
        .unwrap();
}

/// The page realm cannot close itself, and its `createPage` is the
/// document's `page`, as before frames existed.
#[test]
fn the_page_realm_cannot_close_itself() {
    let (mut js, mut runtime, elements) = runtime();
    runtime
        .run_main_thread_script(
            &mut js,
            r"
        import { closeRealm, createPage } from 'bobcat-internal:host';
        let refused;
        try { closeRealm(); } catch (error) { refused = error.message; }
        if (refused !== 'the page realm cannot close itself') throw Error(refused);
        if (__GetElementUniqueID(__CreatePage()) !== createPage()) throw Error('two pages');
        globalThis.page = createPage();
    ",
            "app:///page-closes-nothing.js",
        )
        .unwrap();
    assert_eq!(elements.tree().document_element().id(), node_id(2));
}

/// The first element child of the document's page: the frame the page
/// realm's script appended first.
fn first_frame(tree: &LynxDocument) -> dom::NodeId {
    tree.document_element()
        .children()
        .find(|child| child.tag_name() == Some("frame"))
        .map(dom::Node::id)
        .expect("the page holds a frame")
}

/// The nested page the `frame` component built in `frame`'s shadow root.
fn frame_page_of(tree: &LynxDocument, frame: dom::NodeId) -> dom::NodeId {
    let page = nested_page(tree, frame).expect("a frame holds its nested page");
    let node = tree.get(page).expect("live");
    assert_eq!(node.tag_name(), Some("page"));
    assert_eq!(node.attribute("part"), Some("page"));
    page
}

/// A child handed a frame's nested page renders into it: `framePage` reads
/// the page the component built, `__BobcatUsePageRoot` makes it what the
/// child's `__CreatePage` answers, what the child builds under it lands in
/// the frame's box, and the child's flush — the page realm's flush, the same
/// code — commits it.
#[test]
fn a_child_renders_under_the_nested_page_it_was_handed() {
    let (mut js, mut runtime, elements) = runtime();
    runtime
        .run_main_thread_script(
            &mut js,
            &format!(
                r"{FRAME_REALM}
        const page = __CreatePage();
        const spacer = __CreateView(0);
        __SetInlineStyles(spacer, 'height:20px');
        __AppendElement(page, spacer);
        const frame = __CreateElement('frame', 0);
        __SetInlineStyles(frame, 'width:100px;height:50px');
        __AppendElement(page, frame);
        __FlushElementTree();
        globalThis.realm = await frameRealm();
        const nested = realm.evaluate(`frame => {{
            const page = H.framePage(frame);
            if (H.framePage(frame) !== page) throw Error('framePage moved');
            E.__BobcatUsePageRoot(page);
            const own = E.__CreatePage();
            if (E.__GetElementUniqueID(own) !== page) throw Error('createPage is not the nested page');
            if (E.__CreatePage() !== own) throw Error('two page handles');
            let again;
            try {{ E.__BobcatUsePageRoot(page); }} catch (error) {{ again = error.message; }}
            if (!again.includes('already chosen')) throw Error('chosen twice: ' + again);
            const text = E.__CreateText(0);
            E.__AppendElement(text, E.__CreateRawText('hi'));
            E.__AppendElement(own, text);
            E.__FlushElementTree();
            return page;
        }}`)(__GetElementUniqueID(frame));
        if (typeof nested !== 'number') throw Error('framePage answered ' + nested);
    "
            ),
            "app:///frame-page.js",
        )
        .unwrap();
    let tree = elements.tree();
    let frame = first_frame(&tree);
    let page = frame_page_of(&tree, frame);
    let text = tree
        .get(page)
        .and_then(|page| page.children().find(|child| child.is_element()))
        .map(dom::Node::id)
        .expect("the child's text is under the nested page");
    assert_eq!(tree.get(text).and_then(dom::Node::tag_name), Some("text"));

    let frame_box = tree
        .bounding_client_rect(frame)
        .expect("the frame has a box");
    assert_eq!(
        (
            frame_box.origin.y,
            frame_box.size.width,
            frame_box.size.height
        ),
        (20.0, 100.0, 50.0)
    );
    // The UA sheet's `page` rule fills the frame, as web-core's
    // `[part="page"]` does.
    assert_eq!(
        tree.bounding_client_rect(page).expect("laid out"),
        frame_box,
        "the nested page fills the frame"
    );
    let text_box = tree.bounding_client_rect(text).expect("the text has a box");
    assert!(
        frame_box.contains_rect(&text_box) && text_box.size.height > 0.0,
        "the child's text lays out inside the frame: {text_box:?} in {frame_box:?}"
    );
    assert!(
        !tree.needs_render(),
        "the child's flush committed what it built"
    );
}

/// Every refusal `framePage` makes is a JavaScript exception and the view
/// goes on: a node that is not a frame, an id that names nothing any more,
/// and one that never named anything. It is a member of every realm.
#[test]
fn frame_page_refuses_what_is_no_frame() {
    let (mut js, mut runtime, _elements) = runtime();
    runtime
        .run_main_thread_script(
            &mut js,
            r"
        import { createElement, dropElement, framePage } from 'bobcat-internal:host';
        const view = __CreateView(0);
        __AppendElement(__CreatePage(), view);
        const gone = createElement('frame');
        dropElement(gone);
        const outcomes = [__GetElementUniqueID(view), gone, 2 ** 50].map(id => {
            try { return String(framePage(id)); } catch (error) { return 'refused: ' + error.message; }
        });
        const expected = ['requires a frame element', 'stale element id', 'is no element id'];
        expected.forEach((message, index) => {
            if (!outcomes[index].startsWith('refused: ') || !outcomes[index].includes(message))
                throw Error(index + ': ' + outcomes[index]);
        });
        // A detached frame has its nested page already.
        if (!(framePage(createElement('frame')) > 0)) throw Error('a detached frame has no page');
    ",
            "app:///frame-page-refusals.js",
        )
        .unwrap();
}

/// Frames nest — each one in the nested page of the one before — and
/// `framePage` answers for the eighth frame deep and refuses the ninth, so
/// a card that embeds itself stops there. Nothing about it is a child's:
/// the page realm builds the whole chain.
#[test]
fn frame_page_refuses_past_eight_frames_deep() {
    let (mut js, mut runtime, _elements) = runtime();
    runtime
        .run_main_thread_script(
            &mut js,
            r"
        import { createElement, framePage, insertBefore } from 'bobcat-internal:host';
        const frames = [__CreateElement('frame', 0)];
        __AppendElement(__CreatePage(), frames[0]);
        let parent = framePage(__GetElementUniqueID(frames[0]));
        const ids = [__GetElementUniqueID(frames[0])];
        for (let depth = 2; depth <= 9; ++depth) {
            const frame = createElement('frame');
            insertBefore(parent, frame, undefined);
            ids.push(frame);
            if (depth < 9) parent = framePage(frame);
        }
        let refused;
        try { framePage(ids[8]); } catch (error) { refused = error.message; }
        if (!refused || !refused.includes('frames nest at most 8 deep')) throw Error(refused);
    ",
            "app:///nested-frames.js",
        )
        .unwrap();
}

/// A child's adoption names the frame's shadow root, and mounts there: a
/// `.box` inside the frame takes the sheet, and the host's own `.box` does
/// not. A target that is no shadow root is refused before anything is
/// requested. `resetFrame` then frees what the child built under the nested
/// page and removes the sheet, and the nested page stays.
#[test]
#[expect(clippy::float_cmp, reason = "rounded widths are exact CSS pixels")]
fn a_frames_sheet_styles_its_shadow_tree_alone_until_the_frame_is_reset() {
    for text in [true, false] {
        let (mut js, mut runtime, elements, far) = runtime_over_watching_names(ingredients());
        let mut notices = far.0.notices;
        let host = std::thread::spawn(move || {
            let (url, completion) = requested_stylesheet(&mut notices);
            assert_eq!(url, "app:///frame.css");
            completion.complete(Ok(sheet_source(text, "40px")));
        });
        runtime
            .run_main_thread_script(
                &mut js,
                &format!(
                    r"{FRAME_REALM}
            const page = __CreatePage();
            const box = __CreateView(0);
            __SetClasses(box, 'box');
            __SetInlineStyles(box, 'height:10px');
            __AppendElement(page, box);
            const frame = __CreateElement('frame', 0);
            __SetInlineStyles(frame, 'width:100px;height:50px');
            __AppendElement(page, frame);
            __FlushElementTree();
            globalThis.frame = frame;
            const realm = await frameRealm();
            globalThis.realm = realm;
            realm.evaluate(`frame => {{
                const page = H.framePage(frame);
                E.__BobcatUsePageRoot(page);
                let refused;
                try {{ H.adoptStyleSheet('app:///frame.css', page); }}
                catch (error) {{ refused = error.message; }}
                if (!refused.includes('is not a live shadow root')) throw Error(refused);
                const inner = E.__CreateView(0);
                E.__SetClasses(inner, 'box');
                E.__SetInlineStyles(inner, 'height:10px');
                E.__AppendElement(E.__CreatePage(), inner);
                H.adoptStyleSheet('app:///frame.css', H.parentNode(page));
                E.__FlushElementTree();
            }}`)(__GetElementUniqueID(frame));
        "
                ),
                "app:///frame-sheet.js",
            )
            .unwrap();
        host.join().unwrap();
        let (page, inner) = {
            let tree = elements.tree();
            let frame = first_frame(&tree);
            let page = frame_page_of(&tree, frame);
            let inner = tree.get(page).unwrap().child_ids()[0];
            let outer = node_id(3);
            assert_eq!(
                tree.rounded_layout(inner).unwrap().size.width,
                40.0,
                "text {text}"
            );
            assert_eq!(
                tree.rounded_layout(outer).unwrap().size.width,
                393.0,
                "the host's .box is outside the frame's sheet: text {text}"
            );
            (page, inner)
        };

        runtime
            .evaluate_module(
                &mut js,
                r"
                import { __FlushElementTree, __GetElementUniqueID } from 'bobcat:element';
                import { createElement, insertBefore, resetFrame, setAttribute } from 'bobcat-internal:host';
                // In the order `bobcat:frame` lets go of a card: the child
                // first, so its handles' drops are silent, then the frame.
                globalThis.realm.evaluate('H.closeRealm()');
                resetFrame(__GetElementUniqueID(globalThis.frame));
                // A box the next card would build, through the host alone.
                const next = createElement('view');
                setAttribute(next, 'class', 'box');
                setAttribute(next, 'style', 'height:10px');
                insertBefore(H_PAGE, next, undefined);
                globalThis.next = next;
                __FlushElementTree();
            "
                .replace("H_PAGE", &packed_node_id(page).to_string())
                .as_str(),
                "app:///reset-frame.js",
                "resetting the frame",
            )
            .unwrap();
        let tree = elements.tree();
        assert!(tree.get(inner).is_none(), "the child's box was freed");
        assert_eq!(
            frame_page_of(&tree, first_frame(&tree)),
            page,
            "the nested page stays"
        );
        let next = tree.get(page).unwrap().child_ids()[0];
        assert_eq!(
            tree.rounded_layout(next).unwrap().size.width,
            100.0,
            "the frame's sheet is gone with the card: text {text}"
        );
    }
}

/// `closeRealm` retires the child that calls it, which the owner's release
/// step frees though the page still holds its `ShadowRealm`; until then,
/// the child's `dropElement` — what its handles' finalizers call — is
/// silent, even for an id that names nothing.
#[test]
fn close_realm_retires_the_child_and_silences_its_drops() {
    let (mut js, mut runtime, _elements) = runtime();
    runtime
        .run_main_thread_script(
            &mut js,
            &format!(
                r"{FRAME_REALM}
        globalThis.realm = await frameRealm();
        const before = realm.evaluate(`(() => {{
            try {{ H.dropElement(2 ** 50); return 'dropped'; }} catch (error) {{ return error.message; }}
        }})()`);
        if (!before.includes('is no element id')) throw Error(before);
        realm.evaluate('H.closeRealm()');
        const after = realm.evaluate(`(() => {{
            try {{ return String(H.dropElement(2 ** 50)); }} catch (error) {{ return error.message; }}
        }})()`);
        if (after !== 'undefined') throw Error(after);
    "
            ),
            "app:///close-realm.js",
        )
        .unwrap();
    assert_eq!(runtime.core().children.release_unneeded(), 1);
    assert_eq!(runtime.child_realms(), 0);
}

/// A frame whose handle the page realm lets go of is dropped like any
/// element, and its shadow tree — the nested page and everything under it
/// — goes with it. Nothing frame-specific runs in the host.
#[test]
fn dropping_a_frame_frees_its_nested_page() {
    let (mut js, mut runtime, elements) = runtime();
    runtime
        .run_main_thread_script(
            &mut js,
            r"
        import { createElement, framePage, insertBefore } from 'bobcat-internal:host';
        const page = __CreatePage();
        globalThis.frame = __CreateElement('frame', 0);
        __AppendElement(page, frame);
        insertBefore(framePage(__GetElementUniqueID(frame)), createElement('view'), undefined);
        __RemoveElement(page, frame);
    ",
            "app:///dropped-frame.js",
        )
        .unwrap();
    let (nested, inner) = {
        let tree = elements.tree();
        let page = frame_page_of(&tree, node_id(3));
        (page, tree.get(page).unwrap().child_ids()[0])
    };
    // Let go of the handle only now: a collection the engine starts on its
    // own may otherwise free it before the probe above.
    runtime
        .evaluate_module(
            &mut js,
            "globalThis.frame = undefined;",
            "app:///forget-frame.js",
            "letting go of the frame",
        )
        .unwrap();
    runtime.collect_garbage(&mut js).unwrap();
    let tree = elements.tree();
    assert!(tree.get(node_id(3)).is_none(), "the frame was dropped");
    assert!(tree.get(nested).is_none(), "and its nested page with it");
    assert!(tree.get(inner).is_none(), "and what was under the page");
}

/// The published names are the union of every realm's: a child that adds
/// and removes `tap`, which the page also holds, leaves it published, and a
/// name only a child held is unpublished when the child closes itself — its
/// `closeRealm` takes its rows out of the listener table — not later, when
/// the release step frees it.
#[test]
fn a_frame_realm_closes_only_its_own_listener_names() {
    let (mut js, mut runtime, _elements, mut names) = runtime_over_watching_names(ingredients());
    runtime
        .run_main_thread_script(
            &mut js,
            &format!(
                r"{FRAME_REALM}
        const page = __CreatePage();
        const view = __CreateView(0);
        __AppendElement(page, view);
        __AddEvent(view, 'bindEvent', 'tap', '3:0:bindtap');
        const frame = __CreateElement('frame', 0);
        __AppendElement(page, frame);
        globalThis.realm = await frameRealm();
        realm.evaluate(`frame => {{
            E.__BobcatUsePageRoot(H.framePage(frame));
            const inner = E.__CreateView(0);
            E.__AppendElement(E.__CreatePage(), inner);
            E.__AddEvent(inner, 'bindEvent', 'tap', 'child');
            E.__AddEvent(inner, 'bindEvent', 'tap', null);
            E.__AddEvent(inner, 'bindEvent', 'scroll', 'child');
        }}`)(__GetElementUniqueID(frame));
    "
            ),
            "app:///frame-names.js",
        )
        .unwrap();
    assert!(names.contains("tap"), "the page's tap stays published");
    assert!(names.contains("scroll"), "the child's scroll is published");
    runtime
        .evaluate_module(
            &mut js,
            "globalThis.realm.evaluate('H.closeRealm()');",
            "app:///close-names.js",
            "closing the frame's realm",
        )
        .unwrap();
    assert!(
        !names.contains("scroll"),
        "a closed child's names are unpublished with it, before the release step"
    );
    assert!(names.contains("tap"));
    assert_eq!(runtime.core().children.release_unneeded(), 1);
    assert!(!names.contains("scroll"));
    assert!(
        names.contains("tap"),
        "freeing the child closes nothing more"
    );
}

// ---------------------------------------------------------------------------
// The kernel's walk calls each listening realm once, at the path's top.
// ---------------------------------------------------------------------------

/// The page realm's part of a dispatch as the one call before the listener
/// table encoded it: every capture step read backwards, less every step
/// inside a shadow tree.
fn whole_path_encoding(
    document: &LynxDocument,
    steps: &dom::event::EventSteps,
) -> (String, String) {
    let mut nodes = String::new();
    let mut targets = String::new();
    for step in steps
        .steps()
        .iter()
        .rev()
        .filter(|step| step.capture && document.shadow_root_of(step.node).is_none())
    {
        if !nodes.is_empty() {
            nodes.push(',');
            targets.push(',');
        }
        write!(nodes, "{}", packed_node_id(step.node)).unwrap();
        write!(targets, "{}", packed_node_id(step.target)).unwrap();
    }
    (nodes, targets)
}

/// The one encoding every realm is handed is the page realm's call as it
/// always was, for a target in the document tree and one in a detached
/// tree: the whole path, called at its first step. For a target in a UA
/// shadow tree it carries the shadow content's steps beside those — every
/// step but the shadow root — which the page realm holds no handle for and
/// skips, so the steps it runs are the old ones. A non-composed event inside
/// a shadow tree, whose path is the shadow tree's alone, invites no realm
/// that holds nothing there.
#[test]
fn the_page_realms_call_carries_exactly_the_path_it_always_did() {
    let page_realm = RealmId::Page.listener_key();
    let (mut js, mut runtime, elements) = runtime();
    runtime
        .run_main_thread_script(&mut js, "__CreatePage();", "app:///path-encoding.js")
        .unwrap();
    let (inner, shadowed, detached) = {
        let mut document = elements.tree();
        let page = document.document_element().id();
        let outer = document.create_element("view", ());
        let inner = document.create_element("view", ());
        document.append_child(page, outer);
        document.append_child(outer, inner);
        let host = document.create_element("view", ());
        document.append_child(inner, host);
        let shadow = document.attach_shadow(host, dom::ShadowRootMode::Open);
        let shadowed = document.create_element("view", ());
        document.append_child(shadow, shadowed);
        let loose = document.create_element("view", ());
        let detached = document.create_element("view", ());
        document.append_child(loose, detached);
        for node in [outer, loose] {
            document.add_listener(node, "tap", ListenerKinds::PATH_CAPTURE, page_realm);
        }
        (inner, shadowed, detached)
    };
    let slot = elements.slot.borrow();
    let document = slot.document.as_ref().unwrap();
    let encoded = |steps: &dom::event::EventSteps| {
        let path = encode_path(document, steps);
        (path.nodes, path.targets)
    };
    for target in [inner, detached] {
        let steps = document.event_steps(target, true, true);
        let calls = realm_calls(document, &steps, "tap", std::iter::once(page_realm));
        assert_eq!(calls.as_slice(), [page_realm], "{target:?}");
        assert_eq!(
            encoded(&steps),
            whole_path_encoding(document, &steps),
            "{target:?}"
        );
    }
    let steps = document.event_steps(shadowed, true, true);
    assert_eq!(
        realm_calls(document, &steps, "tap", std::iter::once(page_realm)).len(),
        1
    );
    let (nodes, targets) = encoded(&steps);
    let outside: (Vec<&str>, Vec<&str>) = nodes
        .split(',')
        .zip(targets.split(','))
        .filter(|(node, _)| {
            let id = steps
                .steps()
                .iter()
                .find(|step| packed_node_id(step.node).to_string() == *node)
                .unwrap()
                .node;
            document.shadow_root_of(id).is_none()
        })
        .unzip();
    let (old_nodes, old_targets) = whole_path_encoding(document, &steps);
    assert_eq!(
        (outside.0.join(","), outside.1.join(",")),
        (old_nodes.clone(), old_targets),
        "less the shadow content, the steps the old call carried"
    );
    assert_eq!(
        nodes.split(',').next(),
        Some(packed_node_id(shadowed).to_string().as_str()),
        "the shadow content first, which the page realm skips"
    );
    assert!(
        steps
            .steps()
            .iter()
            .filter(|step| document.get(step.node).unwrap().is_shadow_root())
            .all(|step| !nodes
                .split(',')
                .any(|id| id == packed_node_id(step.node).to_string())),
        "no shadow root"
    );
    let page = packed_node_id(dom::NodeId::DOCUMENT_ELEMENT).to_string();
    assert!(
        old_nodes.split(',').any(|id| id == page),
        "the document element is on it, with the document node above it"
    );
    let steps = document.event_steps(shadowed, true, false);
    assert!(
        realm_calls(document, &steps, "tap", std::iter::once(page_realm)).is_empty(),
        "a non-composed path inside a shadow tree invites no realm with nothing there"
    );
}

/// A name some element listens for, dispatched where nothing on the path
/// listens and no realm holds a global registration for it, calls no realm
/// at all: the dispatch answers that nothing was published. At the element
/// that does listen, the page realm is called and its listener runs.
#[test]
fn an_event_nothing_on_its_path_listens_for_calls_no_realm() {
    let (mut js, mut runtime, elements) = runtime();
    runtime
        .run_main_thread_script(
            &mut js,
            r"
        const page = __CreatePage();
        const listening = __CreateView(0);
        const quiet = __CreateView(0);
        __AppendElement(page, listening);
        __AppendElement(page, quiet);
        globalThis.held = [listening, quiet];
        globalThis.runs = 0;
        __AddEventListener(listening, 'tap', () => { runs += 1; }, {});
    ",
            "app:///nothing-on-the-path.js",
        )
        .unwrap();
    let (listening, quiet) = {
        let tree = elements.tree();
        let children = tree.document_element().child_ids().to_vec();
        (children[0], children[1])
    };
    assert!(elements.tree().hears("tap"));
    assert!(
        !runtime
            .dispatch_for_test(&mut js, quiet, &tap(), event_point())
            .expect("dispatch"),
        "no realm was called"
    );
    assert!(
        runtime
            .dispatch_for_test(&mut js, listening, &tap(), event_point())
            .expect("dispatch")
    );
    runtime
        .evaluate_module(
            &mut js,
            "if (globalThis.runs !== 1) throw Error('ran ' + globalThis.runs);",
            "app:///nothing-on-the-path-verify.js",
            "verifying",
        )
        .unwrap();
}

/// A `global-bindEvent` registration alone is enough for the page realm to
/// be called for an event of its name anywhere, with the registering element
/// as the delivery's `currentTarget` and the event's own target.
#[test]
fn a_global_registration_alone_calls_its_realm() {
    let (mut js, mut runtime, elements) = runtime();
    runtime
        .run_main_thread_script(
            &mut js,
            r"
        globalThis.runWorklet = (value, params) => value.body(params[0]);
        globalThis.seen = [];
        const page = __CreatePage();
        const registered = __CreateView(0);
        const elsewhere = __CreateView(0);
        __AppendElement(page, registered);
        __AppendElement(page, elsewhere);
        globalThis.held = [registered, elsewhere];
        __AddEvent(registered, 'global-bindEvent', 'tap', { type: 'worklet', value: {
            body: (e) => seen.push(e.target.uid + '@' + e.currentTarget.uid),
        } });
    ",
            "app:///global-alone.js",
        )
        .unwrap();
    let (registered, elsewhere) = {
        let tree = elements.tree();
        let children = tree.document_element().child_ids().to_vec();
        (children[0], children[1])
    };
    assert!(
        runtime
            .dispatch_for_test(&mut js, elsewhere, &tap(), event_point())
            .expect("dispatch")
    );
    runtime
        .evaluate_module(
            &mut js,
            &format!(
                "if (seen.join('|') !== '{}@{}') throw Error(seen.join('|'));",
                packed_node_id(elsewhere),
                packed_node_id(registered)
            ),
            "app:///global-alone-verify.js",
            "verifying",
        )
        .unwrap();
}

/// The page every frame-call test runs: a 20px spacer, then a 100×50 frame,
/// whose border box therefore starts at (0, 20); the page realm
/// `capture-catch`es `tap` on the frame and binds `keydown` on its page. A
/// `ShadowRealm` child is made the frame's card: it renders a 100×50 `inner`
/// under the nested page, binds `tap` there and files a `global-bindEvent`
/// for `keydown` on it. Every handler, in either realm, reports
/// `<label> <type> <target uid>` and the `detail`'s `x,y` when it has them to
/// the page realm's `seen`, through a function the page handed the child, so
/// the order across realms is the order they ran in.
const FRAME_CALLS: &str = r"
globalThis.runWorklet = (value, params) => value.body(params[0]);
globalThis.seen = [];
globalThis.report = (line) => { seen.push(line); };
const line = (label, e) => label + ' ' + e.type + ' ' + e.target.uid +
    (e.detail && e.detail.x !== undefined ? ' ' + e.detail.x + ',' + e.detail.y : '');
const log = (label) => ({ type: 'worklet', value: { body: (e) => report(line(label, e)) } });
const page = __CreatePage();
const spacer = __CreateView(0);
__SetInlineStyles(spacer, 'height:20px');
__AppendElement(page, spacer);
const frame = __CreateElement('frame', 0);
__SetInlineStyles(frame, 'width:100px;height:50px');
__AppendElement(page, frame);
__AddEvent(frame, 'capture-catch', 'tap', log('page-frame'));
__AddEvent(page, 'bindEvent', 'keydown', log('page-key'));
__FlushElementTree();
globalThis.realm = await frameRealm();
realm.evaluate(`(frame, report) => {
    globalThis.runWorklet = (value, params) => value.body(params[0]);
    const line = (label, e) => label + ' ' + e.type + ' ' + e.target.uid +
        (e.detail && e.detail.x !== undefined ? ' ' + e.detail.x + ',' + e.detail.y : '');
    const log = (label) => ({ type: 'worklet', value: { body: (e) => report(line(label, e)) } });
    E.__BobcatUsePageRoot(H.framePage(frame));
    const inner = E.__CreateView(0);
    E.__SetInlineStyles(inner, 'width:100px;height:50px');
    E.__AppendElement(E.__CreatePage(), inner);
    E.__AddEvent(inner, 'bindEvent', 'tap', log('card-inner'));
    E.__AddEvent(inner, 'global-bindEvent', 'keydown', log('card-key'));
    E.__FlushElementTree();
}`)(__GetElementUniqueID(frame), (text) => report(text));
";

/// [`FRAME_CALLS`] booted: the frame and the child's `inner`.
fn frame_calls() -> (
    ScriptRuntime,
    MainThreadRuntime,
    DocumentProbe,
    dom::NodeId,
    dom::NodeId,
) {
    let (mut js, mut runtime, elements) = runtime();
    runtime
        .run_main_thread_script(
            &mut js,
            &format!("{FRAME_REALM}{FRAME_CALLS}"),
            "app:///frame-calls.js",
        )
        .unwrap();
    let (frame, inner) = {
        let tree = elements.tree();
        let frame = first_frame(&tree);
        let page = frame_page_of(&tree, frame);
        (frame, tree.get(page).unwrap().child_ids()[0])
    };
    (js, runtime, elements, frame, inner)
}

/// What the realms of [`FRAME_CALLS`] reported, in order, and empties it.
fn frame_lines(js: &mut ScriptRuntime, runtime: &mut MainThreadRuntime) -> Vec<String> {
    runtime
        .evaluate_module(
            js,
            "import { __SetAttribute, __CreatePage } from 'bobcat:element';
             __SetAttribute(__CreatePage(), 'data-seen', seen.join('|'));
             seen.length = 0;",
            "app:///frame-lines.js",
            "reading what the realms reported",
        )
        .unwrap();
    let probe = runtime.with_document(|tree| {
        tree.document_element()
            .attribute("data-seen")
            .unwrap_or_default()
            .to_owned()
    });
    if probe.is_empty() {
        return Vec::new();
    }
    probe.split('|').map(str::to_owned).collect()
}

/// A tap inside a frame's card calls the page realm at the path's top — its
/// `capture-catch` on the frame hears it retargeted to the frame, the first
/// step it holds a handle for, with the viewport's coordinates — and then
/// the card's realm, directly, at the same step, handed the same path: its
/// `bindtap` hears its own node as the target, `x`/`y` relative to the
/// frame's border box. The page's `catch` ends the page's walk alone.
#[test]
fn a_cards_bindtap_is_called_in_its_own_realm_after_the_pages_walk() {
    let (mut js, mut runtime, _elements, frame, inner) = frame_calls();
    assert!(
        runtime
            .dispatch_for_test(&mut js, inner, &tap(), dom::Point2D::new(30.0, 40.0))
            .expect("dispatch")
    );
    assert_eq!(
        frame_lines(&mut js, &mut runtime),
        [
            format!("page-frame tap {} 30,40", packed_node_id(frame)),
            format!("card-inner tap {} 30,20", packed_node_id(inner)),
        ]
    );
}

/// A key at the document element calls the page realm, whose `bindkeydown`
/// on its page is on the path, and the card's realm by its
/// `global-bindkeydown`, after the page's. The key's path does not pass
/// through the frame's shadow root, so the card's global registration is
/// not reached by the tree rule: the card is called by web-core's
/// document-level rule ([`DOCUMENT_LEVEL_EVENTS`]), holds no step of the
/// path, and runs its global pass, the registering element its target.
#[test]
fn a_key_calls_every_realm_that_registered_it() {
    let (mut js, mut runtime, _elements, _frame, inner) = frame_calls();
    let payload = InputEventPayload {
        detail: dom::event::EventDetail::Key {
            key: Arc::from("a"),
            code: Arc::from("KeyA"),
            repeat: false,
            modifiers: dom::input::Modifiers::default(),
        },
        timestamp: 0.0,
    };
    runtime
        .dispatch_input_event(
            &mut js,
            dom::NodeId::DOCUMENT_ELEMENT,
            "keydown",
            &payload,
            None,
        )
        .expect("dispatch");
    assert_eq!(
        frame_lines(&mut js, &mut runtime),
        [
            format!(
                "page-key keydown {}",
                packed_node_id(dom::NodeId::DOCUMENT_ELEMENT)
            ),
            format!("card-key keydown {}", packed_node_id(inner)),
        ]
    );
}

/// A card's `global-bindEvent` is reached only by an event whose path
/// passes through the frame's shadow root, the root of the card's tree: a
/// tap outside the frame, where the card holds nothing on the path, calls
/// no realm at all, as web-core's card listens on its own shadow root.
#[test]
fn a_cards_global_registration_does_not_hear_a_tap_outside_its_frame() {
    let (mut js, mut runtime, elements, _frame, inner) = frame_calls();
    runtime
        .evaluate_module(
            &mut js,
            &format!(
                "globalThis.realm.evaluate(`(report) => {{
                    const inner = E.__BobcatHandleOf({inner});
                    E.__AddEvent(inner, 'global-bindEvent', 'tap', {{ type: 'worklet', value: {{
                        body: (e) => report('card-global ' + e.type + ' ' + e.target.uid),
                    }} }});
                }}`)((text) => report(text));",
                inner = packed_node_id(inner)
            ),
            "app:///card-global-tap.js",
            "filing the card's global tap",
        )
        .unwrap();
    let spacer = elements.tree().document_element().child_ids()[0];
    assert!(
        !runtime
            .dispatch_for_test(&mut js, spacer, &tap(), dom::Point2D::new(5.0, 5.0))
            .expect("dispatch"),
        "no realm was called"
    );
    assert_eq!(frame_lines(&mut js, &mut runtime), Vec::<String>::new());

    // Inside the frame it is reached, after the card's own `bindtap`.
    runtime
        .dispatch_for_test(&mut js, inner, &tap(), dom::Point2D::new(30.0, 40.0))
        .expect("dispatch");
    assert_eq!(
        frame_lines(&mut js, &mut runtime).last(),
        Some(&format!("card-global tap {}", packed_node_id(inner)))
    );
}

/// A page-level `global-bindtap`, on a light-tree node, still hears a tap
/// inside a frame's card: the document node is on that composed path. Its
/// target is the frame, the first step the page realm holds, and the page's
/// `capture-catch` on the frame does not keep it from running.
#[test]
fn a_page_global_registration_hears_a_tap_inside_a_card() {
    let (mut js, mut runtime, elements, frame, inner) = frame_calls();
    let spacer = elements.tree().document_element().child_ids()[0];
    runtime
        .evaluate_module(
            &mut js,
            &format!(
                "import {{ __AddEvent, __BobcatHandleOf }} from 'bobcat:element';
                 __AddEvent(__BobcatHandleOf({spacer}), 'global-bindEvent', 'tap', {{
                     type: 'worklet',
                     value: {{ body: (e) => report('page-global ' + e.type + ' ' + e.target.uid) }},
                 }});",
                spacer = packed_node_id(spacer)
            ),
            "app:///page-global-tap.js",
            "filing the page's global tap",
        )
        .unwrap();
    runtime
        .dispatch_for_test(&mut js, inner, &tap(), dom::Point2D::new(30.0, 40.0))
        .expect("dispatch");
    assert_eq!(
        frame_lines(&mut js, &mut runtime),
        [
            format!("page-frame tap {} 30,40", packed_node_id(frame)),
            format!("page-global tap {}", packed_node_id(frame)),
            format!("card-inner tap {} 30,20", packed_node_id(inner)),
        ]
    );
}

/// A child's `closeRealm` takes every row it filed out of the listener table
/// at once: the next tap inside its frame calls the page realm alone.
#[test]
fn close_realm_clears_the_childs_rows() {
    let (mut js, mut runtime, elements, frame, inner) = frame_calls();
    let child = RealmId::Child(runtime.core().children.keys()[0]).listener_key();
    let card_rows = |elements: &DocumentProbe| {
        let tree = elements.tree();
        let steps = tree.event_steps(inner, true, true);
        (
            tree.listeners_on_path(&steps, "tap").contains(&child),
            tree.global_registrants("keydown").contains(&child),
        )
    };
    assert_eq!(card_rows(&elements), (true, true));
    runtime
        .evaluate_module(
            &mut js,
            "globalThis.realm.evaluate('H.closeRealm()');",
            "app:///close-frame-calls.js",
            "closing the card's realm",
        )
        .unwrap();
    assert_eq!(card_rows(&elements), (false, false));

    runtime
        .dispatch_for_test(&mut js, inner, &tap(), dom::Point2D::new(30.0, 40.0))
        .expect("dispatch");
    assert_eq!(
        frame_lines(&mut js, &mut runtime),
        [format!("page-frame tap {} 30,40", packed_node_id(frame))]
    );
}

/// A listener that throws in one realm does not keep the next from hearing
/// the event: both are called, and the failure is answered once.
#[test]
fn a_realm_that_throws_does_not_keep_the_next_from_hearing_the_event() {
    let (mut js, mut runtime, _elements, frame, inner) = frame_calls();
    runtime
        .evaluate_module(
            &mut js,
            "import { __AddEventListener, __GetElementUniqueID } from 'bobcat:element';
             import { __BobcatHandleOf } from 'bobcat:element';
             const frame = __BobcatHandleOf(FRAME);
             // Capturing, beside the `capture-catch` that stops the page's
             // walk at the frame: the same node's listeners still run.
             __AddEventListener(frame, 'tap', () => { throw Error('page listener'); },
                 { capture: true });"
                .replace("FRAME", &packed_node_id(frame).to_string())
                .as_str(),
            "app:///throwing-page.js",
            "filing a throwing listener",
        )
        .unwrap();
    let failed = runtime
        .dispatch_for_test(&mut js, inner, &tap(), dom::Point2D::new(30.0, 40.0))
        .expect_err("the page listener threw");
    let failures: Vec<MainThreadError> = failed.into_errors().collect();
    assert_eq!(failures.len(), 1);
    assert!(
        failures[0].to_string().contains("page listener"),
        "{}",
        failures[0]
    );
    let lines = frame_lines(&mut js, &mut runtime);
    assert!(
        lines.contains(&format!("card-inner tap {} 30,20", packed_node_id(inner))),
        "{lines:?}"
    );
}

/// Exposure follows the listener table, whichever realm's rows it holds: an
/// element the page realm listens on for `uiappear` stays registered while a
/// second realm's row for it remains, and goes with the last.
#[test]
fn exposure_follows_every_realms_rows_for_an_element() {
    let (mut js, mut runtime, elements) = runtime();
    runtime
        .run_main_thread_script(
            &mut js,
            &format!(
                r"{FRAME_REALM}
        const page = __CreatePage();
        const view = __CreateView(0);
        __AppendElement(page, view);
        globalThis.view = view;
        globalThis.listener = () => {{}};
        __AddEventListener(view, 'uiappear', listener, {{}});
        globalThis.realm = await frameRealm();
        realm.evaluate(`id => H.listenerOpened(id, 'uidisappear', 4)`)(__GetElementUniqueID(view));
        __RemoveEventListener(view, 'uiappear', listener, {{}});
    "
            ),
            "app:///exposure-two-realms.js",
        )
        .unwrap();
    let view = first_card_element(&elements);
    assert!(
        exposure_observer(&elements, view).is_some(),
        "the child's row keeps the element registered"
    );
    runtime
        .evaluate_module(
            &mut js,
            "import { __GetElementUniqueID } from 'bobcat:element';
             globalThis.realm.evaluate(`id => H.listenerClosed(id, 'uidisappear')`)(
                 __GetElementUniqueID(globalThis.view));",
            "app:///exposure-two-realms-close.js",
            "closing the child's row",
        )
        .unwrap();
    assert_eq!(exposure_observer(&elements, view), None);
}

/// A freed element takes its registrations with it, in the document's free
/// path, and the last removal of a name only it held reaches the painter at
/// the next flush, with no word from the realm.
#[test]
fn a_freed_elements_last_registration_unpublishes_its_name_at_the_next_flush() {
    let (mut js, mut runtime, _elements, mut names) = runtime_over_watching_names(ingredients());
    runtime
        .run_main_thread_script(
            &mut js,
            r"
        import { dropElement } from 'bobcat-internal:host';
        const page = __CreatePage();
        const view = __CreateView(0);
        __AppendElement(page, view);
        __AddEventListener(view, 'longpress', () => {}, {});
        globalThis.view = view;
    ",
            "app:///freed-registration.js",
        )
        .unwrap();
    assert!(names.contains("longpress"));
    runtime
        .evaluate_module(
            &mut js,
            "import { dropElement } from 'bobcat-internal:host';
             import { __CreatePage, __FlushElementTree, __GetElementUniqueID, __RemoveElement }
                 from 'bobcat:element';
             __RemoveElement(__CreatePage(), globalThis.view);
             dropElement(__GetElementUniqueID(globalThis.view));
             __FlushElementTree();",
            "app:///free-the-registration.js",
            "freeing the registered element",
        )
        .unwrap();
    assert!(!names.contains("longpress"));
}
