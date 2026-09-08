//! `bobcat-workers`: the thread the group's worker realms live on.
//!
//! It owns one `QuickJS` runtime, one realm per live worker, and nothing else
//! at all — no document, no style pool, no fetcher. Every worker's whole life
//! is a state in the tables here: named and waiting for a script, running, or
//! gone.

use std::cell::Cell;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::rc::Rc;

use quickjs_rust_bridge::HostArgument;
use rustc_hash::FxHashMap;

use super::scope::{
    WORKER_DELIVER_EXPORT, WORKER_MODULE_SPECIFIER, install_worker_members, install_worker_modules,
    worker_boot_source,
};
use super::{WorkerCommand, WorkerEvent, WorkerKey, WorkerPayload, WorkerScript, WorkerStart};
use crate::mailbox::{Mailbox, Sender};
use crate::main::quickjs::{ScriptEngine, ScriptRuntime};
use crate::script::ScriptError;
use crate::threads::{panic_message, platform_script_error};
use crate::timers::{TimerState, run_due_timers};
use crate::view::{ToMain, ViewId};

/// One thing a worker realm said, on its way to the view whose realm created
/// it.
///
/// The address *is* the routing: `bobcat-main` already drops a message for a
/// view it no longer carries, which is exactly what should happen to a worker
/// whose view was released while its news was in flight.
fn report_event(to_main: &Sender<ToMain>, event: WorkerEvent) {
    let WorkerEvent { view, key, payload } = event;
    let _ = to_main.send((Some(view), ToMain::Worker { key, payload }));
}

/// Everything this thread knows, and the only place any of it is known.
#[derive(Default)]
struct Workers {
    /// Named by a `new Worker(...)`, still waiting for its script.
    pending: FxHashMap<WorkerKey, Pending>,
    /// Up: a realm, its timers, and its closing flag.
    realms: FxHashMap<WorkerKey, WorkerRealm>,
}

/// A worker between `new Worker(...)` and its realm.
struct Pending {
    view: ViewId,
    name: String,
    /// HTML queues what is posted before a worker's global scope exists and
    /// delivers it once the scope is up, which is what this is: without it
    /// the commonest shape there is — construct, then post — would lose its
    /// first message.
    queued: Vec<String>,
}

/// One live worker on the thread: its realm and everything that realm owns.
struct WorkerRealm {
    view: ViewId,
    engine: ScriptEngine,
    timers: Rc<TimerState>,
    /// Set by the native `closeWorker` export. A flag rather than a direct
    /// teardown because it is written from inside the realm it would tear
    /// down: the thread reads it once the task that set it has returned.
    closing: Rc<Cell<bool>>,
}

/// The thread's whole body.
pub(super) fn run(commands: &Mailbox<WorkerCommand>, to_main: &Sender<ToMain>) {
    // A runtime that cannot be built is not fatal to the group: it is the
    // failure of every worker that would have run on it, and each hears about
    // it when it is asked for.
    let mut runtime = ScriptRuntime::new()
        .and_then(|mut runtime| install_worker_modules(&mut runtime).map(|()| runtime));
    let mut workers = Workers::default();
    let served = catch_unwind(AssertUnwindSafe(|| {
        serve(&mut runtime, commands, to_main, &mut workers);
    }));
    if let Err(payload) = served {
        let error = platform_script_error(format!(
            "the worker thread panicked: {}",
            panic_message(payload.as_ref())
        ));
        // Every worker of every view, running or still waiting for a script:
        // the thread is over, so none of them will ever be heard from again.
        let ended = workers
            .realms
            .into_iter()
            .map(|(key, realm)| (key, realm.view))
            .chain(
                workers
                    .pending
                    .into_iter()
                    .map(|(key, pending)| (key, pending.view)),
            );
        for (key, view) in ended {
            report_event(
                to_main,
                WorkerEvent {
                    view,
                    key,
                    payload: WorkerPayload::Failed(error.clone()),
                },
            );
        }
    }
}

fn serve(
    runtime: &mut Result<ScriptRuntime, ScriptError>,
    commands: &Mailbox<WorkerCommand>,
    to_main: &Sender<ToMain>,
    workers: &mut Workers,
) {
    loop {
        let deadline = workers
            .realms
            .values()
            .filter_map(|realm| realm.timers.next_deadline())
            .min();
        // This thread waits out its own realms' timers, which `bobcat-main`
        // no longer does: a view's deadline is announced to its painter and
        // the host waits it out on the loop it already parks on. A worker
        // realm has no painter and no host turn, so there is nobody to hand
        // the deadline to — this thread is the only one left that owns a
        // clock, and it owns one because a worker's `setTimeout` has no other
        // way to come due.
        //
        // Everything here is group-scoped, so every message is addressed to
        // the group and the address is ignored on arrival.
        match commands.recv(deadline) {
            Ok((_, first)) => {
                let rest: Vec<_> = commands.drain().collect();
                for command in std::iter::once(first).chain(rest.into_iter().map(|(_, m)| m)) {
                    apply(runtime, to_main, workers, command);
                }
            }
            Err(flume::RecvTimeoutError::Timeout) => {}
            Err(flume::RecvTimeoutError::Disconnected) => return,
        }
        fire_timers(runtime, to_main, workers);
        reap_closed(to_main, workers);
    }
}

fn apply(
    runtime: &mut Result<ScriptRuntime, ScriptError>,
    to_main: &Sender<ToMain>,
    workers: &mut Workers,
    command: WorkerCommand,
) {
    match command {
        WorkerCommand::Start(WorkerStart { key, view, name }) => {
            // Nothing to arrange and nothing that can fail: the painter has
            // already been told to fetch, by the same realm, in the same
            // call. All this makes is somewhere to queue.
            workers.pending.insert(
                key,
                Pending {
                    view,
                    name,
                    queued: Vec::new(),
                },
            );
        }
        WorkerCommand::Script { key, script } => {
            let Some(pending) = workers.pending.remove(&key) else {
                // Terminated, or its view released, while the fetch was in
                // flight. Nobody is owed this answer.
                return;
            };
            start_worker(runtime, to_main, workers, key, pending, script);
        }
        WorkerCommand::Message { key, data } => {
            if let Some(pending) = workers.pending.get_mut(&key) {
                pending.queued.push(data);
                return;
            }
            let (Ok(js_runtime), Some(realm)) = (runtime.as_mut(), workers.realms.get_mut(&key))
            else {
                return;
            };
            deliver(js_runtime, to_main, realm, key, &data);
        }
        WorkerCommand::Terminate { key } => {
            workers.pending.remove(&key);
            workers.realms.remove(&key);
        }
        WorkerCommand::ReleaseView(view) => {
            workers.pending.retain(|_, pending| pending.view != view);
            workers.realms.retain(|_, realm| realm.view != view);
        }
    }
}

/// The script arrived, or the reason it did not.
///
/// Building the realm and flushing what was posted while it loaded are one
/// step, so nothing a card sent can arrive out of order.
fn start_worker(
    runtime: &mut Result<ScriptRuntime, ScriptError>,
    to_main: &Sender<ToMain>,
    workers: &mut Workers,
    key: WorkerKey,
    pending: Pending,
    script: Result<WorkerScript, String>,
) {
    let Pending { view, name, queued } = pending;
    let script = match script {
        Ok(script) => script,
        Err(message) => {
            report_event(
                to_main,
                WorkerEvent {
                    view,
                    key,
                    payload: WorkerPayload::Failed(platform_script_error(format!(
                        "loading the worker's script: {message}"
                    ))),
                },
            );
            return;
        }
    };
    if let Err(error) = boot(runtime, to_main, workers, key, view, &name, script) {
        report_event(
            to_main,
            WorkerEvent {
                view,
                key,
                payload: WorkerPayload::Failed(error),
            },
        );
        return;
    }
    let Ok(js_runtime) = runtime.as_mut() else {
        return;
    };
    for data in queued {
        let Some(realm) = workers.realms.get_mut(&key) else {
            // Its own script called `close()` on the way up.
            return;
        };
        deliver(js_runtime, to_main, realm, key, &data);
    }
}

/// Builds one worker's realm, then runs its script in it.
///
/// `Err` is reserved for what leaves no worker at all — a runtime that never
/// came up, a script that could not be registered, a realm that could not be
/// created or furnished. The script's *own* outcome is not among them: by the
/// time it runs, the realm is built and in the table, so a script that throws
/// on load is reported exactly like a listener or a timer callback that
/// throws later, and leaves a worker that is up and listening. That is what
/// HTML's "run a worker" does — it reports the exception and goes on to enable
/// the port queue and run the event loop — and it matters because a script
/// registers its handlers before whatever optional work fails.
fn boot(
    runtime: &mut Result<ScriptRuntime, ScriptError>,
    to_main: &Sender<ToMain>,
    workers: &mut Workers,
    key: WorkerKey,
    view: ViewId,
    name: &str,
    script: WorkerScript,
) -> Result<(), ScriptError> {
    let js_runtime = match runtime {
        Ok(runtime) => runtime,
        // The runtime failed once, for every worker that will ever be asked
        // for. Each hears the same reason.
        Err(error) => return Err(error.clone()),
    };
    let WorkerScript { source, url } = script;
    let mut engine = js_runtime
        .create_realm()
        .map_err(|error| context_of("creating the worker realm", error))?;
    let timers = Rc::new(TimerState::new());
    let closing = Rc::new(Cell::new(false));
    install_worker_members(&mut engine, js_runtime, &timers, &closing, {
        let to_main = to_main.clone();
        move |data| {
            report_event(
                &to_main,
                WorkerEvent {
                    view,
                    key,
                    payload: WorkerPayload::Message(data),
                },
            );
        }
    })?;
    let source = worker_boot_source(name, &source);
    // In the table before the script runs, which is the whole point.
    workers.realms.insert(
        key,
        WorkerRealm {
            view,
            engine,
            timers,
            closing,
        },
    );
    let realm = workers
        .realms
        .get_mut(&key)
        .expect("the realm was just put in the table");
    if let Err(error) = realm.engine.execute_module(js_runtime, &source, &url) {
        // Nothing to clean up after: a throw at this module's top level
        // rejects through the runtime's shared job queue, and what it leaves
        // there is this realm's — it waits for this worker rather than
        // reaching the next realm to be entered on this runtime.
        report(to_main, realm, key, "running the worker's script", error);
    }
    Ok(())
}

/// Hands one JSON message to a realm that is up.
fn deliver(
    js_runtime: &mut ScriptRuntime,
    to_main: &Sender<ToMain>,
    realm: &mut WorkerRealm,
    key: WorkerKey,
    data: &str,
) {
    // Closed, with its realm still standing until this round's tail: HTML
    // discards whatever was queued behind a `close()`, so this message is
    // dropped rather than delivered to a worker that has already ended itself.
    if realm.closing.get() {
        return;
    }
    let delivered = realm.engine.call_module_export(
        js_runtime,
        WORKER_MODULE_SPECIFIER,
        WORKER_DELIVER_EXPORT,
        &[HostArgument::String(data)],
    );
    if let Err(error) = delivered {
        report(
            to_main,
            realm,
            key,
            "delivering a message to a worker",
            error,
        );
    }
}

/// Reports what a worker's realm threw, without ending it.
///
/// The one error policy for everything a realm does — loading its script,
/// taking a message, running a timer. HTML reports an uncaught exception at
/// the worker and then at its parent and leaves both running, which is exactly
/// what `Errored` means and `Failed` does not.
fn report(
    to_main: &Sender<ToMain>,
    realm: &WorkerRealm,
    key: WorkerKey,
    context: &str,
    error: ScriptError,
) {
    report_event(
        to_main,
        WorkerEvent {
            view: realm.view,
            key,
            payload: WorkerPayload::Errored(context_of(context, error)),
        },
    );
}

/// Runs every timer that came due, in every realm that armed one.
fn fire_timers(
    runtime: &mut Result<ScriptRuntime, ScriptError>,
    to_main: &Sender<ToMain>,
    workers: &mut Workers,
) {
    let Ok(js_runtime) = runtime.as_mut() else {
        return;
    };
    for (key, realm) in &mut workers.realms {
        // A `close()` discards this worker's armed timers along with its
        // queued messages; the realm itself goes at the round's tail.
        if realm.closing.get() {
            continue;
        }
        for error in
            run_due_timers(&mut realm.engine, js_runtime, &realm.timers).unwrap_or_default()
        {
            report(
                to_main,
                realm,
                *key,
                "running a worker's timer callback",
                error,
            );
        }
    }
}

/// Drops every realm whose script called `close()` during this round, and
/// tells the realm that created it.
fn reap_closed(to_main: &Sender<ToMain>, workers: &mut Workers) {
    workers.realms.retain(|key, realm| {
        if !realm.closing.get() {
            return true;
        }
        report_event(
            to_main,
            WorkerEvent {
                view: realm.view,
                key: *key,
                payload: WorkerPayload::Closed,
            },
        );
        false
    });
}

/// Prefixes a failure with what the host was doing, the way `MainThreadError`
/// does for the errors that reach an embedder through a view.
fn context_of(context: &str, mut error: ScriptError) -> ScriptError {
    error.message = std::sync::Arc::from(format!("{context}: {}", error.message));
    error
}
