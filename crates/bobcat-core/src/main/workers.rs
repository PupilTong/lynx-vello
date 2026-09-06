//! The group's worker realms: a second thread, and the second `QuickJS`
//! runtime every `Worker` in the group shares.
//!
//! A worker runtime is separate from `bobcat-main`'s for the reason a worker
//! exists at all — script that must not stop the thread that owns the
//! document. `QuickJS` binds a runtime to one thread, so putting the workers'
//! runtime on a thread of its own is also what makes "a worker cannot touch
//! the document" a fact about the program rather than a rule someone has to
//! keep: there is no path from a worker realm to a `LynxDocument`, and no
//! value of either runtime can be named by the other.
//!
//! One thread per group, not per worker: every `Worker` any view in the group
//! creates gets a realm on this one runtime, the way every view gets a realm
//! on `bobcat-main`'s. Realms on one runtime share a heap, an atom table and a
//! job queue, so a second worker costs a global object and a module graph
//! rather than a heap — at the price of the group's workers taking turns.
//!
//! The thread is started by the first `new Worker(...)` in the group and lives
//! until the group's thread ends. A group whose cards never construct one
//! never pays for it.

use std::cell::{Cell, RefCell};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::rc::Rc;
#[cfg(not(target_arch = "wasm32"))]
use std::thread::Builder as ThreadBuilder;

use quickjs_rust_bridge::HostArgument;
use rustc_hash::FxHashMap;
#[cfg(target_arch = "wasm32")]
use wasm_thread::Builder as ThreadBuilder;

use super::quickjs::{ScriptEngine, ScriptRuntime};
use super::runtime::worker_scope::{
    WORKER_BOOT_SPECIFIER, WORKER_DELIVER_EXPORT, WORKER_MODULE_SPECIFIER, install_worker_members,
    install_worker_modules, worker_boot_source,
};
use super::runtime::{TimerState, run_due_timers};
use super::wait::{Woken, wait_on};
use super::{MainJoinHandle, panic_payload, platform_script_error};
use crate::script::ScriptError;
use crate::view::ViewId;

/// Names one `Worker` for the life of its group.
///
/// Issued by `bobcat-main` and never reused, so a message still in flight for
/// a worker that has ended cannot find a later one wearing its name. It is
/// also the number the realm holds: keys cross the host boundary as `f64`,
/// which represents every one of them exactly.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) struct WorkerKey(u64);

/// The largest integer an `f64` represents exactly, and so the last key a
/// realm could hold without rounding. A group would have to construct one
/// worker per microsecond for nearly three centuries to reach it.
const MAX_EXACT_KEY: f64 = 9_007_199_254_740_992.0;

impl WorkerKey {
    #[allow(
        clippy::cast_precision_loss,
        reason = "keys stop below 2^53, where every integer is exact"
    )]
    pub(crate) fn as_number(self) -> f64 {
        self.0 as f64
    }

    /// The key a realm named, or `None` for a number that was never one.
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "the integer and range checks above make the value a representable key"
    )]
    pub(crate) fn from_number(value: f64) -> Option<Self> {
        if !value.is_finite() || value < 0.0 || value.fract() != 0.0 || value >= MAX_EXACT_KEY {
            return None;
        }
        Some(Self(value as u64))
    }
}

/// One worker to start: everything its realm needs, and nothing the group
/// already has.
pub(crate) struct WorkerStart {
    pub(crate) key: WorkerKey,
    /// The view whose realm created it, and the only one its messages reach.
    pub(crate) view: ViewId,
    /// The worker's `self.name`, empty when the constructor named none.
    pub(crate) name: String,
    /// The URL the script was fetched from, which is the name it is
    /// registered and reported under.
    pub(crate) url: String,
    pub(crate) source: String,
}

/// `bobcat-main` → the worker thread.
pub(crate) enum WorkerCommand {
    Start(Box<WorkerStart>),
    /// One JSON-encoded message for a worker's realm.
    Message {
        key: WorkerKey,
        data: String,
    },
    /// `Worker.terminate()`: end it between tasks and drop its realm.
    Terminate {
        key: WorkerKey,
    },
    /// A view is gone, and with it every worker it created.
    ReleaseView(ViewId),
}

/// The worker thread → `bobcat-main`.
pub(crate) struct WorkerEvent {
    pub(crate) view: ViewId,
    pub(crate) key: WorkerKey,
    pub(crate) payload: WorkerPayload,
}

pub(crate) enum WorkerPayload {
    /// A JSON-encoded `postMessage` from the worker.
    Message(String),
    /// Something in the worker threw and it is still running — a timer
    /// callback, which HTML reports at the worker and then at its parent
    /// without ending either.
    Errored(ScriptError),
    /// The worker's script threw on load, or its realm could not be built.
    /// The realm is gone; nothing more will ever arrive under this key.
    Failed(ScriptError),
    /// The worker ended itself with `close()`.
    Closed,
}

/// The right to talk to the worker thread, and to join it.
struct WorkerHome {
    commands: flume::Sender<WorkerCommand>,
    thread: MainJoinHandle,
}

impl WorkerHome {
    /// Ends the thread and waits for it.
    ///
    /// By value, because the goodbye *is* dropping the only sender: a join
    /// that could be reached with the sender still alive would wait forever,
    /// so the type is what makes the order right.
    fn join(self) {
        let Self { commands, thread } = self;
        drop(commands);
        // The same reasoning as `GroupHome::join`: under `panic = "abort"` a
        // trapped Worker runs no destructors and never signals its join
        // handle, and no check can outrun a trap.
        #[cfg(all(target_arch = "wasm32", panic = "abort"))]
        drop(thread);
        #[cfg(not(all(target_arch = "wasm32", panic = "abort")))]
        {
            let _ = thread.join();
        }
    }
}

/// What a group holds on behalf of its workers, shared with the host
/// callbacks that create them.
///
/// `Rc` rather than owned because the two ends are different stack frames on
/// `bobcat-main`: the native `createWorker` export a realm calls, and the
/// command loop that routes what comes back.
pub(crate) struct WorkerHub {
    next_key: Cell<u64>,
    /// `None` until the group's first `new Worker(...)`.
    home: RefCell<Option<WorkerHome>>,
    /// The event receiver, waiting for the command loop to pick it up on its
    /// next round. It is here rather than passed back from `start` because a
    /// worker is started from inside a realm, several frames below the loop
    /// that has to wait on it.
    fresh_events: RefCell<Option<flume::Receiver<WorkerEvent>>>,
    /// Workers that failed before the thread could hear of them — the thread
    /// would not start at all. Drained by the same round tail that drains the
    /// thread's own events, so a card cannot tell the two apart.
    stillborn: RefCell<Vec<WorkerEvent>>,
}

impl WorkerHub {
    pub(crate) fn new() -> Self {
        Self {
            next_key: Cell::new(1),
            home: RefCell::new(None),
            fresh_events: RefCell::new(None),
            stillborn: RefCell::new(Vec::new()),
        }
    }

    /// The next key no worker has ever held.
    pub(crate) fn next_key(&self) -> WorkerKey {
        let key = self.next_key.get();
        self.next_key.set(key + 1);
        WorkerKey(key)
    }

    /// Starts one worker, starting the thread it runs on if this is the
    /// group's first.
    ///
    /// A thread that will not start — or one that has since trapped, which a
    /// closed command channel is the fact of — is this worker's failure
    /// rather than the group's: it is reported the way a script error is, so
    /// a card hears it as one `error` event on the worker it just
    /// constructed, and the group's other views go on.
    pub(crate) fn start(&self, start: Box<WorkerStart>) {
        let mut home = self.home.borrow_mut();
        if home.is_none() {
            let (commands, command_receiver) = flume::unbounded();
            let (events, event_receiver) = flume::unbounded();
            let thread = ThreadBuilder::new()
                .name("bobcat-workers".to_owned())
                .spawn(move || run_workers(&command_receiver, &events));
            match thread {
                Ok(thread) => {
                    *self.fresh_events.borrow_mut() = Some(event_receiver);
                    *home = Some(WorkerHome { commands, thread });
                }
                Err(error) => {
                    drop(home);
                    self.stillborn(
                        &start,
                        &format!("the worker thread would not start: {error}"),
                    );
                    return;
                }
            }
        }
        let sent = home
            .as_ref()
            .expect("the worker thread was just started")
            .commands
            .send(WorkerCommand::Start(start));
        if let Err(flume::SendError(WorkerCommand::Start(start))) = sent {
            drop(home);
            self.stillborn(&start, "the worker thread is gone");
        }
    }

    /// Records a worker that failed before the thread could hear of it.
    fn stillborn(&self, start: &WorkerStart, message: &str) {
        self.stillborn.borrow_mut().push(WorkerEvent {
            view: start.view,
            key: start.key,
            payload: WorkerPayload::Failed(platform_script_error(message.to_owned())),
        });
    }

    /// Tells the worker thread something, if there is one. A group whose
    /// thread never started has no worker the command could name.
    pub(crate) fn send(&self, command: WorkerCommand) {
        if let Some(home) = self.home.borrow().as_ref() {
            let _ = home.commands.send(command);
        }
    }

    /// The event receiver, the one round it becomes available.
    pub(crate) fn take_fresh_receiver(&self) -> Option<flume::Receiver<WorkerEvent>> {
        self.fresh_events.borrow_mut().take()
    }

    pub(crate) fn take_stillborn(&self) -> Vec<WorkerEvent> {
        std::mem::take(&mut self.stillborn.borrow_mut())
    }

    /// Ends the worker thread and waits for it, once the group's own command
    /// loop has returned.
    pub(crate) fn shutdown(&self) {
        let home = self.home.borrow_mut().take();
        if let Some(home) = home {
            home.join();
        }
    }
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

/// The worker thread. It owns one `QuickJS` runtime, one realm per live
/// worker, and nothing else at all — no document, no style pool, no fetcher.
fn run_workers(commands: &flume::Receiver<WorkerCommand>, events: &flume::Sender<WorkerEvent>) {
    // A runtime that cannot be built is not fatal to the group: it is the
    // failure of every worker that would have run on it, and each hears about
    // it when it is asked for.
    let mut runtime = ScriptRuntime::new()
        .and_then(|mut runtime| install_worker_modules(&mut runtime).map(|()| runtime));
    let mut realms = FxHashMap::default();
    let served = catch_unwind(AssertUnwindSafe(|| {
        serve_workers(&mut runtime, commands, events, &mut realms);
    }));
    if let Err(payload) = served {
        let error = platform_script_error(format!(
            "the worker thread panicked: {}",
            panic_payload(payload.as_ref())
        ));
        for (key, realm) in realms {
            let _ = events.send(WorkerEvent {
                view: realm.view,
                key,
                payload: WorkerPayload::Failed(error.clone()),
            });
        }
    }
}

fn serve_workers(
    runtime: &mut Result<ScriptRuntime, ScriptError>,
    commands: &flume::Receiver<WorkerCommand>,
    events: &flume::Sender<WorkerEvent>,
    realms: &mut FxHashMap<WorkerKey, WorkerRealm>,
) {
    // Every worker script already registered on this runtime, by URL. A
    // source is runtime-wide and compiles per realm, so two workers over one
    // script share the registration and each still gets its own module
    // instance.
    let mut registered = FxHashMap::default();
    loop {
        let deadline = realms
            .values()
            .filter_map(|realm| realm.timers.next_deadline())
            .min();
        match wait_on(commands, deadline) {
            Woken::Command(first) => {
                for command in std::iter::once(first).chain(commands.drain()) {
                    apply(runtime, events, realms, &mut registered, command);
                }
            }
            Woken::Deadline => {}
            Woken::Disconnected => return,
        }
        fire_timers(runtime, events, realms);
        reap_closed(events, realms);
    }
}

fn apply(
    runtime: &mut Result<ScriptRuntime, ScriptError>,
    events: &flume::Sender<WorkerEvent>,
    realms: &mut FxHashMap<WorkerKey, WorkerRealm>,
    registered: &mut FxHashMap<String, Vec<RegisteredScript>>,
    command: WorkerCommand,
) {
    match command {
        WorkerCommand::Start(start) => {
            let key = start.key;
            let view = start.view;
            // Only a worker that could not be *built* fails: everything the
            // script itself does, including throwing on load, is reported
            // from inside as the realm's own work.
            if let Err(error) = boot(runtime, events, realms, registered, *start) {
                let _ = events.send(WorkerEvent {
                    view,
                    key,
                    payload: WorkerPayload::Failed(error),
                });
            }
        }
        WorkerCommand::Message { key, data } => {
            let (Ok(js_runtime), Some(realm)) = (runtime.as_mut(), realms.get_mut(&key)) else {
                return;
            };
            // Closed, with its realm still standing until this round's tail:
            // HTML discards whatever was queued behind a `close()`, so this
            // message is dropped rather than delivered to a worker that has
            // already ended itself.
            if realm.closing.get() {
                return;
            }
            let delivered = realm.engine.call_module_export(
                js_runtime,
                WORKER_MODULE_SPECIFIER,
                WORKER_DELIVER_EXPORT,
                &[HostArgument::String(&data)],
            );
            if let Err(error) = delivered {
                report(
                    events,
                    realm,
                    key,
                    "delivering a message to a worker",
                    error,
                );
            }
        }
        WorkerCommand::Terminate { key } => {
            realms.remove(&key);
        }
        WorkerCommand::ReleaseView(view) => realms.retain(|_, realm| realm.view != view),
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
    events: &flume::Sender<WorkerEvent>,
    realms: &mut FxHashMap<WorkerKey, WorkerRealm>,
    registered: &mut FxHashMap<String, Vec<RegisteredScript>>,
    start: WorkerStart,
) -> Result<(), ScriptError> {
    let js_runtime = match runtime {
        Ok(runtime) => runtime,
        // The runtime failed once, for every worker that will ever be asked
        // for. Each hears the same reason.
        Err(error) => return Err(error.clone()),
    };
    let WorkerStart {
        key,
        view,
        name,
        url,
        source,
    } = start;
    let specifier = register_script(js_runtime, registered, &url, source)?;
    let mut engine = js_runtime
        .create_realm()
        .map_err(|error| context_of("creating the worker realm", error))?;
    let timers = Rc::new(TimerState::new());
    let closing = Rc::new(Cell::new(false));
    install_worker_members(&mut engine, js_runtime, &timers, &closing, {
        let events = events.clone();
        move |data| {
            let _ = events.send(WorkerEvent {
                view,
                key,
                payload: WorkerPayload::Message(data),
            });
        }
    })?;
    let source = worker_boot_source(&name, &specifier);
    // In the table before the script runs, which is the whole point.
    realms.insert(
        key,
        WorkerRealm {
            view,
            engine,
            timers,
            closing,
        },
    );
    let realm = realms
        .get_mut(&key)
        .expect("the realm was just put in the table");
    if let Err(error) = realm
        .engine
        .execute_module(js_runtime, &source, WORKER_BOOT_SPECIFIER)
    {
        report(events, realm, key, "running the worker's script", error);
        // The throw arrived through the job queue — the boot module awaits the
        // script — so it also stopped that checkpoint mid-drain. Settle it
        // here, now that it has been reported, or the next realm to enter this
        // runtime pays for it with a task of its own.
        js_runtime.settle_after_failure();
    }
    Ok(())
}

/// Reports what a worker's realm threw, without ending it.
///
/// The one error policy for everything a realm does — loading its script,
/// taking a message, running a timer. HTML reports an uncaught exception at
/// the worker and then at its parent and leaves both running, which is exactly
/// what `Errored` means and `Failed` does not.
fn report(
    events: &flume::Sender<WorkerEvent>,
    realm: &WorkerRealm,
    key: WorkerKey,
    context: &str,
    error: ScriptError,
) {
    let _ = events.send(WorkerEvent {
        view: realm.view,
        key,
        payload: WorkerPayload::Errored(context_of(context, error)),
    });
}

/// One worker script already on this runtime.
struct RegisteredScript {
    /// The exact bytes registered under `specifier`.
    ///
    /// Kept, rather than trusting the URL alone, because a URL does not name
    /// a body here: every view has its own `ResourceFetcher`, so two views in
    /// one group can resolve one URL to different scripts, and a runtime
    /// holds exactly one source per module name.
    source: String,
    specifier: String,
}

/// Registers one worker's script, or finds the registration it can share, and
/// answers with the specifier its boot module should import.
///
/// Two things are load-bearing here. **The registration happens before it is
/// recorded**, never the other way round: `register_module_source` fails for
/// reasons that have nothing to do with this source — a checkpoint another
/// worker left incomplete is enough — and recording first would leave the
/// runtime believing in a module it does not have, so every later worker over
/// that URL would fail at `await import` with a misleading reason, forever.
/// **A second body under one URL gets a second name**, because sharing the
/// first one would silently run one view's script in another view's worker.
fn register_script(
    js_runtime: &mut ScriptRuntime,
    registered: &mut FxHashMap<String, Vec<RegisteredScript>>,
    url: &str,
    source: String,
) -> Result<String, ScriptError> {
    let bodies = registered.entry(url.to_owned()).or_default();
    if let Some(shared) = bodies.iter().find(|script| script.source == source) {
        return Ok(shared.specifier.clone());
    }
    // The first body keeps the URL, which is what a stack trace should say;
    // only the unusual case pays for the disambiguation.
    let specifier = if bodies.is_empty() {
        url.to_owned()
    } else {
        format!("{url}#bobcat-worker-body-{}", bodies.len())
    };
    js_runtime
        .register_module_source(&specifier, &source)
        .map_err(|error| context_of("registering the worker's script", error))?;
    bodies.push(RegisteredScript {
        source,
        specifier: specifier.clone(),
    });
    Ok(specifier)
}

/// Runs every timer that came due, in every realm that armed one.
fn fire_timers(
    runtime: &mut Result<ScriptRuntime, ScriptError>,
    events: &flume::Sender<WorkerEvent>,
    realms: &mut FxHashMap<WorkerKey, WorkerRealm>,
) {
    let Ok(js_runtime) = runtime.as_mut() else {
        return;
    };
    for (key, realm) in realms.iter_mut() {
        // A `close()` discards this worker's armed timers along with its
        // queued messages; the realm itself goes at the round's tail.
        if realm.closing.get() {
            continue;
        }
        for error in
            run_due_timers(&mut realm.engine, js_runtime, &realm.timers).unwrap_or_default()
        {
            report(
                events,
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
fn reap_closed(
    events: &flume::Sender<WorkerEvent>,
    realms: &mut FxHashMap<WorkerKey, WorkerRealm>,
) {
    realms.retain(|key, realm| {
        if !realm.closing.get() {
            return true;
        }
        let _ = events.send(WorkerEvent {
            view: realm.view,
            key: *key,
            payload: WorkerPayload::Closed,
        });
        false
    });
}

/// Prefixes a failure with what the host was doing, the way `MainThreadError`
/// does for the errors that reach an embedder through a view.
fn context_of(context: &str, mut error: ScriptError) -> ScriptError {
    error.message = std::sync::Arc::from(format!("{context}: {}", error.message));
    error
}
