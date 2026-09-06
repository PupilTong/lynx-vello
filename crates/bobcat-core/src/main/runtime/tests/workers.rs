//! `Worker`, driven through a real second `QuickJS` runtime on a real second
//! thread.
//!
//! Every test here plays the two halves the engine's own threads play: the
//! painter, which answers a script request with bytes, and the group's command
//! loop, which routes what the worker thread says back into the realm. What is
//! real is everything between — the runtime, the thread, the realm on it, and
//! both directions of the boundary.

use super::*;
use crate::main::workers::{WorkerEvent, WorkerHub, WorkerKey};
use crate::view::WorkerScript;

struct WorkerHarness {
    link: PainterLink,
    hub: Rc<WorkerHub>,
    events: Option<flume::Receiver<WorkerEvent>>,
}

impl WorkerHarness {
    /// Every script the realm has asked for since the last call.
    fn requests(&mut self) -> Vec<(WorkerKey, String)> {
        self.link.sync();
        self.link.take_worker_requests()
    }

    /// Answers the one outstanding request with `source`, as a fetch would.
    fn answer(
        &mut self,
        runtime: &mut MainThreadRuntime<NoWakeup>,
        js_runtime: &mut ScriptRuntime,
        source: &str,
    ) {
        let mut requests = self.requests();
        assert_eq!(requests.len(), 1, "exactly one worker was constructed");
        let (key, url) = requests.remove(0);
        runtime
            .worker_script_loaded(
                js_runtime,
                key,
                Ok(WorkerScript {
                    source: source.to_owned(),
                    url,
                }),
            )
            .expect("answering the script request");
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
        let events = self
            .events
            .get_or_insert_with(|| self.hub.take_fresh_receiver().expect("a worker started"));
        events
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("the worker thread answered")
    }
}

impl Drop for WorkerHarness {
    fn drop(&mut self) {
        self.hub.shutdown();
    }
}

/// Answers the next outstanding script request with `source`, as a fetch
/// would. Requests are answered in the order they were made, which is the
/// order the realm constructed its workers in.
fn answer_next(
    runtime: &mut MainThreadRuntime<NoWakeup>,
    js_runtime: &mut ScriptRuntime,
    pending: &mut Vec<(WorkerKey, String)>,
    source: &str,
) {
    assert!(!pending.is_empty(), "a worker was constructed");
    let (key, url) = pending.remove(0);
    runtime
        .worker_script_loaded(
            js_runtime,
            key,
            Ok(WorkerScript {
                source: source.to_owned(),
                url,
            }),
        )
        .expect("answering the script request");
}

fn worker_runtime() -> (ScriptRuntime, MainThreadRuntime<NoWakeup>, WorkerHarness) {
    let (link, main) = detached_link(Arc::new(NoWakeup));
    let mut js_runtime = ScriptRuntime::new().expect("the test runtime starts");
    install_shared_modules(&mut js_runtime).expect("the shared modules register");
    let hub = Rc::new(WorkerHub::new());
    let runtime = MainThreadRuntime::new(
        &mut js_runtime,
        new_document(Viewport::new(393.0, 727.0), PageConfig::default()),
        main.notify,
        DETACHED_VIEW,
        Rc::clone(&hub),
    )
    .expect("main-thread runtime");
    (
        js_runtime,
        runtime,
        WorkerHarness {
            link,
            hub,
            events: None,
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

    workers.answer(
        &mut runtime,
        &mut js_runtime,
        "onmessage = (event) => postMessage({ pong: event.data.ping + 1 });",
    );
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
    workers.answer(
        &mut runtime,
        &mut js_runtime,
        "onmessage = (event) => postMessage(event.data.toUpperCase());",
    );
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
        &mut runtime,
        &mut js_runtime,
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
    workers.answer(
        &mut runtime,
        &mut js_runtime,
        "import { __CreateView } from 'bobcat:element';\n__CreateView(0);",
    );
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
        &mut runtime,
        &mut js_runtime,
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
    runtime
        .worker_script_loaded(&mut js_runtime, key, Err("404 Not Found".to_owned()))
        .expect("reporting the failed fetch");

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

    workers.answer(&mut runtime, &mut js_runtime, "postMessage('hello');");
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

    workers.answer(
        &mut runtime,
        &mut js_runtime,
        "setTimeout(() => postMessage('late'), 1);",
    );
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
        &mut runtime,
        &mut js_runtime,
        &mut pending,
        "onmessage = () => postMessage('first');",
    );
    answer_next(
        &mut runtime,
        &mut js_runtime,
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
        &mut runtime,
        &mut js_runtime,
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
        &mut runtime,
        &mut js_runtime,
        &mut pending,
        "for (let i = 0; i < 3000; i += 1) { Promise.resolve().then(() => {}); }",
    );
    answer_next(
        &mut runtime,
        &mut js_runtime,
        &mut pending,
        "postMessage('pooled');",
    );
    answer_next(
        &mut runtime,
        &mut js_runtime,
        &mut pending,
        "postMessage('pooled');",
    );

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

    workers.answer(
        &mut runtime,
        &mut js_runtime,
        "onmessage = () => postMessage('once');",
    );
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
        &mut runtime,
        &mut js_runtime,
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
