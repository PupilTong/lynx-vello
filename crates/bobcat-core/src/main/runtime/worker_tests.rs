//! Real `QuickJS` on both sides, with the test acting as the resource-owning
//! painter. No GPU is needed to verify contexts, transport and teardown.
use std::time::Duration;

use super::*;
use crate::background::WorkerHome;
use crate::mailbox::Mailbox;
use crate::main::StartupControl;
use crate::main::workers::WorkerFactory;
use crate::resource::{LoadedSource, SourceCompletion, SourceRequest};
use crate::view::{DETACHED_VIEW, NoWakeup, ToMain};

struct Pair {
    runtime: Option<MainThreadRuntime<NoWakeup>>,
    js: ScriptRuntime,
    events: Mailbox<ToMain>,
    notifications: Mailbox<ToPainter>,
    home: WorkerHome,
}

impl Pair {
    fn new(script: &str) -> Self {
        let (to_main, events) = Mailbox::channel();
        let home = WorkerHome::start(to_main).unwrap();
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
                Arc::new(StartupControl::default()),
            )
            .unwrap();
        runtime
            .run_main_thread_script(&mut js, script, "app:///nested/main.js")
            .unwrap();
        Self {
            runtime: Some(runtime),
            js,
            events,
            notifications: received,
            home,
        }
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
