//! `bobcat-workers`: the thread the group's worker realms live on.
//!
//! It owns one `QuickJS` runtime, a task set per live worker, and nothing
//! else at all — no document, no style pool, no fetcher.
//!
//! A worker takes the shape a view has on `bobcat-main`, for the same reason:
//! each thing it can wait for is a task of its own, and tokio is what polls,
//! parks and wakes them. An owner ([`serve_worker`]) waits only for the end;
//! [`boot_worker`] waits for the script; [`consume_messages`] is the one
//! ordered consumer of what is posted; [`wait_timers`] owns this realm's one
//! pinned sleep; [`follow_checkpoints`] watches the runtime-wide checkpoint
//! generation, because the job queue every worker realm here drains is the
//! runtime's and a sibling's entry can finish this realm's jobs. Every one of
//! them reaches the realm through [`Worker::enter`], which runs one
//! synchronous operation and then settles what it left owing: the timers that
//! came due, a `close()` it may have called, the next deadline, and the
//! checkpoint generation as of that entry.
//!
//! The `select!`s on this thread are of three kinds, and none of them
//! dispatches anything. [`serve_workers`] is task lifetime: attach versus
//! join. Each [`boot_worker`] has a pre-boot wait of its own, on the script
//! versus a `Terminate` that must win, which is one task's two-source wait
//! rather than a scheduler. Each live worker realm has one [`wait_timers`],
//! waiting on its deadline versus the re-arm that moves it.

use std::cell::{Cell, RefCell};
use std::future::Future;
use std::rc::Rc;

use quickjs_rust_bridge::HostArgument;
use rustc_hash::FxHashMap;
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::{self, JoinError, JoinSet, LocalSet};

use super::scope::{
    WORKER_DELIVER_EXPORT, WORKER_MODULE_SPECIFIER, install_worker_members, install_worker_modules,
    worker_boot_source,
};
use super::{WorkerCommand, WorkerEvent, WorkerKey, WorkerMessage, WorkerPayload, WorkerStart};
use crate::clock::ClockInstant;
use crate::main::quickjs::{ScriptEngine, ScriptRuntime};
use crate::resource::LoadedSource;
use crate::script::ScriptError;
use crate::threads::{panic_message, platform_script_error};
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
    let mut builder = tokio::runtime::Builder::new_current_thread();
    // A worker realm waits out its own timers, so natively this thread runs
    // tokio's; on wasm32 `crate::clock::sleep_until` serves the same waits
    // without one.
    #[cfg(not(target_arch = "wasm32"))]
    builder.enable_time();
    let runtime = builder
        .build()
        .expect("a current-thread runtime asks the platform for nothing");
    let local = LocalSet::new();
    local.block_on(&runtime, serve_workers(js, commands));
}

/// Starts a task per worker and reports the ones that trapped.
async fn serve_workers(js: WorkerRuntime, mut commands: mpsc::UnboundedReceiver<WorkerCommand>) {
    let mut workers = JoinSet::new();
    let mut reporters: FxHashMap<task::Id, (WorkerKey, mpsc::UnboundedSender<WorkerEvent>)> =
        FxHashMap::default();
    loop {
        tokio::select! {
            command = commands.recv() => match command {
                Some(WorkerCommand::Start(start)) => {
                    let reporter = (start.key, start.events.clone());
                    let handle = workers.spawn_local(serve_worker(Rc::clone(&js), start));
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
    let error = platform_script_error(format!(
        "the worker thread panicked: {}",
        panic_message(error.into_panic().as_ref())
    ));
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

/// Why a worker ended. The first one wins.
enum WorkerEnd {
    /// `Worker.terminate()`, or the channel it would have arrived on closing
    /// with the realm that created this worker.
    Terminated,
    /// The script called `close()`.
    Closed,
    /// A failure this worker has already been reported.
    Failed,
    /// A task of this worker panicked, and nothing has been reported yet.
    ///
    /// A `Drop` is handed no payload, so the report is the owner's, out of the
    /// handle it awaits — and that handle can be gone by then, because
    /// [`Worker::spawn`] prunes the handles of tasks that have finished. This
    /// reason is what is left of the panic when it is.
    Trapped,
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
    /// This realm's earliest armed timer, for [`wait_timers`].
    deadline: watch::Sender<Option<ClockInstant>>,
    /// The runtime-wide checkpoint generation as of this worker's own last
    /// entry, which is what lets [`follow_checkpoints`] ignore its own bumps.
    own_checkpoint: Cell<u64>,
    tasks: RefCell<Vec<task::JoinHandle<()>>>,
    end: mpsc::UnboundedSender<WorkerEnd>,
    ended: Cell<bool>,
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
    ) -> (Rc<Self>, mpsc::UnboundedReceiver<WorkerEnd>) {
        let (end, ended) = mpsc::unbounded_channel();
        let worker = Self {
            js,
            key,
            events,
            state: RefCell::new(WorkerState::Loading),
            deadline: watch::channel(None).0,
            own_checkpoint: Cell::new(0),
            tasks: RefCell::new(Vec::new()),
            end,
            ended: Cell::new(false),
            #[cfg(test)]
            epilogues: Cell::new(0),
        };
        (Rc::new(worker), ended)
    }

    /// Starts one more task of this worker. A panic anywhere in it ends the
    /// worker, and the owner turns the handle's report into a `Failed`.
    fn spawn(self: &Rc<Self>, future: impl Future<Output = ()> + 'static) {
        let worker = Rc::clone(self);
        let handle = task::spawn_local(async move {
            let _guard = EndOnUnwind(worker);
            future.await;
        });
        let mut tasks = self.tasks.borrow_mut();
        tasks.retain(|task| !task.is_finished());
        tasks.push(handle);
    }

    /// Ends this worker, once. `true` for the call that did it.
    fn end(&self, reason: WorkerEnd) -> bool {
        if self.ended.replace(true) {
            return false;
        }
        self.deadline
            .send_if_modified(|deadline| deadline.take().is_some());
        let _ = self.end.send(reason);
        true
    }

    fn ended(&self) -> bool {
        self.ended.get()
    }

    /// The worker is over and nothing of it will ever run: its script never
    /// arrived, or its realm could not be built.
    fn failed(&self, error: ScriptError) {
        if self.end(WorkerEnd::Failed) {
            let _ = self.events.send(WorkerEvent {
                key: self.key,
                payload: WorkerPayload::Failed(error),
            });
        }
    }

    /// Runs one synchronous operation against this worker's realm and settles
    /// what it owes. `None` is a realm that is not live, or a worker that has
    /// ended.
    fn enter<T>(
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

    /// The epilogue alone, for a wake that carries no operation of its own —
    /// this realm's own deadline passing.
    fn settle(self: &Rc<Self>) {
        self.enter(|_, _| ());
    }

    /// Everything one entry into this realm leaves owing: the timers that
    /// have come due, a `close()` whatever just ran may have called, the next
    /// deadline this task waits out, and the checkpoint generation as of this
    /// entry.
    fn epilogue(&self, realm: &mut WorkerRealm, js: &mut ScriptRuntime) {
        if self.ended() {
            return;
        }
        #[cfg(test)]
        self.epilogues.set(self.epilogues.get() + 1);
        fire_timers(&self.events, self.key, realm, js);
        // A `close()` from the script that just ran ends the worker rather
        // than parking it again, and discards what it had armed and queued.
        if realm.closing.get() {
            if self.end(WorkerEnd::Closed) {
                let _ = self.events.send(WorkerEvent {
                    key: self.key,
                    payload: WorkerPayload::Closed,
                });
            }
            return;
        }
        let deadline = realm.timers.next_deadline();
        self.deadline.send_if_modified(|armed| {
            if *armed == deadline {
                return false;
            }
            *armed = deadline;
            true
        });
        // Last, so it names the generation this entry ran up rather than the
        // one it started from.
        self.own_checkpoint.set(js.checkpoint_generation());
    }

    /// Builds this worker's realm and runs its script in it, and answers with
    /// the checkpoint receiver its follower will watch. `None` is what leaves
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
                Ok(js) => open_realm(js, self.events.clone(), self.key).map(|mut realm| {
                    let (source, url) = script;
                    let source = worker_boot_source(name, &source);
                    if let Err(error) = realm.engine.execute_module(js, &source, &url) {
                        // Nothing to clean up after: a throw at this module's
                        // top level rejects through the runtime's shared job
                        // queue, and what it leaves there is this realm's — it
                        // waits for this worker rather than reaching the next
                        // realm to be entered on this runtime.
                        report(&self.events, self.key, "running the worker's script", error);
                    }
                    // Both under the borrow the script ran under, so a
                    // sibling's bump between this boot and the follower's
                    // first poll is neither lost nor mistaken for this
                    // worker's own.
                    let checkpoints = js.checkpoints();
                    self.own_checkpoint.set(js.checkpoint_generation());
                    (realm, checkpoints)
                }),
            }
        };
        match opened {
            Ok((realm, checkpoints)) => {
                *self.state.borrow_mut() = WorkerState::Live(realm);
                Some(checkpoints)
            }
            Err(error) => {
                self.failed(error);
                None
            }
        }
    }

    /// Reclaims every task of this worker, reports a panic once, then releases
    /// its realm.
    ///
    /// As on `bobcat-main`, the report does not depend on a handle still being
    /// here: a handle that panicked carries the payload, and a handle
    /// [`Self::spawn`] pruned once it had finished leaves only
    /// [`WorkerEnd::Trapped`] behind. Either way the creating view hears one
    /// `Failed`, which is what a worker nothing will be heard from again is.
    async fn reap(&self, reason: WorkerEnd) {
        let tasks = std::mem::take(&mut *self.tasks.borrow_mut());
        for task in &tasks {
            task.abort();
        }
        let mut trapped: Option<String> = None;
        for task in tasks {
            let Err(error) = task.await else { continue };
            if error.is_panic() && trapped.is_none() {
                trapped = Some(format!(
                    "the worker thread panicked: {}",
                    panic_message(error.into_panic().as_ref())
                ));
            }
        }
        let message = trapped.or_else(|| {
            matches!(reason, WorkerEnd::Trapped).then(|| "the worker thread panicked".to_owned())
        });
        if let Some(message) = message {
            let _ = self.events.send(WorkerEvent {
                key: self.key,
                payload: WorkerPayload::Failed(platform_script_error(message)),
            });
        }
        *self.state.borrow_mut() = WorkerState::Gone;
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

/// Ends the worker if the task it guards is unwinding.
struct EndOnUnwind(Rc<Worker>);

impl Drop for EndOnUnwind {
    fn drop(&mut self) {
        if std::thread::panicking() {
            // The payload is not reachable from here, so the reason is all
            // this can say; the owner turns it into the report.
            self.0.end(WorkerEnd::Trapped);
        }
    }
}

/// One worker's whole life on this thread: build it, start its boot, wait for
/// the end, reclaim. Task lifetime and nothing else.
async fn serve_worker(js: WorkerRuntime, start: WorkerStart) {
    let WorkerStart {
        key,
        name,
        script,
        messages,
        events,
    } = start;
    let (worker, mut ended) = Worker::new(js, key, events);
    worker.spawn(boot_worker(Rc::clone(&worker), name, script, messages));
    // Every reason takes the same path from here; what differs is only what
    // the creating realm has already been told. `Trapped` is the one the owner
    // still owes a report for, which is why the reason is read rather than
    // dropped.
    let reason = ended.recv().await.unwrap_or(WorkerEnd::Terminated);
    worker.reap(reason).await;
}

/// The worker's boot future: wait for the script, evaluate it, then start the
/// two waits a live worker has.
///
/// The `select!` below is one task's two-source wait rather than a
/// dispatcher, and it is `biased` for the reason HTML's "terminate a worker"
/// aborts the fetch: a `Terminate` that lands in the same instant as the
/// script must win, so a worker told to stop before it booted never boots.
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
    let mut queued: Vec<String> = Vec::new();
    let loaded = loop {
        tokio::select! {
            biased;
            message = messages.recv() => match message {
                // Either way this worker is over. `Terminate` is what the
                // realm that created it says — on `terminate()`, and again as
                // it is released — and a closed channel is the backstop, for
                // a realm that was gone before it could say anything.
                None | Some(WorkerMessage::Terminate) => {
                    worker.end(WorkerEnd::Terminated);
                    return;
                }
                Some(WorkerMessage::Post(data)) => queued.push(data),
            },
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
    let Some(checkpoints) = worker.boot(&name, source) else {
        return;
    };
    for data in queued {
        worker.enter(|realm, js| deliver(&worker.events, worker.key, realm, js, &data));
    }
    // A load-time `close()` is reported here, and the first deadline is
    // published here, for a script that armed a timer and was posted nothing.
    worker.settle();
    if worker.ended() {
        return;
    }
    worker.spawn(consume_messages(Rc::clone(&worker), messages));
    worker.spawn(wait_timers(Rc::clone(&worker), worker.deadline.subscribe()));
    worker.spawn(follow_checkpoints(Rc::clone(&worker), checkpoints));
}

/// The one ordered consumer of what is posted to this worker.
async fn consume_messages(
    worker: Rc<Worker>,
    mut messages: mpsc::UnboundedReceiver<WorkerMessage>,
) {
    while let Some(message) = messages.recv().await {
        match message {
            // As before the boot: the realm's `Terminate`, or — below — the
            // channel it would have sent one on having gone with it.
            WorkerMessage::Terminate => break,
            WorkerMessage::Post(data) => {
                worker.enter(|realm, js| deliver(&worker.events, worker.key, realm, js, &data));
            }
        }
    }
    worker.end(WorkerEnd::Terminated);
}

/// This realm's one wait on its own clock.
///
/// The `select!` is this task's own wait — a deadline, or the re-arm that
/// moves it — and dispatches nothing: the worker's epilogue is what fires the
/// timers a passed deadline named. One `Sleep` for the whole task, re-armed
/// only when the deadline moved, because a `Sleep` registers with the
/// platform's timer on its first poll.
async fn wait_timers(worker: Rc<Worker>, mut deadlines: watch::Receiver<Option<ClockInstant>>) {
    let mut armed: Option<ClockInstant> = None;
    let mut sleep = std::pin::pin!(crate::clock::sleep_until(ClockInstant::now()));
    loop {
        if worker.ended() {
            return;
        }
        let deadline = *deadlines.borrow_and_update();
        if deadline != armed {
            if let Some(deadline) = deadline {
                sleep.set(crate::clock::sleep_until(deadline));
            }
            armed = deadline;
        }
        match armed {
            None => {
                if deadlines.changed().await.is_err() {
                    return;
                }
            }
            Some(_) => {
                tokio::select! {
                    () = &mut sleep => {
                        // Consumed: the next turn arms a wait of its own
                        // rather than polling this one again.
                        armed = None;
                        worker.settle();
                    }
                    changed = deadlines.changed() => if changed.is_err() { return },
                }
            }
        }
    }
}

/// Watches the runtime-wide checkpoint generation for a sibling worker's entry
/// into JavaScript.
///
/// Every worker realm on this thread shares one runtime, so it shares one
/// promise-job queue: a sibling's checkpoint drains this realm's jobs too, and
/// a continuation of this worker's can therefore run inside an entry that had
/// nothing to do with it. What that continuation arms is a deadline only this
/// worker's epilogue publishes, so without this task the sleep in
/// [`wait_timers`] would never be told to move. Equality with this worker's own
/// generation is what keeps its own entries from waking it.
async fn follow_checkpoints(worker: Rc<Worker>, mut checkpoints: watch::Receiver<u64>) {
    while checkpoints.changed().await.is_ok() {
        if worker.ended() {
            return;
        }
        if *checkpoints.borrow_and_update() == worker.own_checkpoint.get() {
            continue;
        }
        worker.settle();
    }
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
        Ok(Err(error)) => Err(error.to_string()),
        Err(_) => Err("the fetcher dropped the request".to_owned()),
    }
}

fn open_realm(
    js_runtime: &mut ScriptRuntime,
    events: mpsc::UnboundedSender<WorkerEvent>,
    key: WorkerKey,
) -> Result<WorkerRealm, ScriptError> {
    let mut engine = js_runtime
        .create_realm()
        .map_err(|error| context_of("creating the worker realm", error))?;
    let timers = Rc::new(TimerState::new());
    let closing = Rc::new(Cell::new(false));
    install_worker_members(&mut engine, js_runtime, &timers, &closing, move |data| {
        let _ = events.send(WorkerEvent {
            key,
            payload: WorkerPayload::Message(data),
        });
    })?;
    Ok(WorkerRealm {
        engine,
        timers,
        closing,
    })
}

/// Hands one JSON message to a realm that is up.
fn deliver(
    events: &mpsc::UnboundedSender<WorkerEvent>,
    key: WorkerKey,
    realm: &mut WorkerRealm,
    js_runtime: &mut ScriptRuntime,
    data: &str,
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
        &[HostArgument::String(data)],
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

    /// One started worker, with the test holding every end of it.
    struct Started {
        worker: Rc<Worker>,
        messages: mpsc::UnboundedSender<WorkerMessage>,
        /// Held so the channels stay open for as long as the worker does.
        _events: mpsc::UnboundedReceiver<WorkerEvent>,
        _ended: mpsc::UnboundedReceiver<WorkerEnd>,
    }

    /// Starts one worker on `js`, with its script already answered.
    ///
    /// The script does nothing, because what these pins are about is which
    /// task ran rather than what the script said.
    fn start(js: &WorkerRuntime, key: u64) -> Started {
        let (events, events_rx) = mpsc::unbounded_channel();
        let (messages, messages_rx) = mpsc::unbounded_channel();
        let (script, script_rx) = oneshot::channel();
        let _ = script.send(Ok(LoadedSource::Entry {
            source: String::new(),
            url: format!("app:///worker{key}.js"),
        }));
        let (worker, ended) = Worker::new(Rc::clone(js), WorkerKey::new(key), events);
        worker.spawn(boot_worker(
            Rc::clone(&worker),
            String::new(),
            script_rx,
            messages_rx,
        ));
        Started {
            worker,
            messages,
            _events: events_rx,
            _ended: ended,
        }
    }

    /// A sibling worker's entry drains the promise-job queue every realm on
    /// this runtime shares, so this worker has to settle what its own realm
    /// owes. Its follower is what tells it to: without one, the only thing
    /// that could is an entry of its own.
    #[test]
    fn a_sibling_workers_entry_settles_this_worker() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .expect("a current-thread runtime asks the platform for nothing");
        LocalSet::new().block_on(&runtime, async {
            let mut js = ScriptRuntime::new().expect("a QuickJS runtime");
            install_worker_modules(&mut js).expect("the worker modules register");
            let js: WorkerRuntime = Rc::new(RefCell::new(Ok(js)));
            let first = start(&js, 1);
            let second = start(&js, 2);
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
                .send(WorkerMessage::Post("\"ping\"".to_owned()))
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
}
