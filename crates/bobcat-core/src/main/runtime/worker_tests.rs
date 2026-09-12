//! Real `QuickJS` on both sides, with the test acting as the resource-owning
//! embedder. No GPU is needed to verify contexts, transport and teardown.
use std::time::Duration;

use tokio::sync::mpsc;

use super::*;
use crate::background::{WorkerEvent, WorkerHome, WorkerPayload};
use crate::link::{DetachedView, ViewNotice, block_on_deadline, detached_outbox};
use crate::main::workers::WorkerFactory;
use crate::resource::{LoadedSource, SourceCompletion, SourceRequest};
use crate::view::NoWakeup;

/// How long a test waits for a thread that should already be working.
const PATIENCE: Duration = Duration::from_secs(30);

struct Pair {
    runtime: Option<MainThreadRuntime>,
    js: ScriptRuntime,
    /// What this view's workers said, which is where every worker event
    /// arrives now — one channel per view rather than one per group.
    events: mpsc::UnboundedReceiver<WorkerEvent>,
    /// The host's end of the view's link: the test plays the embedder, so it
    /// is what answers every source request.
    view: DetachedView,
    /// This view's end signal, the one every completion it hands out was
    /// built with and the parent of every worker token its realm mints. The
    /// test plays the embedder, so releasing a view is something it has to
    /// spell.
    cancel: tokio_util::sync::CancellationToken,
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
        let home = match background_source {
            Some(source) => {
                WorkerHome::with_entry_for_test((source.to_owned(), "test:bts-entry".to_owned()))
            }
            None => WorkerHome::start().unwrap(),
        };
        let (outbox, view) = detached_outbox(Arc::new(NoWakeup));
        let cancel = view.token.clone();
        let mut js = ScriptRuntime::new().unwrap();
        install_shared_modules(&mut js).unwrap();
        let ingredients = DocumentIngredients::for_test(
            crate::view::Viewport::new(32.0, 24.0),
            crate::main::tree::PageConfig::default(),
        );
        let (runtime, events) = MainThreadRuntime::new(
            &mut js,
            ingredients,
            outbox,
            &WorkerFactory::new(home.commands()),
            "app:///nested/main.js",
            background_source.map(|_| "test:bts-entry".to_owned()),
            PageData::default(),
        )
        .unwrap();
        Self {
            runtime: Some(runtime),
            js,
            events,
            view,
            cancel,
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

    /// The next source request the realm made, which for these tests is
    /// always a worker script.
    fn source(&mut self) -> SourceCompletion {
        loop {
            let notice = self.view.notices.try_recv().expect("source requested");
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
        block_on_deadline(self.events.recv(), ClockInstant::now() + PATIENCE).flatten()
    }

    /// How many workers this realm still holds the right to stop.
    fn live_workers(&self) -> usize {
        self.runtime.as_ref().unwrap().live_workers()
    }

    /// Everything the realm has said to its host so far.
    fn notices(&mut self) -> Vec<ViewNotice> {
        let mut notices = Vec::new();
        while let Ok(notice) = self.view.notices.try_recv() {
            notices.push(notice);
        }
        notices
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
fn engine_render_delivers_lifecycle_to_the_current_background_app_hook() {
    let mut pair = Pair::with_background(
        r"
        globalThis.results = [];
        lynx.getJSContext().addEventListener('reply', e => results.push(e.data));
        globalThis.processData = () => ({ answer: 42 });
        globalThis.renderPage = false;
        await Promise.resolve();
        lynx.getEngine().addEventListener('__RenderPage', e => {
            __OnLifecycleEvent(['render', e.data]);
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
            e.detail.answer = 99;
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
        .dispatch_event(
            &mut pair.js,
            dom::NodeId::from_bits(3).unwrap(),
            "tap",
            r#"{"answer":42}"#,
        )
        .unwrap();
    pair.deliver();
    pair.deliver();
    pair.check(
        r"
        const [child, page] = results;
        if (child[0] !== '' || page[0] !== 'opaque:root') throw Error('handler name changed');
        const e = child[1];
        if (e.target.id !== 'button' || e.currentTarget.uid !== 3 ||
            e.target.dataset.itemName !== 'first' || e.detail.answer !== 42 ||
            e.target.dataset.count !== 7 || e.target.dataset.nested.value !== 'before' ||
            page[1].target.dataset.nested.value !== 'after' ||
            'elementRefptr' in e.target || 'stopPropagation' in e ||
            page[1].currentTarget.uid !== 2) throw Error(JSON.stringify(results));
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
        lynx.getJSContext().dispatchEvent({ type: 'install' });
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
        lynx.getJSContext().dispatchEvent({ type: 'install' });
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
fn a_js_lifetime_event_calls_the_background_hook_without_releasing_the_worker() {
    let mut pair = Pair::with_background(
        r"
        globalThis.results = [];
        lynx.getJSContext().addEventListener('reply', e => results.push(e.data));
        lynx.getEngine().dispatchEvent({ type: '__DestroyLifetime' });
        ",
        Some(
            r"
        const app = lynx.getApp();
        const core = lynx.getCoreContext();
        app.callDestroyLifetimeFun = function(...args) {
            if (this !== app || args.length !== 0) throw Error('wrong lifetime call');
            core.dispatchEvent({ type: 'reply', data: 'hook' });
            throw Error('lifetime hook failure');
        };
        core.addEventListener('ping', () => core.dispatchEvent({ type: 'reply', data: 'alive' }));
        ",
        ),
    );
    pair.deliver();
    pair.deliver();
    assert!(worker_failed(&pair.notices(), "lifetime hook failure"));
    pair.check(
        r#"
        import { lynx } from 'bobcat:runtime';
        if (JSON.stringify(results) !== '["hook"]') throw Error(JSON.stringify(results));
        lynx.getJSContext().dispatchEvent({ type: 'ping' });
        "#,
    );
    pair.deliver();
    pair.check(r#"if (JSON.stringify(results) !== '["hook","alive"]') throw Error(JSON.stringify(results));"#);
}

#[test]
fn constructor_creates_distinct_contexts_and_queues_messages_in_order() {
    let mut pair = Pair::new(
        r"
        import { Worker } from 'bobcat-internal';
        if (typeof globalThis.Worker !== 'undefined') throw Error('unexpected global');
        globalThis.results = [];
        for (const name of ['one', 'two']) {
            const worker = new Worker('./worker.js', { name, type: 'module' });
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
        worker.onerror = e => errors.push(e.message);
        worker.addEventListener('message', e => messages.push(e.data));
        worker.postMessage('ping');
    ",
    );
    pair.answer("onmessage = e => postMessage(e.data); throw Error('worker boom');");
    pair.deliver();
    pair.deliver();
    pair.check("if (errors.length !== 1 || !errors[0].includes('worker boom') || messages[0] !== 'ping') throw Error('worker error recovery');");
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

#[test]
fn dropping_the_view_cancels_io_without_keeping_the_worker_thread_alive() {
    let mut pair =
        Pair::new("import { Worker } from 'bobcat-internal'; new Worker('./worker.js');");
    let completion = pair.source();
    // Where `Painter::shutdown` sets it: first, before anything this view
    // owns is released, so a host still holding a completion learns that its
    // load no longer matters without waiting for a turn of the engine's.
    pair.cancel.cancel();
    assert!(
        completion.is_cancelled(),
        "cancellation precedes releasing the fetcher"
    );
    // The cancel above already ended that worker's task: it is parked on its
    // script, and the token it holds is a child of the one just cancelled.
    // Releasing the realm drops the one sender it was listening on, which is
    // the backstop for a worker whose realm was gone before it could speak —
    // and this must finish even though the host is still holding the
    // completion.
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

/// A released realm stops the workers it created, rather than leaving each of
/// them to discover that nobody is talking to it any more.
///
/// Pinned with a worker that would never end on its own: it arms an interval,
/// so its task always has work and a thread that waited for it to finish would
/// wait forever. What ends it is the `Terminate` its realm sends as it drops.
///
/// The message is the protocol and the channel closing behind it is the
/// backstop, but the two are not separately observable from here: the same
/// statement sends the one and drops the other. So what this pins is that the
/// worker's task ends, not which of the two ended it — read through the events
/// channel, whose senders are the realm's own and one clone per live worker
/// task.
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
/// running, which is what a release sends its `Terminate`s to.
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
        "if (JSON.stringify(result) !== '{\"type\":\"reply\",\"data\":[\"ordinary\",42]}') throw Error(JSON.stringify(result));",
    );
}

#[test]
fn background_contexts_exchange_typed_events_and_flush_early_payload_references_in_order() {
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
            if (this !== context) throw Error('listener receiver');
            results.push(event.data);
        });
        const first = {type: 'request', data: {value: 1}};
        if (context.dispatchEvent(first) !== 3) throw Error('dispatch result');
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
            if (this !== core) throw Error('listener receiver');
            if (core.dispatchEvent({type: 'reply', data: event.data}) !== 3) {
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
        Some("lynx.getCoreContext().dispatchEvent({type: 'ready'});"),
    );
    pair.check("if (!connected) throw Error('BTS was not connected');");
    pair.deliver();
    assert!(!asked_for_a_worker(&pair.notices()));
}

#[test]
fn context_post_message_remains_a_noop_on_both_realms() {
    let mut pair = Pair::with_background(
        r"
        const context = lynx.getJSContext();
        globalThis.results = [];
        context.addEventListener('ignored', () => { throw Error('postMessage delivered'); });
        context.addEventListener('ready', e => results.push(e.data));
        context.addEventListener('reply', e => results.push(e.data));
        context.postMessage({type: 'request', data: 'ignored'});
        context.dispatchEvent({type: 'request', data: 'typed'});
    ",
        Some(
            r"
        const core = lynx.getCoreContext();
        core.addEventListener('request', e => core.dispatchEvent({type: 'reply', data: e.data}));
        core.postMessage({type: 'ignored', data: 'ignored'});
        core.dispatchEvent({type: 'ready'});
    ",
        ),
    );
    pair.deliver();
    pair.deliver();
    pair.check(
        "if (JSON.stringify(results) !== '[{},\"typed\"]') throw Error(JSON.stringify(results));",
    );
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

#[test]
fn bts_invoke_reports_failures_and_later_queries_still_complete() {
    let mut pair = Pair::with_background(
        r"
        const page = __CreatePage();
        const item = __CreateView(); __SetID(item, 'item');
        __AppendElement(page, item);
        globalThis.result = null;
        lynx.getJSContext().addEventListener('queryDone', event => { result = event.data; });
        ",
        Some(
            r"
        void (async () => {
            const query = lynx.createSelectorQuery();
            const invoke = nodes => new Promise(resolve => nodes.invoke({
                method:'boundingClientRect', fail:resolve,
                success:() => { throw Error('UI method unexpectedly implemented'); },
            }).exec());
            const unsupported = await invoke(query.select('#item'));
            const missing = await invoke(query.select('#absent'));
            const multiple = await invoke(query.selectAll('view'));
            const after = await new Promise(resolve => query.select('#item').fields(
                {id:true}, (data, status) => resolve({data, status}),
            ).exec());
            lynx.getCoreContext().dispatchEvent({type:'queryDone', data:{unsupported, missing, multiple, after}});
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
        if (result.unsupported.code !== 1 || !result.unsupported.data.includes('not implemented')) throw Error(JSON.stringify(result));
        if (result.missing.code !== 2 || result.multiple.code !== 5) throw Error(JSON.stringify(result));
        if (result.after.status.code !== 0 || result.after.data.id !== 'item') throw Error(JSON.stringify(result));
    ");
    assert!(worker_failures(pair.notices()).is_empty());
}
