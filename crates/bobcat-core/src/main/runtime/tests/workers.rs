//! `Worker`, driven through a real second `QuickJS` runtime on a real second
//! thread.
//!
//! Every test here plays the two halves the engine's own threads play: the
//! painter, which answers a fetch request with bytes, and the group's command
//! loop, which routes what the worker thread says back into the realm. What
//! is real is everything between — the runtime, the thread, the realm on it,
//! and both directions of the boundary.

use super::*;
use crate::background::{WorkerCommand, WorkerEvent, WorkerHome, WorkerKey, WorkerScript};
use crate::mailbox::{Mailbox, Sender};
use crate::resource::SourceRequest;
use crate::view::{DETACHED_VIEW, ToMain};

/// How long a test waits for a thread that should already be working.
const PATIENCE: std::time::Duration = std::time::Duration::from_secs(10);

struct WorkerHarness {
    /// The painter's end of this view's link: where the realm's `Worker`
    /// leaves its fetch requests, and where the test picks them up.
    link: PainterLink,
    /// The line to `bobcat-workers`. The realm holds a clone; the test uses
    /// this one to answer a fetch the way a painter would.
    workers: Sender<WorkerCommand>,
    /// The group's own FIFO, which is where a worker's news arrives.
    from_workers: Mailbox<ToMain>,
    /// Held so the thread lives as long as the test does.
    #[expect(dead_code, reason = "held for its lifetime, never read")]
    home: WorkerHome,
}

impl WorkerHarness {
    /// Every script the realm has asked for since the last call.
    ///
    /// No waiting: `new Worker(...)` queues the notification on this link
    /// before it returns, so a drain right after the script that constructed
    /// one already has it.
    fn requests(&mut self) -> Vec<(WorkerKey, String)> {
        self.link.sync();
        self.link
            .take_source_requests()
            .into_iter()
            .map(|(request, worker)| match (request, worker) {
                (SourceRequest::WorkerScript(url), Some(key)) => (key, url),
                (other, _) => panic!("a running realm asks for nothing else: {other:?}"),
            })
            .collect()
    }

    /// Answers the one outstanding request with `source`, as a fetch would.
    fn answer(&mut self, source: &str) {
        let mut requests = self.requests();
        assert_eq!(requests.len(), 1, "exactly one worker was constructed");
        answer_next(&self.workers, &mut requests, source);
    }

    /// The next thing a worker realm said, delivered into the view's realm
    /// the way the group's command loop delivers it.
    fn deliver_next(
        &mut self,
        runtime: &mut MainThreadRuntime<NoWakeup>,
        js_runtime: &mut ScriptRuntime,
    ) -> Result<(), MainThreadError> {
        let event = self.next_event();
        runtime.deliver_worker_event(js_runtime, event.key, event.payload)
    }

    fn next_event(&mut self) -> WorkerEvent {
        let (view, command) = self
            .from_workers
            .recv(Some(std::time::Instant::now() + PATIENCE))
            .expect("the worker thread answered");
        let (Some(view), ToMain::Worker { key, payload }) = (view, command) else {
            panic!("the worker thread sends nothing else, and always addressed")
        };
        WorkerEvent { view, key, payload }
    }
}

// No `Drop`: the realm holds a sender on this channel too, and a test's
// locals drop the harness *before* the runtime that owns the realm. Joining
// here would wait on a thread whose last sender has not been dropped yet.
// Detaching is right for a test — the thread ends when both senders do —
// and the group's own join is exercised where a group is built.

/// Answers the next outstanding script request with `source`, as a fetch
/// would. Requests are answered in the order they were made, which is the
/// order the realm constructed its workers in.
fn answer_next(
    workers: &Sender<WorkerCommand>,
    pending: &mut Vec<(WorkerKey, String)>,
    source: &str,
) {
    assert!(!pending.is_empty(), "a worker was constructed");
    let (key, url) = pending.remove(0);
    let _ = workers.send((
        None,
        WorkerCommand::Script {
            key,
            script: Ok(WorkerScript {
                source: source.to_owned(),
                url,
            }),
        },
    ));
}

fn worker_runtime() -> (ScriptRuntime, MainThreadRuntime<NoWakeup>, WorkerHarness) {
    let (link, main) = detached_link(Arc::new(NoWakeup));
    let mut js_runtime = ScriptRuntime::new().expect("the test runtime starts");
    install_shared_modules(&mut js_runtime).expect("the shared modules register");
    let (to_main, from_workers) = Mailbox::channel();
    let home = WorkerHome::start(to_main).expect("the worker thread starts");
    let workers = home.commands();
    let runtime = MainThreadRuntime::new(
        &mut js_runtime,
        new_document(Viewport::new(393.0, 727.0), PageConfig::default()),
        main.notify,
        DETACHED_VIEW,
        home.commands(),
    )
    .expect("main-thread runtime");
    (
        js_runtime,
        runtime,
        WorkerHarness {
            link,
            workers,
            from_workers,
            home,
        },
    )
}

/// Asserts inside the realm: a module that throws is a failed test.
fn verify(runtime: &mut MainThreadRuntime<NoWakeup>, js_runtime: &mut ScriptRuntime, source: &str) {
    runtime
        .evaluate_module(js_runtime, source, "app:///verify.js", "verifying")
        .expect("verification");
}

#[test]
fn a_worker_receives_what_the_realm_posts_and_answers_it() {
    let (mut js_runtime, mut runtime, mut workers) = worker_runtime();
    runtime
        .run_main_thread_script(
            &mut js_runtime,
            r#"
                globalThis.seen = [];
                const worker = new Worker("app:///echo.js");
                worker.onmessage = (event) => seen.push(event.data.pong);
                worker.postMessage({ ping: 7 });
                "#,
            "app:///worker-entry.js",
        )
        .expect("main-thread script");

    workers.answer("onmessage = (event) => postMessage({ pong: event.data.ping + 1 });");
    workers
        .deliver_next(&mut runtime, &mut js_runtime)
        .expect("delivering the worker's message");

    verify(
        &mut runtime,
        &mut js_runtime,
        "if (seen.join('|') !== '8') throw new Error(seen.join('|'));",
    );
}

#[test]
fn what_is_posted_before_the_script_arrives_is_delivered_in_order() {
    let (mut js_runtime, mut runtime, mut workers) = worker_runtime();
    runtime
        .run_main_thread_script(
            &mut js_runtime,
            r#"
                globalThis.seen = [];
                const worker = new Worker("app:///order.js");
                worker.onmessage = (event) => seen.push(event.data);
                worker.postMessage("a");
                worker.postMessage("b");
                "#,
            "app:///order-entry.js",
        )
        .expect("main-thread script");

    // Both were posted before the worker had a realm to receive them, so both
    // waited on this side and cross the moment it does.
    workers.answer("onmessage = (event) => postMessage(event.data.toUpperCase());");
    for _ in 0..2 {
        workers
            .deliver_next(&mut runtime, &mut js_runtime)
            .expect("delivering the worker's message");
    }

    verify(
        &mut runtime,
        &mut js_runtime,
        "if (seen.join('|') !== 'A|B') throw new Error(seen.join('|'));",
    );
}

#[test]
fn a_worker_realm_shares_no_global_with_the_main_thread() {
    let (mut js_runtime, mut runtime, mut workers) = worker_runtime();
    runtime
        .run_main_thread_script(
            &mut js_runtime,
            r#"
                globalThis.mainOnly = "main";
                globalThis.seen = [];
                const worker = new Worker("app:///isolated.js");
                worker.onmessage = (event) => seen.push(event.data);
                worker.postMessage(null);
                "#,
            "app:///isolated-entry.js",
        )
        .expect("main-thread script");

    workers.answer(
        r#"
        globalThis.mainOnly = "worker";
        onmessage = () => postMessage(typeof globalThis.__CreateView);
        "#,
    );
    workers
        .deliver_next(&mut runtime, &mut js_runtime)
        .expect("delivering the worker's message");

    verify(
        &mut runtime,
        &mut js_runtime,
        r#"
        // The worker assigned its own `mainOnly`, on its own global, on its
        // own runtime: nothing about it can be seen from here.
        if (globalThis.mainOnly !== "main") throw new Error(globalThis.mainOnly);
        // And the Element PAPI is not installed on a global anywhere, least
        // of all a worker's.
        if (seen.join("|") !== "undefined") throw new Error(seen.join("|"));
        "#,
    );
}

#[test]
fn a_worker_cannot_import_the_element_papi() {
    let (mut js_runtime, mut runtime, mut workers) = worker_runtime();
    runtime
        .run_main_thread_script(
            &mut js_runtime,
            r#"
                globalThis.failures = [];
                const worker = new Worker("app:///reaching.js");
                worker.onerror = (event) => failures.push(event.message);
                "#,
            "app:///reaching-entry.js",
        )
        .expect("main-thread script");

    // `bobcat:element` is registered on `bobcat-main`'s runtime and on no
    // other, so a worker asking for the document cannot even resolve it. The
    // realm survives — see `a_worker_whose_script_throws_on_load_keeps_running`
    // for why the engine cannot tell this apart from a script that ran and
    // threw — but it has no handlers, so it answers nothing.
    workers.answer("import { __CreateView } from 'bobcat:element';\n__CreateView(0);");
    workers
        .deliver_next(&mut runtime, &mut js_runtime)
        .expect("delivering the worker's failure");

    verify(
        &mut runtime,
        &mut js_runtime,
        "if (failures.length !== 1) throw new Error(String(failures.length));",
    );
}

#[test]
fn a_worker_that_closes_itself_reports_and_stops() {
    let (mut js_runtime, mut runtime, mut workers) = worker_runtime();
    runtime
        .run_main_thread_script(
            &mut js_runtime,
            r#"
                globalThis.seen = [];
                globalThis.errors = 0;
                globalThis.worker = new Worker("app:///closing.js");
                worker.onmessage = (event) => seen.push(event.data);
                worker.onerror = () => { errors += 1; };
                // Two, so the second is queued behind the `close()` the
                // first causes and must be discarded rather than delivered.
                worker.postMessage("go");
                worker.postMessage("after");
                "#,
            "app:///closing-entry.js",
        )
        .expect("main-thread script");

    workers.answer(
        r"
        onmessage = (event) => {
          postMessage(event.data);
          close();
        };
        ",
    );
    // The message it sent before closing, then the close itself. If the
    // second message had been delivered there would be a third.
    for _ in 0..2 {
        workers
            .deliver_next(&mut runtime, &mut js_runtime)
            .expect("delivering the worker's news");
    }

    verify(
        &mut runtime,
        &mut js_runtime,
        r#"
        if (seen.join("|") !== "go") throw new Error(seen.join("|"));
        // A close is not a failure, so it fires nothing.
        if (errors !== 0) throw new Error(String(errors));
        // And posting to a worker that ended is a no-op rather than a throw.
        worker.postMessage("again");
        "#,
    );
}

#[test]
fn a_worker_script_that_will_not_load_fires_one_error() {
    let (mut js_runtime, mut runtime, mut workers) = worker_runtime();
    runtime
        .run_main_thread_script(
            &mut js_runtime,
            r#"
                globalThis.failures = [];
                const worker = new Worker("app:///missing.js");
                worker.addEventListener("error", (event) => failures.push(event.message));
                "#,
            "app:///missing-entry.js",
        )
        .expect("main-thread script");

    let mut requests = workers.requests();
    assert_eq!(requests.len(), 1);
    let (key, _) = requests.remove(0);
    let _ = workers.workers.send((
        None,
        WorkerCommand::Script {
            key,
            script: Err("404 Not Found".to_owned()),
        },
    ));
    // A fetch that failed is the group's news like any other: the thread that
    // was waiting for the script is the thread that reports there is none.
    workers
        .deliver_next(&mut runtime, &mut js_runtime)
        .expect("delivering the failure");

    verify(
        &mut runtime,
        &mut js_runtime,
        r#"
        if (failures.length !== 1) throw new Error(String(failures.length));
        if (!failures[0].includes("404 Not Found")) throw new Error(failures[0]);
        "#,
    );
}

#[test]
fn a_key_this_realm_was_never_issued_is_a_throw_and_not_another_view_s_worker() {
    let (mut js_runtime, mut runtime, mut workers) = worker_runtime();
    runtime
        .run_main_thread_script(
            &mut js_runtime,
            r#"
                globalThis.worker = new Worker("app:///mine.js");
                "#,
            "app:///invented-entry.js",
        )
        .expect("main-thread script");
    workers.answer("onmessage = (event) => postMessage(event.data);");

    // The `Worker` object is not the boundary: `bobcat-internal:host` is
    // importable by anything in the realm, so a card can call the member
    // directly with a number it made up. Only the key set stops it, and the
    // answer has to be synchronous.
    verify(
        &mut runtime,
        &mut js_runtime,
        r#"
        import { postWorkerMessage, terminateWorker } from "bobcat-internal:host";
        function refuses(call) {
          try { call(); } catch (error) { return true; }
          return false;
        }
        // A number no worker ever wore, one that is not a key at all, and a
        // key belonging to a *later* worker of some other view.
        for (const invented of [9999, -1, 0.5, Number.MAX_SAFE_INTEGER]) {
          if (!refuses(() => postWorkerMessage(invented, '["hi"]'))) {
            throw new Error(`postWorkerMessage accepted ${invented}`);
          }
          if (!refuses(() => terminateWorker(invented))) {
            throw new Error(`terminateWorker accepted ${invented}`);
          }
        }
        "#,
    );

    // And the worker this realm *was* issued still answers, so the check
    // refuses the invented keys rather than everything.
    verify(
        &mut runtime,
        &mut js_runtime,
        r#"worker.postMessage("still mine");"#,
    );
    let event = workers.next_event();
    assert!(matches!(
        event.payload,
        crate::background::WorkerPayload::Message(ref data) if data == "[\"still mine\"]"
    ));
}

#[test]
fn a_terminated_worker_is_no_longer_a_worker_this_realm_may_name() {
    let (mut js_runtime, mut runtime, mut workers) = worker_runtime();
    runtime
        .run_main_thread_script(
            &mut js_runtime,
            r#"
                globalThis.worker = new Worker("app:///terminated.js");
                "#,
            "app:///terminated-entry.js",
        )
        .expect("main-thread script");

    workers.answer("postMessage('hello');");
    verify(
        &mut runtime,
        &mut js_runtime,
        r#"
        worker.terminate();
        // Idempotent, and everything after it is a no-op rather than a throw.
        worker.terminate();
        worker.postMessage("ignored");
        "#,
    );

    // Whatever the worker managed to say before it was terminated is
    // delivered to a `Worker` that has forgotten it, and reaches nothing.
    let event = workers.next_event();
    runtime
        .deliver_worker_event(&mut js_runtime, event.key, event.payload)
        .expect("delivering to a terminated worker");
}

#[test]
fn a_worker_keeps_its_own_timers() {
    let (mut js_runtime, mut runtime, mut workers) = worker_runtime();
    runtime
        .run_main_thread_script(
            &mut js_runtime,
            r#"
                globalThis.seen = [];
                const worker = new Worker("app:///timers.js");
                worker.onmessage = (event) => seen.push(event.data);
                "#,
            "app:///timers-entry.js",
        )
        .expect("main-thread script");

    workers.answer("setTimeout(() => postMessage('late'), 1);");
    workers
        .deliver_next(&mut runtime, &mut js_runtime)
        .expect("delivering the worker's message");

    verify(
        &mut runtime,
        &mut js_runtime,
        "if (seen.join('|') !== 'late') throw new Error(seen.join('|'));",
    );
}

#[test]
fn two_workers_over_one_url_with_different_bodies_each_run_their_own() {
    let (mut js_runtime, mut runtime, mut workers) = worker_runtime();
    // One URL, two workers. Every view has its own fetcher, so one URL does
    // not name one body — and the group's worker runtime holds exactly one
    // source per module name, which is the trap.
    runtime
        .run_main_thread_script(
            &mut js_runtime,
            r#"
                globalThis.seen = [];
                for (const worker of [
                  new Worker("app:///same.js"),
                  new Worker("app:///same.js"),
                ]) {
                  worker.onmessage = (event) => seen.push(event.data);
                  worker.postMessage(null);
                }
                "#,
            "app:///same-url-entry.js",
        )
        .expect("main-thread script");

    let mut pending = workers.requests();
    assert_eq!(pending.len(), 2, "each constructor asks for its own script");
    answer_next(
        &workers.workers,
        &mut pending,
        "onmessage = () => postMessage('first');",
    );
    answer_next(
        &workers.workers,
        &mut pending,
        "onmessage = () => postMessage('second');",
    );
    for _ in 0..2 {
        workers
            .deliver_next(&mut runtime, &mut js_runtime)
            .expect("delivering the worker's message");
    }

    verify(
        &mut runtime,
        &mut js_runtime,
        r#"
        seen.sort();
        if (seen.join("|") !== "first|second") throw new Error(seen.join("|"));
        "#,
    );
}

#[test]
fn a_worker_whose_listener_throws_keeps_running() {
    let (mut js_runtime, mut runtime, mut workers) = worker_runtime();
    runtime
        .run_main_thread_script(
            &mut js_runtime,
            r#"
                globalThis.seen = [];
                globalThis.errors = [];
                globalThis.worker = new Worker("app:///throwing.js");
                worker.onmessage = (event) => seen.push(event.data);
                worker.onerror = (event) => errors.push(event.message);
                worker.postMessage("boom");
                "#,
            "app:///throwing-entry.js",
        )
        .expect("main-thread script");

    workers.answer(
        r"
        onmessage = (event) => {
          if (event.data === 'boom') throw new Error('handler exploded');
          postMessage(event.data);
        };
        ",
    );
    // The throw, reported at the parent — and the worker is still there.
    workers
        .deliver_next(&mut runtime, &mut js_runtime)
        .expect("delivering the worker's failure");
    verify(
        &mut runtime,
        &mut js_runtime,
        r#"
        if (errors.length !== 1) throw new Error(String(errors.length));
        if (!errors[0].includes("handler exploded")) throw new Error(errors[0]);
        worker.postMessage("again");
        "#,
    );
    workers
        .deliver_next(&mut runtime, &mut js_runtime)
        .expect("delivering the worker's message");

    verify(
        &mut runtime,
        &mut js_runtime,
        r#"
        // HTML reports an uncaught exception at the worker and then at its
        // parent, and leaves both running.
        if (seen.join("|") !== "again") throw new Error(seen.join("|"));
        "#,
    );
}

#[test]
fn a_script_stays_registerable_after_a_registration_fails() {
    let (mut js_runtime, mut runtime, mut workers) = worker_runtime();
    runtime
        .run_main_thread_script(
            &mut js_runtime,
            r#"
                globalThis.seen = [];
                globalThis.errors = [];
                for (const url of ["app:///bomb.js", "app:///pool.js", "app:///pool.js"]) {
                  const worker = new Worker(url);
                  worker.onmessage = (event) => seen.push(event.data);
                  worker.onerror = (event) => errors.push(event.message);
                }
                "#,
            "app:///poison-entry.js",
        )
        .expect("main-thread script");

    let mut pending = workers.requests();
    assert_eq!(pending.len(), 3);
    // Enough queued microtasks to outlast two whole job checkpoints, so the
    // runtime is left mid-drain and the *next* worker's registration fails
    // for a reason that has nothing to do with its own source.
    answer_next(
        &workers.workers,
        &mut pending,
        "for (let i = 0; i < 3000; i += 1) { Promise.resolve().then(() => {}); }",
    );
    answer_next(&workers.workers, &mut pending, "postMessage('pooled');");
    answer_next(&workers.workers, &mut pending, "postMessage('pooled');");

    // Whatever the first two did, the third worker over that URL must run:
    // a registration that failed leaves nothing recorded, so the URL is not
    // spent. Drain until the message arrives or the workers are all done.
    for _ in 0..3 {
        workers
            .deliver_next(&mut runtime, &mut js_runtime)
            .expect("delivering the worker's news");
    }

    verify(
        &mut runtime,
        &mut js_runtime,
        r#"
        if (!seen.includes("pooled")) {
            throw new Error(`seen=${seen.join("|")} errors=${errors.join("|")}`);
        }
        "#,
    );
}

#[test]
fn an_on_message_handler_and_an_identical_listener_are_two_registrations() {
    let (mut js_runtime, mut runtime, mut workers) = worker_runtime();
    // The DOM keeps a handler attribute and an explicit listener apart even
    // when they are the same function, because the attribute registers an
    // internal callback rather than the author's own.
    runtime
        .run_main_thread_script(
            &mut js_runtime,
            r#"
                globalThis.seen = [];
                globalThis.worker = new Worker("app:///twice.js");
                globalThis.handler = (event) => seen.push(event.data);
                worker.onmessage = handler;
                worker.addEventListener("message", handler);
                worker.postMessage(null);
                "#,
            "app:///twice-entry.js",
        )
        .expect("main-thread script");

    workers.answer("onmessage = () => postMessage('once');");
    workers
        .deliver_next(&mut runtime, &mut js_runtime)
        .expect("delivering the worker's message");

    verify(
        &mut runtime,
        &mut js_runtime,
        r#"
        if (seen.join("|") !== "once|once") throw new Error(seen.join("|"));
        // Removing the explicit listener leaves the handler attribute, and
        // clearing the attribute leaves nothing.
        seen.length = 0;
        worker.removeEventListener("message", handler);
        worker.dispatchEvent({ type: "message", data: "after-remove" });
        if (seen.join("|") !== "after-remove") throw new Error(seen.join("|"));
        seen.length = 0;
        worker.onmessage = null;
        worker.dispatchEvent({ type: "message", data: "silenced" });
        if (seen.length !== 0) throw new Error(seen.join("|"));
        "#,
    );
}

#[test]
fn a_worker_whose_script_throws_on_load_keeps_running() {
    let (mut js_runtime, mut runtime, mut workers) = worker_runtime();
    runtime
        .run_main_thread_script(
            &mut js_runtime,
            r#"
                globalThis.seen = [];
                globalThis.errors = [];
                globalThis.worker = new Worker("app:///half-broken.js");
                worker.onmessage = (event) => seen.push(event.data);
                worker.onerror = (event) => errors.push(event.message);
                "#,
            "app:///half-broken-entry.js",
        )
        .expect("main-thread script");

    // The shape that matters: handlers registered, then some optional piece of
    // start-up throws. HTML reports the exception and goes on to enable the
    // port queue and run the event loop, so the worker is up and listening.
    workers.answer(
        r"
        onmessage = (event) => postMessage(event.data.toUpperCase());
        throw new Error('optional init exploded');
        ",
    );
    workers
        .deliver_next(&mut runtime, &mut js_runtime)
        .expect("delivering the load failure");

    verify(
        &mut runtime,
        &mut js_runtime,
        r#"
        if (errors.length !== 1) throw new Error(String(errors.length));
        if (!errors[0].includes("optional init exploded")) throw new Error(errors[0]);
        // Still a worker this realm may name, and still one that answers.
        worker.postMessage("alive");
        worker.postMessage("again");
        "#,
    );
    for _ in 0..2 {
        workers
            .deliver_next(&mut runtime, &mut js_runtime)
            .expect("delivering the worker's message");
    }

    verify(
        &mut runtime,
        &mut js_runtime,
        r#"
        // Two, because the throw arrived through the job queue and the
        // checkpoint it stopped used to be finished by the next realm entry —
        // which ate the first message whole.
        if (seen.join("|") !== "ALIVE|AGAIN") {
            throw new Error(`seen=[${seen.join("|")}] errors=[${errors.join("|")}]`);
        }
        if (errors.length !== 1) throw new Error(errors.join("|"));
        "#,
    );
}

#[test]
fn worker_is_reached_only_through_the_import_never_through_a_global() {
    let (mut js_runtime, mut runtime, mut workers) = worker_runtime();
    runtime
        .run_main_thread_script(
            &mut js_runtime,
            r#"
                globalThis.seen = [];
                // The entry has the prepended import, so the binding is in
                // scope here — but it is a lexical binding and nothing more.
                if (typeof Worker !== "function") throw new Error(typeof Worker);
                if (typeof globalThis.Worker !== "undefined") {
                    throw new Error("Worker leaked onto globalThis");
                }
                const worker = new Worker("app:///nested.js");
                worker.onmessage = (event) => seen.push(event.data);
                worker.postMessage(null);
                "#,
            "app:///reach-entry.js",
        )
        .expect("main-thread script");

    workers.answer("onmessage = () => postMessage(typeof globalThis.Worker);");
    workers
        .deliver_next(&mut runtime, &mut js_runtime)
        .expect("delivering the worker's message");

    verify(
        &mut runtime,
        &mut js_runtime,
        r#"
        // A worker realm has no `Worker` either, so nothing nests: this realm
        // never registered `bobcat:runtime` on the worker runtime, and the
        // engine installs no global anywhere.
        if (seen.join("|") !== "undefined") throw new Error(seen.join("|"));
        // And this module — an ordinary one, with no prepended import — sees
        // no binding at all. `typeof` on an undeclared name is the safe probe;
        // naming it outright is the ReferenceError below.
        if (typeof Worker !== "undefined") throw new Error(typeof Worker);
        let named = false;
        try {
            Worker;
        } catch (error) {
            named = error instanceof ReferenceError;
        }
        if (!named) throw new Error("a bare `Worker` should be a ReferenceError");
        "#,
    );
}

#[test]
fn a_worker_cannot_import_the_runtime_that_would_let_it_nest() {
    let (mut js_runtime, mut runtime, mut workers) = worker_runtime();
    runtime
        .run_main_thread_script(
            &mut js_runtime,
            r#"
                globalThis.failures = [];
                const worker = new Worker("app:///nesting.js");
                worker.onerror = (event) => failures.push(event.message);
                "#,
            "app:///nesting-entry.js",
        )
        .expect("main-thread script");

    // `bobcat:runtime` is registered on `bobcat-main`'s runtime and on no
    // other, so the only route to `Worker` does not exist here.
    workers.answer("import { Worker } from 'bobcat:runtime';\nnew Worker('app:///deeper.js');");
    workers
        .deliver_next(&mut runtime, &mut js_runtime)
        .expect("delivering the worker's failure");

    verify(
        &mut runtime,
        &mut js_runtime,
        "if (failures.length !== 1) throw new Error(String(failures.length));",
    );
}
