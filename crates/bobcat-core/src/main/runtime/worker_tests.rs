//! Real `QuickJS` on both sides, with the test acting as the resource-owning
//! embedder. No GPU is needed to verify contexts, transport and teardown.
use std::collections::VecDeque;
use std::time::Duration;

use tokio::sync::mpsc;

use super::*;
use crate::background::{WorkerEvent, WorkerHome, WorkerPayload};
use crate::esm::build_runtime;
use crate::jobs::JsThread;
use crate::link::{DetachedView, ViewNotice, block_on_deadline, detached_outbox};
use crate::main::workers::WorkerFactory;
use crate::resource::{LoadedSource, SourceCompletion, SourceRequest};
use crate::view::NoWakeup;

/// How long a test waits for a thread that should already be working.
const PATIENCE: Duration = Duration::from_secs(30);

/// What a test's BTS entry posts as its own last statement, for a test that
/// must not proceed until that entry has evaluated.
///
/// Nothing announces the BTS entry any more — the view is ready on MTS boot
/// alone — so a test that needs the entry's hook installed or its animation
/// frame requested says so in the entry itself.
const BTS_ENTRY_RAN: &str = "bts-entry-ran";

/// Whether the worker posted exactly this string.
///
/// A string crosses the transport as itself rather than inside an encoding,
/// so this is the whole comparison; anything that is not a primitive is a
/// structured clone, read back through `wire_json`.
fn posted(value: &HostValue, text: &str) -> bool {
    matches!(value, HostValue::String(value) if value == text)
}

struct Pair {
    runtime: Option<MainThreadRuntime>,
    js: ScriptRuntime,
    /// What this view's workers said, which is where every worker event
    /// arrives now — one channel per view rather than one per group.
    events: mpsc::UnboundedReceiver<WorkerEvent>,
    /// The host's end of the view's link: the test plays the embedder, so it
    /// is what answers every source request.
    view: DetachedView,
    frame_demand: crate::link::FrameDemand,
    deferred_notices: VecDeque<ViewNotice>,
    /// The host's view cancellation signal. Worker handles and their sources
    /// have independent lifetimes, which these tests exercise explicitly.
    cancel: tokio_util::sync::CancellationToken,
    /// The engine thread the MTS realm was opened with, held for its life:
    /// `__AdoptStyleSheet` would park on it.
    _thread: Rc<JsThread>,
    /// The group's worker thread. `Option` only so a test can drop it in the
    /// middle of its body: that is what waits for the thread, and two pins here
    /// ask what the realm's channels hold once it has returned.
    ///
    /// **Last field, and it must stay last.** Fields drop in declaration order,
    /// so the realm — whose `WorkerOwner` holds a sender on that thread — goes
    /// before the home whose drop closes the last one and waits.
    home: Option<WorkerHome>,
}

impl Pair {
    fn new(script: &str) -> Self {
        Self::with_background(script, None)
    }

    fn with_background(script: &str, background_source: Option<&str>) -> Self {
        let mut pair = Self::unbooted(background_source);
        pair.boot(script).unwrap();
        pair
    }

    fn unbooted(background_source: Option<&str>) -> Self {
        Self::unbooted_with_data(background_source, RealmStartup::default())
    }

    /// A booted pair whose view was built with these native modules, as
    /// `create_lynx_view` would have encoded them. The test is the embedder,
    /// so what it plays is the other half: it takes the calls off the notice
    /// queue and answers their callbacks by hand.
    fn with_native_modules(
        script: &str,
        background_source: &str,
        modules: &[(&str, &[&str])],
    ) -> Self {
        let table = modules
            .iter()
            .map(|(name, methods)| {
                (
                    (*name).to_owned(),
                    methods.iter().map(|method| (*method).to_owned()).collect(),
                )
            })
            .collect();
        let mut pair = Self::unbooted_with_data(
            Some(background_source),
            RealmStartup {
                native_modules: crate::native_module::encode_table(&table),
                ..RealmStartup::default()
            },
        );
        pair.boot(script).unwrap();
        pair
    }

    /// Waits for the next native-module call the BTS realm made, and
    /// assembles it out of the reply handle this test registered from
    /// `WorkerCreated` — which is exactly what `LynxView::pump` does.
    fn module_call(&mut self) -> (String, crate::native_module::ModuleCall) {
        let deadline = ClockInstant::now() + PATIENCE;
        loop {
            self.pump_host();
            let waiting = self
                .deferred_notices
                .iter()
                .position(|notice| matches!(notice, ViewNotice::NativeModuleCall { .. }));
            if let Some(position) = waiting
                && let Some(ViewNotice::NativeModuleCall {
                    worker,
                    call,
                    module,
                    method,
                    arguments,
                    callbacks,
                }) = self.deferred_notices.remove(position)
            {
                let reply = self
                    .frame_demand
                    .sender(worker)
                    .expect("the worker announced itself before it called");
                return (
                    module,
                    crate::native_module::ModuleCall::assemble(
                        call, method, arguments, &callbacks, &reply,
                    ),
                );
            }
            assert!(
                ClockInstant::now() < deadline,
                "the BTS realm called a native module"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    /// `startup` carries the page data and module table a test wants; this
    /// fills in the BTS entry, and the main script — which is what answers
    /// this realm's entry request and so names the base URL a worker
    /// specifier resolves against — is evaluated afterwards by [`Self::boot`].
    fn unbooted_with_data(background_source: Option<&str>, startup: RealmStartup) -> Self {
        Self::unbooted_with_config(
            background_source,
            startup,
            crate::main::tree::PageConfig::default(),
        )
    }

    /// The same, over a page configuration of the test's own: the boot module
    /// is written with it, so it has to be in place before the realm is
    /// opened.
    fn unbooted_with_config(
        background_source: Option<&str>,
        mut startup: RealmStartup,
        config: crate::main::tree::PageConfig,
    ) -> Self {
        startup.background_entry = background_source.map(|_| "test:bts-entry".to_owned());
        let home = match background_source {
            Some(source) => {
                WorkerHome::with_entry_for_test((source.to_owned(), "test:bts-entry".to_owned()))
            }
            None => WorkerHome::start().unwrap(),
        };
        let (outbox, view) = detached_outbox(Arc::new(NoWakeup));
        let cancel = view.token.clone();
        let mut js = build_runtime().unwrap();
        let ingredients =
            DocumentIngredients::for_test(crate::view::Viewport::new(32.0, 24.0), config);
        let thread = JsThread::new();
        let (runtime, events) = MainThreadRuntime::new(
            &mut js,
            ingredients,
            crate::main::runtime::bound_metrics(crate::view::Viewport::new(32.0, 24.0)),
            outbox,
            &WorkerFactory::new(home.commands(), home.trapped()),
            thread.handle(),
            startup,
        )
        .unwrap();
        Self {
            runtime: Some(runtime),
            js,
            events,
            view,
            frame_demand: crate::link::FrameDemand::default(),
            deferred_notices: VecDeque::new(),
            cancel,
            _thread: thread,
            home: Some(home),
        }
    }

    fn boot(&mut self, script: &str) -> Result<(), MainThreadError> {
        self.runtime.as_mut().unwrap().run_main_thread_script(
            &mut self.js,
            script,
            "app:///nested/main.js",
        )
    }

    /// Answers the next module one of this view's realms asks for, spinning
    /// the way an embedder's own turn does. A compiled BTS bundle reaches its
    /// manifest paths through the host now, and the load parks the worker's
    /// job until this answers it.
    fn serve_module(&mut self, url: &str, source: &str) {
        let deadline = ClockInstant::now() + PATIENCE;
        loop {
            self.pump_host();
            let waiting = self.deferred_notices.iter().position(|notice| {
                matches!(notice, ViewNotice::RequestSource {
                    request: SourceRequest::Module(requested), ..
                } if requested == url)
            });
            if let Some(position) = waiting
                && let Some(ViewNotice::RequestSource { completion, .. }) =
                    self.deferred_notices.remove(position)
            {
                completion.complete(Ok(LoadedSource::Entry {
                    source: source.to_owned(),
                    url: url.to_owned(),
                }));
                return;
            }
            assert!(ClockInstant::now() < deadline, "the realm asked for {url}");
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    /// The next source request the realm made, which for these tests is
    /// always a worker script.
    fn source(&mut self) -> SourceCompletion {
        self.pump_host();
        loop {
            let notice = self.deferred_notices.pop_front().expect("source requested");
            if let ViewNotice::RequestSource {
                request,
                completion,
            } = notice
            {
                assert!(
                    matches!(request, SourceRequest::Worker { specifier, base_url }
                    if specifier == "./worker.js" && base_url == "app:///nested/main.js")
                );
                return completion;
            }
        }
    }

    fn answer(&mut self, source: &str) {
        self.source().complete(Ok(LoadedSource::Entry {
            source: source.into(),
            url: "app:///nested/worker.js".into(),
        }));
    }

    /// Waits for one worker event and hands it to the realm, as the view's
    /// own task does.
    fn deliver(&mut self) {
        let event = self.next_event().expect("a worker event arrives");
        self.runtime
            .as_mut()
            .unwrap()
            .dispatch_worker_event(&mut self.js, event.key, event.payload)
            .unwrap();
    }

    fn next_event(&mut self) -> Option<WorkerEvent> {
        let deadline = ClockInstant::now() + PATIENCE;
        block_on_deadline(self.events.recv(), deadline).flatten()
    }

    /// Waits for the marker a test's BTS entry posts last, dispatching
    /// whatever it said before it and consuming the marker itself.
    fn await_background_entry(&mut self) {
        loop {
            let event = self.next_event().expect("the BTS entry's own marker");
            if matches!(&event.payload, WorkerPayload::Message(value)
                if posted(value, BTS_ENTRY_RAN))
            {
                return;
            }
            self.runtime
                .as_mut()
                .unwrap()
                .dispatch_worker_event(&mut self.js, event.key, event.payload)
                .unwrap();
        }
    }

    /// How many workers this realm still holds the right to stop.
    fn live_workers(&self) -> usize {
        self.runtime.as_ref().unwrap().live_workers()
    }

    /// Everything the realm has said to its host so far.
    fn pump_host(&mut self) {
        collect_host_notices(
            &mut self.frame_demand,
            &mut self.view.notices,
            &mut self.deferred_notices,
        );
    }

    fn notices(&mut self) -> Vec<ViewNotice> {
        self.pump_host();
        self.deferred_notices.drain(..).collect()
    }

    fn check(&mut self, source: &str) {
        self.runtime
            .as_mut()
            .unwrap()
            .evaluate_module(
                &mut self.js,
                source,
                "app:///assert.js",
                "asserting Worker behavior",
            )
            .unwrap();
    }
    /// Drive the same MTS module and Worker event stream as the page owner.
    fn dispose(&mut self) -> Vec<String> {
        self.runtime
            .as_mut()
            .unwrap()
            .begin_dispose(&mut self.js)
            .unwrap();
        let mut messages = Vec::new();
        while !self.runtime.as_mut().unwrap().disposal_finished().unwrap() {
            let event = self.next_event().expect("BTS acknowledges disposal");
            if let WorkerPayload::Message(ref data) = event.payload {
                messages.push(crate::background::wire_json(data));
            }
            self.runtime
                .as_mut()
                .unwrap()
                .dispatch_worker_event(&mut self.js, event.key, event.payload)
                .unwrap();
        }
        messages
    }

    /// Release the creating realm and wait for every worker owner to return.
    /// Keep the receiver to detect anything posted after JS disposal.
    fn finish(&mut self) -> Vec<WorkerEvent> {
        drop(self.runtime.take());
        drop(self.home.take());
        let mut events = Vec::new();
        while let Ok(event) = self.events.try_recv() {
            events.push(event);
        }
        events
    }
}

fn collect_host_notices(
    demand: &mut crate::link::FrameDemand,
    incoming: &mut mpsc::UnboundedReceiver<ViewNotice>,
    other: &mut VecDeque<ViewNotice>,
) {
    while let Ok(notice) = incoming.try_recv() {
        match notice {
            ViewNotice::WorkerCreated { key, messages } => demand.register_worker(key, messages),
            ViewNotice::ScriptFrameDemand { worker, pending } => demand.set(worker, pending),
            notice => other.push_back(notice),
        }
    }
}

/// Whether any notice reports a worker failure with this message.
fn worker_failed(notices: &[ViewNotice], message: &str) -> bool {
    notices.iter().any(|notice| {
        matches!(notice, ViewNotice::Engine(crate::EngineEvent::WorkerFailed(error))
            if error.message.contains(message))
    })
}

/// Every worker failure a batch of notices carries.
fn worker_failures(notices: Vec<ViewNotice>) -> Vec<crate::script::ScriptError> {
    notices
        .into_iter()
        .filter_map(|notice| match notice {
            ViewNotice::Engine(crate::EngineEvent::WorkerFailed(error)) => Some(error),
            _ => None,
        })
        .collect()
}

/// Whether any notice asks the host for a worker's script.
fn asked_for_a_worker(notices: &[ViewNotice]) -> bool {
    notices.iter().any(|notice| {
        matches!(
            notice,
            ViewNotice::RequestSource {
                request: SourceRequest::Worker { .. },
                ..
            }
        )
    })
}

#[test]
fn bts_entry_receives_processed_initial_data_before_it_installs_app_hooks() {
    let mut pair = Pair::unbooted_with_data(
        Some(
            r"
        const params = lynx.getApp()._params;
        if (params.initData !== null || !Array.isArray(params.cacheData) || params.cacheData.length) throw Error('native initial slots');
        const data = params.updateData;
        // The structured-clone transport keeps what JSON lost: an
        // `undefined`-valued member stays an own member, the nonfinite
        // numbers stay themselves, and negative zero stays negative.
        if (data !== lynx.__initData || data.count !== 42 || 'raw' in data ||
            !Object.hasOwn(data, 'missing') || data.missing !== undefined ||
            !Number.isNaN(data.nan) || data.infinity !== Infinity || !Object.is(data.zero, -0) ||
            !Object.hasOwn(data, '__proto__') || data.__proto__.own !== true || data.own !== undefined ||
            data.nested.count !== 42 || lynx.__globalProps.theme !== 'dark') throw Error('BTS bootstrap data');
        lynx.getCoreContext().dispatchEvent({type:'reply',data});
        ",
        ),
        RealmStartup {
            initial_processor: String::new(),
            init_data: Some(serde_json::json!({"raw":41}).to_string()),
            global_props: Some(serde_json::json!({"theme":"dark"}).to_string()),
            native_modules: String::new(),
            ..RealmStartup::default()
        },
    );

    pair.boot(r"
        globalThis.results = [];
        lynx.getJSContext().addEventListener('reply', e => results.push(e.data));
        const initial = lynx.__initData;
        lynx.__initData = {};
        await Promise.resolve();
        globalThis.processData = function(data, name) {
            if (data !== initial || arguments.length !== 2 || name !== '') throw Error('processor arguments');
            return Object.fromEntries([
                ['count', data.raw + 1], ['missing', undefined], ['nan', NaN], ['infinity', Infinity],
                ['zero', -0], ['__proto__', {own:true}], ['nested', {count:42}],
            ]);
        };
        globalThis.renderPage = data => {
            if (data.count !== 42 || !Number.isNaN(data.nan)) throw Error('MTS processor result');
            // The data posted to BTS is already a snapshot of the result.
            data.nested.count = 99;
        };
        ").unwrap();
    pair.deliver();
    pair.check(
        r"
        if (results.length !== 1 || results[0].count !== 42 || results[0].nested.count !== 42 ||
            !Object.hasOwn(results[0], 'missing') || !Object.is(results[0].zero, -0) ||
            !Number.isNaN(results[0].nan)) throw Error('BTS result');
        ",
    );
    assert!(worker_failures(pair.notices()).is_empty());
}

#[test]
fn host_updates_preserve_json_text_and_processor_strings_until_js() {
    let mut pair = Pair::new("__CreatePage();");
    pair.check(r#"
        globalThis.updates = [];
        globalThis.processData = (data, name) => {
            if (!Object.is(data.zero, -0) || data.large !== Infinity ||
                data.text !== '\"\\\n\u0000中文' ||
                !Object.hasOwn(data, '__proto__') || data.__proto__.own !== true ||
                data.own !== undefined) throw Error('JSON was not parsed by JS');
            if (name !== 'selected\'\"\\\n\u0000中文') throw Error('processor string changed');
            return data;
        };
        globalThis.updatePage = (_data, options) => updates.push([options.resetPageData, options.reloadTemplate]);
    "#);
    let data = r#"{"zero":-0,"large":1e400,"text":"\"\\\n\u0000中文","__proto__":{"own":true}}"#;
    let processor = "selected'\"\\\n\0中文";
    for update in [
        crate::link::PageUpdate::Data {
            data: data.into(),
            processor_name: processor.into(),
            reset: false,
        },
        crate::link::PageUpdate::Data {
            data: data.into(),
            processor_name: processor.into(),
            reset: true,
        },
        crate::link::PageUpdate::Reload {
            data: data.into(),
            processor_name: processor.into(),
        },
    ] {
        pair.runtime
            .as_mut()
            .unwrap()
            .apply_page_update(&mut pair.js, &update)
            .unwrap();
    }
    pair.check(
        r"
        if (JSON.stringify(updates) !== '[[false,false],[true,false],[false,true]]')
            throw Error('update/reset/reload options');
    ",
    );
    assert!(!pair.notices().iter().any(|notice| matches!(
        notice,
        ViewNotice::Engine(crate::EngineEvent::ScriptReported { .. })
    )));
}

#[test]
fn lifecycle_hooks_and_bts_snapshots_precede_queued_mts_jobs() {
    for engine_hooks in [false, true] {
        let mut pair = Pair::unbooted_with_data(
            Some(
                r"
            const app = lynx.getApp();
            const reply = (kind, data) => lynx.getCoreContext().dispatchEvent({type:'reply', data:[kind,data]});
            reply('initial', app._params.updateData);
            app.updateCardData = data => reply('update', data);
            app.onAppReload = data => reply('reload', data);
        ",
            ),
            RealmStartup {
                initial_processor: String::new(),
                init_data: Some(serde_json::json!({"count":1}).to_string()),
                global_props: None,
                native_modules: String::new(),
                ..RealmStartup::default()
            },
        );

        pair.boot(&format!(r"
            globalThis.results = [];
            lynx.getJSContext().addEventListener('reply', event => results.push(event.data));
            globalThis.current = undefined;
            globalThis.processData = data => {{
                current = {{count:data.count}};
                const result = current;
                Promise.resolve().then(() => Promise.resolve().then(() => result.count += 2));
                return result;
            }};
            const render = data => {{
                if (data.count !== 1) throw Error('processor jobs ran before render');
                Promise.resolve().then(() => data.afterRender = true);
            }};
            const remove = () => {{
                Promise.resolve().then(() => Promise.resolve().then(() => current.removed = true));
            }};
            const update = (data, options) => {{
                if (data.count !== (options.reloadTemplate ? 7 : 4)) throw Error('processor jobs ran before update');
                if (options.reloadTemplate && data.removed !== undefined) throw Error('removal jobs ran before update');
                Promise.resolve().then(() => Promise.resolve().then(() => data.afterUpdate = true));
            }};
            if ({engine_hooks}) {{
                const engine = lynx.getEngine();
                engine.addEventListener('__RenderPage', event => render(...event.data));
                engine.addEventListener('__RemoveComponents', remove);
                engine.addEventListener('__UpdatePage', event => update(...event.data));
            }} else {{
                globalThis.renderPage = render;
                globalThis.removeComponents = remove;
                globalThis.updatePage = update;
            }}
        ")).unwrap();
        pair.deliver();
        for (count, reload) in [(4, false), (7, true)] {
            let data = serde_json::json!({"count":count}).to_string();
            let update = if reload {
                crate::link::PageUpdate::Reload {
                    data,
                    processor_name: String::new(),
                }
            } else {
                crate::link::PageUpdate::Data {
                    data,
                    processor_name: String::new(),
                    reset: false,
                }
            };
            pair.runtime
                .as_mut()
                .unwrap()
                .apply_page_update(&mut pair.js, &update)
                .unwrap();
            pair.deliver();
        }
        pair.check(r"
            const expected = [['initial',{count:1}],['update',{count:4}],['reload',{count:7}]];
            if (JSON.stringify(results) !== JSON.stringify(expected)) throw Error(JSON.stringify(results));
            if (current.count !== 9 || !current.removed || !current.afterUpdate) throw Error('queued jobs did not finish');
        ");
        assert!(!pair.notices().iter().any(|notice| matches!(
            notice,
            ViewNotice::Engine(
                crate::EngineEvent::ScriptReported { .. } | crate::EngineEvent::WorkerFailed(_)
            )
        )));
    }
}

#[test]
fn global_props_initialize_bts_before_hooks_and_notify_before_mts_events() {
    let mut pair = Pair::unbooted_with_data(
        Some(
            r"
        const props=lynx.__globalProps;
        if (props.seed!==1 || props.keep!==1 || props.nested.value!==2) throw Error('BTS initial props');
        lynx.getApp().updateGlobalProps=data=>{
            if (data.keep!==1 || data.nested.value!==2 || 'scriptOnly' in data) throw Error('host props mutated');
            lynx.getCoreContext().dispatchEvent({type:'reply',data:['update',data.seed]});
        };
        lynx.getCoreContext().addEventListener('mts-props', e=>{
            lynx.getCoreContext().dispatchEvent({type:'reply',data:['mts',e.data]});
        });
        lynx.getCoreContext().dispatchEvent({type:'reply',data:['initial',props.seed]});
    ",
        ),
        RealmStartup {
            initial_processor: String::new(),
            init_data: None,
            global_props: Some(
                serde_json::json!({"seed":1,"keep":1,"nested":{"value":2}}).to_string(),
            ),
            native_modules: String::new(),
            ..RealmStartup::default()
        },
    );

    pair.boot(r"
        globalThis.results=[];
        lynx.getJSContext().addEventListener('reply', e=>results.push(e.data));
        const initial=lynx.__globalProps;
        globalThis.renderPage=()=>{
            if (initial.seed!==1) throw Error('initial render props');
            initial.nested.value=99;
            initial.scriptOnly=true;
        };
        lynx.getEngine().addEventListener('__UpdateGlobalProps',e=>{
            if ('origin' in e || e.data.length!==1 || e.data[0].seed!==4 || lynx.__globalProps.seed!==4 ||
                lynx.__globalProps===initial || initial.seed!==1) throw Error('MTS props delivery');
            lynx.getJSContext().dispatchEvent({type:'mts-props',data:e.data[0].seed});
            throw Error('props hook failed');
        });
    ").unwrap();
    pair.deliver();
    pair.check(
        r#"if (JSON.stringify(results)!=='[["initial",1]]') throw Error('initial hook count');"#,
    );
    pair.runtime
        .as_mut()
        .unwrap()
        .apply_page_update(
            &mut pair.js,
            &crate::link::PageUpdate::GlobalProps(r#"{"seed":4}"#.into()),
        )
        .unwrap();
    pair.deliver();
    pair.deliver();
    pair.check(r#"if (JSON.stringify(results)!=='[["initial",1],["update",4],["mts",4]]') throw Error('props event order');"#);
    assert!(pair.notices().iter().any(|notice| matches!(notice,
        ViewNotice::Engine(crate::EngineEvent::ScriptReported {message,..}) if message.contains("props hook failed"))));
}

#[test]
fn initial_processor_preserves_its_string_and_reads_the_page_config_switch() {
    let processor = "selected'\"\\\n中文";
    // This is a JS assertion literal, while RealmStartup receives the original Rust string.
    let expected_processor = r#""selected'\"\\\n中文""#;
    for enable_js_data_processor in [false, true] {
        let expected_name = if enable_js_data_processor {
            expected_processor
        } else {
            "''"
        };
        let expected_value = if enable_js_data_processor { 3 } else { 4 };
        let mut pair = Pair::unbooted_with_config(
            Some(&format!(
                r"
                const params=lynx.getApp()._params;
                if (params.processorName !== {expected_name} || params.updateData.value !== {expected_value}) throw Error('BTS processor parameters');
                lynx.getCoreContext().dispatchEvent({{type:'reply',data:params.updateData.value}});
                ",
            )),
            RealmStartup {
                initial_processor: processor.to_owned(),
                init_data: Some(serde_json::json!({"value":3}).to_string()),
                global_props: None,
                native_modules: String::new(),
                ..RealmStartup::default()
            },
            crate::main::tree::PageConfig {
                enable_js_data_processor,
                ..crate::main::tree::PageConfig::default()
            },
        );
        let render_processor = if enable_js_data_processor {
            expected_processor
        } else {
            "undefined"
        };
        pair.boot(&format!(r"
            globalThis.results=[];
            lynx.getJSContext().addEventListener('reply',e=>results.push(e.data));
            globalThis.processData=(data,name)=>{{
                if ({enable_js_data_processor} || name!=={expected_processor}) throw Error('unexpected processor');
                return {{value:data.value+1}};
            }};
            globalThis.renderPage=(data,options)=>{{
                if (data.value !== {expected_value} || options.processorName !== {render_processor}) throw Error('MTS processor parameters');
            }};
        ")).unwrap();
        pair.deliver();
        pair.check(&format!(
            "if (results.length!==1 || results[0]!=={expected_value}) throw Error('BTS data');",
        ));
        assert!(!pair.notices().iter().any(|notice| matches!(
            notice,
            ViewNotice::Engine(
                crate::EngineEvent::ScriptReported { .. } | crate::EngineEvent::WorkerFailed(_)
            )
        )));
    }
}

#[test]
fn initial_processor_non_tables_and_exceptions_preserve_host_data_in_both_realms() {
    for result in [
        "undefined",
        "null",
        "[]",
        "7",
        "(() => {throw Error('processor failed')})()",
    ] {
        let mut pair = Pair::unbooted_with_data(
            Some(
                r"
            const params = lynx.getApp()._params;
        if (params.initData !== null || !Array.isArray(params.cacheData) || params.cacheData.length) throw Error('native initial slots');
        const data = params.updateData;
            if (data.seed !== 3) throw Error('BTS fallback data');
            lynx.getCoreContext().dispatchEvent({type:'reply',data});
            ",
            ),
            RealmStartup {
                initial_processor: String::new(),
                init_data: Some(serde_json::json!({"seed":3}).to_string()),
                global_props: None,
                native_modules: String::new(),
                ..RealmStartup::default()
            },
        );

        pair.boot(&format!(r"
            globalThis.results = [];
            lynx.getJSContext().addEventListener('reply', e => results.push(e.data));
            globalThis.processData = () => {result};
            globalThis.renderPage = data => {{ if (data.seed !== 3) throw Error('MTS fallback data'); }};
            ")).unwrap();
        pair.deliver();
        pair.check(
            "if (results.length !== 1 || results[0].seed !== 3) throw Error('missing fallback');",
        );
        let notices = pair.notices();
        let reports = notices.iter().filter(|notice| matches!(notice,
            ViewNotice::Engine(crate::EngineEvent::ScriptReported {message,..}) if message.contains("processor failed"))).count();
        assert_eq!(reports, usize::from(result.contains("throw")));
        assert!(worker_failures(notices).is_empty());
    }
}

#[test]
fn engine_render_delivers_lifecycle_to_the_current_background_app_hook() {
    let mut pair = Pair::with_background(
        r"
        globalThis.results = [];
        lynx.getJSContext().addEventListener('reply', e => results.push(e.data));
        globalThis.processData = () => ({ answer: 42 });
        globalThis.renderPage = false;
        await Promise.resolve();
        lynx.getEngine().addEventListener('__RenderPage', e => {
            __OnLifecycleEvent(['render', e.data[0]]);
        });
        ",
        Some(
            r"
        const app = lynx.getApp();
        app.OnLifecycleEvent = function(data) {
            if (this !== app) throw Error('wrong app receiver');
            lynx.getCoreContext().dispatchEvent({ type: 'reply', data });
            this.OnLifecycleEvent = data => {
                lynx.getCoreContext().dispatchEvent({ type: 'reply', data: ['replacement', data] });
            };
        };
        ",
        ),
    );
    pair.deliver();
    pair.check(
        r#"
        import { __OnLifecycleEvent } from 'bobcat:runtime';
        if (JSON.stringify(results) !== '[["render",{"answer":42}]]') throw Error(JSON.stringify(results));
        __OnLifecycleEvent(['update', 7]);
        "#,
    );
    pair.deliver();
    pair.check(
        r#"if (JSON.stringify(results[1]) !== '["replacement",["update",7]]') throw Error(JSON.stringify(results));"#,
    );
}

#[test]
fn string_handlers_reach_background_with_event_snapshots() {
    let mut pair = Pair::with_background(
        r"
        globalThis.results = [];
        lynx.getJSContext().addEventListener('reply', e => results.push(e.data));
        const page = __CreatePage('card', 0);
        const child = __CreateView(0);
        __SetID(child, 'button');
        __SetAttribute(child, 'data-item-name', 'first');
        __SetDataset(child, {count:7, nested:{value:'before'}});
        __AppendElement(page, child);
        __AddEvent(page, 'bindEvent', 'tap', 'opaque:root');
        __AddEvent(child, 'bindEvent', 'tap', '');
        __AddEventListener(child, 'tap', e => {
            e.detail.x = 99;
            __AddDataset(child, 'nested', {value:'after'});
            __SetID(child, 'changed');
        });
        ",
        Some(
            r"
        const app = lynx.getApp();
        app.publishEvent = function(name, event) {
            if (this !== app) throw Error('wrong publish receiver');
            lynx.getCoreContext().dispatchEvent({ type: 'reply', data: [name, event] });
        };
        ",
        ),
    );
    pair.runtime
        .as_mut()
        .unwrap()
        .dispatch_for_test(
            &mut pair.js,
            dom::NodeId::from_bits(3).unwrap(),
            "tap",
            dom::Point2D::new(12.0, 30.0),
        )
        .unwrap();
    pair.deliver();
    pair.deliver();
    pair.check(
        r"
        const [child, page] = results;
        if (child[0] !== '' || page[0] !== 'opaque:root') throw Error('handler name changed');
        const e = child[1];
        // The `target` object is one for the whole walk, but its dataset is
        // read again at each step, so what the listener wrote reaches the
        // step after it. `id` is the one field the cache does freeze. The
        // `detail` the realm built from the host's two numbers is one object
        // for the walk too, copied at each send: the child's step carries the
        // position, the page's what the listener wrote over it.
        if (e.target.id !== 'button' || e.currentTarget.uid !== 3 ||
            e.target.dataset.itemName !== 'first' ||
            e.detail.x !== 12 || e.detail.y !== 30 ||
            e.target.dataset.count !== 7 || e.target.dataset.nested.value !== 'before' ||
            page[1].target.id !== 'button' ||
            page[1].target.dataset.nested.value !== 'after' ||
            page[1].detail.x !== 99 ||
            'elementRefptr' in e.target || 'stopPropagation' in e ||
            page[1].currentTarget.uid !== 2) throw Error(JSON.stringify(results));
        ",
    );
}

/// What a Context event carries is a structured clone in both directions, so
/// the values a JSON transport could not spell survive the two threads: an
/// `undefined`-valued key, `NaN`, a `Date`, a typed array, a `BigInt`, and a
/// cycle.
#[test]
fn context_events_carry_structured_values_in_both_directions() {
    let mut pair = Pair::with_background(
        r"
        globalThis.results = [];
        const context = lynx.getJSContext();
        context.addEventListener('reply', e => results.push(e.data));
        const sent = { tag: 'mts', missing: undefined, nan: NaN,
                       at: new Date(1700000000123),
                       bytes: new Uint8Array([1, 2, 255]),
                       big: 9007199254740993n };
        sent.self = sent;
        context.dispatchEvent({ type: 'request', data: sent });
        ",
        Some(
            r"
        const core = lynx.getCoreContext();
        core.addEventListener('request', event => {
            const d = event.data;
            const seen = [
                typeof d, d.tag, 'missing' in d, d.missing === undefined,
                Number.isNaN(d.nan),
                d.at instanceof Date && d.at.getTime() === 1700000000123,
                d.bytes instanceof Uint8Array && Array.from(d.bytes).join(',') === '1,2,255',
                d.big === 9007199254740993n, d.self === d,
            ].join(':');
            const back = { seen, at: new Date(42), bytes: new Uint8Array([9]),
                           big: -9007199254740993n, missing: undefined };
            back.self = back;
            core.dispatchEvent({ type: 'reply', data: back });
        });
        ",
        ),
    );
    pair.deliver();
    pair.check(
        r"
        const d = results[0];
        const expected = 'object:mts:true:true:true:true:true:true:true';
        if (d.seen !== expected) throw Error('MTS -> BTS: ' + d.seen);
        if (!(d.at instanceof Date) || d.at.getTime() !== 42) throw Error('Date');
        if (!(d.bytes instanceof Uint8Array) || d.bytes[0] !== 9) throw Error('Uint8Array');
        if (d.big !== -9007199254740993n) throw Error('BigInt');
        if (!('missing' in d) || d.missing !== undefined) throw Error('undefined-valued key');
        if (d.self !== d) throw Error('cycle');
        ",
    );
}

/// A value the serializer refuses is refused where it was written, so the
/// poster hears about it rather than the other thread.
#[test]
fn posting_a_function_to_a_worker_throws_in_the_poster_and_sends_nothing() {
    let mut pair = Pair::new(
        r"
        import { Worker } from 'bobcat-internal';
        globalThis.seen = [];
        globalThis.thrown = null;
        globalThis.worker = new Worker('./worker.js');
        worker.onmessage = event => seen.push(event.data);
        try { worker.postMessage(() => 1); } catch (error) { thrown = error; }
        worker.postMessage('fine');
        ",
    );
    pair.answer("onmessage = event => postMessage(event.data);");
    pair.deliver();
    pair.check(
        r#"
        if (!(thrown instanceof TypeError)) throw Error('expected a TypeError, got ' + thrown);
        if (JSON.stringify(seen) !== '["fine"]') throw Error(JSON.stringify(seen));
        "#,
    );
}

/// The published snapshot of a DOM event is the same shape it was under the
/// JSON transport: the two stop methods are gone rather than present as
/// `undefined`, the element handles are replaced by values, and nothing else
/// was added or lost.
#[test]
fn a_published_dom_event_carries_values_only_and_no_propagation_methods() {
    let mut pair = Pair::with_background(
        r"
        globalThis.results = [];
        lynx.getJSContext().addEventListener('reply', e => results.push(e.data));
        const page = __CreatePage('card', 0);
        const child = __CreateView(0);
        __SetID(child, 'button');
        __SetAttribute(child, 'data-item-name', 'first');
        __AppendElement(page, child);
        __AddEvent(child, 'bindEvent', 'tap', 'handler');
        ",
        Some(
            r"
        lynx.getApp().publishEvent = (name, event) => {
            lynx.getCoreContext().dispatchEvent({ type: 'reply', data: {
                keys: Object.keys(event).sort().join(','),
                targetKeys: Object.keys(event.target).sort().join(','),
                shape: [name, event.type, event.eventPhase, event.target.id,
                        event.target.dataset.itemName, event.target.uid,
                        event.currentTarget.uid, event.detail.x, event.detail.y,
                        event.timestamp, JSON.stringify(event.params),
                        'stopPropagation' in event,
                        'stopImmediatePropagation' in event,
                        'elementRefptr' in event.target].join(':'),
            }});
        };
        ",
        ),
    );
    pair.runtime
        .as_mut()
        .unwrap()
        .dispatch_for_test(
            &mut pair.js,
            dom::NodeId::from_bits(3).unwrap(),
            "tap",
            dom::Point2D::new(12.0, 30.0),
        )
        .unwrap();
    pair.deliver();
    pair.check(
        r"
        const d = results[0];
        const keys = 'currentTarget,detail,eventPhase,params,target,timestamp,type';
        if (d.keys !== keys) throw Error(d.keys);
        if (d.targetKeys !== 'dataset,id,uid') throw Error(d.targetKeys);
        const expected = 'handler:tap:2:button:first:3:3:12:30:0:{}:false:false:false';
        if (d.shape !== expected) throw Error(d.shape);
        ",
    );
}

#[test]
fn publish_hooks_install_lazily_and_component_ids_stay_opaque() {
    let mut pair = Pair::with_background(
        r"
        import { __BobcatPublishEvent } from 'bobcat:runtime';
        globalThis.results = [];
        lynx.getJSContext().addEventListener('reply', e => results.push(e.data));
        __BobcatPublishEvent(undefined, 'first', { value: 1 });
        __BobcatPublishEvent('component:7', 'second', { value: 2 });
        lynx.getJSContext().dispatchEvent({ type: 'install', data: undefined });
        __BobcatPublishEvent(undefined, 'third', { value: 3 });
        ",
        Some(
            r"
        const app = lynx.getApp();
        const core = lynx.getCoreContext();
        const reply = data => core.dispatchEvent({ type: 'reply', data });
        core.addEventListener('install', () => {
            app.publishEvent = function(...args) {
                if (this !== app) throw Error('wrong page receiver');
                reply(args);
            };
            app.publicComponentEvent = function(...args) {
                if (this !== app) throw Error('wrong component receiver');
                reply(args);
            };
            app.publishEvent = (...args) => reply(['replacement', ...args]);
        });
        ",
        ),
    );
    for _ in 0..3 {
        pair.deliver();
    }
    pair.check(
        r#"
        const expected = [["first",{"value":1}],["component:7","second",{"value":2}],["replacement","third",{"value":3}]];
        if (JSON.stringify(results) !== JSON.stringify(expected)) throw Error(JSON.stringify(results));
        "#,
    );
}

#[test]
fn a_late_publish_hook_failure_does_not_discard_later_queued_events() {
    let mut pair = Pair::with_background(
        r"
        import { __BobcatPublishEvent } from 'bobcat:runtime';
        globalThis.results = [];
        lynx.getJSContext().addEventListener('reply', e => results.push(e.data));
        __BobcatPublishEvent(undefined, 'throws', {});
        __BobcatPublishEvent(undefined, 'survives', {});
        lynx.getJSContext().dispatchEvent({ type: 'install', data: undefined });
        ",
        Some(
            r"
        const core = lynx.getCoreContext();
        core.addEventListener('install', () => {
            lynx.getApp().publishEvent = name => {
                if (name === 'throws') throw Error('queued publish failure');
                core.dispatchEvent({ type: 'reply', data: name });
            };
        });
        ",
        ),
    );
    pair.deliver();
    pair.deliver();
    pair.check(
        r#"if (JSON.stringify(results) !== '["survives"]') throw Error(JSON.stringify(results));"#,
    );
    assert!(worker_failed(&pair.notices(), "queued publish failure"));
}

#[test]
fn lepus_calls_return_async_results_to_the_matching_background_callback() {
    let mut pair = Pair::with_background(
        r"
        globalThis.results = [];
        lynx.getJSContext().addEventListener('reply', e => results.push(e.data));
        globalThis.echo = async function(data) {
            if (this !== globalThis) throw Error('wrong main receiver');
            await Promise.resolve();
            return { answer: data.value + 1 };
        };
        ",
        Some(
            r"
        const native = lynx.getNativeApp();
        if (native !== lynx.getNativeApp()) throw Error('unstable native app');
        let calls = 0;
        for (const value of [1, 5]) {
            const returned = native.callLepusMethod('echo', {value}, data => {
                calls++;
                lynx.getCoreContext().dispatchEvent({ type: 'reply', data: [value, data] });
            });
            if (returned !== undefined || calls !== 0) throw Error('callback was synchronous');
        }
        native.callLepusMethod('missing', {}, data => {
            lynx.getCoreContext().dispatchEvent({ type: 'reply', data: ['missing', data === undefined] });
        });
        ",
        ),
    );
    // Three requests followed by their three replies through the same FIFO.
    for _ in 0..6 {
        pair.deliver();
    }
    pair.check(
        r#"
        const expected = [[1,{"answer":2}],[5,{"answer":6}],["missing",true]];
        if (JSON.stringify(results) !== JSON.stringify(expected)) throw Error(JSON.stringify(results));
        "#,
    );
}

#[test]
fn lepus_failures_report_without_success_callbacks_and_leave_bts_usable() {
    let mut pair = Pair::with_background(
        r"
        globalThis.results = [];
        lynx.getJSContext().addEventListener('reply', e => results.push(e.data));
        globalThis.afterFailure = () => 'alive';
        globalThis.failSync = () => { throw Error('sync lepus failure'); };
        globalThis.failAsync = async () => { throw Error('async lepus failure'); };
        ",
        Some(
            r"
        const core = lynx.getCoreContext();
        lynx.getNativeApp().callLepusMethod('failSync', {}, () => {
            core.dispatchEvent({ type: 'reply', data: 'unexpected callback' });
        });
        lynx.getNativeApp().callLepusMethod('failAsync', {}, () => {
            core.dispatchEvent({ type: 'reply', data: 'unexpected async callback' });
        });
        lynx.getNativeApp().callLepusMethod('failAsync', {});
        core.addEventListener('ping', () => lynx.getNativeApp().callLepusMethod('afterFailure', {}, data => core.dispatchEvent({ type: 'reply', data })));
        ",
        ),
    );
    for _ in 0..6 {
        pair.deliver();
    }
    let errors = worker_failures(pair.notices());
    assert_eq!(errors.len(), 3);
    assert!(errors[0].message.contains("sync lepus failure"));
    assert!(errors[1].message.contains("async lepus failure"));
    pair.check(
        "import { lynx } from 'bobcat:runtime'; lynx.getJSContext().dispatchEvent({ type: 'ping', data: undefined });",
    );
    pair.deliver();
    pair.deliver();
    pair.check(
        r#"if (JSON.stringify(results) !== '["alive"]') throw Error(JSON.stringify(results));"#,
    );
}

#[test]
fn constructor_creates_distinct_contexts_and_queues_messages_in_order() {
    let mut pair = Pair::new(
        r"
        import { Worker } from 'bobcat-internal';
        if (typeof globalThis.Worker !== 'undefined') throw Error('unexpected global');
        globalThis.results = [];
        globalThis.keptWorkers = [];
        for (const name of ['one', 'two']) {
            const worker = new Worker('./worker.js', { name, type: 'module' });
            keptWorkers.push(worker);
            worker.onmessage = e => results.push(e.data);
            worker.postMessage(1);
            worker.postMessage(2);
        }
    ",
    );
    for _ in 0..2 {
        pair.answer(
            r"
            if (typeof globalThis.counter !== 'undefined') throw Error('shared context');
            if (typeof globalThis.lynx !== 'undefined') throw Error('ordinary worker has BTS globals');
            globalThis.counter = 0;
            onmessage = e => postMessage([name, ++counter, e.data, typeof __CreatePage]);
        ",
        );
    }
    for _ in 0..4 {
        pair.deliver();
    }
    pair.check(
        r"
        for (const name of ['one', 'two']) {
            const messages = results.filter(r => r[0] === name);
            if (JSON.stringify(messages) !== JSON.stringify([
                [name, 1, 1, 'undefined'], [name, 2, 2, 'undefined']
            ])) throw Error(JSON.stringify(results));
        }
    ",
    );
}

#[test]
fn terminate_discards_events_already_queued_on_main() {
    let mut pair = Pair::new(
        r"
        import { Worker } from 'bobcat-internal';
        globalThis.worker = new Worker('./worker.js');
        worker.onmessage = () => { throw Error('late delivery'); };
    ",
    );
    pair.answer("postMessage('already queued');");
    // Waiting for the worker's response proves it has run before terminate.
    let event = pair.next_event().expect("the worker answered");
    pair.check("worker.terminate(); worker.terminate(); worker.postMessage('ignored');");
    pair.runtime
        .as_mut()
        .unwrap()
        .dispatch_worker_event(&mut pair.js, event.key, event.payload)
        .unwrap();
}

#[test]
fn worker_errors_reach_parent_and_leave_both_realms_usable() {
    let mut pair = Pair::new(
        r"
        import { Worker } from 'bobcat-internal';
        globalThis.errors = [];
        globalThis.messages = [];
        const worker = new Worker('./worker.js');
        worker.onerror = e => {
            if (e.target !== worker) throw Error('wrong error target');
            errors.push([e.message, e.filename, e.lineno, e.colno]);
        };
        worker.addEventListener('message', e => messages.push(e.data));
        worker.postMessage('ping');
    ",
    );
    pair.answer("onmessage = e => postMessage(e.data); throw Error('worker boom');");
    let mut event = pair.next_event().expect("worker error");
    let WorkerPayload::Errored(error) = &mut event.payload else {
        panic!("expected a recoverable worker error");
    };
    error.message = "worker boom'\"\\\n\0中文".into();
    error.location = Some(crate::script::ScriptSourceLocation {
        source: Some("worker'\"\\\n\0中文.js".into()),
        line: Some(u32::MAX),
        column: Some(17),
    });
    pair.runtime
        .as_mut()
        .unwrap()
        .dispatch_worker_event(&mut pair.js, event.key, event.payload)
        .unwrap();
    pair.deliver();
    pair.check(r#"
        const expected = [['worker boom\'\"\\\n\u0000中文', 'worker\'\"\\\n\u0000中文.js', 4294967295, 17]];
        if (JSON.stringify(errors) !== JSON.stringify(expected) || messages[0] !== 'ping')
            throw Error('worker error recovery or diagnostic arguments');
    "#);
}

#[test]
fn unanswered_script_dispatches_one_error_and_ends_the_handle() {
    let mut pair = Pair::new(
        r"
        import { Worker } from 'bobcat-internal';
        globalThis.errors = [];
        globalThis.worker = new Worker('./worker.js');
        worker.onerror = e => errors.push(e.message);
    ",
    );
    drop(pair.source());
    pair.deliver();
    pair.check("if (errors.length !== 1 || !errors[0].includes('without completing')) throw Error('lost source'); worker.postMessage('ignored'); worker.terminate();");
}

/// A `Worker` constructed once `bobcat-workers` has trapped is never sent
/// there: it fails at once, as one whose script could not be fetched does,
/// with no `WorkerCreated`, no `Start` and no request to the host.
#[test]
fn a_worker_created_after_its_thread_trapped_fails_without_starting() {
    let mut pair = Pair::new("globalThis.errors = [];");
    pair.home
        .as_ref()
        .unwrap()
        .trapped()
        .store(true, std::sync::atomic::Ordering::Release);
    pair.check(
        r"
        import { Worker } from 'bobcat-internal';
        globalThis.worker = new Worker('./worker.js');
        worker.onerror = e => errors.push(e.message);
    ",
    );
    assert!(
        !asked_for_a_worker(&pair.notices()),
        "a worker that failed at once asks the host for nothing"
    );
    let event = pair.next_event().expect("the worker's failure");
    assert!(
        pair.frame_demand.sender(event.key).is_none(),
        "the view never heard of the worker"
    );
    pair.runtime
        .as_mut()
        .unwrap()
        .dispatch_worker_event(&mut pair.js, event.key, event.payload)
        .unwrap();
    let failures = worker_failures(pair.notices());
    assert_eq!(failures.len(), 1);
    assert!(
        failures[0].message.contains("the worker thread has ended"),
        "{}",
        failures[0].message
    );
    pair.check(
        "if (errors.length !== 1 || !errors[0].includes('the worker thread has ended')) throw Error(JSON.stringify(errors));",
    );
    assert_eq!(pair.live_workers(), 1, "only the built-in BTS remains");
}

#[test]
fn dropping_the_view_cancels_io_without_keeping_the_worker_thread_alive() {
    let mut pair =
        Pair::new("import { Worker } from 'bobcat-internal'; new Worker('./worker.js');");
    let completion = pair.source();
    pair.cancel.cancel();
    assert!(
        !completion.is_cancelled(),
        "the live MTS handle still owns this load"
    );
    // Dropping the realm releases its senders even while the host retains the
    // source completion. No view-token propagation is needed to end the worker.
    drop(pair.runtime.take());
    drop(pair.home.take());
    assert!(
        completion.is_cancelled(),
        "nobody is waiting for this script any more"
    );
    completion.complete(Ok(LoadedSource::Entry {
        source: "throw Error('cancelled worker ran');".into(),
        url: "app:///late.js".into(),
    }));
    assert!(pair.events.try_recv().is_err());
}

/// Dropping the MTS realm closes its last Worker sender. Even an interval
/// cannot keep that Worker or its thread alive after the channel is gone.
#[test]
fn releasing_a_realm_ends_a_worker_that_would_never_end_on_its_own() {
    let mut pair = Pair::new(
        "import { Worker } from 'bobcat-internal'; globalThis.worker = new Worker('./worker.js');",
    );
    pair.answer("setInterval(() => postMessage('tick'), 1);");
    // The first tick proves the realm booted and its interval is running, so
    // what the drop below has to stop is a live worker.
    pair.next_event().expect("the worker's interval fires");
    drop(pair.runtime.take());
    // One deadline for the whole loop rather than one per iteration: this
    // worker posts a tick every millisecond, so a per-iteration deadline is
    // one a live worker keeps resetting and the failure this test names would
    // never arrive.
    let deadline = ClockInstant::now() + PATIENCE;
    loop {
        match block_on_deadline(pair.events.recv(), deadline) {
            // Every sender is gone: the realm's own, and the clone the
            // worker's task held for as long as it ran.
            Some(None) => break,
            // A tick the worker had already sent, or sent before it read the
            // message that ends it. The clock is checked here too because
            // `block_on_deadline` polls before it consults it, so a ready tick
            // is handed back even past the deadline.
            Some(Some(_)) if ClockInstant::now() < deadline => {}
            _ => panic!("the worker outlived the realm that created it"),
        }
    }
}

/// A worker that ended on its own is forgotten by the realm that created it.
///
/// `close()` ends the worker's task, and the `Closed` event is where this side
/// learns of it — so that is where the right to tell that worker to stop stops
/// being worth keeping. What the realm holds afterwards is the workers still
/// running; releasing the realm closes only those remaining senders.
#[test]
fn a_worker_that_closes_itself_is_forgotten_by_the_realm() {
    let mut pair = Pair::new(
        "import { Worker } from 'bobcat-internal'; globalThis.worker = new Worker('./worker.js');",
    );
    pair.answer("close();");
    // Boot creates the BTS worker beside the entry's own, so the realm has
    // two of them and this close accounts for exactly one.
    assert_eq!(pair.live_workers(), 2, "the entry's worker and lynx-bg");
    let event = pair
        .next_event()
        .expect("the worker reports that it closed");
    assert!(matches!(event.payload, WorkerPayload::Closed));
    pair.runtime
        .as_mut()
        .unwrap()
        .dispatch_worker_event(&mut pair.js, event.key, event.payload)
        .unwrap();
    assert_eq!(
        pair.live_workers(),
        1,
        "a worker that closed itself is no longer one of the realm's"
    );
}

#[test]
fn terminating_before_fetch_prevents_the_context_from_starting() {
    let mut pair = Pair::new(
        r"
        import { Worker } from 'bobcat-internal';
        const cancelled = new Worker('./worker.js');
        cancelled.onmessage = () => { throw Error('cancelled worker ran'); };
        cancelled.terminate();
        const alive = new Worker('./worker.js');
        globalThis.answer = null;
        alive.onmessage = e => answer = e.data;
    ",
    );
    pair.answer("postMessage('cancelled');");
    pair.answer("postMessage('alive');");
    pair.deliver();
    pair.check(
        "if (answer !== 'alive') throw Error('termination did not discard the pending script');",
    );
}

#[test]
fn worker_close_keeps_its_last_message_and_disables_future_posts() {
    let mut pair = Pair::new(
        r"
        import { Worker } from 'bobcat-internal';
        globalThis.answer = null;
        globalThis.worker = new Worker('./worker.js');
        worker.onmessage = e => answer = e.data;
    ",
    );
    pair.answer("postMessage('last'); close();");
    pair.deliver();
    pair.deliver();
    pair.check("if (answer !== 'last') throw Error('lost final message'); worker.postMessage('ignored'); worker.terminate();");
}

#[test]
fn unsupported_worker_options_fail_before_requesting_a_context() {
    let mut pair = Pair::new(
        r"
        import { Worker } from 'bobcat-internal';
        for (const create of [() => new Worker(), () => Worker('./worker.js'),
            () => new Worker('./worker.js', {type: 'classic'}),
            () => new Worker('./worker.js', 'wrong options')]) {
            let threw = false;
            try { create(); } catch (error) { threw = error instanceof TypeError; }
            if (!threw) throw Error('expected TypeError');
        }
    ",
    );
    assert!(!asked_for_a_worker(&pair.notices()));
    pair.check(
        "if (typeof globalThis.Worker !== 'undefined') throw Error('Worker leaked into globals');",
    );
}

#[test]
fn an_ordinary_worker_can_install_bts_through_its_own_import() {
    let mut pair = Pair::new(
        r"
        import { Worker } from 'bobcat-internal';
        globalThis.result = null;
        const worker = new Worker('./worker.js', {name: 'ordinary'});
        worker.onmessage = event => result = event.data;
        worker.postMessage({type: 'request', data: 42});
        ",
    );
    pair.answer(
        r"
        import { lynx } from 'bobcat:bts-runtime';
        if ('lynx' in globalThis) throw Error('BTS lynx leaked into globals');
        const core = lynx.getCoreContext();
        core.addEventListener('request', event => {
            core.dispatchEvent({type: 'reply', data: [name, event.data]});
        });
        ",
    );
    pair.deliver();
    pair.check(
        "if (JSON.stringify(result) !== '{\"type\":\"reply\",\"data\":[\"ordinary\",42],\"origin\":\"JSContext\"}') throw Error(JSON.stringify(result));",
    );
}

#[test]
fn background_contexts_exchange_native_events_and_flush_early_payload_references_in_order() {
    let mut pair = Pair::with_background(
        r"
        import { EventTarget } from 'bobcat:event-target';
        if ('lynx' in globalThis) throw Error('MTS lynx leaked into globals');
        globalThis.context = lynx.getJSContext();
        if (!(context instanceof EventTarget)) throw Error('JS context must inherit EventTarget');
        if (context !== lynx.getJSContext()) throw Error('unstable JS context');
        globalThis.results = [];
        context.addEventListener('request', () => { throw Error('local echo'); });
        context.addEventListener('reply', function (event) {
            if (this !== undefined) throw Error('listener receiver');
            results.push(event.data);
        });
        const first = {type: 'request', data: {value: 1}};
        if (context.dispatchEvent(first) !== 0) throw Error('dispatch result');
        context.dispatchEvent({type: 'request', data: {value: 2}});
        await Promise.resolve();
        first.data.value = 3;
        globalThis.renderPage = () => {
            first.data.value = 99;
            context.dispatchEvent({type: 'request', data: {value: 4}});
        };
    ",
        Some(
            r"
        import { EventTarget } from 'bobcat:event-target';
        export const ready = await Promise.resolve(true);
        if ('lynx' in globalThis) throw Error('BTS lynx leaked into globals');
        const core = lynx.getCoreContext();
        if (!(core instanceof EventTarget)) throw Error('core context must inherit EventTarget');
        if (core !== lynx.getCoreContext()) throw Error('unstable core context');
        if (name !== 'lynx-bg') throw Error('wrong background name');
        if (typeof document !== 'undefined' || typeof __CreatePage !== 'undefined') {
            throw Error('BTS reached the document');
        }
        core.addEventListener('reply', () => { throw Error('local echo'); });
        core.addEventListener('request', function (event) {
            if (this !== undefined) throw Error('listener receiver');
            if (core.dispatchEvent({type: 'reply', data: event.data}) !== 0) {
                throw Error('dispatch result');
            }
        });
    ",
        ),
    );
    for _ in 0..3 {
        pair.deliver();
    }
    pair.check(
        "if (JSON.stringify(results) !== '[{\"value\":3},{\"value\":2},{\"value\":4}]') throw Error(JSON.stringify(results));",
    );
}

#[test]
fn background_starts_only_after_the_awaited_main_entry_finishes() {
    let mut pair = Pair::with_background(
        r"
        import { Worker } from 'bobcat-internal';
        globalThis.finished = false;
        globalThis.connected = false;
        const add = Worker.prototype.addEventListener;
        Worker.prototype.addEventListener = function (...args) {
            if (!finished) throw Error('BTS connected before MTS entry finished');
            connected = true;
            return add.apply(this, args);
        };
        await Promise.resolve();
        finished = true;
        ",
        Some("lynx.getCoreContext().dispatchEvent({ type: 'ready', data: undefined });"),
    );
    pair.check("if (!connected) throw Error('BTS was not connected');");
    pair.deliver();
    assert!(!asked_for_a_worker(&pair.notices()));
}

#[test]
fn context_post_message_delivers_message_events_in_both_directions() {
    let mut pair = Pair::with_background(
        r"
        const context = lynx.getJSContext();
        globalThis.results = [];
        context.addEventListener('message', e => results.push([e.data, e.origin]));
        context.postMessage({value: 7});
        ",
        Some(
            r"
        const core = lynx.getCoreContext();
        core.addEventListener('message', e => {
            if (e.origin !== 'CoreContext') throw Error('wrong origin');
            core.postMessage(e.data);
        });
        ",
        ),
    );
    pair.deliver();
    pair.check(r#"if (JSON.stringify(results) !== '[[{"value":7},"JSContext"]]') throw Error(JSON.stringify(results));"#);
}

#[test]
fn background_listener_failure_is_nonfatal_and_later_context_events_still_arrive() {
    let mut pair = Pair::with_background(
        r"
        globalThis.results = [];
        const context = lynx.getJSContext();
        context.addEventListener('reply', e => results.push(e.data));
        context.dispatchEvent({type: 'request', data: 0});
        context.dispatchEvent({type: 'request', data: 1});
    ",
        Some(
            r"
        const core = lynx.getCoreContext();
        core.addEventListener('request', event => {
            if (event.data === 0) throw Error('BTS listener boom');
            core.dispatchEvent({type: 'reply', data: event.data});
        });
    ",
        ),
    );
    pair.deliver();
    pair.deliver();
    let failures = worker_failures(pair.notices());
    assert_eq!(failures.len(), 1);
    assert!(failures[0].message.contains("BTS listener boom"));
    pair.check(
        r"
        import { lynx } from 'bobcat:runtime';
        if (JSON.stringify(results) !== '[1]') throw Error('lost recovery event');
        lynx.getJSContext().dispatchEvent({type: 'request', data: 2});
    ",
    );
    pair.deliver();
    pair.check("if (JSON.stringify(results) !== '[1,2]') throw Error('BTS stopped');");
}

#[test]
fn an_omitted_background_entry_boots_without_host_io() {
    let mut pair = Pair::new(
        r"
        import { Worker } from 'bobcat-internal';
        const context = lynx.getJSContext();
        if (context !== lynx.getJSContext()) throw Error('unstable JS context');
        context.dispatchEvent({type: 'unobserved', data: 'empty BTS'});
        globalThis.answer = null;
        const worker = new Worker('./worker.js');
        worker.onmessage = event => answer = event.data;
        worker.postMessage('barrier');
    ",
    );
    // The built-in BTS worker is started before this one and answers its own
    // script without the host, so an ordinary worker that replies proves the
    // thread served both.
    pair.answer("onmessage = event => postMessage(event.data);");
    pair.deliver();
    pair.check("if (answer !== 'barrier') throw Error('worker barrier failed');");
    let notices = pair.notices();
    assert!(!asked_for_a_worker(&notices));
    assert!(worker_failures(notices).is_empty());
}

#[test]
fn a_rejected_main_entry_never_starts_its_background_context() {
    let mut pair = Pair::unbooted(Some("throw Error('BTS must not run');"));
    let error = pair
        .boot(
            r"
            lynx.getJSContext().dispatchEvent({type: 'queued', data: 1});
            await Promise.resolve();
            throw Error('main entry rejected');
        ",
        )
        .unwrap_err();
    assert!(error.to_string().contains("main entry rejected"));
    assert!(!asked_for_a_worker(&pair.notices()));
    drop(pair.runtime.take());
    drop(pair.home.take());
    assert!(pair.events.try_recv().is_err());
}

#[test]
fn bts_node_queries_read_real_nodes_and_retain_native_tokens_and_statuses() {
    let mut pair = Pair::with_background(
        r"
        const page = __CreatePage();
        const parent = __CreateView(); __SetID(parent, 'scope');
        const first = __CreateView(); __SetID(first, 'first'); __SetClasses(first, 'item marked');
        __SetAttribute(first, 'count', 7);
        const details = {nested:{value:1}};
        __SetAttribute(first, 'details', details); details.nested.value = 2;
        __SetAttribute(first, 'callback', () => {});
        __SetDataset(first, {value: {n:1}, nil:null});
        __SetDataset(first, {next:2});
        const last = __CreateView(); __SetID(last, 'last'); __SetClasses(last, 'item');
        __AppendElement(page, parent); __AppendElement(parent, first); __AppendElement(parent, last);
        globalThis.results = null;
        lynx.getJSContext().addEventListener('queryDone', event => { results = event.data; });
    ",
        Some(
            r"
        void (async () => {
        const read = (nodes, fields) => new Promise(resolve => nodes.fields(fields, (data, status) => resolve({data,status})).exec());
        const query = lynx.createSelectorQuery();
        const scoped = await read(query.select('#scope'), {query:true});
        const scopeItself = await read(scoped.data.query.select('#scope'), {id:true});
        const children = await read(scoped.data.query.selectAll('.item'), {
            id:true, tag:true, class:true, index:true, unique_id:true, attribute:true, dataset:true,
        });
        const byId = await read(query.selectUniqueID(children.data[1].unique_id), {id:true});
        const missing = await read(query.selectAll('.absent'), {id:true});
        const invalid = await read(query.select('['), {id:true});
        const path = await new Promise(resolve => query.select('#first').path((data, status) => resolve({data,status})).exec());
        lynx.getCoreContext().dispatchEvent({type:'queryDone', data:{scopeItself, children, byId, missing, invalid, path}});
        })();
    ",
        ),
    );
    // Seven requests and then the application result, delivered through the
    // same FIFO as React's hydration and patch calls.
    for _ in 0..8 {
        pair.deliver();
    }
    pair.check(r"
        const r = results;
        if (!r || r.children.status.code !== 0 || r.children.data.length !== 2) throw Error(JSON.stringify(r));
        const first = r.children.data[0];
        if (r.scopeItself.data.id !== 'scope') throw Error('query must include its root');
        if (first.id !== 'first' || first.tag !== 'view' || first.index !== 0 || first.class.join(' ') !== 'item marked') throw Error(JSON.stringify(first));
        if (first.attribute.count !== 7 || 'id' in first.attribute || 'class' in first.attribute) throw Error('attribute value semantics');
        if (first.attribute.details.nested.value !== 1 || 'callback' in first.attribute) throw Error('attribute snapshot/function semantics');
        if (first.dataset.value.n !== 1 || first.dataset.nil !== null || first.dataset.next !== 2) throw Error('dataset merge/value semantics');
        if (r.byId.data.id !== 'last' || r.missing.status.code !== 2 || r.missing.data.length !== 0) throw Error('selection result');
        if (r.invalid.status.code !== 5 || r.invalid.data !== null) throw Error('invalid selector status');
        if (r.path.data.map(node => node.id).join('/') !== 'first/scope/') throw Error(JSON.stringify(r.path));
    ");
    assert!(!pair.notices().iter().any(|notice| matches!(
        notice,
        ViewNotice::Engine(crate::EngineEvent::WorkerFailed(_))
    )));
}

#[test]
fn bts_native_props_mutate_the_document_before_the_next_query() {
    let mut pair = Pair::with_background(
        r"
        const page = __CreatePage();
        globalThis.item = __CreateView(); __SetID(item, 'item');
        __SetInlineStyles(item, 'width:10px;height:10px;background-color:pink');
        __AppendElement(page, item);
        globalThis.result = null;
        lynx.getJSContext().addEventListener('queryDone', event => { result = event.data; });
    ",
        Some(
            r"
        const query = lynx.createSelectorQuery();
        const props = {width:'20px', 'background-color':'green', role:'changed'};
        query.select('#item').setNativeProps(props).exec();
        props.role = 'too-late';
        query.select('#item').fields({attribute:true}, (data,status) => {
            lynx.getCoreContext().dispatchEvent({type:'queryDone', data:{data,status}});
        }).exec();
    ",
        ),
    );
    for _ in 0..3 {
        pair.deliver();
    }
    pair.check(r"
        import {__GetAttributeByName} from 'bobcat:element';
        if (result.status.code !== 0 || result.data.attribute.role !== 'changed') throw Error(JSON.stringify(result));
        if ('width' in result.data.attribute || 'background-color' in result.data.attribute) throw Error('CSS incorrectly stored as attributes');
        const style = __GetAttributeByName(item, 'style');
        if (!style.includes('20px') || !style.includes('10px') || !style.includes('green')) throw Error(style);
    ");
    assert!(!pair.notices().iter().any(|notice| matches!(
        notice,
        ViewNotice::Engine(crate::EngineEvent::WorkerFailed(_))
    )));
}

/// The background thread's `invoke` reaches the same `__InvokeUIMethod` the
/// main thread's does, and the geometry it answers is current without the
/// query flushing for it: the entry that built the tree had already run a
/// pass over it — boot's own deferred flush here, the dirty-commit epilogue
/// for every entry after it — by the time the Worker's request crosses back.
/// Node resolution keeps its own codes either side of that: 2 for a selector
/// that matched nothing, and 5 for the `selectAll` an `invoke` cannot take,
/// which `selector-query.ts` refuses locally without a crossing at all.
#[test]
fn bts_invoke_measures_the_committed_tree_and_keeps_its_node_resolution_codes() {
    let mut pair = Pair::with_background(
        r"
        const page = __CreatePage();
        const item = __CreateView(); __SetID(item, 'item');
        __SetInlineStyles(item, 'width:100px;height:50px;margin-left:20px');
        __AppendElement(page, item);
        globalThis.result = null;
        lynx.getJSContext().addEventListener('queryDone', event => { result = event.data; });
        ",
        Some(
            r"
        void (async () => {
            const query = lynx.createSelectorQuery();
            const invoke = nodes => new Promise(resolve => nodes.invoke({
                method:'boundingClientRect', fail:resolve, success:resolve,
            }).exec());
            const measured = await invoke(query.select('#item'));
            const missing = await invoke(query.select('#absent'));
            const multiple = await invoke(query.selectAll('view'));
            const after = await new Promise(resolve => query.select('#item').fields(
                {id:true}, (data, status) => resolve({data, status}),
            ).exec());
            lynx.getCoreContext().dispatchEvent({type:'queryDone', data:{measured, missing, multiple, after}});
        })();
        ",
        ),
    );
    // The selectAll failure is local. Two invokes and one field request cross
    // the Worker, followed by the application result.
    for _ in 0..4 {
        pair.deliver();
    }
    pair.check(r"
        const rect = result.measured;
        if (rect.id !== 'item' || typeof rect.dataset !== 'object') throw Error(JSON.stringify(result));
        if (rect.left !== 20 || rect.top !== 0 || rect.width !== 100 || rect.height !== 50) throw Error(JSON.stringify(result));
        if (rect.right !== 120 || rect.bottom !== 50) throw Error(JSON.stringify(result));
        if (result.missing.code !== 2 || result.multiple.code !== 5) throw Error(JSON.stringify(result));
        if (result.after.status.code !== 0 || result.after.data.id !== 'item') throw Error(JSON.stringify(result));
    ");
    assert!(worker_failures(pair.notices()).is_empty());
}

#[test]
fn diagnostics_cross_the_worker_channel_and_keep_both_realms_usable() {
    let mut pair = Pair::with_background(
        r"
        __CreatePage();
        _ReportError(new Error('MTS warning'), {level:'warning'});
        console.info('MTS', {value:1});
        globalThis.alive = false;
        lynx.getJSContext().addEventListener('alive', () => { alive = true; });
        ",
        Some(
            r"
        import {console} from 'bobcat:bts-runtime';
        lynx.reportError(new Error('BTS fatal label'), {level:'fatal'});
        console.warn('BTS', [1,2]);
        lynx.getCoreContext().dispatchEvent({type:'alive', data:undefined});
        ",
        ),
    );
    for _ in 0..3 {
        pair.deliver();
    }
    pair.check("if (!alive) throw Error('a diagnostic stopped delivery');");
    let diagnostics: Vec<_> = pair
        .notices()
        .into_iter()
        .filter_map(|notice| match notice {
            ViewNotice::Engine(crate::EngineEvent::ScriptReported { level, message }) => {
                Some((true, level, message))
            }
            ViewNotice::Engine(crate::EngineEvent::ConsoleMessage { level, message }) => {
                Some((false, level, message))
            }
            ViewNotice::Engine(crate::EngineEvent::WorkerFailed(error)) => panic!("{error}"),
            _ => None,
        })
        .collect();
    assert_eq!(diagnostics.len(), 4);
    assert_eq!(
        (diagnostics[0].0, diagnostics[0].1.as_str()),
        (true, "warning")
    );
    assert!(diagnostics[0].2.contains("MTS warning"));
    assert!(diagnostics[0].2.contains("app:///nested/main.js"));
    assert_eq!(
        diagnostics[1],
        (false, "info".into(), "MTS {\"value\":1}".into())
    );
    assert_eq!(
        (diagnostics[2].0, diagnostics[2].1.as_str()),
        (true, "fatal")
    );
    assert!(diagnostics[2].2.contains("BTS fatal label"));
    assert_eq!(diagnostics[3], (false, "warn".into(), "BTS [1,2]".into()));
}

#[test]
fn host_global_events_reach_the_bts_emitter_in_order_after_a_listener_throws() {
    let mut pair = Pair::with_background(
        r"
        __CreatePage();
        globalThis.results = [];
        lynx.getJSContext().addEventListener('reply', e => results.push(e.data));
        ",
        Some(
            r"
        const emitter = lynx.getJSModule('GlobalEventEmitter');
        if (emitter !== lynx.getApp().GlobalEventEmitter) throw Error('emitter identity');
        emitter.addListener('host-event', function (value, extra) {
            if (this !== emitter) throw Error('emitter receiver');
            if (value === 0) throw Error('event failed');
            lynx.getCoreContext().dispatchEvent({type:'reply', data:[value, extra]});
        });
        ",
        ),
    );
    for value in 0..3 {
        pair.runtime
            .as_mut()
            .unwrap()
            .apply_page_update(
                &mut pair.js,
                &crate::link::PageUpdate::GlobalEvent {
                    name: "host-event".into(),
                    arguments: serde_json::json!([value, {"nested":value}]).to_string(),
                },
            )
            .unwrap();
    }
    for _ in 0..3 {
        pair.deliver();
    }
    let failures = worker_failures(pair.notices());
    assert_eq!(failures.len(), 1);
    assert!(failures[0].message.contains("event failed"));
    pair.check(r#"if (JSON.stringify(results) !== '[[1,{"nested":1}],[2,{"nested":2}]]') throw Error(JSON.stringify(results));"#);
}

#[test]
fn mts_boot_finishes_without_waiting_for_bts() {
    // Boot is the MTS entry's own fact. A BTS entry whose top-level await
    // never settles does not hold it back, and there is nothing further to
    // wait for once the entry module has evaluated.
    for background in [None, Some("await new Promise(() => {});")] {
        let mut pair = Pair::with_background("__CreatePage();", background);
        assert!(
            pair.runtime
                .as_mut()
                .unwrap()
                .main_module_finished()
                .unwrap(),
            "MTS never awaits BTS"
        );
    }
}

#[test]
fn a_throwing_bts_entry_reports_at_the_worker_and_leaves_bts_running() {
    let mut pair = Pair::with_background(
        r"__CreatePage();
globalThis.results = [];
lynx.getJSContext().addEventListener('reply', e => results.push(e.data));",
        Some(
            r"lynx.getJSModule('GlobalEventEmitter').addListener('host-event', value => lynx.getCoreContext().dispatchEvent({type:'reply', data:value}));
throw Error('BTS entry failed');",
        ),
    );
    assert!(
        pair.runtime
            .as_mut()
            .unwrap()
            .main_module_finished()
            .unwrap()
    );
    let event = pair.next_event().expect("the BTS entry reports its throw");
    let message = match &event.payload {
        WorkerPayload::Errored(error) => error.message.to_string(),
        _ => panic!("a throwing entry is an ordinary worker error"),
    };
    assert!(message.contains("BTS entry failed"), "{message}");
    pair.runtime
        .as_mut()
        .unwrap()
        .dispatch_worker_event(&mut pair.js, event.key, event.payload)
        .unwrap();
    let notices = pair.notices();
    assert!(
        !notices.iter().any(|notice| matches!(
            notice,
            ViewNotice::Engine(crate::EngineEvent::StartupFailed(_))
        )),
        "a BTS entry that throws never ends the view"
    );
    let failures = worker_failures(notices);
    assert_eq!(failures.len(), 1);
    assert!(failures[0].message.contains("BTS entry failed"));
    assert_eq!(pair.live_workers(), 1);
    // The realm that threw still has its listeners and still takes messages.
    pair.runtime
        .as_mut()
        .unwrap()
        .apply_page_update(
            &mut pair.js,
            &crate::link::PageUpdate::GlobalEvent {
                name: "host-event".into(),
                arguments: "[7]".into(),
            },
        )
        .unwrap();
    pair.deliver();
    pair.check("if (JSON.stringify(results) !== '[7]') throw Error(JSON.stringify(results));");
}

#[test]
fn engine_listeners_finish_the_walk_before_jobs_and_report_each_failure() {
    let mut pair = Pair::with_background(
        r"
        globalThis.order = [];
        const engine = lynx.getEngine();
        engine.addEventListener('__RenderPage', function() {
            if (this !== engine) throw Error('engine listener receiver');
            order.push('first');
            Promise.resolve().then(() => { order.push('first-job'); });
            throw Error('first listener failed');
        });
        engine.addEventListener('__RenderPage', function() {
            order.push('second');
            return Promise.resolve().then(() => {
                order.push('second-job');
                Promise.resolve().then(() => { order.push('nested-job'); });
            });
        });
        engine.addEventListener('__RenderPage', function() {
            if (order.join(',') !== 'first,second') throw Error(order);
            order.push('third');
            throw Error('third listener failed');
        });
        engine.addEventListener('__RenderPage', () => { order.push('fourth'); });
    ",
        None,
    );
    pair.check("if (order.join(',') !== 'first,second,third,fourth,first-job,second-job,nested-job') throw Error(order);");
    let reports: Vec<_> = pair
        .notices()
        .into_iter()
        .filter_map(|notice| match notice {
            ViewNotice::Engine(crate::EngineEvent::ScriptReported { message, .. }) => Some(message),
            _ => None,
        })
        .collect();
    assert_eq!(reports.len(), 2, "{reports:?}");
    assert!(reports[0].contains("first listener failed"));
    assert!(reports[1].contains("third listener failed"));
}

#[test]
fn a_throwing_render_hook_reports_without_failing_bts_startup() {
    let mut pair = Pair::with_background(
        r"
        globalThis.renderPage = () => {
            Promise.resolve().then(() => globalThis.renderJobFinished = true);
            throw Error('render hook failed');
        };
    ",
        Some("postMessage('BTS started');"),
    );
    assert!(matches!(pair.next_event().unwrap().payload,
        WorkerPayload::Message(ref value) if posted(value, "BTS started")));
    pair.check("if (!renderJobFinished) throw Error('outer checkpoint lost render jobs');");
    assert!(pair.notices().iter().any(|notice| matches!(notice,
        ViewNotice::Engine(crate::EngineEvent::ScriptReported { message, .. })
        if message.contains("render hook failed"))));
}

#[test]
fn mts_disposal_calls_the_current_bts_hook_once_before_js_terminates_the_worker() {
    for throws in [false, true] {
        let mut pair = Pair::with_background(
            "lynx.getJSContext().addEventListener('repeat-dispose', () => lynx.getEngine().dispatchEvent({type:'__DestroyLifetime'}));",
            Some(&format!(
                r"
                const app = lynx.getApp();
                app.callDestroyLifetimeFun = () => postMessage('stale-hook');
                app.callDestroyLifetimeFun = function(...args) {{
                    postMessage(['cleanup', this === app, args.length]);
                    lynx.getCoreContext().dispatchEvent({{type:'repeat-dispose', data:undefined}});
                    Promise.resolve().then(() => postMessage('cleanup-job'));
                    if ({throws}) throw Error('cleanup failed');
                }};
                postMessage('{BTS_ENTRY_RAN}');
            "
            )),
        );
        // The hook must be installed before disposal asks for it, and the
        // marker is consumed here so it is not one of the messages disposal
        // is compared against.
        pair.await_background_entry();
        // Host cancellation leaves the live MTS Worker alone until JS disposal.
        pair.cancel.cancel();
        assert_eq!(pair.live_workers(), 1);
        let messages = pair.dispose();
        assert_eq!(
            messages
                .iter()
                .filter(|m| m.as_str() == r#"["cleanup",true,0]"#)
                .count(),
            1
        );
        assert!(
            messages.iter().any(|m| m == r#""cleanup-job""#),
            "{messages:?}"
        );
        assert!(messages.last().unwrap().contains("disposed"));
        assert_eq!(
            pair.live_workers(),
            0,
            "the acknowledgement lets JS terminate"
        );
        let reports = pair.notices();
        assert_eq!(
            reports
                .iter()
                .filter(|notice| matches!(notice,
            ViewNotice::Engine(crate::EngineEvent::ScriptReported {message, ..})
            if message.contains("cleanup failed")))
                .count(),
            usize::from(throws)
        );
        assert!(
            pair.dispose().is_empty(),
            "a second disposal does not repeat cleanup"
        );
        assert!(pair.finish().is_empty());
    }
}

#[test]
fn disposal_remains_deliverable_while_the_bts_entry_is_loading() {
    let mut pair = Pair::with_background(
        "",
        Some(
            r"
        lynx.getApp().callDestroyLifetimeFun = () => postMessage('cleanup');
        await import('app:///pending.js');
        postMessage('late-entry');
    ",
        ),
    );
    let completion = loop {
        let notice = block_on_deadline(pair.view.notices.recv(), ClockInstant::now() + PATIENCE)
            .flatten()
            .expect("the BTS import asks for its source");
        if let ViewNotice::RequestSource {
            request: SourceRequest::Module(url),
            completion,
        } = notice
        {
            assert_eq!(url, "app:///pending.js");
            break completion;
        }
    };
    pair.cancel.cancel();
    assert!(!completion.is_cancelled());
    let messages = pair.dispose();
    assert_eq!(messages.first().unwrap(), r#""cleanup""#);
    assert!(messages.last().unwrap().contains("disposed"));
    assert!(pair.finish().is_empty());
    assert!(completion.is_cancelled());
    completion.complete(Ok(LoadedSource::Entry {
        source: "postMessage('late-module');".into(),
        url: "app:///pending.js".into(),
    }));
    assert!(pair.events.try_recv().is_err());
}

#[test]
fn disposal_does_not_wait_for_a_worker_that_already_closed() {
    let mut pair = Pair::with_background("", Some("close();"));
    pair.deliver();
    assert_eq!(pair.live_workers(), 0);
    assert!(pair.dispose().is_empty());
}

#[test]
fn unreachable_mts_worker_is_collected_without_releasing_the_view() {
    let mut pair = Pair::new(
        r"
        import {Worker} from 'bobcat-internal';
        globalThis.worker = new Worker('./worker.js');
        globalThis.received = [];
        worker.onmessage = e => received.push(e.data);
        worker.cycle = worker;
    ",
    );
    pair.answer(
        "onmessage = e => postMessage(e.data); setInterval(() => {}, 1000); postMessage('ready');",
    );
    pair.deliver();
    pair.runtime
        .as_mut()
        .unwrap()
        .collect_garbage(&mut pair.js)
        .unwrap();
    assert_eq!(pair.live_workers(), 2, "a reachable Worker survives GC");
    pair.check("worker.postMessage('alive');");
    pair.deliver();
    pair.check("if (received[1] !== 'alive') throw Error('retained worker stopped'); delete globalThis.worker;");
    pair.runtime
        .as_mut()
        .unwrap()
        .collect_garbage(&mut pair.js)
        .unwrap();
    // QuickJS discovers cycles in one collection and processes their weak
    // registrations in the next. Neither pass may retain the Worker.
    pair.runtime
        .as_mut()
        .unwrap()
        .collect_garbage(&mut pair.js)
        .unwrap();
    assert_eq!(pair.live_workers(), 1, "only the built-in BTS remains");
    assert!(!pair.cancel.is_cancelled(), "GC does not release the view");
    pair.dispose();
    assert!(pair.finish().is_empty());
}

/// The shape a page writes by habit — construct, install a handler, post —
/// keeps its `Worker` alive only through that handler's own closure, which is
/// a cycle rather than a root. A collection between the worker's answer and
/// its delivery clears the routing weak reference, and the answer is then
/// dropped with no diagnostic anywhere: the page simply never hears back.
///
/// Pinned here because the collection is not the page's to schedule. Every
/// view of a group shares one `QuickJS` runtime, so a sibling view's boot can
/// run the collection that takes this page's workers away. A page that waits
/// for an answer therefore has to name its worker for as long as it wants
/// one, which is what `docs/destruction-runtime.md` means by this engine's
/// handle collection policy. Browsers keep a running worker's object alive
/// instead and deliver the message; that divergence is the policy rather than
/// an oversight here.
#[test]
fn a_message_for_a_collected_worker_handle_is_dropped() {
    let mut pair = Pair::new(
        r"
        import {Worker} from 'bobcat-internal';
        globalThis.received = [];
        {
            const worker = new Worker('./worker.js');
            worker.onmessage = event => received.push(event.data);
            worker.postMessage('ready');
        }
    ",
    );
    pair.answer("onmessage = () => postMessage('answer');");
    let event = pair.next_event().expect("the worker answers");
    // QuickJS discovers the cycle in one collection and processes its weak
    // registrations in the next, so the handle is released by the second.
    for _ in 0..2 {
        pair.runtime
            .as_mut()
            .unwrap()
            .collect_garbage(&mut pair.js)
            .unwrap();
    }
    assert_eq!(pair.live_workers(), 1, "only the built-in BTS remains");
    pair.runtime
        .as_mut()
        .unwrap()
        .dispatch_worker_event(&mut pair.js, event.key, event.payload)
        .unwrap();
    pair.check(
        "if (received.length !== 0) throw Error('a collected Worker dispatched: ' + received.join());",
    );
    pair.dispose();
    assert!(pair.finish().is_empty());
}

/// The same page with the one reference that makes the worker the page's: its
/// answer arrives however many collections run in between.
#[test]
fn a_named_worker_survives_a_collection_and_still_delivers() {
    let mut pair = Pair::new(
        r"
        import {Worker} from 'bobcat-internal';
        globalThis.received = [];
        globalThis.running = [];
        {
            const worker = new Worker('./worker.js');
            running.push(worker);
            worker.onmessage = event => received.push(event.data);
            worker.postMessage('ready');
        }
    ",
    );
    pair.answer("onmessage = () => postMessage('answer');");
    let event = pair.next_event().expect("the worker answers");
    for _ in 0..2 {
        pair.runtime
            .as_mut()
            .unwrap()
            .collect_garbage(&mut pair.js)
            .unwrap();
    }
    assert_eq!(pair.live_workers(), 2, "a named Worker survives GC");
    pair.runtime
        .as_mut()
        .unwrap()
        .dispatch_worker_event(&mut pair.js, event.key, event.payload)
        .unwrap();
    pair.check(
        "if (received[0] !== 'answer') throw Error('the named worker was not heard: ' + received.join());",
    );
    pair.dispose();
    assert!(pair.finish().is_empty());
}

#[test]
fn collecting_a_worker_cancels_its_pending_source() {
    let mut pair = Pair::new(
        "import {Worker} from 'bobcat-internal'; globalThis.worker = new Worker('./worker.js');",
    );
    let completion = pair.source();
    pair.check("delete globalThis.worker;");
    pair.runtime
        .as_mut()
        .unwrap()
        .collect_garbage(&mut pair.js)
        .unwrap();
    assert_eq!(pair.live_workers(), 1);
    pair.dispose();
    pair.finish();
    assert!(completion.is_cancelled());
}

#[test]
fn ordinary_worker_does_not_acquire_app_teardown_by_importing_bts_or_using_its_name() {
    let mut pair = Pair::new(
        r"
        import {Worker} from 'bobcat-internal';
        globalThis.worker = new Worker('./worker.js', {name:'lynx-bg'});
    ",
    );
    pair.answer(
        r"
        import {lynx} from 'bobcat:bts-runtime';
        lynx.getApp().callDestroyLifetimeFun = () => postMessage('cleanup');
        postMessage('ready');
    ",
    );
    assert!(matches!(pair.next_event().unwrap().payload,
        WorkerPayload::Message(ref value) if posted(value, "ready")));
    pair.check("worker.terminate();");
    assert!(pair.finish().is_empty());
}

impl Pair {
    fn frame(&mut self, milliseconds: f64) {
        self.pump_host();
        let (commands, mut incoming) = mpsc::unbounded_channel();
        self.frame_demand.dispatch(milliseconds, &commands);
        while let Ok(crate::link::ToMain::Vsync(milliseconds)) = incoming.try_recv() {
            self.runtime
                .as_mut()
                .unwrap()
                .vsync(&mut self.js, milliseconds)
                .unwrap();
        }
    }
}

#[test]
fn vsync_can_resume_mts_and_bts_entries_awaiting_their_first_frame() {
    let mut pair = Pair::with_background(
        "globalThis.firstFrame = await new Promise(resolve => lynx.requestAnimationFrame(resolve));",
        Some(
            r"
            const time = await new Promise(resolve => {
                lynx.requestAnimationFrame(resolve);
                globalThis.postMessage('waiting for vsync');
            });
            if (time !== 500) throw Error('BTS frame timestamp');
            postMessage('bts-entry-ran');
        ",
        ),
    );
    // The boot module starts BTS only after the MTS entry finishes.
    pair.frame(250.0);
    pair.check("if (globalThis.firstFrame !== 250) throw Error('MTS frame timestamp');");
    let event = pair.next_event().unwrap();
    assert!(
        matches!(event.payload, WorkerPayload::Message(ref value) if posted(value, "waiting for vsync"))
    );
    pair.frame(500.0);
    pair.await_background_entry();
    assert!(worker_failures(pair.notices()).is_empty());
}

#[test]
fn animation_callbacks_use_display_timestamps_and_defer_nested_requests() {
    let mut pair = Pair::with_background(
        "globalThis.frames = []; lynx.getJSContext().addEventListener('frame', e => frames.push(e.data));",
        Some(
            r"
            const send = (label, time) => lynx.getCoreContext().dispatchEvent({type:'frame', data:[label,time]});
            lynx.requestAnimationFrame(time => {
                send('first', time);
                lynx.cancelAnimationFrame(cancelled);
                lynx.requestAnimationFrame(time => send('nested', time));
            });
            const cancelled = lynx.requestAnimationFrame(() => {throw Error('cancelled callback ran');});
            lynx.requestAnimationFrame(time => send('third', time));
            postMessage('bts-entry-ran');
        ",
        ),
    );
    // Every BTS frame request must be armed before the frame is dispatched.
    pair.await_background_entry();
    pair.frame(1250.0);
    pair.deliver();
    pair.deliver();
    // Wait until the callback's nested request is armed, independently of
    // delivery of the console/Context messages it posted earlier.
    let deadline = ClockInstant::now() + PATIENCE;
    loop {
        pair.pump_host();
        if pair.frame_demand.is_pending() {
            break;
        }
        assert!(
            ClockInstant::now() < deadline,
            "nested frame request was not armed"
        );
        std::thread::yield_now();
    }
    pair.check(r#"if (JSON.stringify(frames) !== '[["first",1250],["third",1250]]') throw Error(JSON.stringify(frames));"#);
    pair.frame(1500.0);
    pair.deliver();
    pair.check(r#"if (JSON.stringify(frames) !== '[["first",1250],["third",1250],["nested",1500]]') throw Error(JSON.stringify(frames));"#);
    assert!(worker_failures(pair.notices()).is_empty());
}

#[test]
fn bts_animation_frames_continue_while_an_mts_callback_is_blocked() {
    let mut pair = Pair::unbooted(Some(
        r"
        function frame(time) {
            lynx.requestAnimationFrame(frame);
            globalThis.postMessage({frame:time});
        }
        lynx.requestAnimationFrame(frame);
        globalThis.postMessage('bts-entry-ran');
    ",
    ));
    let (blocked, waiting) = std::sync::mpsc::channel();
    let (release, released) = std::sync::mpsc::channel();
    pair.runtime
        .as_mut()
        .unwrap()
        .core
        .engine
        .register_host_module_function(
            &mut pair.js,
            "test:gate",
            "block",
            0,
            Box::new(move |_| {
                blocked.send(()).unwrap();
                released
                    .recv_timeout(PATIENCE)
                    .map_err(|error| error.to_string())?;
                Ok(HostValue::Undefined)
            }),
        )
        .unwrap();
    pair.boot("import {block} from 'test:gate'; lynx.requestAnimationFrame(() => block());")
        .unwrap();
    // The BTS frame request must be armed before the first dispatch below.
    pair.await_background_entry();
    let mut events = std::mem::replace(&mut pair.events, mpsc::unbounded_channel().1);
    pair.pump_host();
    let (main_commands, mut incoming) = mpsc::unbounded_channel();
    pair.frame_demand.dispatch(1000.0, &main_commands);
    let mut frames = std::mem::take(&mut pair.frame_demand);
    let mut notices = std::mem::replace(&mut pair.view.notices, mpsc::unbounded_channel().1);
    let mut other = VecDeque::new();
    let observer = std::thread::spawn(move || {
        waiting.recv_timeout(PATIENCE).unwrap();
        for milliseconds in [1000, 2000, 3000] {
            if milliseconds != 1000 {
                collect_host_notices(&mut frames, &mut notices, &mut other);
                frames.dispatch(f64::from(milliseconds), &main_commands);
            }
            let event = block_on_deadline(events.recv(), ClockInstant::now() + PATIENCE)
                .flatten()
                .expect("BTS advances while MTS is blocked");
            let WorkerPayload::Message(message) = event.payload else {
                panic!("unexpected worker event");
            };
            assert_eq!(
                crate::background::wire_json(&message),
                format!(r#"{{"frame":{milliseconds}}}"#)
            );
        }
        release.send(()).unwrap();
        (events, frames, notices, other)
    });
    let crate::link::ToMain::Vsync(milliseconds) = incoming.try_recv().unwrap() else {
        panic!("MTS vsync");
    };
    pair.runtime
        .as_mut()
        .unwrap()
        .vsync(&mut pair.js, milliseconds)
        .unwrap();
    let (events, frames, notices, other) = observer.join().unwrap();
    pair.events = events;
    pair.frame_demand = frames;
    pair.view.notices = notices;
    pair.deferred_notices.extend(other);
    assert!(worker_failures(pair.notices()).is_empty());
}

#[test]
fn mts_animation_frames_continue_while_a_bts_callback_is_busy() {
    let mut pair = Pair::with_background(
        r"globalThis.frames = []; function frame(time) {
            frames.push(time); lynx.requestAnimationFrame(frame);
        } lynx.requestAnimationFrame(frame);",
        Some(
            r"
            lynx.requestAnimationFrame(() => {
                globalThis.postMessage('busy');
                const until = Date.now() + 4000;
                while (Date.now() < until) {}
                globalThis.postMessage('finished');
            });
            globalThis.postMessage('bts-entry-ran');
        ",
        ),
    );
    pair.await_background_entry();
    pair.frame(1000.0);
    let event = pair.next_event().unwrap();
    assert!(matches!(event.payload, WorkerPayload::Message(ref value) if posted(value, "busy")));
    pair.frame(2000.0);
    pair.frame(3000.0);
    pair.check(
        r"if (JSON.stringify(frames) !== '[1000,2000,3000]') throw Error(JSON.stringify(frames));",
    );
    assert!(
        matches!(
            pair.events.try_recv(),
            Err(mpsc::error::TryRecvError::Empty)
        ),
        "MTS frames finish before the long BTS callback returns"
    );
    let event = pair.next_event().unwrap();
    assert!(
        matches!(event.payload, WorkerPayload::Message(ref value) if posted(value, "finished"))
    );
}

#[path = "../../../../../packages/reactlynx-test-fixtures/fixtures.rs"]
mod fixtures;

#[test]
fn releasing_a_view_unmounts_the_real_native_react_tree() {
    verify_react_teardown(false, false);
}

#[test]
fn releasing_a_reloaded_view_unmounts_the_new_react_tree() {
    verify_react_teardown(true, false);
}

#[test]
fn releasing_a_development_view_unmounts_the_current_react_tree() {
    verify_react_teardown(false, true);
    verify_react_teardown(true, true);
}

/// A compiled `ReactLynx` `<list>`, end to end over both realms.
///
/// A card never builds a list's children: the framework records them as index
/// operations, writes them with `__SetAttribute(list, 'update-list-info', …)`
/// and files the `componentAtIndex`/`enqueueComponent` pair that builds and
/// retires them. What this pins is that the cells arrive as real element
/// children of the list element, in `item-key` order, each carrying its own
/// `<text>` — and that a tap whose handler lives on the *background* thread
/// produces a second batch whose removals and insertion land in the same
/// tree, through the same pair.
#[test]
fn a_compiled_react_list_receives_its_cells_and_a_tap_edits_them() {
    const BUNDLE_URL: &str = "app:///nested/card.lynx.bundle";

    let bytes = fixtures::fixture("react-list").page;
    let template = bobcat_source::native::decode(bytes).unwrap();
    let background = format!(
        "import {{__BobcatRegisterBundle}} from 'bobcat:bts-runtime';\n\
         __BobcatRegisterBundle({url});\n\
         lynx.requireModule('/app-service.js');",
        url = serde_json::to_string(BUNDLE_URL).unwrap(),
    );
    let mut pair = Pair::unbooted_with_data(Some(&background), RealmStartup::default());
    pair.boot(&template.lepus_code["react-list__main-thread"])
        .unwrap();

    // The first screen is the main thread's own render, and it ends in the
    // checkpoint that drains the protocol's microtask: the forty cells are
    // there before the background thread has been asked for anything.
    let first_screen = pair.runtime.as_mut().unwrap().with_document(list_cells);
    assert_eq!(first_screen.len(), 40, "{first_screen:?}");
    assert_eq!(
        first_screen,
        (0..40)
            .map(|row| format!("cell-{row}=row {row}"))
            .collect::<Vec<_>>(),
    );

    // In the order the card asks for them, each a worker job parked on this
    // answer.
    serve_bundle_path(&mut pair, &template, "/app-service.js");
    for path in template.manifest.keys() {
        if path != "/app-service.js" {
            serve_bundle_path(&mut pair, &template, path);
        }
    }

    // The tap's handler is a background-thread one, so the edit is a round
    // trip: publish, re-render there, hydrate here. The three dropped rows
    // become a `removeAction` of ascending *old* indices and the appended
    // row an `insertAction`; both are one batch.
    let header = pair.runtime.as_mut().unwrap().with_document(|document| {
        document
            .query_selector(document.document_element().id(), "#header")
            .expect("a valid selector")
            .expect("the card's header")
    });
    let edited: Vec<String> = [1, 3, 5]
        .iter()
        .fold(
            (0..40).map(|row| format!("cell-{row}=row {row}")).collect(),
            |rows: Vec<String>, dropped| {
                rows.into_iter()
                    .filter(|row| !row.starts_with(&format!("cell-{dropped}=")))
                    .collect()
            },
        )
        .into_iter()
        .chain(std::iter::once("cell-40=row 40".to_owned()))
        .collect();
    pair.runtime
        .as_mut()
        .unwrap()
        .dispatch_for_test(&mut pair.js, header, "tap", dom::Point2D::new(4.0, 4.0))
        .expect("the tap is delivered");
    let deadline = ClockInstant::now() + PATIENCE;
    loop {
        assert!(
            ClockInstant::now() < deadline,
            "the background thread's edit reached the list"
        );
        pair.deliver();
        assert!(worker_failures(pair.notices()).is_empty());
        if pair.runtime.as_mut().unwrap().with_document(list_cells) == edited {
            break;
        }
    }
}

/// Every element child of the card's list, as `<item-key>=<its text>`.
///
/// Asserting the shape here rather than in the test body keeps the two
/// call sites reading as the orders they are about.
fn list_cells(document: &mut LynxDocument) -> Vec<String> {
    let list = document
        .query_selector(document.document_element().id(), "#cells")
        .expect("a valid selector")
        .expect("the card's list element");
    let list = document.get(list).expect("a queried node is live");
    assert_eq!(list.tag_name(), Some("list"));
    list.children()
        .map(|cell| {
            assert_eq!(cell.tag_name(), Some("list-item"));
            let text = cell.children().next().expect("the cell's own text");
            assert_eq!(text.tag_name(), Some("text"));
            format!(
                "{}={}",
                cell.attribute("item-key").expect("every cell is keyed"),
                generated_text(text),
            )
        })
        .collect()
}

/// The text a `<text>` renders, wherever the framework put it: a `raw-text`
/// element's `text` attribute for generated content, a host text node's data
/// otherwise.
fn generated_text(node: &dom::Node<()>) -> String {
    let mut text = String::new();
    for child in node.children() {
        if let Some(data) = child.text() {
            text.push_str(data);
        } else if let Some(attribute) = child.attribute("text") {
            text.push_str(attribute);
        } else {
            text.push_str(&generated_text(child));
        }
    }
    text
}

/// Answers one manifest path of a native bundle, at the URL `PageSource`
/// registers it under — beside the bundle's own URL — with the body served
/// through the same adaptation `PageSource` applies at registration.
///
/// The card asks for each of them itself: `lynx.requireModule` builds this
/// URL and loads it synchronously, so a load is a worker job parked on this
/// answer, in the order the card asks, not a boot-time import of every path.
fn serve_bundle_path(pair: &mut Pair, template: &bobcat_source::web::WebTemplate, path: &str) {
    pair.serve_module(
        &format!("app:///nested{path}"),
        &bobcat_source::bts_module_source(
            bobcat_source::BundleTarget::Lynx,
            &template.manifest[path],
        ),
    );
}

fn verify_react_teardown(reload: bool, development: bool) {
    const BUNDLE_URL: &str = "app:///nested/card.lynx.bundle";

    let bytes: &[u8] = if development {
        fixtures::fixture("react-reload-development").page
    } else {
        fixtures::fixture("react-reload").page
    };
    let template = bobcat_source::native::decode(bytes).unwrap();
    // What `PageSource` writes for a source-based native bundle: the bundle's
    // own URL as the base every path of it resolves against, then the
    // `requireModule` that loads and starts the card. Nothing of the
    // container's bodies is in it.
    //
    // `lynx` is the test entry preamble's own import; only the registration
    // is named here, as `PageSource`'s own boot script names it.
    let background = format!(
        "import {{__BobcatRegisterBundle}} from 'bobcat:bts-runtime';\n\
         __BobcatRegisterBundle({url});\n\
         lynx.requireModule('/app-service.js');",
        url = serde_json::to_string(BUNDLE_URL).unwrap(),
    );
    let mut pair = Pair::unbooted_with_data(
        Some(&background),
        RealmStartup {
            init_data: Some(serde_json::json!({"seed":0,"keep":"retained"}).to_string()),
            ..RealmStartup::default()
        },
    );

    pair.boot(&template.lepus_code["react-reload__main-thread"])
        .unwrap();
    // In the order the card asks for them, which is its own: the entry, then
    // the chunk its `init` reaches for. Each is a job of the worker parked on
    // this answer — nothing imported either ahead of time.
    serve_bundle_path(&mut pair, &template, "/app-service.js");
    for path in template.manifest.keys() {
        if path != "/app-service.js" {
            serve_bundle_path(&mut pair, &template, path);
        }
    }
    let mut missing_websocket_warnings = 0;
    for seed in if reload { &[0, 2][..] } else { &[0][..] } {
        if *seed == 2 {
            pair.runtime
                .as_mut()
                .unwrap()
                .apply_page_update(
                    &mut pair.js,
                    &crate::link::PageUpdate::Reload {
                        data: r#"{"seed":2}"#.into(),
                        processor_name: String::new(),
                    },
                )
                .unwrap();
        }
        // Complete actual hydration and its patch acknowledgement, so the
        // fixture's useEffect and GlobalEventEmitter listener have mounted.
        let deadline = ClockInstant::now() + PATIENCE;
        loop {
            assert!(ClockInstant::now() < deadline, "React effect did not mount");
            pair.deliver();
            let notices = pair.notices();
            assert!(!worker_failed(&notices, ""));
            for notice in &notices {
                if let ViewNotice::Engine(crate::EngineEvent::ScriptReported { level, message }) =
                    notice
                {
                    assert!(development && level == "warning"
                        && message.contains("WebSocket is not found. Please use Lynx >= 2.16 or consider using a polyfill."),
                        "unexpected React report: {message}");
                    missing_websocket_warnings += 1;
                }
            }
            if notices.iter().any(|notice| {
                matches!(notice,
                ViewNotice::Engine(crate::EngineEvent::ConsoleMessage { message, .. })
                if message == &format!("reload-mount {seed} retained 1"))
            }) {
                break;
            }
        }
    }
    assert_eq!(missing_websocket_warnings, usize::from(development));
    pair.cancel.cancel();
    let events = pair.dispose();
    let cleanups = events
        .iter()
        .filter_map(|event| {
            let message: serde_json::Value = serde_json::from_str(event).unwrap();
            assert_ne!(message["method"], "reportError", "{event}");
            (message["method"] == "console")
                .then(|| message["message"].as_str().unwrap().to_owned())
                .filter(|message| message.starts_with("reload-cleanup "))
        })
        .collect::<Vec<_>>();
    assert_eq!(
        cleanups.len(),
        1,
        "the compiled useEffect cleanup runs on view release"
    );
    assert_eq!(
        cleanups[0],
        if reload {
            "reload-cleanup 2"
        } else {
            "reload-cleanup 0"
        }
    );
}

#[test]
fn a_background_module_call_reaches_the_embedder_and_its_callback_answers_the_realm() {
    let mut pair = Pair::with_native_modules(
        r"
        globalThis.results = [];
        lynx.getJSContext().addEventListener('reply', e => results.push(e.data));
        ",
        r"
        const answered = lynx.getApp().NativeModules.Echo.ping(1, {x:1}, (...args) => {
            lynx.getCoreContext().dispatchEvent({ type: 'reply', data: args });
        }, 's');
        if (answered !== undefined) throw Error('a module method answers nothing');
        ",
        &[("Echo", &["ping"])],
    );

    let (module, call) = pair.module_call();
    assert_eq!(module, "Echo");
    assert_eq!(call.method, "ping");
    assert_eq!(
        call.arguments, r#"[1,{"x":1},null,"s"]"#,
        "the function argument is null in the JSON and a callback beside it"
    );
    let [callback] = <[_; 1]>::try_from(call.callbacks).expect("one function argument");
    assert_eq!(callback.argument_index(), 2);
    assert!(!callback.is_cancelled());
    callback.invoke(r#"["pong",2]"#.to_owned());

    pair.deliver();
    pair.check(
        r#"if (JSON.stringify(results) !== '[["pong",2]]') throw Error(JSON.stringify(results));"#,
    );
    assert!(worker_failures(pair.notices()).is_empty());
}

#[test]
fn a_callback_released_uninvoked_never_runs_and_fails_nothing() {
    let mut pair = Pair::with_native_modules(
        r"
        globalThis.results = [];
        lynx.getJSContext().addEventListener('reply', e => results.push(e.data));
        ",
        r"
        const core = lynx.getCoreContext();
        const modules = lynx.getApp().NativeModules;
        modules.Echo.ping(() => core.dispatchEvent({ type: 'reply', data: 'released' }));
        modules.Echo.ping(() => core.dispatchEvent({ type: 'reply', data: 'answered' }));
        ",
        &[("Echo", &["ping"])],
    );

    // Dropped rather than invoked: the realm hears that its function is over,
    // and the function itself never runs.
    let (_, released) = pair.module_call();
    drop(released);
    let (_, answered) = pair.module_call();
    let [callback] = <[_; 1]>::try_from(answered.callbacks).expect("one function argument");
    callback.invoke("[]".to_owned());

    pair.deliver();
    pair.check(
        r#"if (JSON.stringify(results) !== '["answered"]') throw Error(JSON.stringify(results));"#,
    );
    assert!(worker_failures(pair.notices()).is_empty());
}

#[test]
fn a_module_the_view_lacks_and_a_method_it_did_not_declare_are_both_undefined() {
    let mut pair = Pair::with_native_modules(
        "",
        &format!(
            r"
            const modules = lynx.getApp().NativeModules;
            if (modules.Missing !== undefined) throw Error('an absent module is undefined');
            if (modules.Echo.nope !== undefined) throw Error('an undeclared method is undefined');
            if (typeof modules.Echo.ping !== 'function') throw Error('a declared method is callable');
            postMessage('{BTS_ENTRY_RAN}');
            "
        ),
        &[("Echo", &["ping"])],
    );
    pair.await_background_entry();
    assert!(worker_failures(pair.notices()).is_empty());
}

#[test]
fn a_callback_for_a_worker_that_has_ended_says_so_and_invoking_it_does_nothing() {
    let mut pair = Pair::with_native_modules(
        "",
        r"lynx.getApp().NativeModules.Echo.ping(() => postMessage('unreachable'));",
        &[("Echo", &["ping"])],
    );
    let (_, call) = pair.module_call();
    let [callback] = <[_; 1]>::try_from(call.callbacks).expect("one function argument");

    // The realm that made the call is gone with the runtime that owned its
    // Worker handle, so there is nothing left to answer.
    let events = pair.finish();
    assert!(
        callback.is_cancelled(),
        "a callback whose worker has ended says so"
    );
    callback.invoke("[]".to_owned());
    assert!(
        !events.iter().any(|event| matches!(
            event.payload,
            WorkerPayload::Message(ref value) if posted(value, "unreachable")
        )),
        "the released realm ran nothing"
    );
}
