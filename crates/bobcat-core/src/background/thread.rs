//! `bobcat-workers`: the thread the group's worker realms live on.
//!
//! It owns one `QuickJS` runtime, a task set per live worker, and nothing
//! else at all — no document, no style pool, no fetcher.
//!
//! It is one engine thread of [`crate::jobs`], the same shape `bobcat-main` is,
//! which is what lets an in-band [`WorkerMessage::Terminate`] end a realm whose
//! job is parked on a synchronous wait.
//!
//! A worker takes the shape a view has on `bobcat-main`, for the same reason:
//! each thing it can wait for is a task of its own, and tokio is what polls,
//! parks and wakes them. An owner ([`serve_worker`]) waits only for the end;
//! [`boot_worker`] opens the realm as the worker's first job, the way a view's
//! realm opens as its first; [`consume_messages`], started beside that job,
//! is the one ordered consumer of what is posted and of the worker's script
//! when the host was asked for it; [`serve_clock`] owns this realm's one
//! pinned sleep and watches the runtime-wide checkpoint generation, because the
//! job queue every worker realm here drains is the runtime's and a sibling's
//! entry can finish this realm's jobs. Every one of them reaches the realm
//! through [`owner::enter`], the driver a worker shares with every view, which
//! queues one job: it runs one synchronous operation and then the driver's
//! epilogue, which settles what it left owing — the timers that came due, a
//! `close()` it may have called, the root module finishing, the imports and
//! futures it left waiting, the next deadline, and the checkpoint generation
//! as of that entry. What a worker adds to that epilogue is its
//! [`RealmOwner`] impl. What a failure in a worker's realm is reported as —
//! `Errored`, or a `Failed` that ends the worker — is the worker table in
//! [`policy`], read by the scene the failure happened in, and every report
//! here goes through [`policy::report`]; a panic is that table's Panic row,
//! whether the worker's own owner saw it or the thread did.
//!
//! A worker's tasks, the token that ends them and the latch this thread reads
//! are one [`Lifetime`], the same helper a view on `bobcat-main` is built
//! from. Its token belongs to this worker; the MTS object's termination or
//! collection sends `Terminate`, while releasing the MTS realm closes its
//! channel. There is no app-specific destruction path on this thread.
//!
//! The `select!`s on this thread are of five kinds, and none of them
//! dispatches anything:
//!
//! - the top loop's own turn, waiting on its `main` task finishing versus a job having been pushed;
//! - [`serve_workers`], which is that `main` task, waiting on attach versus join;
//! - each [`serve_worker`], waiting on its worker's [`Lifetime`]: the end, versus the next task of
//!   that worker to finish;
//! - each [`consume_messages`]' wait on what is posted, versus the worker's script while it is
//!   outstanding, versus the realm's root module finishing until it has — one task's wait rather
//!   than a scheduler;
//! - one [`serve_clock`] per live worker realm, waiting on its deadline, the re-arm that moves it,
//!   and a sibling's checkpoint.

use std::cell::{Cell, RefCell};
use std::panic::AssertUnwindSafe;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use quickjs_rust_bridge::{HostArgument, HostValue};
use rustc_hash::FxHashMap;
use tokio::sync::{mpsc, watch};
use tokio::task::{self, JoinError, JoinSet};
use tokio_util::sync::CancellationToken;

use super::scope::{WORKER_DELIVER_EXPORT, install_worker_members};
use super::{WorkerCommand, WorkerEvent, WorkerKey, WorkerMessage, WorkerPayload, WorkerStart};
use crate::esm::{WORKER_MODULE_SPECIFIER, build_runtime};
use crate::jobs::{JsThread, JsThreadHandle};
use crate::lifetime::{Lifetime, run_job, serve_clock};
use crate::link::{HostOutbox, SourceAnswer};
use crate::main::quickjs::{
    ScriptRuntime, SharedRuntime, mark_checkpoint_later, normalize_module_url,
};
use crate::realm::owner::{self, RealmOwner};
use crate::realm::policy::{self, Row, Scene};
use crate::realm::{self, RealmCore};
use crate::script::ScriptError;
use crate::threads::platform_script_error;
use crate::view::ScriptSource;

/// Who each worker task reports to: its worker's key and the creating view's
/// channel, beside the worker's own token. Kept by [`serve_workers`] until it
/// joins the task, and read by [`report_thread_trap`] when the whole thread is
/// over.
type Reporters = Rc<RefCell<FxHashMap<task::Id, Reporter>>>;

/// One entry of [`Reporters`].
///
/// The token is the one [`owner::end`] cancels, and every way a worker ends
/// goes through that call: a `Failed` or `Closed` it reported, a panic of its
/// own, a `Terminate`, its channel closing. A task stays in the table until it
/// is joined, which is some time after its worker ended, so the token is what
/// tells a worker that is over from a live one.
struct Reporter {
    key: WorkerKey,
    events: mpsc::UnboundedSender<WorkerEvent>,
    token: CancellationToken,
}

/// The thread's whole body.
pub(super) fn run(commands: mpsc::UnboundedReceiver<WorkerCommand>, trapped: &Arc<AtomicBool>) {
    serve(Rc::new(RefCell::new(build_runtime())), commands, trapped);
}

/// Preloads a fixture through the existing runtime API for Context tests: a
/// BTS entry registered under `url`, as it is written, so `bobcat:bts`
/// imports it without a fetch and it runs with exactly the imports a fetched
/// one would.
#[cfg(test)]
pub(super) fn run_with_entry(
    commands: mpsc::UnboundedReceiver<WorkerCommand>,
    entry: (String, String),
    trapped: &Arc<AtomicBool>,
) {
    let (source, url) = entry;
    let mut runtime = build_runtime().unwrap();
    runtime.register_module_source(&url, &source).unwrap();
    serve(Rc::new(RefCell::new(Ok(runtime))), commands, trapped);
}

/// Runs the thread's loop, and reports its trap to every worker still on it.
///
/// A panic in [`serve_workers`] — the loop's `main` task — is resumed by
/// [`JsThread::run`] and ends every worker here at once. It is caught around
/// that call and reported before the unwind goes on: the worker tasks are
/// dropped with the loop, and their own owners report nothing. Under
/// `panic = "abort"` nothing unwinds and nothing is caught, so on wasm the
/// panic hook reports it instead, through the same function.
fn serve(
    js: SharedRuntime,
    commands: mpsc::UnboundedReceiver<WorkerCommand>,
    trapped: &Arc<AtomicBool>,
) {
    let reporters = Reporters::default();
    // Installed here as well as on `bobcat-main`, because this thread is
    // started first.
    #[cfg(all(target_arch = "wasm32", panic = "abort"))]
    {
        crate::threads::install_script_panic_hook();
        let trapped = Arc::clone(trapped);
        let reporters = Rc::clone(&reporters);
        crate::threads::add_script_panic_reporter(Box::new(move |detail| {
            report_thread_trap(&trapped, &reporters, &|| {
                (policy::worker_panic().event)(platform_script_error(format!(
                    "the worker thread {detail}"
                )))
            });
        }));
    }
    let thread = JsThread::new();
    let served = std::panic::catch_unwind(AssertUnwindSafe(|| {
        thread.run(serve_workers(
            js,
            commands,
            thread.handle(),
            Rc::clone(&reporters),
        ));
    }));
    if let Err(payload) = served {
        report_thread_trap(trapped, &reporters, &|| {
            policy::worker_panic().event_for_panic(payload.as_ref())
        });
        std::panic::resume_unwind(payload);
    }
    drop(thread);
}

/// Starts a task per worker, and finishes each one that ends.
async fn serve_workers(
    js: SharedRuntime,
    mut commands: mpsc::UnboundedReceiver<WorkerCommand>,
    thread: JsThreadHandle,
    reporters: Reporters,
) {
    let mut workers = JoinSet::new();
    loop {
        tokio::select! {
            command = commands.recv() => match command {
                Some(WorkerCommand::Start(start)) => {
                    let reporter = Reporter {
                        key: start.key,
                        events: start.events.clone(),
                        token: start.token.clone(),
                    };
                    let handle = workers.spawn_local(
                        serve_worker(Rc::clone(&js), start, thread.clone()),
                    );
                    reporters.borrow_mut().insert(handle.id(), reporter);
                }
                #[cfg(test)]
                Some(WorkerCommand::Panic) => panic!("the test asked the worker thread to trap"),
                // Every realm that could name a worker is gone, and with it
                // every message sender, so the tasks below are ending too.
                None => break,
            },
            Some(finished) = workers.join_next_with_id(), if !workers.is_empty() => {
                finish_worker_task(&js, &thread, finished, &reporters);
            }
        }
    }
    while let Some(finished) = workers.join_next_with_id().await {
        finish_worker_task(&js, &thread, finished, &reporters);
    }
}

/// A worker task ended. If it trapped, the view that created its worker hears
/// `Failed` — a worker nothing will ever be heard from again, which is exactly
/// what `Failed` means. Either way the other realms on this runtime settle
/// what their own realms owe, because a task that ended part-way through may
/// have left the shared job queue with work in it.
///
/// The bump is made for a task that returned as well: a panic inside one of
/// its jobs is caught by [`run_job`], and the task that queued that job then
/// returns normally.
fn finish_worker_task(
    js: &SharedRuntime,
    thread: &JsThreadHandle,
    finished: Result<(task::Id, ()), JoinError>,
    reporters: &Reporters,
) {
    let trapped = match finished {
        Ok((id, ())) => {
            reporters.borrow_mut().remove(&id);
            None
        }
        Err(error) => {
            let reporter = reporters.borrow_mut().remove(&error.id());
            reporter
                .filter(|_| error.is_panic())
                .map(|reporter| (reporter, error))
        }
    };
    if let Some((reporter, error)) = trapped {
        let _ = reporter.events.send(WorkerEvent {
            key: reporter.key,
            payload: policy::worker_panic().event_for_panic(error.into_panic().as_ref()),
        });
    }
    mark_checkpoint_later(js, thread);
}

/// The whole thread is over: the flag `bobcat-main` reads before each `Start`
/// is set, and the creator of every worker still live on it hears `Failed`,
/// one that `failed` builds per worker with the worker table's Panic row —
/// over the panic's payload natively, and over the panic hook's own detail on
/// wasm.
///
/// The flag goes first, so a `Worker` constructed after any of these reports
/// fails at once. `try_borrow`, because on wasm this runs from the panic hook
/// at the panic itself, which may be inside a borrow of the table; such a trap
/// sets the flag and reports nothing else.
///
/// A worker whose token is already cancelled is skipped: it ended before the
/// trap, and either it has already reported its end or its creator ended it
/// (a `terminate()`, or the release of the creating realm). No token is
/// cancelled by the trap itself before this runs: on wasm this runs at the
/// panic, and natively the worker tasks, whose unwind guards cancel their
/// tokens, are dropped only after this report.
fn report_thread_trap(
    trapped: &AtomicBool,
    reporters: &Reporters,
    failed: &dyn Fn() -> WorkerPayload,
) {
    trapped.store(true, Ordering::Release);
    let Ok(reporters) = reporters.try_borrow() else {
        return;
    };
    for reporter in reporters.values() {
        if reporter.token.is_cancelled() {
            continue;
        }
        let _ = reporter.events.send(WorkerEvent {
            key: reporter.key,
            payload: failed(),
        });
    }
}

/// One live worker on this thread: its realm and everything that realm owns.
///
/// `core` is what [`realm::open_realm`] builds for every realm; the two flags
/// are the state the worker's own host module, `bobcat-internal:worker`, adds
/// to it.
struct WorkerRealm {
    core: RealmCore,
    /// Set as `bobcat:worker` finishes running in this realm: whether the
    /// realm has the global scope a posted message is delivered to.
    scope_installed: Rc<Cell<bool>>,
    /// Set by the native `closeWorker` export. A flag rather than a direct
    /// teardown because it is written from inside the realm it would tear
    /// down: the task reads it once the call that set it has returned.
    closing: Rc<Cell<bool>>,
}

/// One worker: what its tasks act on, driven through [`owner`] as a view is.
///
/// The same shape a view has on `bobcat-main` — an owner that waits only for
/// the end, one ordered consumer of the message stream, one wait on this
/// realm's own clock — because a worker realm is a realm: it takes what is
/// posted to it and fires what it armed, and both of those settle the same
/// things afterwards.
struct Worker {
    js: SharedRuntime,
    key: WorkerKey,
    /// What this worker's realm is named by in the diagnostics it reports:
    /// the background thread, or the `Worker` its key names.
    source: ScriptSource,
    /// The name the realm loads as its root module: the script's URL,
    /// normalized the way the module loader normalizes it from itself, which
    /// for the absolute URL or engine name `createWorker` joined is the
    /// identity. The root is the module at this name and nothing else. The
    /// epilogue never asks the host for it, because `createWorker` already
    /// has, and [`consume_messages`] completes it under this name from that
    /// answer. An engine name, such as the BTS's `bobcat:bts`, never becomes
    /// a request at all: the realm's own loader loads it.
    entry: String,
    /// Where this worker reports, which is the creating view's own channel.
    events: mpsc::UnboundedSender<WorkerEvent>,
    /// This worker's realm: `None` until the worker's first job has opened
    /// it, and again once it could not be built or the worker is over and the
    /// owner has released it.
    realm: RefCell<Option<Box<WorkerRealm>>>,
    /// Every task of this worker, the token that ends them, the latch this
    /// thread reads, the latches its `Failed` or `Closed` and its panic go
    /// through, and the two numbers this realm's clock task waits on — the
    /// deadline it armed and the generation its own last entry recorded.
    lifetime: Lifetime,
    /// What this worker's realm asks the view's host through. It carries
    /// this worker's own token rather than the view's.
    host: HostOutbox,
    /// Messages wait for entry evaluation, including imports and top-level
    /// await. Timers and module completions continue to enter the realm.
    boot_finished: watch::Sender<bool>,
}

impl Worker {
    #[expect(
        clippy::too_many_arguments,
        reason = "one worker's whole identity: its key, URL and source, where it reports and \
                  asks, its token, and the runtime and thread it runs on"
    )]
    fn new(
        js: SharedRuntime,
        key: WorkerKey,
        url: &str,
        source: ScriptSource,
        events: mpsc::UnboundedSender<WorkerEvent>,
        token: CancellationToken,
        host: HostOutbox,
        thread: JsThreadHandle,
    ) -> Rc<Self> {
        // A URL the loader refused would fail the root's load with the
        // loader's own message and ask for nothing; the name its answer is
        // then completed under is one nothing loads.
        let entry = normalize_module_url(url, url).unwrap_or_else(|_| url.to_owned());
        Rc::new(Self {
            js,
            key,
            source,
            entry,
            events,
            realm: RefCell::new(None),
            lifetime: Lifetime::new(token, thread),
            host,
            boot_finished: watch::channel(false).0,
        })
    }

    fn ended(&self) -> bool {
        self.lifetime.ended()
    }

    /// Opens this worker's realm and starts the load of its root module in
    /// it, and answers with the checkpoint receiver its clock task will
    /// watch. `None` is what leaves no worker at all — a runtime that never
    /// came up, a realm that could not be created or furnished, or a worker
    /// that ended before it booted.
    ///
    /// The root module is the module at the worker's URL, loaded as an
    /// `import()` of it would be: the engine writes nothing around it and
    /// installs no global scope first. `bobcat:bts` imports `bobcat:worker`
    /// and `bobcat:timers` itself, and so does any worker script that wants
    /// `postMessage` or `setTimeout`.
    ///
    /// The worker's first job, queued as its `Start` is served, the way a
    /// view's realm opens as that view's first job. Nothing is waited for
    /// first: the load waits for a script the host was asked for, which
    /// [`consume_messages`] completes once it arrives, and the realm's own
    /// loader answers an engine name such as the BTS's `bobcat:bts` at once,
    /// which then evaluates inside this job. So a runtime that never came up
    /// fails the worker here, whether or not its script has been answered.
    ///
    /// The script's *own* outcome is not among the failures: by the time it
    /// runs the realm is built, so a script that throws on load is reported
    /// exactly like a timer callback that throws later, and leaves a worker
    /// that is up and listening. That is what HTML's "run a worker" does — it
    /// reports the exception and goes on to enable the port queue and run the
    /// event loop — and it matters because a script registers its handlers
    /// before whatever optional work fails.
    ///
    /// A job like every other entry, and the one that does not go through
    /// [`owner::enter`], because the realm it would enter does not exist until
    /// it returns. [`boot_worker`] is what queues it.
    fn boot(self: &Rc<Self>, name: String) -> Option<watch::Receiver<u64>> {
        // A worker whose lifetime already ended opens nothing.
        if self.ended() {
            return None;
        }
        let opened = {
            let mut runtime = self.js.borrow_mut();
            match runtime.as_mut() {
                // The runtime failed once, for every worker that will ever be
                // asked for. Each hears the same reason.
                Err(error) => Err(error.clone()),
                Ok(js) => {
                    // This worker's own token is the one `host` carries, so
                    // what a `Future.wait` or a `require` parks on is
                    // cancelled with the worker rather than with the view: a
                    // `Terminate` read while the job is parked ends the wait,
                    // because that token is the one [`owner::end`] cancels.
                    realm::open_realm(
                        js,
                        &self.host,
                        self.lifetime.thread().clone(),
                        Some(self.key),
                        self.source,
                        |engine, js| {
                            let events = self.events.clone();
                            let key = self.key;
                            install_worker_members(engine, js, key, &self.host, name, move |data| {
                                let _ = events.send(WorkerEvent {
                                    key,
                                    payload: WorkerPayload::Message(data),
                                });
                            })
                        },
                    )
                    .map(|(core, flags)| WorkerRealm {
                        core,
                        scope_installed: flags.scope_installed,
                        closing: flags.closing,
                    })
                    .map(|mut realm| {
                        if let Err(error) = realm.core.engine.load_root_module(js, &self.entry) {
                            // Nothing to clean up after: a throw at this module's
                            // top level rejects through the runtime's shared job
                            // queue, and what it leaves there is this realm's — it
                            // waits for this worker rather than reaching the next
                            // realm to be entered on this runtime.
                            policy::report(self, Scene::Boot, error);
                        }
                        // Both under the borrow the root module's load ran
                        // under, so a sibling's bump between this boot and
                        // the clock task's first poll is neither lost nor
                        // mistaken for this worker's own.
                        let checkpoints = js.checkpoints();
                        self.lifetime.record_checkpoint(js.checkpoint_generation());
                        (realm, checkpoints)
                    })
                }
            }
        };
        match opened {
            Ok((realm, checkpoints)) => {
                *self.realm.borrow_mut() = Some(Box::new(realm));
                // The first epilogue, inline: this is already job context and
                // the borrows the stretch above held are released, so what
                // the root module's load left owing is settled here rather
                // than a queue trip later. A root that is an engine name has
                // finished, which for the BTS is what lets `initialize`
                // through; one whose script the host was asked for waits for
                // that script, which this does not ask the host for again.
                let _ = owner::enter_now(self, |_, _| ());
                Some(checkpoints)
            }
            // The runtime never came up or the realm could not be built: the
            // worker is over and nothing of it will ever run.
            Err(error) => {
                policy::report(self, Scene::Open, error);
                None
            }
        }
    }

    /// Whether this worker's realm is up, for a test that has to wait for it.
    #[cfg(test)]
    fn is_live(&self) -> bool {
        self.realm.borrow().is_some()
    }

    /// Whether this worker's root module — the module at its URL — has
    /// finished, for a test that has to wait for that rather than for the
    /// realm, which is up before a fetched script has arrived.
    #[cfg(test)]
    fn is_booted(&self) -> bool {
        *self.boot_finished.borrow()
    }

    /// How many times the epilogue has run.
    #[cfg(test)]
    fn epilogue_count(&self) -> u64 {
        self.lifetime.epilogue_count()
    }

    /// [`owner::spawn`], for a test that starts a task of this worker's
    /// itself.
    #[cfg(test)]
    fn spawn(self: &Rc<Self>, future: impl std::future::Future<Output = ()> + 'static) {
        owner::spawn(self, future);
    }

    /// [`owner::enter_now`], for a test that stands in for the thread's top
    /// loop and runs an entry's body itself.
    #[cfg(test)]
    fn enter_now<T>(
        self: &Rc<Self>,
        operation: impl FnOnce(&mut WorkerRealm, &mut ScriptRuntime) -> T,
    ) -> Option<T> {
        owner::enter_now(self, operation)
    }
}

/// What the driver in [`owner`] is told about a worker: where its realm,
/// runtime and host are, that its reports go to the realm that created it,
/// and what a worker adds to the epilogue — a `close()` that ends it, the
/// root module finishing, and its script, which it completes itself. Neither
/// the end nor the release owes anything beyond the driver's own steps.
impl RealmOwner for Worker {
    type Realm = WorkerRealm;
    type Event = WorkerPayload;

    /// A rejected root module is not reported by the epilogue, which reads
    /// the root module's load only to learn that it has settled. Only the
    /// host holds that load's promise, so nothing in the realm can handle
    /// its rejection, and a checkpoint of this realm reports it as it
    /// reports every rejection nothing handles. The checkpoint that ends the
    /// entry the load settled in — the boot job, the completion of the
    /// script or of a module it imports, a timer — reports it named by that
    /// entry, or drops it with the other leftovers of the one failure that
    /// entry reported. So a failure of the root module is reported once,
    /// whichever job it is in, and the worker stays up, as HTML's "run a
    /// worker" leaves it. A BTS's root module, `bobcat:bts`, imports
    /// registered modules only: its entry is imported later, once
    /// `initialize` has arrived, and what the entry throws reaches the worker
    /// global's `reportError` instead.
    const BOOT_REJECTION: Option<(Scene, Option<&'static str>)> = None;

    fn lifetime(&self) -> &Lifetime {
        &self.lifetime
    }

    fn runtime(&self) -> &SharedRuntime {
        &self.js
    }

    fn realm(&self) -> &RefCell<Option<Box<WorkerRealm>>> {
        &self.realm
    }

    fn core(realm: &mut WorkerRealm) -> &mut RealmCore {
        &mut realm.core
    }

    fn host(&self) -> &HostOutbox {
        &self.host
    }

    fn send(&self, payload: WorkerPayload) {
        let _ = self.events.send(WorkerEvent {
            key: self.key,
            payload,
        });
    }

    /// The worker table: a worker whose realm or script could not be made
    /// ready is over, which is `Failed`, and anything its realm threw after
    /// that is `Errored`.
    fn row(scene: Scene) -> Row<WorkerPayload> {
        policy::worker(scene)
    }

    /// A panic is a `Failed`: a worker nothing will be heard from again.
    ///
    /// It is sent even after the worker reported an end of its own, because
    /// the panic latch is not the one that end went through. The creating
    /// realm drops it if it has already delivered that first end, though:
    /// delivering a worker's end removes the key's source, and a key without
    /// a source is reported to no one.
    fn panic_row() -> Row<WorkerPayload> {
        policy::worker_panic()
    }

    /// Every timer this realm armed that has come due — none once the script
    /// has called `close()`, which discards the worker's armed timers along
    /// with its queued messages; the realm itself goes with the epilogue
    /// that reports it.
    fn run_due_timers(realm: &mut WorkerRealm, js: &mut ScriptRuntime) -> Vec<ScriptError> {
        if realm.closing.get() {
            return Vec::new();
        }
        crate::timers::run_due_timers(&mut realm.core.engine, js, &realm.core.timers)
            .unwrap_or_default()
    }

    /// A `close()` from the script that just ran ends the worker rather than
    /// parking it again, and discards what it had armed and queued.
    fn after_timers(worker: &Rc<Self>, realm: &mut WorkerRealm) {
        if realm.closing.get() {
            owner::terminal(worker, WorkerPayload::Closed);
        }
    }

    fn booted(&self) -> bool {
        *self.boot_finished.borrow()
    }

    /// Lets through what [`consume_messages`] held while the root module was
    /// running, a rejected one included: the worker is up either way.
    fn mark_booted(&self) {
        self.boot_finished.send_replace(true);
    }

    /// The worker's script is answered by [`consume_messages`] from the
    /// answer to the request `createWorker` made. A URL that is an engine
    /// name, the BTS's `bobcat:bts` among them, was never requested, and the
    /// realm's own loader answers it without a module request.
    fn entry_name(&self, _realm: &WorkerRealm) -> Option<String> {
        Some(self.entry.clone())
    }
}

/// One worker's whole life on this thread: build it, start its boot, wait for
/// the end, reclaim. Task lifetime and nothing else.
async fn serve_worker(js: SharedRuntime, start: WorkerStart, thread: JsThreadHandle) {
    let WorkerStart {
        key,
        name,
        url,
        script,
        source,
        messages,
        events,
        token,
        sources,
    } = start;
    let worker = Worker::new(js, key, &url, source, events, token, sources, thread);
    owner::spawn(
        &worker,
        boot_worker(Rc::clone(&worker), name, script, messages),
    );
    owner::run_owner(&worker).await;
}

/// The worker's boot future: queue the worker's first job, which opens its
/// realm and loads its root module, start its message consumer beside that
/// job, and once the job has run start the clock a live worker has.
///
/// Every worker's root module is the module at its URL, the BTS's
/// `bobcat:bts` included. `script`, the answer to the request `createWorker`
/// made when it made one, goes to [`consume_messages`], which completes the
/// root module the load is waiting for.
///
/// The consumer does not wait for the boot job. That job can sit in the
/// queue behind a sibling's job parked on a synchronous wait, which runs no
/// other job, and a `Terminate` sent meanwhile still has to end this worker
/// at once: the end is what cancels the fetch of its script, and the boot
/// job opens nothing for a worker that has ended. Every job the consumer
/// queues runs after the boot job, because that one was queued first.
async fn boot_worker(
    worker: Rc<Worker>,
    name: String,
    script: Option<SourceAnswer>,
    messages: mpsc::UnboundedReceiver<WorkerMessage>,
) {
    let booted = run_job(&worker, move |worker| worker.boot(name));
    owner::spawn(
        &worker,
        consume_messages(Rc::clone(&worker), messages, script),
    );
    // The boot job runs the first epilogue itself, so the first deadline has
    // already been published by the time this returns — and that epilogue
    // may have ended the worker.
    let Some(checkpoints) = booted.await else {
        return;
    };
    if worker.ended() {
        return;
    }
    owner::spawn(
        &worker,
        serve_clock(Rc::clone(&worker), worker.lifetime.deadlines(), checkpoints),
    );
}

/// The painter's message is processed on this worker's event loop, by
/// `bobcat:animation-frame`: the module that filed the callbacks, whether the
/// BTS runtime or the worker's own script imported it.
///
/// Queued rather than awaited, for the reason [`consume_messages`] queues
/// everything: the consumer has to go on reading, so that a `Terminate` behind
/// this reaches [`owner::end`] even while the job this queued is parked.
fn deliver_vsync(worker: &Rc<Worker>, milliseconds: f64) {
    let reporting = Rc::clone(worker);
    drop(owner::enter(worker, move |realm, js| {
        if let Err(error) = realm.core.engine.call_module_export(
            js,
            crate::esm::ANIMATION_FRAME_MODULE_SPECIFIER,
            "__BobcatBeginFrame",
            &[HostArgument::Number(milliseconds)],
        ) {
            policy::report(&reporting, Scene::Frame, error);
        }
    }));
}

/// One native module's answer to one function argument of one call this
/// worker's realm made, handed to it by [`crate::native_module::deliver`].
fn deliver_module_callback(worker: &Rc<Worker>, call: u64, index: u32, arguments: Option<String>) {
    let reporting = Rc::clone(worker);
    drop(owner::enter(worker, move |realm, js| {
        let delivered = crate::native_module::deliver(
            &mut realm.core.engine,
            js,
            call,
            index,
            arguments.as_deref(),
        );
        if let Err(error) = delivered {
            policy::report(&reporting, Scene::HostCall, error);
        }
    }));
}

/// The one ordered consumer of what is posted to this worker, and of its
/// script when the host was asked for it.
///
/// **It never waits for a delivery it queued.** `Terminate` is in-band, behind
/// whatever was posted before it, so the consumer has to go on reading: each
/// delivery is one job queued and forgotten, and the queue's FIFO is what keeps
/// them in order. A `Terminate` — or the channel closing — therefore reaches
/// [`owner::end`] at once, even while an earlier delivery's job is parked on a
/// synchronous wait, and that end is what the wait itself listens for.
///
/// Posts that were queued ahead of a `Terminate` but whose jobs have not run
/// are discarded by the same mechanism: their jobs find the worker ended and do
/// nothing. That is HTML's terminate, which discards what is queued.
///
/// Until the root module has finished, a post is held here rather than
/// delivered: HTML queues what is posted before a worker's script has run and
/// delivers it after, which is what lets the commonest shape there is —
/// construct, then post — keep its first message. The `select!` is `biased`,
/// messages first, for the reason HTML's "terminate a worker" aborts the
/// fetch: a `Terminate` that lands in the same instant as the script must
/// win, so a worker told to stop before its script arrived never runs it.
/// Returning drops the script's receiving end, which is what cancels that
/// fetch.
///
/// The script's answer is read whenever it arrives, and it is the root module
/// the load waits for. There is none for a URL that is an engine name, the
/// BTS's `bobcat:bts` or `new Worker("bobcat:timers")` alike: `createWorker`
/// asks the host for nothing, and the realm's own loader loads the name or
/// refuses it, so the root module finishes without one.
async fn consume_messages(
    worker: Rc<Worker>,
    mut messages: mpsc::UnboundedReceiver<WorkerMessage>,
    mut script: Option<SourceAnswer>,
) {
    // What is posted before the root module has finished; `None` once it has
    // and what was held has been delivered.
    let mut held: Option<Vec<HostValue>> = Some(Vec::new());
    let mut ready = worker.boot_finished.subscribe();
    loop {
        if let Some(queued) = held.take_if(|_| *ready.borrow_and_update()) {
            for data in queued {
                deliver_post(&worker, data);
            }
        }
        tokio::select! {
            biased;
            message = messages.recv() => match message {
                // Explicit termination or collection of the MTS handle, or
                // the release of its realm.
                None | Some(WorkerMessage::Terminate) => break,
                Some(WorkerMessage::Post(data)) => match held.as_mut() {
                    Some(held) => held.push(data),
                    None => deliver_post(&worker, data),
                },
                Some(WorkerMessage::Vsync(milliseconds)) => deliver_vsync(&worker, milliseconds),
                // Immediately, like a frame and unlike a post: the entry that
                // has not finished importing may itself be awaiting this
                // answer, so queuing it behind boot would deadlock the call.
                Some(WorkerMessage::ModuleCallback { call, index, arguments }) => {
                    deliver_module_callback(&worker, call, index, arguments);
                }
            },
            // Polled only while it is still outstanding: a one-shot that has
            // answered must not be polled again.
            answered = async {
                owner::await_source(
                    script.as_mut().expect("the arm is enabled only while it is held"),
                )
                .await
            }, if script.is_some() => {
                script = None;
                let loaded = answered
                    .map_err(|error| error.to_string())
                    .and_then(|answer| owner::module_answer(&worker.entry, answer));
                match loaded {
                    Ok((url, source)) => complete_script(&worker, url, source),
                    Err(reason) => {
                        let error = platform_script_error(format!(
                            "loading the worker's script: {reason}"
                        ));
                        policy::report(&worker, Scene::Open, error);
                        return;
                    }
                }
            }
            changed = ready.changed(), if held.is_some() => if changed.is_err() { return; },
        }
    }
    owner::end(&worker);
}

/// Queues the completion of the worker's script, from the answer to the
/// request `createWorker` made for it, under the name the realm's load of its
/// root module asked for. The script is that root module. The response URL is
/// the module's URL: its `import.meta.url`, and the base its own imports
/// resolve against.
///
/// A module completed in a realm is that realm's own source and is never
/// named on the runtime, so two views that answer one URL with different
/// bytes each run their own, and a worker leaves no registration behind.
/// Nothing is written around the script, so it keeps its own line numbers.
///
/// A script that throws at its top level rejects the root module's load, and
/// the completion's checkpoint reports that rejection as this entry's
/// failure. The epilogue's read of the load after it only learns that the
/// load has settled (see `BOOT_REJECTION` in the worker's [`RealmOwner`]
/// impl), so the throw is reported once.
fn complete_script(worker: &Rc<Worker>, url: String, source: String) {
    let completing = Rc::clone(worker);
    drop(owner::enter(worker, move |realm, js| {
        let completed =
            realm
                .core
                .engine
                .complete_module(js, &completing.entry, Ok((&url, &source)));
        if let Err(error) = completed {
            policy::report(&completing, Scene::Boot, error);
        }
    }));
}

/// Queues one posted value for this worker's realm.
fn deliver_post(worker: &Rc<Worker>, data: HostValue) {
    let delivering = Rc::clone(worker);
    drop(owner::enter(worker, move |realm, js| {
        deliver(&delivering, realm, js, &data);
    }));
}

/// Hands one message value to a realm that is up.
fn deliver(
    worker: &Rc<Worker>,
    realm: &mut WorkerRealm,
    js_runtime: &mut ScriptRuntime,
    data: &HostValue,
) {
    // Closed, with its realm still standing until this entry's epilogue: HTML
    // discards whatever was queued behind a `close()`, so this message is
    // dropped rather than delivered to a worker that has already ended
    // itself.
    if realm.closing.get() {
        return;
    }
    // No global scope, so nothing in this realm receives a message: the
    // engine installs `bobcat:worker` in no realm, and this one's script
    // never imported it, or imported it in a module graph that has not run.
    // The message is dropped, and nothing is reported: a realm with no
    // `onmessage` to call has nothing to report either. The module having
    // *run* to its last statement, which reads `workerName`, is the test, not
    // the realm having an instance of it: a graph that failed to load, or is
    // still loading, leaves its modules compiled but never linked, and
    // reading the namespace of such a module crashes `QuickJS`. A module that
    // has run was linked first.
    //
    // Nothing else this thread delivers needs the test. A frame, a due
    // timer, a native module's answer and a future's settle each answer a
    // host member that `bobcat:animation-frame`, `bobcat:timers`,
    // `bobcat:native-modules` or `bobcat:future` called while it ran, so the
    // module each is delivered to has run. A post is the one delivery no
    // module's call requested.
    if !realm.scope_installed.get() {
        return;
    }
    let delivered = realm.core.engine.call_module_export(
        js_runtime,
        WORKER_MODULE_SPECIFIER,
        WORKER_DELIVER_EXPORT,
        &[data.as_argument()],
    );
    if let Err(error) = delivered {
        policy::report(worker, Scene::Listener, error);
    }
}

#[cfg(test)]
mod tests {
    //! Two worker realms on one runtime, driven the way [`serve_worker`]
    //! drives one — except that the test keeps both [`Worker`]s, so it can
    //! count the epilogues each of them ran.

    use tokio::sync::oneshot;

    use super::*;
    use crate::resource::{LoadedSource, SourceRequest};

    /// How many times the test lets every ready task run before it gives up on
    /// something happening. A hang detector rather than a schedule: everything
    /// here is on one thread and cooperative.
    const TURNS: usize = 512;

    /// The script [`start`] answers with: what a worker script that wants
    /// `postMessage`, `onmessage` and the timer globals imports, and nothing
    /// else.
    const GLOBAL_SCOPE: &str = "import 'bobcat:worker'; import 'bobcat:timers';";

    /// Runs one test body as the `main` task of a [`JsThread`], which is the
    /// shape this thread itself runs: the body's turns are scheduler turns, and
    /// the jobs the workers queue run between them.
    fn on_a_js_thread<F, B>(body: B)
    where
        F: Future<Output = ()> + 'static,
        B: FnOnce(JsThreadHandle) -> F,
    {
        let thread = JsThread::new();
        let body = body(thread.handle());
        thread.run(body);
    }

    /// One started worker, with the test holding every end of it.
    struct Started {
        worker: Rc<Worker>,
        messages: mpsc::UnboundedSender<WorkerMessage>,
        /// Held so the channels stay open for as long as the worker does.
        events: mpsc::UnboundedReceiver<WorkerEvent>,
        /// The host end of what this worker asks for. Held rather than
        /// dropped: a dropped receiver answers every request with nothing,
        /// and the pin below needs one load that stays out.
        sources: mpsc::UnboundedReceiver<crate::link::ViewNotice>,
    }

    /// Starts one worker on `js`, with its script already answered.
    ///
    /// The script imports the worker's global scope and does nothing else,
    /// because what most of these pins are about is which task ran rather
    /// than what the script said. The import is what makes a post to it
    /// enter JavaScript: a realm without that scope drops what is posted.
    fn start(js: &SharedRuntime, thread: &JsThreadHandle, key: u64) -> Started {
        start_running(js, thread, key, GLOBAL_SCOPE.to_owned())
    }

    /// The same, over a script of the test's own.
    fn start_running(
        js: &SharedRuntime,
        thread: &JsThreadHandle,
        key: u64,
        source: String,
    ) -> Started {
        let (events, events_rx) = mpsc::unbounded_channel();
        let (messages, messages_rx) = mpsc::unbounded_channel();
        let (script, script_rx) = oneshot::channel();
        let url = format!("app:///worker{key}.js");
        let _ = script.send(Ok(LoadedSource::Module {
            source,
            url: url.clone(),
        }));
        let key = WorkerKey::new(key);
        // A token of its own rather than a child of anything: no view created
        // this worker, and nothing here releases one. The outbox carries that
        // same token, the way `WorkerOwner::start` hands it over, so what this
        // worker asks the host for ends when this worker does.
        let token = CancellationToken::new();
        let (sources, sources_rx) = mpsc::unbounded_channel();
        let worker = Worker::new(
            Rc::clone(js),
            key,
            &url,
            ScriptSource::Worker(crate::view::WorkerId::from(key)),
            events,
            token.clone(),
            HostOutbox::new(
                sources,
                std::sync::Arc::new(crate::NoWakeup),
                token,
                None,
                crate::link::detached_base(),
            ),
            thread.clone(),
        );
        worker.spawn(boot_worker(
            Rc::clone(&worker),
            String::new(),
            Some(script_rx),
            messages_rx,
        ));
        Started {
            worker,
            messages,
            events: events_rx,
            sources: sources_rx,
        }
    }

    /// One worker runtime, furnished the way [`run`] furnishes this thread's.
    fn worker_runtime() -> SharedRuntime {
        let js = build_runtime().expect("the worker runtime builds");
        Rc::new(RefCell::new(Ok(js)))
    }

    /// A sibling worker's entry drains the promise-job queue every realm on
    /// this runtime shares, so this worker has to settle what its own realm
    /// owes. Its clock task's checkpoint arm is what tells it to: without that
    /// arm, the only thing that could is an entry of its own.
    #[test]
    fn a_sibling_workers_entry_settles_this_worker() {
        on_a_js_thread(|thread| async move {
            let js = worker_runtime();
            let first = start(&js, &thread, 1);
            let second = start(&js, &thread, 2);
            // Booted rather than live: a realm is up before its script has
            // run, and the job that runs it is an entry of its own.
            for _ in 0..TURNS {
                if first.worker.is_booted() && second.worker.is_booted() {
                    break;
                }
                task::yield_now().await;
            }
            assert!(
                first.worker.is_booted() && second.worker.is_booted(),
                "both workers booted on the one runtime"
            );

            // Parked: each has settled its own boot and the other's, and a
            // settle that finds nothing due enters no JavaScript, so nothing
            // is left bumping the generation.
            for _ in 0..8 {
                task::yield_now().await;
            }
            let settled = first.worker.epilogue_count();

            // One entry of the sibling's, which runs a checkpoint over the
            // queue both realms share.
            second
                .messages
                .send(WorkerMessage::Post(HostValue::String("ping".to_owned())))
                .expect("the second worker is still serving");
            for _ in 0..TURNS {
                if first.worker.epilogue_count() > settled {
                    break;
                }
                task::yield_now().await;
            }
            assert_eq!(
                first.worker.epilogue_count(),
                settled + 1,
                "the sibling's checkpoint is what settles this worker, once"
            );
        });
    }

    /// A worker task that ends may have left the job queue every realm here
    /// shares with work in it — a panic in one of its jobs is caught and the
    /// task returns normally — so its ending settles every other realm on
    /// the runtime once, the way a view task's ending does on `bobcat-main`.
    ///
    /// The task that ends runs no JavaScript of its own: its realm opens and
    /// starts the load of its root module as its `Start` is served, which
    /// runs a checkpoint that settles the parked worker as well, and the
    /// count is taken only once that has stopped moving. Its script, which is
    /// that root module, never arrives, and a `Terminate` ends it while the
    /// load waits for it. So the one settle counted is the ending's and
    /// nothing else's.
    #[test]
    fn a_finished_worker_task_makes_a_parked_sibling_settle() {
        on_a_js_thread(|thread| async move {
            let js = worker_runtime();
            let first = start(&js, &thread, 1);
            assert!(
                until(|| first.worker.is_booted()).await,
                "the first worker booted"
            );

            let (commands, receiver) = mpsc::unbounded_channel();
            task::spawn_local(serve_workers(
                Rc::clone(&js),
                receiver,
                thread.clone(),
                Rc::default(),
            ));
            // Held for the whole test: a dropped script sender would fail the
            // second worker, which ends its task before the `Terminate`.
            let (_script, script) = oneshot::channel();
            let (messages, incoming) = mpsc::unbounded_channel();
            let (events, _events) = mpsc::unbounded_channel();
            let (notices, _notices) = mpsc::unbounded_channel();
            commands
                .send(WorkerCommand::Start(dedicated_start(
                    2, script, incoming, events, notices,
                )))
                .expect("the loop is serving");

            // Parked: whatever the `Start` set off — the load of the second
            // realm's root module ran a checkpoint over the queue both realms
            // share — has settled, and a settle that finds nothing due enters
            // no JavaScript, so the count stops moving.
            let mut settled = first.worker.epilogue_count();
            for _ in 0..TURNS {
                for _ in 0..8 {
                    task::yield_now().await;
                }
                let count = first.worker.epilogue_count();
                if count == settled {
                    break;
                }
                settled = count;
            }

            messages
                .send(WorkerMessage::Terminate)
                .expect("the second worker is waiting for its script");
            assert!(
                until(|| first.worker.epilogue_count() > settled).await,
                "the second worker's ending settled the first"
            );
            for _ in 0..8 {
                task::yield_now().await;
            }
            assert_eq!(
                first.worker.epilogue_count(),
                settled + 1,
                "the ending is what settles this worker, once"
            );
        });
    }

    /// A worker on a runtime that never came up fails as its `Start` is
    /// served: its realm is opened then, so the failure waits for no script,
    /// and this one's is never answered at all.
    #[test]
    fn a_worker_on_a_runtime_that_never_came_up_fails_at_its_start() {
        on_a_js_thread(|thread| async move {
            let js: SharedRuntime = Rc::new(RefCell::new(Err(platform_script_error(
                "injected".to_owned(),
            ))));
            let (commands, receiver) = mpsc::unbounded_channel();
            task::spawn_local(serve_workers(
                Rc::clone(&js),
                receiver,
                thread.clone(),
                Rc::default(),
            ));
            let (script, answer) = oneshot::channel();
            let (_messages, incoming) = mpsc::unbounded_channel();
            let (events, mut reported) = mpsc::unbounded_channel();
            let (notices, _notices) = mpsc::unbounded_channel();
            commands
                .send(WorkerCommand::Start(dedicated_start(
                    1, answer, incoming, events, notices,
                )))
                .expect("the loop is serving");

            let Some(event) = next(&mut reported).await else {
                panic!("the worker reported its end without its script")
            };
            assert_eq!(event.key, WorkerKey::new(1));
            let WorkerPayload::Failed(error) = event.payload else {
                panic!("a worker whose realm cannot be built is over")
            };
            assert!(error.message.contains("injected"), "{}", error.message);
            assert!(
                until(|| script.is_closed()).await,
                "the ended worker let go of its unanswered script"
            );
        });
    }

    /// A `Start` for the worker `key` over `app:///worker<key>.js`, whose
    /// script the test answers through the sending end of `script`, or holds
    /// unanswered.
    fn dedicated_start(
        key: u64,
        script: SourceAnswer,
        messages: mpsc::UnboundedReceiver<WorkerMessage>,
        events: mpsc::UnboundedSender<WorkerEvent>,
        notices: mpsc::UnboundedSender<crate::link::ViewNotice>,
    ) -> WorkerStart {
        let token = CancellationToken::new();
        let key = WorkerKey::new(key);
        WorkerStart {
            key,
            name: String::new(),
            url: format!("app:///worker{}.js", key.get()),
            script: Some(script),
            source: ScriptSource::Worker(crate::view::WorkerId::from(key)),
            messages,
            events,
            token: token.clone(),
            sources: HostOutbox::new(
                notices,
                Arc::new(crate::NoWakeup),
                token,
                None,
                crate::link::detached_base(),
            ),
        }
    }

    impl Started {
        /// Runs one entry's body here and now, which is what the engine
        /// thread's top loop does with a queued job: these two pins are about
        /// what `QuickJS` finalization does across two realms, so standing in
        /// for the loop is simpler than queueing and draining.
        fn execute(&self, source: &str) {
            self.worker
                .enter_now(|realm, js| {
                    realm
                        .core
                        .engine
                        .start_module(js, source, "app:///observer-test.js")
                        .unwrap();
                    assert!(realm.core.engine.module_finished().unwrap());
                })
                .expect("the worker is live");
        }

        fn collect(&self) {
            self.worker
                .enter_now(|realm, js| realm.core.engine.collect_garbage(js).unwrap())
                .expect("the worker is live");
        }

        /// What this worker posted, each rendered as JSON for comparison.
        /// The transport carries a structured clone, which Rust cannot read
        /// into; `wire_json` reads it back in a realm of its own.
        fn messages(&mut self) -> Vec<String> {
            let mut messages = Vec::new();
            while let Ok(event) = self.events.try_recv() {
                match event.payload {
                    WorkerPayload::Message(message) => {
                        messages.push(super::super::wire_json(&message));
                    }
                    WorkerPayload::Errored(error) | WorkerPayload::Failed(error) => {
                        panic!("observer error: {}", error.message)
                    }
                    WorkerPayload::Closed => panic!("observer worker closed"),
                }
            }
            messages
        }
    }

    /// Drive actual `QuickJS` finalization without exposing a GC API to app code.
    fn with_observers(test: impl FnOnce(&mut Started, &mut Started) + 'static) {
        on_a_js_thread(|thread| async move {
            let js = worker_runtime();
            let mut first = start(&js, &thread, 1);
            let mut second = start(&js, &thread, 2);
            // Booted rather than live: `execute` evaluates a module of its
            // own, which would take the place of a root module whose load is
            // still outstanding.
            for _ in 0..TURNS {
                if first.worker.is_booted() && second.worker.is_booted() {
                    break;
                }
                task::yield_now().await;
            }
            assert!(first.worker.is_booted() && second.worker.is_booted());
            test(&mut first, &mut second);
        });
    }

    #[test]
    fn object_destruction_observers_finalize_in_js_once_without_retaining_the_target() {
        with_observers(|first, _| {
            first.execute(
                r"
                import {lynx} from 'bobcat:bts-runtime';
                const create = lynx.getNativeApp().createJSObjectDestructionObserver;
                globalThis.observer = create(function(...args) {
                    postMessage(['finalized', this === undefined, args.length]);
                });
                if (Object.keys(observer).length || observer.anything !== undefined)
                    throw Error('observer is not initially empty');
                observer.field = 1;
                if (observer.field !== 1 || Object.getPrototypeOf(observer) !== Object.prototype)
                    throw Error('observer must be an ordinary object');
            ",
            );
            first.collect();
            assert!(
                first.messages().is_empty(),
                "the live target retains its registration"
            );
            first.execute("delete globalThis.observer; postMessage('script-end');");
            first.collect();
            assert_eq!(
                first.messages(),
                [r#""script-end""#, r#"["finalized",true,0]"#]
            );
            first.collect();
            assert!(first.messages().is_empty(), "a target finalizes once");
        });
    }

    #[test]
    fn object_observer_cleanup_uses_the_shared_js_checkpoint() {
        with_observers(|first, second| {
            first.execute(r"
                import {lynx} from 'bobcat:bts-runtime';
                globalThis.observer = lynx.getNativeApp().createJSObjectDestructionObserver(() => postMessage('finalized'));
            ");
            first.execute("delete globalThis.observer;");
            second.collect();
            assert_eq!(first.messages(), [r#""finalized""#]);
            assert!(second.messages().is_empty());
        });
    }

    /// Lets every ready task run until `ready` says so, or gives up after
    /// [`TURNS`] of them. `false` is having given up.
    async fn until(mut ready: impl FnMut() -> bool) -> bool {
        for _ in 0..TURNS {
            if ready() {
                return true;
            }
            task::yield_now().await;
        }
        false
    }

    /// The next thing to arrive on `channel`, waited out the same way.
    /// `None` is nothing having arrived.
    async fn next<T>(channel: &mut mpsc::UnboundedReceiver<T>) -> Option<T> {
        let mut arrived = None;
        until(|| {
            arrived = channel.try_recv().ok();
            arrived.is_some()
        })
        .await;
        arrived
    }

    /// A `Terminate` is in band behind whatever was posted before it, so the
    /// consumer cannot wait for the deliveries it queued: if it did, a worker
    /// whose job is parked on a synchronous wait could never be told to stop.
    ///
    /// The wait is a real one — the handler's `require`, asking this worker's
    /// host for a module the test receives and deliberately never answers.
    /// What the pin asserts is the pair: the terminate reaches [`owner::end`]
    /// while the job is parked, which is what ends the wait and throws into
    /// the handler's `catch`; and the post queued between the two is
    /// discarded rather than delivered, because its job finds the worker
    /// ended.
    #[test]
    fn an_in_band_terminate_ends_a_worker_whose_job_is_waiting_and_discards_what_is_behind_it() {
        on_a_js_thread(|thread| async move {
            let js = worker_runtime();
            // Every path through the handler posts, so a delivery that
            // happened at all would be heard.
            let mut started = start_running(
                &js,
                &thread,
                1,
                r"import 'bobcat:worker'; import 'bobcat:timers';
                import { createRequire } from 'bobcat:module';
                const require = createRequire(import.meta.url);
                onmessage = event => {
                    try { postMessage('loaded ' + require('./never.cjs')); }
                    catch (error) { postMessage(event.data + ': ' + String(error)); }
                };
                "
                .to_owned(),
            );
            assert!(
                until(|| started.worker.is_live()).await,
                "the worker booted"
            );

            started
                .messages
                .send(WorkerMessage::Post(HostValue::String("first".to_owned())))
                .expect("the worker is serving");
            // This test's own body is a task, so it goes on running inside the
            // job's wait — which is the property the whole model rests on, and
            // which is how the request below is read at all.
            let Some(crate::link::ViewNotice::RequestSource {
                request,
                completion,
            }) = next(&mut started.sources).await
            else {
                panic!("the require asked its host for a module");
            };
            assert!(
                matches!(&request, SourceRequest::Module(url) if url == "app:///never.cjs"),
                "the require resolved against the worker's own URL"
            );
            assert!(
                next(&mut started.events).await.is_none(),
                "nothing was posted: the handler is inside its require"
            );

            // Both sent while that job is still parked on `completion`, which
            // is held rather than answered or dropped: the consumer reads them
            // anyway, which is what this pins.
            started
                .messages
                .send(WorkerMessage::Post(HostValue::String(
                    "behind the wait".to_owned(),
                )))
                .expect("the worker is serving");
            started
                .messages
                .send(WorkerMessage::Terminate)
                .expect("the worker is serving");
            assert!(
                until(|| started.worker.ended()).await,
                "the terminate ended the worker while its job was parked"
            );
            assert!(
                completion.is_cancelled(),
                "the request the host still holds is cancelled with the worker"
            );

            // The end is the wait's other arm, so the `require` threw where
            // the load would have returned.
            let posted = next(&mut started.events).await;
            let Some(WorkerPayload::Message(message)) = posted.map(|event| event.payload) else {
                panic!("the handler posted what its require did");
            };
            let caught = super::super::wire_json(&message);
            assert!(
                caught.starts_with("\"first: ") && caught.contains("app:///never.cjs"),
                "the require threw into the handler's catch: {caught}"
            );

            // And the post between the two never reaches the realm: its job
            // was queued behind the waiting one and finds a worker that ended.
            assert!(
                next(&mut started.events).await.is_none(),
                "a post queued behind a terminate is discarded, not delivered"
            );
        });
    }

    /// Starts a worker whose script arms a zero-delay timer running `callback`
    /// and one more timer that never comes due, waits until the callback is
    /// parked in the `require` of `app:///never.cjs` it ends with, and
    /// terminates the worker there. Answers with the worker once the
    /// terminate has ended it.
    ///
    /// The `require` is the synchronous wait: tasks go on running inside it,
    /// so the consumer reads the `Terminate` and ends the worker while the
    /// timer's callback is still on the stack. The end makes the `require`
    /// throw, into the callback's own `catch`.
    async fn terminated_inside_a_timer(thread: &JsThreadHandle, callback: &str) -> Started {
        let js = worker_runtime();
        let mut started = start_running(
            &js,
            thread,
            1,
            format!(
                r"import 'bobcat:worker'; import 'bobcat:timers';
                import {{ createRequire }} from 'bobcat:module';
                const require = createRequire(import.meta.url);
                setTimeout(() => {{}}, 60000);
                setTimeout(() => {{
                    {callback}
                    try {{ require('./never.cjs'); }} catch {{}}
                }}, 0);
                "
            ),
        );
        let Some(crate::link::ViewNotice::RequestSource {
            request,
            completion,
        }) = next(&mut started.sources).await
        else {
            panic!("the timer's require asked its host for a module");
        };
        assert!(
            matches!(&request, SourceRequest::Module(url) if url == "app:///never.cjs"),
            "the require is the timer's"
        );
        started
            .messages
            .send(WorkerMessage::Terminate)
            .expect("the worker is serving");
        assert!(
            until(|| started.worker.ended()).await,
            "the terminate ended the worker while its timer's callback was parked"
        );
        assert!(
            completion.is_cancelled(),
            "the require's request is cancelled with the worker"
        );
        started
    }

    /// Every module request that reaches the host from here on, once every
    /// ready task has had its turns.
    async fn requests_from_now_on(started: &mut Started) -> Vec<SourceRequest> {
        let mut requests = Vec::new();
        until(|| {
            while let Ok(notice) = started.sources.try_recv() {
                if let crate::link::ViewNotice::RequestSource { request, .. } = notice {
                    requests.push(request);
                }
            }
            false
        })
        .await;
        requests
    }

    /// A worker that ended while a due timer's callback was parked on a
    /// synchronous wait is over when that callback returns: the epilogue it
    /// returns into asks the host for none of the imports the callback
    /// started, reports nothing, and re-arms none of the timers the end
    /// withdrew.
    #[test]
    fn a_worker_ended_inside_a_timer_callback_asks_for_nothing_after_it() {
        on_a_js_thread(|thread| async move {
            let mut started =
                terminated_inside_a_timer(&thread, "import('./later.js').catch(() => {});").await;
            let requests = requests_from_now_on(&mut started).await;
            assert!(
                requests.is_empty(),
                "an ended worker's epilogue asks the host for nothing: {requests:?}"
            );
            assert!(
                next(&mut started.events).await.is_none(),
                "an ended worker reports nothing"
            );
            assert_eq!(
                started.worker.lifetime.armed_deadline(),
                None,
                "the deadline the end withdrew stays withdrawn"
            );
        });
    }

    /// A `close()` a timer's callback made before its worker was terminated
    /// is not reported: the terminate ended the worker first, and an ended
    /// worker's epilogue does not reach the step that reports `Closed`.
    #[test]
    fn a_close_made_before_a_terminate_inside_a_timer_callback_is_not_reported() {
        on_a_js_thread(|thread| async move {
            let mut started = terminated_inside_a_timer(&thread, "close();").await;
            assert!(
                next(&mut started.events).await.is_none(),
                "a worker its creator terminated does not report `Closed`"
            );
        });
    }
}
