//! Real `QuickJS` on both sides, with the test acting as the resource-owning
//! painter. No GPU is needed to verify contexts, transport and teardown.
use std::time::Duration;

use super::*;
use crate::background::{WorkerHome, WorkerScript};
use crate::mailbox::Mailbox;
use crate::main::StartupControl;
use crate::main::workers::WorkerFactory;
use crate::resource::{LoadedSource, SourceCompletion, SourceRequest};
use crate::view::{DETACHED_VIEW, NoWakeup, ToMain};

struct Pair {
    runtime: Option<MainThreadRuntime<NoWakeup>>,
    js: ScriptRuntime,
    events: Mailbox<ToMain>,
    notifications: Rc<Mailbox<ToPainter>>,
    home: WorkerHome,
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
        let (to_main, events) = Mailbox::channel();
        let home = match background_source {
            Some(source) => WorkerHome::with_entry_for_test(
                to_main,
                WorkerScript {
                    source: source.to_owned(),
                    url: "test:bts-entry".to_owned(),
                },
            ),
            None => WorkerHome::start(to_main).unwrap(),
        };
        let (notifications, received) = Mailbox::channel();
        let notify = ToPainterSender::new(
            DETACHED_VIEW,
            notifications,
            Arc::default(),
            Arc::new(NoWakeup),
        );
        let mut js = ScriptRuntime::new().unwrap();
        install_shared_modules(&mut js).unwrap();
        let document = crate::main::tree::new_document(
            crate::view::Viewport::new(32.0, 24.0),
            crate::main::tree::PageConfig::default(),
        );
        let mut runtime = MainThreadRuntime::new(&mut js, document, notify.clone()).unwrap();
        runtime
            .install_workers(
                &mut js,
                &WorkerFactory::new(home.commands()),
                notify,
                "app:///nested/main.js",
                background_source.map(|_| "test:bts-entry".to_owned()),
                Arc::new(StartupControl::default()),
            )
            .unwrap();
        Self {
            runtime: Some(runtime),
            js,
            events,
            notifications: Rc::new(received),
            home,
        }
    }

    fn boot(&mut self, script: &str) -> Result<(), MainThreadError> {
        self.runtime.as_mut().unwrap().run_main_thread_script(
            &mut self.js,
            script,
            "app:///nested/main.js",
        )
    }

    fn source(&self) -> SourceCompletion {
        loop {
            let (_, notification) = self.notifications.try_recv().expect("source requested");
            if let ToPainter::RequestWorkerSource {
                request,
                completion,
            } = notification
            {
                assert!(
                    matches!(request, SourceRequest::Worker { specifier, base_url }
                    if specifier == "./worker.js" && base_url == "app:///nested/main.js")
                );
                return completion;
            }
        }
    }

    fn answer(&self, source: &str) {
        self.source().complete(Ok(LoadedSource::Entry {
            source: source.into(),
            url: "app:///nested/worker.js".into(),
        }));
    }

    fn deliver(&mut self) {
        let (view, event) = self
            .events
            .recv(Some(ClockInstant::now() + Duration::from_secs(10)))
            .unwrap();
        assert_eq!(view, Some(DETACHED_VIEW));
        let ToMain::Worker { key, payload } = event else {
            panic!("worker event")
        };
        self.runtime
            .as_mut()
            .unwrap()
            .dispatch_worker_event(&mut self.js, key, payload)
            .unwrap();
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

impl Drop for Pair {
    fn drop(&mut self) {
        drop(self.runtime.take());
        self.home.join();
    }
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
    let event = pair
        .events
        .recv(Some(ClockInstant::now() + Duration::from_secs(10)))
        .unwrap();
    pair.check("worker.terminate(); worker.terminate(); worker.postMessage('ignored');");
    let (_, ToMain::Worker { key, payload }) = event else {
        panic!("worker event")
    };
    pair.runtime
        .as_mut()
        .unwrap()
        .dispatch_worker_event(&mut pair.js, key, payload)
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
    drop(pair.runtime.take());
    assert!(completion.is_cancelled());
    // This must finish even though the host still holds the completion.
    pair.home.join();
    completion.complete(Ok(LoadedSource::Entry {
        source: "throw Error('cancelled worker ran');".into(),
        url: "app:///late.js".into(),
    }));
    assert!(pair.events.try_recv().is_err());
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
    assert!(
        !pair
            .notifications
            .drain()
            .any(|(_, event)| matches!(event, ToPainter::RequestWorkerSource { .. }))
    );
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
        import { lynx } from 'bobcat:bts';
        if (lynx !== globalThis.lynx) throw Error('module/global identity');
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
fn background_contexts_exchange_typed_events_and_flush_early_references_in_order() {
    let mut pair = Pair::with_background(
        r"
        import { EventTarget } from 'bobcat:event-target';
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
    assert!(
        !pair
            .notifications
            .drain()
            .any(|(_, event)| matches!(event, ToPainter::RequestWorkerSource { .. }))
    );
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
    let failures: Vec<_> = pair
        .notifications
        .drain()
        .filter_map(|(_, notification)| match notification {
            ToPainter::Engine(crate::EngineEvent::WorkerFailed(error)) => Some(error),
            _ => None,
        })
        .collect();
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
    // The ordinary worker's Script follows the built-in BTS's Start and Script
    // in the same FIFO. Its answer proves that BTS boot has had its turn.
    pair.answer("onmessage = event => postMessage(event.data);");
    pair.deliver();
    pair.check("if (answer !== 'barrier') throw Error('worker barrier failed');");
    assert!(!pair.notifications.drain().any(|(_, notification)| {
        matches!(
            notification,
            ToPainter::RequestWorkerSource { .. }
                | ToPainter::Engine(crate::EngineEvent::WorkerFailed(_))
        )
    }));
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
    assert!(
        !pair
            .notifications
            .drain()
            .any(|(_, event)| matches!(event, ToPainter::RequestWorkerSource { .. }))
    );
    drop(pair.runtime.take());
    pair.home.join();
    assert!(pair.events.try_recv().is_err());
}
