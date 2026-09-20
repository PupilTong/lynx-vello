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
//! [`boot_worker`] waits for the script; [`consume_messages`] is the one
//! ordered consumer of what is posted; [`serve_clock`] owns this realm's one
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
//! - each [`boot_worker`]'s pre-boot wait, on the script versus termination, channel closure or its
//!   own cancellation — one task's three-source wait rather than a scheduler;
//! - one [`serve_clock`] per live worker realm, waiting on its deadline, the re-arm that moves it,
//!   and a sibling's checkpoint.

use std::cell::{Cell, RefCell};
use std::future::Future;
use std::rc::Rc;

use quickjs_rust_bridge::{HostArgument, HostValue};
use rustc_hash::FxHashMap;
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::{self, JoinError, JoinSet};
use tokio_util::sync::CancellationToken;

use super::scope::{
    WORKER_DELIVER_EXPORT, WORKER_MODULE_CALLBACK_EXPORT, WORKER_MODULE_SPECIFIER,
    install_worker_members, install_worker_modules, worker_boot_source,
};
use super::{WorkerCommand, WorkerEvent, WorkerKey, WorkerMessage, WorkerPayload, WorkerStart};
use crate::jobs::{JsThread, JsThreadHandle};
use crate::lifetime::{EndOnUnwind, Lifetime, Settles, run_job, serve_clock};
use crate::link::{HostOutbox, SourceAnswer};
use crate::main::quickjs::{ScriptEngine, ScriptRuntime};
use crate::resource::{LoadedSource, SourceRequest};
use crate::script::ScriptError;
use crate::threads::{panicked, platform_script_error};
use crate::timers::{TimerState, run_due_timers};

/// The runtime every worker realm on this thread is opened on.
///
/// A runtime that could not be built is not fatal to the group: it is the
/// failure of every worker that would have run on it, and each hears about it
/// when it is asked for.
type WorkerRuntime = Rc<RefCell<Result<ScriptRuntime, ScriptError>>>;

/// The thread's whole body.
pub(super) fn run(commands: mpsc::UnboundedReceiver<WorkerCommand>) {
    let runtime = ScriptRuntime::new()
        .and_then(|mut runtime| install_worker_modules(&mut runtime).map(|()| runtime));
    serve(Rc::new(RefCell::new(runtime)), commands);
}

/// Preloads a fixture through the existing runtime API for Context tests.
#[cfg(test)]
pub(super) fn run_with_entry(
    commands: mpsc::UnboundedReceiver<WorkerCommand>,
    entry: (String, String),
) {
    let (source, url) = entry;
    let mut runtime = ScriptRuntime::new().unwrap();
    install_worker_modules(&mut runtime).unwrap();
    let source = format!("{}{source}", crate::esm::BTS_ENTRY_PREAMBLE);
    runtime.register_module_source(&url, &source).unwrap();
    serve(Rc::new(RefCell::new(Ok(runtime))), commands);
}

fn serve(js: WorkerRuntime, commands: mpsc::UnboundedReceiver<WorkerCommand>) {
    let thread = JsThread::new();
    thread.run(serve_workers(js, commands, thread.handle()));
    drop(thread);
}

/// Starts a task per worker and reports the ones that trapped.
async fn serve_workers(
    js: WorkerRuntime,
    mut commands: mpsc::UnboundedReceiver<WorkerCommand>,
    thread: JsThreadHandle,
) {
    let mut workers = JoinSet::new();
    let mut reporters: FxHashMap<task::Id, (WorkerKey, mpsc::UnboundedSender<WorkerEvent>)> =
        FxHashMap::default();
    loop {
        tokio::select! {
            command = commands.recv() => match command {
                Some(WorkerCommand::Start(start)) => {
                    let reporter = (start.key, start.events.clone());
                    let handle = workers.spawn_local(
                        serve_worker(Rc::clone(&js), start, thread.clone()),
                    );
                    reporters.insert(handle.id(), reporter);
                }
                // Every realm that could name a worker is gone, and with it
                // every message sender, so the tasks below are ending too.
                None => break,
            },
            Some(finished) = workers.join_next_with_id(), if !workers.is_empty() => {
                report_trap(finished, &mut reporters);
            }
        }
    }
    while let Some(finished) = workers.join_next_with_id().await {
        report_trap(finished, &mut reporters);
    }
}

/// A worker task that panicked is a worker nothing will ever be heard from
/// again, which is exactly what `Failed` means.
fn report_trap(
    finished: Result<(task::Id, ()), JoinError>,
    reporters: &mut FxHashMap<task::Id, (WorkerKey, mpsc::UnboundedSender<WorkerEvent>)>,
) {
    let error = match finished {
        Ok((id, ())) => {
            reporters.remove(&id);
            return;
        }
        Err(error) => error,
    };
    let Some((key, events)) = reporters.remove(&error.id()) else {
        return;
    };
    if !error.is_panic() {
        return;
    }
    let error = panicked("the worker thread panicked", error.into_panic().as_ref());
    let _ = events.send(WorkerEvent {
        key,
        payload: WorkerPayload::Failed(error),
    });
}

/// One live worker on this thread: its realm and everything that realm owns.
struct WorkerRealm {
    engine: ScriptEngine,
    timers: Rc<TimerState>,
    /// Set by the native `closeWorker` export. A flag rather than a direct
    /// teardown because it is written from inside the realm it would tear
    /// down: the task reads it once the call that set it has returned.
    closing: Rc<Cell<bool>>,
}

/// What a worker is, which is what decides whether a message has anywhere to
/// go.
enum WorkerState {
    /// The script has not arrived, or has not been evaluated yet.
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
    js: WorkerRuntime,
    key: WorkerKey,
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
        js: WorkerRuntime,
        key: WorkerKey,
        events: mpsc::UnboundedSender<WorkerEvent>,
        token: CancellationToken,
        sources: HostOutbox,
        thread: JsThreadHandle,
    ) -> Rc<Self> {
        Rc::new(Self {
            js,
            key,
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
    /// reported a failed script and then traps is still a `Failed` the
    /// creating view is owed.
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
    /// would otherwise take the thread down instead of this one worker.
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
    /// completion, module requests, the next timer deadline, and
    /// the checkpoint generation as of this entry.
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
            let finished = match realm.engine.module_finished() {
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
        while let Some(url) = realm.engine.take_module_request() {
            let answer = self.sources.request(SourceRequest::Module(url.clone()));
            self.spawn(load_module(Rc::clone(self), url, answer));
        }
        self.lifetime.arm_deadline(realm.timers.next_deadline());
        // Last, so it names the generation this entry ran up rather than the
        // one it started from.
        self.lifetime.record_checkpoint(js.checkpoint_generation());
    }

    /// Builds this worker's realm and runs its script in it, and answers with
    /// the checkpoint receiver its clock task will watch. `None` is what leaves
    /// no worker at all — a runtime that never came up, a realm that could not
    /// be created or furnished, or a worker that ended before it booted.
    ///
    /// The script's *own* outcome is not among them: by the time it runs the
    /// realm is built, so a script that throws on load is reported exactly
    /// like a timer callback that throws later, and leaves a worker that is
    /// up and listening. That is what HTML's "run a worker" does — it reports
    /// the exception and goes on to enable the port queue and run the event
    /// loop — and it matters because a script registers its handlers before
    /// whatever optional work fails.
    ///
    /// A job like every other entry, and the one that does not go through
    /// [`Self::enter`], because the realm it would enter does not exist until
    /// it returns. [`boot_worker`] is what queues it.
    fn boot(self: &Rc<Self>, name: &str, script: (String, String)) -> Option<watch::Receiver<u64>> {
        // Again right before the script is evaluated: a `Terminate` that
        // landed while it was being read still wins.
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
                    open_realm(js, self.events.clone(), self.key, &self.sources).map(|mut realm| {
                        let (source, url) = script;
                        let source = worker_boot_source(name, &source);
                        if let Err(error) = realm.engine.start_module(js, &source, &url) {
                            // Nothing to clean up after: a throw at this module's
                            // top level rejects through the runtime's shared job
                            // queue, and what it leaves there is this realm's — it
                            // waits for this worker rather than reaching the next
                            // realm to be entered on this runtime.
                            report(&self.events, self.key, "running the worker's script", error);
                        }
                        // Both under the borrow the script ran under, so a
                        // sibling's bump between this boot and the clock task's
                        // first poll is neither lost nor mistaken for this
                        // worker's own.
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
                // the borrows the stretch above held are released, so a
                // load-time `close()` is reported here and the first deadline
                // is published here — for a script that armed a timer and was
                // posted nothing — rather than a queue trip later.
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
async fn serve_worker(js: WorkerRuntime, start: WorkerStart, thread: JsThreadHandle) {
    let WorkerStart {
        key,
        name,
        script,
        messages,
        events,
        token,
        sources,
    } = start;
    let worker = Worker::new(js, key, events, token, sources, thread);
    worker.spawn(boot_worker(Rc::clone(&worker), name, script, messages));
    worker.run_owner().await;
}

/// The worker's boot future: wait for the script, evaluate it, then start the
/// waits a live worker has.
///
/// The `select!` below is one task's three-source wait rather than a
/// dispatcher, and it is `biased` for the reason HTML's "terminate a worker"
/// aborts the fetch: a `Terminate` that lands in the same instant as the
/// script must win, so a worker told to stop before it booted never boots.
/// The message arm stays first for that reason; the token arm observes this
/// worker's own cancellation scope.
/// Returning here drops the script's receiving end, which is what cancels
/// that fetch.
///
/// HTML queues what is posted before a worker's global scope exists and
/// delivers it once the scope is up, which is what the queue is: without it
/// the commonest shape there is — construct, then post — would lose its first
/// message.
async fn boot_worker(
    worker: Rc<Worker>,
    name: String,
    mut script: oneshot::Receiver<Result<LoadedSource, crate::LynxViewError>>,
    mut messages: mpsc::UnboundedReceiver<WorkerMessage>,
) {
    let mut queued: Vec<HostValue> = Vec::new();
    let loaded = loop {
        tokio::select! {
            biased;
            message = messages.recv() => match message {
                // The MTS handle ended, or its realm released the sender.
                None | Some(WorkerMessage::Terminate) => {
                    worker.end();
                    return;
                }
                Some(WorkerMessage::Post(data)) => queued.push(data),
                // Neither can reach a realm that does not exist yet: nothing
                // has called a module, and the painter's frame is the same
                // kind of nothing to answer.
                Some(WorkerMessage::Vsync(_) | WorkerMessage::ModuleCallback { .. }) => {},
            },
            // A worker that ended before its boot task was polled must not
            // evaluate the arriving source.
            () = worker.lifetime.token().cancelled() => {
                worker.end();
                return;
            }
            answer = &mut script => break answer,
        }
    };
    if worker.ended() {
        return;
    }
    let source = match worker_script(loaded) {
        Ok(source) => source,
        Err(reason) => {
            worker.failed(platform_script_error(format!(
                "loading the worker's script: {reason}"
            )));
            return;
        }
    };
    // The boot job runs the first epilogue itself, so a load-time `close()`
    // has already been reported and the first deadline already published by
    // the time this returns — and either of those, or a `Terminate` a task
    // read while the job held the thread, may have ended the worker.
    let Some(checkpoints) = run_job(&worker, move |worker| worker.boot(&name, source)).await else {
        return;
    };
    if worker.ended() {
        return;
    }
    worker.spawn(consume_messages(Rc::clone(&worker), messages, queued));
    worker.spawn(serve_clock(
        Rc::clone(&worker),
        worker.lifetime.deadlines(),
        checkpoints,
    ));
}

/// The painter's message is processed on this worker's event loop.
///
/// Queued rather than awaited, for the reason [`consume_messages`] queues
/// everything: the consumer has to go on reading, so that a `Terminate` behind
/// this reaches [`Worker::end`] even while the job this queued is parked.
fn deliver_vsync(worker: &Rc<Worker>, milliseconds: f64) {
    let reporting = Rc::clone(worker);
    drop(worker.enter(move |realm, js| {
        if let Err(error) = realm.engine.call_module_export(
            js,
            crate::esm::BTS_RUNTIME_MODULE_SPECIFIER,
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

/// One native module's answer to one function argument of one call.
///
/// `arguments` is the JSON array text the realm spreads; `None` releases the
/// function without calling it, which is what a module that dropped its
/// callback owes. A callback for a call the realm has forgotten is a no-op
/// over there, so nothing here has to know which calls are outstanding.
fn deliver_module_callback(worker: &Rc<Worker>, call: u64, index: u32, arguments: Option<String>) {
    let reporting = Rc::clone(worker);
    drop(worker.enter(move |realm, js| {
        #[allow(
            clippy::cast_precision_loss,
            reason = "the realm mints these counting up from one"
        )]
        let delivered = realm.engine.call_module_export(
            js,
            WORKER_MODULE_SPECIFIER,
            WORKER_MODULE_CALLBACK_EXPORT,
            &[
                HostArgument::Number(call as f64),
                HostArgument::Number(f64::from(index)),
                arguments
                    .as_deref()
                    .map_or(HostArgument::Undefined, HostArgument::String),
            ],
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

/// The one ordered consumer of what is posted to this worker.
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
async fn consume_messages(
    worker: Rc<Worker>,
    mut messages: mpsc::UnboundedReceiver<WorkerMessage>,
    mut queued: Vec<HostValue>,
) {
    let mut ready = worker.boot_finished.subscribe();
    while !*ready.borrow_and_update() {
        tokio::select! {
            biased;
            message = messages.recv() => match message {
                None | Some(WorkerMessage::Terminate) => {
                    worker.end();
                    return;
                }
                Some(WorkerMessage::Post(data)) => queued.push(data),
                Some(WorkerMessage::Vsync(milliseconds)) => deliver_vsync(&worker, milliseconds),
                // Immediately, like a frame and unlike a post: the entry that
                // has not finished importing may itself be awaiting this
                // answer, so queuing it behind boot would deadlock the call.
                Some(WorkerMessage::ModuleCallback { call, index, arguments }) => {
                    deliver_module_callback(&worker, call, index, arguments);
                }
            },
            changed = ready.changed() => if changed.is_err() { return; },
        }
    }
    for data in queued {
        deliver_post(&worker, data);
    }
    while let Some(message) = messages.recv().await {
        match message {
            // Explicit termination or collection of the MTS handle.
            WorkerMessage::Terminate => break,
            WorkerMessage::Vsync(milliseconds) => deliver_vsync(&worker, milliseconds),
            WorkerMessage::ModuleCallback {
                call,
                index,
                arguments,
            } => deliver_module_callback(&worker, call, index, arguments),
            WorkerMessage::Post(data) => deliver_post(&worker, data),
        }
    }
    worker.end();
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
            if let Err(error) = realm.engine.complete_module(js, &url, result) {
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

/// The script and the URL it is named by, or why there is neither.
fn worker_script(
    answer: Result<
        Result<LoadedSource, crate::LynxViewError>,
        tokio::sync::oneshot::error::RecvError,
    >,
) -> Result<(String, String), String> {
    match answer {
        Ok(Ok(LoadedSource::Entry { source, url })) => Ok((source, url)),
        Ok(Ok(LoadedSource::StyleSheet(_))) => {
            Err("the fetcher returned a stylesheet for a worker".to_owned())
        }
        Ok(Ok(LoadedSource::Font(_))) => Err("the fetcher returned a font for a worker".to_owned()),
        Ok(Err(error)) => Err(error.to_string()),
        Err(_) => Err("the fetcher dropped the request".to_owned()),
    }
}

fn open_realm(
    js_runtime: &mut ScriptRuntime,
    events: mpsc::UnboundedSender<WorkerEvent>,
    key: WorkerKey,
    host: &HostOutbox,
) -> Result<WorkerRealm, ScriptError> {
    let mut engine = js_runtime
        .create_realm()
        .map_err(|error| context_of("creating the worker realm", error))?;
    engine.enable_module_loading();
    let frames = host.clone();
    crate::script_frames::install(&mut engine, js_runtime, move |pending| {
        frames.notify(crate::link::ViewNotice::ScriptFrameDemand {
            worker: Some(key),
            pending,
        });
    })?;
    let timers = Rc::new(TimerState::new());
    let closing = Rc::new(Cell::new(false));
    install_worker_members(
        &mut engine,
        js_runtime,
        &timers,
        &closing,
        key,
        host,
        move |data| {
            let _ = events.send(WorkerEvent {
                key,
                payload: WorkerPayload::Message(data),
            });
        },
    )?;
    Ok(WorkerRealm {
        engine,
        timers,
        closing,
    })
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
    let delivered = realm.engine.call_module_export(
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
    let Some(failures) = run_due_timers(&mut realm.engine, js_runtime, &realm.timers) else {
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

/// Prefixes a failure with what the host was doing, the way `MainThreadError`
/// does for the errors that reach an embedder through a view.
fn context_of(context: &str, mut error: ScriptError) -> ScriptError {
    error.message = std::sync::Arc::from(format!("{context}: {}", error.message));
    error
}

#[cfg(test)]
mod tests {
    //! Two worker realms on one runtime, driven the way [`serve_worker`]
    //! drives one — except that the test keeps both [`Worker`]s, so it can
    //! count the epilogues each of them ran.

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
    }

    /// Starts one worker on `js`, with its script already answered.
    ///
    /// The script does nothing, because what most of these pins are about is
    /// which task ran rather than what the script said.
    fn start(js: &WorkerRuntime, thread: &JsThreadHandle, key: u64) -> Started {
        start_running(js, thread, key, String::new())
    }

    /// The same, over a script of the test's own.
    fn start_running(
        js: &WorkerRuntime,
        thread: &JsThreadHandle,
        key: u64,
        source: String,
    ) -> Started {
        let (events, events_rx) = mpsc::unbounded_channel();
        let (messages, messages_rx) = mpsc::unbounded_channel();
        let (script, script_rx) = oneshot::channel();
        let _ = script.send(Ok(LoadedSource::Entry {
            source,
            url: format!("app:///worker{key}.js"),
        }));
        // A token of its own rather than a child of anything: no view created
        // this worker, and nothing here releases one.
        let worker = Worker::new(
            Rc::clone(js),
            WorkerKey::new(key),
            events,
            CancellationToken::new(),
            HostOutbox::new(
                mpsc::unbounded_channel().0,
                std::sync::Arc::new(crate::NoWakeup),
                CancellationToken::new(),
            ),
            thread.clone(),
        );
        worker.spawn(boot_worker(
            Rc::clone(&worker),
            String::new(),
            script_rx,
            messages_rx,
        ));
        Started {
            worker,
            messages,
            events: events_rx,
        }
    }

    /// One worker runtime, furnished the way [`run`] furnishes this thread's.
    fn worker_runtime() -> WorkerRuntime {
        let mut js = ScriptRuntime::new().expect("a QuickJS runtime");
        install_worker_modules(&mut js).expect("the worker modules register");
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
            for _ in 0..TURNS {
                if first.worker.is_live() && second.worker.is_live() {
                    break;
                }
                task::yield_now().await;
            }
            assert!(
                first.worker.is_live() && second.worker.is_live(),
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
    impl Started {
        /// Runs one entry's body here and now, which is what the engine
        /// thread's top loop does with a queued job: these two pins are about
        /// what `QuickJS` finalization does across two realms, so standing in
        /// for the loop is simpler than queueing and draining.
        fn execute(&self, source: &str) {
            self.worker
                .enter_now(|realm, js| {
                    realm
                        .engine
                        .start_module(js, source, "app:///observer-test.js")
                        .unwrap();
                    assert!(realm.engine.module_finished().unwrap());
                })
                .expect("the worker is live");
        }

        fn collect(&self) {
            self.worker
                .enter_now(|realm, js| realm.engine.collect_garbage(js).unwrap())
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
            for _ in 0..TURNS {
                if first.worker.is_live() && second.worker.is_live() {
                    break;
                }
                task::yield_now().await;
            }
            assert!(first.worker.is_live() && second.worker.is_live());
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

    /// A `Terminate` is in band behind whatever was posted before it, so the
    /// consumer cannot wait for the deliveries it queued: if it did, a worker
    /// whose job is parked on a synchronous wait could never be told to stop.
    ///
    /// The job the test queues below stands in for a synchronous host member
    /// this thread does not have yet — its wait is the shape a `require`'s will
    /// be, this worker's own token against an answer that never comes. What the
    /// pin asserts is the pair: the terminate reaches [`Worker::end`] while the
    /// job is parked, which is what ends the wait; and the post queued between
    /// the two is discarded rather than delivered, because its job finds the
    /// worker ended.
    #[test]
    fn an_in_band_terminate_ends_a_worker_whose_job_is_waiting_and_discards_what_is_behind_it() {
        on_a_js_thread(|thread| async move {
            let js = worker_runtime();
            // The script echoes, so a post that was delivered would be heard.
            let started = start_running(
                &js,
                &thread,
                1,
                "onmessage = event => postMessage(event.data);".to_owned(),
            );
            for _ in 0..TURNS {
                if started.worker.is_live() {
                    break;
                }
                task::yield_now().await;
            }
            assert!(started.worker.is_live(), "the worker booted");

            // Queued here rather than sent as a message: the job is pushed by
            // the `enter` call itself, and nothing waits for its answer.
            let (waited, answers) = std::sync::mpsc::channel();
            let waiting = Rc::clone(&started.worker);
            drop(started.worker.enter(move |_, _| {
                let _ = waited.send(false);
                let token = waiting.lifetime.token().clone();
                let ended = waiting.lifetime.thread().wait(async move {
                    tokio::select! {
                        biased;
                        () = token.cancelled() => true,
                        () = std::future::pending() => false,
                    }
                });
                let _ = waited.send(ended);
            }));
            // This test's own body is a task, so it goes on running inside the
            // job's wait — which is the property the whole model rests on.
            for _ in 0..TURNS {
                if answers.try_recv() == Ok(false) {
                    break;
                }
                task::yield_now().await;
            }

            // Both sent while that job is still parked: the consumer reads
            // them anyway, which is what this pins.
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
            for _ in 0..TURNS {
                if started.worker.ended() {
                    break;
                }
                task::yield_now().await;
            }
            assert_eq!(
                answers.try_recv(),
                Ok(true),
                "the terminate ended the worker, and the parked wait heard it"
            );

            // And the post between the two never reaches the realm: its job
            // was queued behind the waiting one and finds a worker that ended.
            for _ in 0..TURNS {
                task::yield_now().await;
            }
            let mut events = started.events;
            while let Ok(event) = events.try_recv() {
                assert!(
                    !matches!(event.payload, WorkerPayload::Message(_)),
                    "a post queued behind a terminate is discarded, not delivered"
                );
            }
        });
    }
}
