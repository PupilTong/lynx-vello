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
//! is the one ordered consumer of what is posted and of a dedicated worker's
//! script; [`serve_clock`] owns this realm's one
//! pinned sleep and watches the runtime-wide checkpoint generation, because the
//! job queue every worker realm here drains is the runtime's and a sibling's
//! entry can finish this realm's jobs. Every one of them reaches the realm
//! through [`Worker::enter`], which queues one job: it runs one synchronous
//! operation and then settles what it left owing — the timers that came due, a
//! `close()` it may have called, the next deadline, and the checkpoint
//! generation as of that entry.
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
//! - each [`consume_messages`]' wait on what is posted, versus a dedicated worker's script while it
//!   is outstanding, versus the realm's root module finishing until it has — one task's wait rather
//!   than a scheduler;
//! - one [`serve_clock`] per live worker realm, waiting on its deadline, the re-arm that moves it,
//!   and a sibling's checkpoint.

use std::cell::{Cell, RefCell};
use std::future::Future;
use std::panic::AssertUnwindSafe;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use quickjs_rust_bridge::{HostArgument, HostValue};
use rustc_hash::FxHashMap;
use tokio::sync::{mpsc, watch};
use tokio::task::{self, JoinError, JoinSet};
use tokio_util::sync::CancellationToken;

use super::scope::{WORKER_DELIVER_EXPORT, install_worker_members, worker_boot_source};
use super::{
    BackgroundStart, WorkerCommand, WorkerEvent, WorkerKey, WorkerMessage, WorkerPayload,
    WorkerRole, WorkerStart,
};
use crate::esm::{WORKER_BOOT_SPECIFIER, WORKER_MODULE_SPECIFIER, build_runtime};
use crate::jobs::{JsThread, JsThreadHandle};
use crate::lifetime::{EndOnUnwind, Lifetime, Settles, run_job, serve_clock};
use crate::link::{HostOutbox, SourceAnswer};
use crate::main::quickjs::{
    ScriptRuntime, SharedRuntime, mark_checkpoint_later, normalize_module_url,
};
use crate::realm::{self, RealmCore, context_of};
use crate::resource::{LoadedSource, SourceRequest};
use crate::script::ScriptError;
use crate::threads::{panicked, platform_script_error};
use crate::timers::run_due_timers;
use crate::view::ScriptSource;

/// Who each worker task reports to: its worker's key and the creating view's
/// channel, beside the worker's own token. Kept by [`serve_workers`] until it
/// joins the task, and read by [`report_thread_trap`] when the whole thread is
/// over.
type Reporters = Rc<RefCell<FxHashMap<task::Id, Reporter>>>;

/// One entry of [`Reporters`].
///
/// The token is the one [`Worker::end`] cancels, and every way a worker ends
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
            report_thread_trap(
                &trapped,
                &reporters,
                &platform_script_error(format!("the worker thread {detail}")),
            );
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
        report_thread_trap(
            trapped,
            &reporters,
            &panicked("the worker thread panicked", payload.as_ref()),
        );
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
        let error = panicked("the worker thread panicked", error.into_panic().as_ref());
        let _ = reporter.events.send(WorkerEvent {
            key: reporter.key,
            payload: WorkerPayload::Failed(error),
        });
    }
    mark_checkpoint_later(js, thread);
}

/// The whole thread is over: the flag `bobcat-main` reads before each `Start`
/// is set, and the creator of every worker still live on it hears `Failed`.
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
fn report_thread_trap(trapped: &AtomicBool, reporters: &Reporters, error: &ScriptError) {
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
            payload: WorkerPayload::Failed(error.clone()),
        });
    }
}

/// One live worker on this thread: its realm and everything that realm owns.
///
/// `core` is what [`realm::open_realm`] builds for every realm; `closing` is
/// the one piece of state the worker's own host module,
/// `bobcat-internal:worker`, adds to it.
struct WorkerRealm {
    core: RealmCore,
    /// Set by the native `closeWorker` export. A flag rather than a direct
    /// teardown because it is written from inside the realm it would tear
    /// down: the task reads it once the call that set it has returned.
    closing: Rc<Cell<bool>>,
}

/// What a worker is, which is what decides whether a message has anywhere to
/// go.
enum WorkerState {
    /// The realm has not been opened yet: the worker's first job, which
    /// opens it, has not run.
    Loading,
    Live(WorkerRealm),
    /// The realm could not be built, or the worker is over and the owner has
    /// reclaimed it.
    Gone,
}

/// One worker: what its tasks act on, and the one boundary they enter its
/// realm through.
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
    /// The name the root module's `import` of a dedicated worker's script
    /// asks the realm for: the script's URL, normalized the way the module
    /// loader normalizes a specifier imported from `bobcat:worker-boot`,
    /// which for that absolute URL is the identity. The epilogue never asks
    /// the host for it, because `createWorker` already has, and
    /// [`consume_messages`] completes it under this name from that answer.
    /// `None` for the BTS, whose root module imports registered modules only.
    entry: Option<String>,
    /// Where this worker reports, which is the creating view's own channel.
    events: mpsc::UnboundedSender<WorkerEvent>,
    state: RefCell<WorkerState>,
    /// Every task of this worker, the token that ends them, the latch this
    /// thread reads, and the two numbers this realm's clock task waits on — the
    /// deadline it armed and the generation its own last entry recorded.
    lifetime: Lifetime,
    /// Whether this worker has already been told why it is over. The first
    /// report wins, so a `Failed` and a `Closed` cannot both arrive. A panic
    /// has a latch of its own on the lifetime.
    reported: Cell<bool>,
    sources: HostOutbox,
    /// Messages wait for entry evaluation, including imports and top-level
    /// await. Timers and module completions continue to enter the realm.
    boot_finished: watch::Sender<bool>,
    /// How many times the epilogue has run, for the test that counts the wakes
    /// a worker answers.
    #[cfg(test)]
    epilogues: Cell<u64>,
}

impl Worker {
    fn new(
        js: SharedRuntime,
        key: WorkerKey,
        role: &WorkerRole,
        events: mpsc::UnboundedSender<WorkerEvent>,
        token: CancellationToken,
        sources: HostOutbox,
        thread: JsThreadHandle,
    ) -> Rc<Self> {
        let entry = match role {
            WorkerRole::Background(_) => None,
            // A URL the loader refused would fail the root module's import
            // with the loader's own message and ask for nothing; the name its
            // answer is then completed under is one nothing imports.
            WorkerRole::Dedicated { url, .. } => Some(
                normalize_module_url(WORKER_BOOT_SPECIFIER, url).unwrap_or_else(|_| url.clone()),
            ),
        };
        Rc::new(Self {
            js,
            key,
            source: role.source(key),
            entry,
            events,
            state: RefCell::new(WorkerState::Loading),
            lifetime: Lifetime::new(token, thread),
            reported: Cell::new(false),
            sources,
            boot_finished: watch::channel(false).0,
            #[cfg(test)]
            epilogues: Cell::new(0),
        })
    }

    /// Starts one more task of this worker.
    ///
    /// A panic anywhere in it ends the worker during the unwind — the guard
    /// holds this worker, so a sibling polled before the owner already finds it
    /// ended — and the owner turns the payload the lifetime hands it into the
    /// `Failed` the creating view hears.
    fn spawn(self: &Rc<Self>, future: impl Future<Output = ()> + 'static) {
        let guard = EndOnUnwind::new(self);
        self.lifetime.spawn(async move {
            let _guard = guard;
            future.await;
        });
    }

    /// Ends this worker, once. `true` for the call that did it.
    ///
    /// The owner calls it after its wait too, mirroring cancellation into the
    /// local latch. This is the lifetime's own end, which also withdraws the
    /// deadline the worker had armed.
    fn end(&self) -> bool {
        self.lifetime.end()
    }

    fn ended(&self) -> bool {
        self.lifetime.ended()
    }

    /// The worker is over and nothing of it will ever run: its script never
    /// arrived, or its realm could not be built.
    fn failed(&self, error: ScriptError) {
        if !self.reported.replace(true) {
            let _ = self.events.send(WorkerEvent {
                key: self.key,
                payload: WorkerPayload::Failed(error),
            });
        }
        self.end();
    }

    /// Reports a panic as this worker's, and ends it.
    ///
    /// One report per worker whichever of the owner's two waits saw the panic
    /// first, and not gated by [`Self::reported`]: a worker that already
    /// reported a failed script and then traps still sends this `Failed`.
    /// The creating realm drops it if it has already delivered that first
    /// one, though: delivering a worker's end removes the key's source, and a
    /// key without a source is reported to no one.
    fn trapped(&self, payload: &(dyn std::any::Any + Send)) {
        if self.lifetime.report_panic() {
            let _ = self.events.send(WorkerEvent {
                key: self.key,
                payload: WorkerPayload::Failed(panicked("the worker thread panicked", payload)),
            });
        }
        self.end();
    }

    /// Queues one synchronous operation against this worker's realm, which
    /// settles what it owes, and answers with what it returned.
    ///
    /// `None` is a realm that is not live, a worker that has ended, an
    /// operation that trapped, or a thread that is over. The whole body runs
    /// under [`run_job`]'s `catch_unwind`, because a job runs in this thread's
    /// top loop rather than inside a task that could catch it: a panic here
    /// would otherwise take the thread down instead of this one worker. A
    /// host function of the worker's realm that panics is caught there too,
    /// once the realm's checkpoint resumes its panic.
    fn enter<T, O>(self: &Rc<Self>, operation: O) -> impl Future<Output = Option<T>> + use<T, O>
    where
        T: 'static,
        O: FnOnce(&mut WorkerRealm, &mut ScriptRuntime) -> T + 'static,
    {
        run_job(self, move |worker| worker.enter_now(operation))
    }

    /// The body of one entry, as the job runs it.
    ///
    /// Both borrows are held for the whole entry, a synchronous wait inside
    /// the operation included. Nothing else can want them: only a job takes
    /// either, and no other job runs until this one returns.
    fn enter_now<T>(
        self: &Rc<Self>,
        operation: impl FnOnce(&mut WorkerRealm, &mut ScriptRuntime) -> T,
    ) -> Option<T> {
        if self.ended() {
            return None;
        }
        let mut runtime = self.js.borrow_mut();
        let Ok(js) = runtime.as_mut() else {
            return None;
        };
        let mut state = self.state.borrow_mut();
        let WorkerState::Live(realm) = &mut *state else {
            return None;
        };
        let value = operation(realm, js);
        self.epilogue(realm, js);
        Some(value)
    }

    /// Everything one entry into this realm leaves owing: the timers that
    /// have come due, a `close()` whatever just ran may have called, entry
    /// completion, module requests, the futures a `.then` asked to settle,
    /// the next timer deadline, and the checkpoint generation as of this
    /// entry.
    fn epilogue(self: &Rc<Self>, realm: &mut WorkerRealm, js: &mut ScriptRuntime) {
        if self.ended() {
            return;
        }
        #[cfg(test)]
        self.epilogues.set(self.epilogues.get() + 1);
        fire_timers(&self.events, self.key, realm, js);
        // A `close()` from the script that just ran ends the worker rather
        // than parking it again, and discards what it had armed and queued.
        if realm.closing.get() {
            if !self.reported.replace(true) {
                let _ = self.events.send(WorkerEvent {
                    key: self.key,
                    payload: WorkerPayload::Closed,
                });
            }
            self.end();
            return;
        }
        if !*self.boot_finished.borrow() {
            let finished = match realm.core.engine.module_finished() {
                Ok(finished) => finished,
                Err(error) => {
                    report(&self.events, self.key, "running the worker's script", error);
                    true
                }
            };
            if finished {
                self.boot_finished.send_replace(true);
            }
        }
        while let Some(url) = realm.core.engine.take_module_request() {
            // A dedicated worker's script is answered by `consume_messages`
            // from the answer to the request `createWorker` made: completing
            // it is what resumes the root module's import, and it must never
            // reach the fetcher a second time.
            if self.entry.as_ref() == Some(&url) {
                continue;
            }
            let answer = self.sources.request(SourceRequest::Module(url.clone()));
            self.spawn(load_module(Rc::clone(self), url, answer));
        }
        for (id, future) in realm.core.futures.take_settle_requests() {
            self.spawn(settle_future(Rc::clone(self), id, future));
        }
        self.lifetime
            .arm_deadline(realm.core.timers.next_deadline());
        // Last, so it names the generation this entry ran up rather than the
        // one it started from.
        self.lifetime.record_checkpoint(js.checkpoint_generation());
    }

    /// Opens this worker's realm and evaluates its root module in it, and
    /// answers with the checkpoint receiver its clock task will watch. `None`
    /// is what leaves no worker at all — a runtime that never came up, a
    /// realm that could not be created or furnished, or a worker that ended
    /// before it booted.
    ///
    /// The worker's first job, queued as its `Start` is served, the way a
    /// view's realm opens as that view's first job. Nothing is waited for
    /// first: a dedicated worker's root module waits in its `import` for the
    /// script, which [`consume_messages`] completes once it arrives, and a
    /// BTS's root module imports registered modules only. So a runtime that
    /// never came up fails the worker here, whether or not its script has
    /// been answered.
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
    /// [`Self::enter`], because the realm it would enter does not exist until
    /// it returns. [`boot_worker`] is what queues it.
    fn boot(
        self: &Rc<Self>,
        name: String,
        background: Option<BackgroundStart>,
        root: &str,
    ) -> Option<watch::Receiver<u64>> {
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
                    // This worker's own token is the one `sources` carries,
                    // so what a `Future.wait` or a `require` parks on is
                    // cancelled with the worker rather than with the view: a
                    // `Terminate` read while the job is parked ends the wait,
                    // because that token is the one [`Worker::end`] cancels.
                    realm::open_realm(
                        js,
                        &self.sources,
                        self.lifetime.thread().clone(),
                        Some(self.key),
                        self.source,
                        |engine, js| {
                            let events = self.events.clone();
                            let key = self.key;
                            install_worker_members(
                                engine,
                                js,
                                key,
                                &self.sources,
                                name,
                                background,
                                move |data| {
                                    let _ = events.send(WorkerEvent {
                                        key,
                                        payload: WorkerPayload::Message(data),
                                    });
                                },
                            )
                        },
                    )
                    .map(|(core, closing)| WorkerRealm { core, closing })
                    .map(|mut realm| {
                        if let Err(error) =
                            realm
                                .core
                                .engine
                                .start_module(js, root, WORKER_BOOT_SPECIFIER)
                        {
                            // Nothing to clean up after: a throw at this module's
                            // top level rejects through the runtime's shared job
                            // queue, and what it leaves there is this realm's — it
                            // waits for this worker rather than reaching the next
                            // realm to be entered on this runtime.
                            report(&self.events, self.key, "running the worker's script", error);
                        }
                        // Both under the borrow the root module ran under, so
                        // a sibling's bump between this boot and the clock
                        // task's first poll is neither lost nor mistaken for
                        // this worker's own.
                        let checkpoints = js.checkpoints();
                        self.lifetime.record_checkpoint(js.checkpoint_generation());
                        (realm, checkpoints)
                    })
                }
            }
        };
        match opened {
            Ok((realm, checkpoints)) => {
                *self.state.borrow_mut() = WorkerState::Live(realm);
                // The first epilogue, inline: this is already job context and
                // the borrows the stretch above held are released, so what
                // the root module left owing is settled here rather than a
                // queue trip later. A BTS's root has finished, which is what
                // lets `initialize` through; a dedicated worker's waits in the
                // import of its script, which this does not ask the host for.
                let _ = self.enter_now(|_, _| ());
                Some(checkpoints)
            }
            Err(error) => {
                self.failed(error);
                None
            }
        }
    }

    /// This worker's whole tail: wait, end, reclaim, release its realm.
    ///
    /// The wait is the lifetime's — the end, or the next task of this worker
    /// to finish — and [`Self::end`] after it is what mirrors a cancellation
    /// onto this worker's local latch. A task that panicked hands
    /// its payload to [`Self::trapped`], so the creating view hears one
    /// `Failed`, which is what a worker nothing will be heard from again is.
    ///
    /// The release is a job and it is awaited, which is what keeps it from
    /// landing on a realm that is under a live JavaScript stack: a job of this
    /// worker's parked on a synchronous wait is ahead of it in the one FIFO.
    async fn run_owner(self: &Rc<Self>) {
        self.lifetime
            .serve(&mut |payload| self.trapped(payload.as_ref()))
            .await;
        self.end();
        self.lifetime
            .reap(&mut |payload| self.trapped(payload.as_ref()))
            .await;
        run_job(self, |worker| {
            *worker.state.borrow_mut() = WorkerState::Gone;
            Some(())
        })
        .await;
    }

    /// Whether this worker's realm is up, for a test that has to wait for it.
    #[cfg(test)]
    fn is_live(&self) -> bool {
        matches!(&*self.state.borrow(), WorkerState::Live(_))
    }

    /// Whether this worker's root module has finished — for a dedicated
    /// worker, its script included — for a test that has to wait for that
    /// rather than for the realm, which is up before the script has arrived.
    #[cfg(test)]
    fn is_booted(&self) -> bool {
        *self.boot_finished.borrow()
    }

    /// How many times the epilogue has run.
    #[cfg(test)]
    fn epilogue_count(&self) -> u64 {
        self.epilogues.get()
    }
}

/// What this worker's clock task, its unwind guard and its jobs reach it
/// through.
impl Settles for Worker {
    fn lifetime(&self) -> &Lifetime {
        &self.lifetime
    }

    /// The epilogue alone, for a wake that carries no operation of its own —
    /// this realm's own deadline passing.
    fn settle(owner: &Rc<Self>) -> impl Future<Output = Option<()>> {
        owner.enter(|_, _| ())
    }

    fn end(owner: &Rc<Self>) {
        owner.end();
    }

    fn trapped(owner: &Rc<Self>, payload: &(dyn std::any::Any + Send)) {
        owner.trapped(payload);
    }
}

/// One worker's whole life on this thread: build it, start its boot, wait for
/// the end, reclaim. Task lifetime and nothing else.
async fn serve_worker(js: SharedRuntime, start: WorkerStart, thread: JsThreadHandle) {
    let WorkerStart {
        key,
        name,
        role,
        messages,
        events,
        token,
        sources,
    } = start;
    let worker = Worker::new(js, key, &role, events, token, sources, thread);
    worker.spawn(boot_worker(Rc::clone(&worker), name, role, messages));
    worker.run_owner().await;
}

/// The worker's boot future: queue the worker's first job, which opens its
/// realm and evaluates its root module, start its message consumer beside
/// that job, and once the job has run start the clock a live worker has.
///
/// The role decides the root module and what the realm is given: a BTS gets
/// its entry for `bobcat:bts` to import and the screen and module table for
/// `bobcat:bts-runtime` to read, and a dedicated worker's answer goes to
/// [`consume_messages`], which completes the script the root module is
/// waiting in the import of.
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
    role: WorkerRole,
    messages: mpsc::UnboundedReceiver<WorkerMessage>,
) {
    let root = worker_boot_source(&role);
    let (background, script) = match role {
        WorkerRole::Background(background) => (Some(background), None),
        WorkerRole::Dedicated { script, .. } => (None, Some(script)),
    };
    let booted = run_job(&worker, move |worker| worker.boot(name, background, &root));
    worker.spawn(consume_messages(Rc::clone(&worker), messages, script));
    // The boot job runs the first epilogue itself, so the first deadline has
    // already been published by the time this returns — and that epilogue
    // may have ended the worker.
    let Some(checkpoints) = booted.await else {
        return;
    };
    if worker.ended() {
        return;
    }
    worker.spawn(serve_clock(
        Rc::clone(&worker),
        worker.lifetime.deadlines(),
        checkpoints,
    ));
}

/// The painter's message is processed on this worker's event loop, by
/// `bobcat:animation-frame`: the module that filed the callbacks, whether the
/// BTS runtime or the worker's own script imported it.
///
/// Queued rather than awaited, for the reason [`consume_messages`] queues
/// everything: the consumer has to go on reading, so that a `Terminate` behind
/// this reaches [`Worker::end`] even while the job this queued is parked.
fn deliver_vsync(worker: &Rc<Worker>, milliseconds: f64) {
    let reporting = Rc::clone(worker);
    drop(worker.enter(move |realm, js| {
        if let Err(error) = realm.core.engine.call_module_export(
            js,
            crate::esm::ANIMATION_FRAME_MODULE_SPECIFIER,
            "__BobcatBeginFrame",
            &[HostArgument::Number(milliseconds)],
        ) {
            report(
                &reporting.events,
                reporting.key,
                "running animation callbacks",
                error,
            );
        }
    }));
}

/// One native module's answer to one function argument of one call this
/// worker's realm made, handed to it by [`crate::native_module::deliver`].
fn deliver_module_callback(worker: &Rc<Worker>, call: u64, index: u32, arguments: Option<String>) {
    let reporting = Rc::clone(worker);
    drop(worker.enter(move |realm, js| {
        let delivered = crate::native_module::deliver(
            &mut realm.core.engine,
            js,
            call,
            index,
            arguments.as_deref(),
        );
        if let Err(error) = delivered {
            report(
                &reporting.events,
                reporting.key,
                "running a native module callback",
                error,
            );
        }
    }));
}

/// The one ordered consumer of what is posted to this worker, and of a
/// dedicated worker's script.
///
/// **It never waits for a delivery it queued.** `Terminate` is in-band, behind
/// whatever was posted before it, so the consumer has to go on reading: each
/// delivery is one job queued and forgotten, and the queue's FIFO is what keeps
/// them in order. A `Terminate` — or the channel closing — therefore reaches
/// [`Worker::end`] at once, even while an earlier delivery's job is parked on a
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
/// The script's answer is read whenever it arrives, after the root module has
/// finished as well. It is what that module waits for, except where the
/// script URL is an engine name — `new Worker("bobcat:timers")` — which the
/// realm's own loader resolves or refuses without the host, so the module
/// finishes first; the host still answers the request `createWorker` made,
/// and an answer that is not a script ends such a worker with `Failed` too.
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
            answer = async {
                script.as_mut().expect("the arm is enabled only while it is held").await
            }, if script.is_some() => {
                script = None;
                match worker_script(answer) {
                    Ok((source, url)) => complete_script(&worker, url, source),
                    Err(reason) => {
                        worker.failed(platform_script_error(format!(
                            "loading the worker's script: {reason}"
                        )));
                        return;
                    }
                }
            }
            changed = ready.changed(), if held.is_some() => if changed.is_err() { return; },
        }
    }
    worker.end();
}

/// Queues the completion of a dedicated worker's script, from the answer to
/// the request `createWorker` made for it, under the name the root module's
/// `import` asked for. The response URL is the module's URL: its
/// `import.meta.url`, and the base its own imports resolve against.
///
/// The root module's evaluation is read once more in the same job. A script
/// that throws at its top level rejects that evaluation, and the
/// completion's checkpoint already reports the rejection; reading it here
/// clears it, so the epilogue does not report it a second time. Only the
/// first of the two errors is reported.
fn complete_script(worker: &Rc<Worker>, url: String, source: String) {
    let completing = Rc::clone(worker);
    drop(worker.enter(move |realm, js| {
        let entry = completing
            .entry
            .as_deref()
            .expect("only a dedicated worker is answered a script, and it has an entry");
        let completed = realm
            .core
            .engine
            .complete_module(js, entry, Ok((&url, &source)));
        let finished = realm.core.engine.module_finished();
        if let Some(error) = completed.err().or_else(|| finished.err()) {
            report(
                &completing.events,
                completing.key,
                "running the worker's script",
                error,
            );
        }
    }));
}

/// Queues one posted value for this worker's realm.
fn deliver_post(worker: &Rc<Worker>, data: HostValue) {
    let delivering = Rc::clone(worker);
    drop(worker.enter(move |realm, js| {
        deliver(&delivering.events, delivering.key, realm, js, &data);
    }));
}

/// One imported resource, awaited by the realm that requested it. The source
/// response supplies the base URL for its own dependencies, just as on MTS.
async fn load_module(worker: Rc<Worker>, url: String, answer: SourceAnswer) {
    let loaded = worker_script(answer.await);
    let completing = Rc::clone(&worker);
    worker
        .enter(move |realm, js| {
            let result = loaded
                .as_ref()
                .map(|(source, resolved)| (resolved.as_str(), source.as_str()))
                .map_err(String::as_str);
            if let Err(error) = realm.core.engine.complete_module(js, &url, result) {
                report(
                    &completing.events,
                    completing.key,
                    "loading an imported worker module",
                    error,
                );
            }
        })
        .await;
}

/// One future a `.then` asked this realm to settle.
///
/// Its shape is [`load_module`]'s, and so is its error policy: a realm that
/// refuses the delivery is reported and goes on running, like a script that
/// threw or a timer callback that did.
async fn settle_future(worker: Rc<Worker>, id: u32, future: crate::future::HostFuture) {
    let outcome = future.await;
    let settling = Rc::clone(&worker);
    worker
        .enter(move |realm, js| {
            if let Err(error) = crate::future::deliver(&mut realm.core.engine, js, id, outcome) {
                report(
                    &settling.events,
                    settling.key,
                    "settling a worker's future",
                    error,
                );
            }
        })
        .await;
}

/// The script and the URL it is named by, or why there is neither.
fn worker_script(
    answer: Result<
        Result<LoadedSource, crate::LynxViewError>,
        tokio::sync::oneshot::error::RecvError,
    >,
) -> Result<(String, String), String> {
    match answer {
        Ok(Ok(LoadedSource::Module { source, url })) => Ok((source, url)),
        Ok(Ok(LoadedSource::StyleSheet(_))) => {
            Err("the fetcher returned a stylesheet for a worker".to_owned())
        }
        Ok(Ok(LoadedSource::Font(_))) => Err("the fetcher returned a font for a worker".to_owned()),
        Ok(Ok(LoadedSource::Fetched)) => {
            Err("the fetcher returned a plain fetch for a worker".to_owned())
        }
        Ok(Err(error)) => Err(error.to_string()),
        Err(_) => Err("the fetcher dropped the request".to_owned()),
    }
}

/// Hands one message value to a realm that is up.
fn deliver(
    events: &mpsc::UnboundedSender<WorkerEvent>,
    key: WorkerKey,
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
    let delivered = realm.core.engine.call_module_export(
        js_runtime,
        WORKER_MODULE_SPECIFIER,
        WORKER_DELIVER_EXPORT,
        &[data.as_argument()],
    );
    if let Err(error) = delivered {
        report(events, key, "delivering a message to a worker", error);
    }
}

/// Runs every timer this realm armed that has come due.
fn fire_timers(
    events: &mpsc::UnboundedSender<WorkerEvent>,
    key: WorkerKey,
    realm: &mut WorkerRealm,
    js_runtime: &mut ScriptRuntime,
) {
    // A `close()` discards this worker's armed timers along with its queued
    // messages; the realm itself goes with the epilogue that reports it.
    if realm.closing.get() {
        return;
    }
    let Some(failures) = run_due_timers(&mut realm.core.engine, js_runtime, &realm.core.timers)
    else {
        return;
    };
    for error in failures {
        report(events, key, "running a worker's timer callback", error);
    }
}

/// Reports what a worker's realm threw, without ending it.
///
/// The one error policy for everything a realm does — loading its script,
/// taking a message, running a timer. HTML reports an uncaught exception at
/// the worker and then at its parent and leaves both running, which is exactly
/// what `Errored` means and `Failed` does not.
fn report(
    events: &mpsc::UnboundedSender<WorkerEvent>,
    key: WorkerKey,
    context: &str,
    error: ScriptError,
) {
    let _ = events.send(WorkerEvent {
        key,
        payload: WorkerPayload::Errored(context_of(context, error)),
    });
}

#[cfg(test)]
mod tests {
    //! Two worker realms on one runtime, driven the way [`serve_worker`]
    //! drives one — except that the test keeps both [`Worker`]s, so it can
    //! count the epilogues each of them ran.

    use tokio::sync::oneshot;

    use super::*;

    /// How many times the test lets every ready task run before it gives up on
    /// something happening. A hang detector rather than a schedule: everything
    /// here is on one thread and cooperative.
    const TURNS: usize = 512;

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
    /// The script does nothing, because what most of these pins are about is
    /// which task ran rather than what the script said.
    fn start(js: &SharedRuntime, thread: &JsThreadHandle, key: u64) -> Started {
        start_running(js, thread, key, String::new())
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
        let role = WorkerRole::Dedicated {
            url,
            script: script_rx,
        };
        // A token of its own rather than a child of anything: no view created
        // this worker, and nothing here releases one. The outbox carries that
        // same token, the way `WorkerOwner::start` hands it over, so what this
        // worker asks the host for ends when this worker does.
        let token = CancellationToken::new();
        let (sources, sources_rx) = mpsc::unbounded_channel();
        let worker = Worker::new(
            Rc::clone(js),
            WorkerKey::new(key),
            &role,
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
            role,
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
    /// The task that ends runs no JavaScript past its root module: its realm
    /// opens and evaluates that module as its `Start` is served, which
    /// settles the parked worker as well, and the count is taken only once
    /// that has stopped moving. Its script never arrives, and a `Terminate`
    /// ends it while its root module waits in the import. So the one settle
    /// counted is the ending's and nothing else's.
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

            // Parked: whatever the `Start` set off — the second realm's root
            // module ran a checkpoint over the queue both realms share — has
            // settled, and a settle that finds nothing due enters no
            // JavaScript, so the count stops moving.
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

    /// A `Start` for the dedicated worker `key` over `app:///worker<key>.js`,
    /// whose script the test answers through the sending end of `script`, or
    /// holds unanswered.
    fn dedicated_start(
        key: u64,
        script: SourceAnswer,
        messages: mpsc::UnboundedReceiver<WorkerMessage>,
        events: mpsc::UnboundedSender<WorkerEvent>,
        notices: mpsc::UnboundedSender<crate::link::ViewNotice>,
    ) -> WorkerStart {
        let token = CancellationToken::new();
        WorkerStart {
            key: WorkerKey::new(key),
            name: String::new(),
            role: WorkerRole::Dedicated {
                url: format!("app:///worker{key}.js"),
                script,
            },
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
            // own, which would take the place of a root module whose import
            // of the script is still outstanding.
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
    /// What the pin asserts is the pair: the terminate reaches [`Worker::end`]
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
                r"
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
}
