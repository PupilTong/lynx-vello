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
//! is the one ordered consumer of what is posted; one [`owner::load_module`]
//! task per module the realm asked its host for, its own script among them,
//! awaits that answer and completes it; [`serve_clock`] owns this realm's one
//! pinned sleep and watches the runtime-wide checkpoint generation, because the
//! job queue every worker realm here drains is the runtime's and a sibling's
//! entry can finish this realm's jobs. Every one of them reaches the realm
//! through [`owner::enter`], the driver a worker shares with every view, which
//! queues one job: it runs one synchronous operation and then the driver's
//! epilogue, which settles what it left owing — the timers that came due, a
//! `close()` it may have called, the root module finishing, the module
//! requests and futures it left waiting, the next deadline, and the checkpoint
//! generation as of that entry. What a worker adds to that epilogue is its
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
//! - each [`consume_messages`]' wait on what is posted, versus the realm's root module finishing
//!   until it has — one task's wait rather than a scheduler;
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

use super::scope::install_worker_members;
use super::{WorkerCommand, WorkerEvent, WorkerKey, WorkerMessage, WorkerPayload, WorkerStart};
use crate::clock::ClockInstant;
use crate::esm::{Export, build_runtime};
use crate::jobs::{JobKind, JsThread, JsThreadHandle, JsThreadName};
use crate::lifetime::{Lifetime, run_job, serve_clock};
use crate::link::HostOutbox;
use crate::main::quickjs::{ScriptRuntime, SharedRuntime, mark_checkpoint_later};
use crate::performance::{MemoryCategory, MemoryItem, WorkerLoadMark};
use crate::realm::owner::{self, RealmOwner};
use crate::realm::policy::{self, Row, Scene};
use crate::realm::{self, RealmCore};
use crate::script::ScriptError;

type Reporters = Rc<RefCell<FxHashMap<task::Id, Reporter>>>;

struct Reporter {
    key: WorkerKey,
    events: mpsc::UnboundedSender<WorkerEvent>,
    token: CancellationToken,
}

pub(super) fn run(commands: mpsc::UnboundedReceiver<WorkerCommand>, trapped: &Arc<AtomicBool>) {
    serve(Rc::new(RefCell::new(build_runtime())), commands, trapped);
}

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

fn serve(
    js: SharedRuntime,
    commands: mpsc::UnboundedReceiver<WorkerCommand>,
    trapped: &Arc<AtomicBool>,
) {
    let reporters = Reporters::default();
    #[cfg(all(target_arch = "wasm32", panic = "abort"))]
    {
        crate::threads::install_script_panic_hook();
        let trapped = Arc::clone(trapped);
        let reporters = Rc::clone(&reporters);
        crate::threads::add_script_panic_reporter(Box::new(move |detail| {
            report_thread_trap(&trapped, &reporters, &|| {
                (policy::worker_panic().event)(crate::threads::platform_script_error(format!(
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

struct WorkerRealm {
    #[cfg(feature = "devtools")]
    devtools: Option<crate::devtools::worker::WorkerSession>,
    core: RealmCore,
    scope_installed: Rc<Cell<bool>>,
    closing: Rc<Cell<bool>>,
    performance_observed: Rc<Cell<bool>>,
}

struct Worker {
    js: SharedRuntime,
    key: WorkerKey,
    url: String,
    events: mpsc::UnboundedSender<WorkerEvent>,
    realm: RefCell<Option<Box<WorkerRealm>>>,
    lifetime: Lifetime,
    host: HostOutbox,
    boot_finished: watch::Sender<bool>,
}

impl Worker {
    fn new(
        js: SharedRuntime,
        key: WorkerKey,
        url: String,
        events: mpsc::UnboundedSender<WorkerEvent>,
        token: CancellationToken,
        host: HostOutbox,
        thread: JsThreadHandle,
    ) -> Rc<Self> {
        Rc::new(Self {
            js,
            key,
            url,
            events,
            realm: RefCell::new(None),
            lifetime: Lifetime::new(token, thread),
            host,
            boot_finished: watch::channel(false).0,
        })
    }

    fn note_wait(&self, enqueued: Option<ClockInstant>) {
        if let Some(enqueued) = enqueued {
            self.host.registry().note_js_blocking(enqueued.elapsed());
        }
    }

    fn ended(&self) -> bool {
        self.lifetime.ended()
    }

    fn boot(self: &Rc<Self>, name: String) -> Option<watch::Receiver<u64>> {
        if self.ended() {
            return None;
        }
        let _ = self
            .host
            .registry()
            .mark_worker_load(self.key, WorkerLoadMark::CoreStart);
        let opened = {
            let mut runtime = self.js.borrow_mut();
            match runtime.as_mut() {
                Err(error) => Err(error.clone()),
                Ok(js) => realm::open_realm(
                    js,
                    &self.host,
                    self.lifetime.thread().clone(),
                    Some(self.key),
                    realm::RealmNames {
                        source: crate::ScriptSource::Worker {
                            id: crate::WorkerId::from(self.key),
                            url: Arc::from(self.url.as_str()),
                        },
                        opener: crate::performance::Opener::Worker(self.key),
                    },
                    None,
                    crate::canvas::CanvasHost::Worker(Arc::clone(self.host.worker_fonts())),
                    |engine| {
                        let events = self.events.clone();
                        let key = self.key;
                        let flags =
                            install_worker_members(engine, key, &self.host, name, move |data| {
                                let _ = events.send(WorkerEvent {
                                    key,
                                    payload: WorkerPayload::Message(data),
                                });
                            })?;
                        let performance =
                            crate::performance::install_worker_members(engine, &self.host, key)?;
                        Ok((flags, performance))
                    },
                )
                .map(|(core, (flags, performance_observed))| WorkerRealm {
                    #[cfg(feature = "devtools")]
                    devtools: None,
                    core,
                    scope_installed: flags.scope_installed,
                    closing: flags.closing,
                    performance_observed,
                })
                .map(|realm| {
                    if let Err(error) = realm.core.engine.load_root_module(js, &self.url) {
                        policy::report(self, Scene::Boot, error);
                    }
                    let checkpoints = js.checkpoints();
                    self.lifetime.record_checkpoint(js.checkpoint_generation());
                    (realm, checkpoints)
                }),
            }
        };
        match opened {
            Ok((realm, checkpoints)) => {
                *self.realm.borrow_mut() = Some(Box::new(realm));
                let _ = owner::enter_now(self, |_, _| ());
                Some(checkpoints)
            }
            Err(error) => {
                policy::report(self, Scene::Open, error);
                None
            }
        }
    }

    #[cfg(test)]
    fn is_live(&self) -> bool {
        self.realm.borrow().is_some()
    }

    #[cfg(test)]
    fn is_booted(&self) -> bool {
        *self.boot_finished.borrow()
    }

    #[cfg(test)]
    fn epilogue_count(&self) -> u64 {
        self.lifetime.epilogue_count()
    }

    #[cfg(test)]
    fn spawn(self: &Rc<Self>, future: impl std::future::Future<Output = ()> + 'static) {
        owner::spawn(self, future);
    }

    #[cfg(test)]
    fn enter_now<T>(
        self: &Rc<Self>,
        operation: impl FnOnce(&mut WorkerRealm, &mut ScriptRuntime) -> T,
    ) -> Option<T> {
        owner::enter_now(self, operation)
    }
}

impl RealmOwner for Worker {
    type Realm = WorkerRealm;
    type Event = WorkerPayload;
    const THREAD: JsThreadName = JsThreadName::Workers;

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

    fn core(realm: &mut WorkerRealm) -> &RealmCore {
        &realm.core
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

    fn row(scene: Scene) -> Row<WorkerPayload> {
        policy::worker(scene)
    }

    fn panic_row() -> Row<WorkerPayload> {
        policy::worker_panic()
    }

    fn run_due_timers(realm: &mut WorkerRealm, js: &mut ScriptRuntime) -> Vec<ScriptError> {
        if realm.closing.get() {
            return Vec::new();
        }
        let mut failures =
            crate::timers::run_due_timers(&realm.core.engine, js, &realm.core.timers)
                .unwrap_or_default();
        failures.extend(realm.core.children.run_due_timers(js).unwrap_or_default());
        failures
    }

    fn after_timers(worker: &Rc<Self>, realm: &mut WorkerRealm) {
        if realm.closing.get() {
            owner::terminal(worker, WorkerPayload::Closed);
        }
    }

    fn booted(&self) -> bool {
        *self.boot_finished.borrow()
    }

    fn mark_booted(&self) {
        self.boot_finished.send_replace(true);
    }
}

async fn serve_worker(js: SharedRuntime, start: WorkerStart, thread: JsThreadHandle) {
    let WorkerStart {
        key,
        name,
        url,
        messages,
        events,
        token,
        sources,
    } = start;
    let registry = Arc::clone(sources.registry());
    let worker = Worker::new(js, key, url, events, token, sources, thread);
    owner::spawn(&worker, boot_worker(Rc::clone(&worker), name, messages));
    owner::run_owner(&worker).await;
    registry.forget_worker(key);
}

async fn boot_worker(
    worker: Rc<Worker>,
    name: String,
    messages: mpsc::UnboundedReceiver<WorkerMessage>,
) {
    let booted = run_job(&worker, JobKind::Start, move |worker| worker.boot(name));
    owner::spawn(&worker, consume_messages(Rc::clone(&worker), messages));
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

fn deliver_vsync(worker: &Rc<Worker>, milliseconds: f64, enqueued: Option<ClockInstant>) {
    let reporting = Rc::clone(worker);
    owner::queue(worker, JobKind::AnimationFrame, move |realm, js| {
        reporting.note_wait(enqueued);
        if let Err(error) = realm.core.engine.call_module_export(
            js,
            Export::BeginFrame,
            &[HostArgument::Number(milliseconds)],
        ) {
            policy::report(&reporting, Scene::Frame, error);
        }
    });
}

fn deliver_module_callback(
    worker: &Rc<Worker>,
    call: u64,
    index: u32,
    arguments: Option<String>,
    enqueued: Option<ClockInstant>,
) {
    let reporting = Rc::clone(worker);
    owner::queue(worker, JobKind::ModuleCallback, move |realm, js| {
        reporting.note_wait(enqueued);
        let delivered = crate::native_module::deliver(
            &realm.core.engine,
            js,
            call,
            index,
            arguments.as_deref(),
        );
        if let Err(error) = delivered {
            policy::report(&reporting, Scene::HostCall, error);
        }
    });
}

async fn consume_messages(
    worker: Rc<Worker>,
    mut messages: mpsc::UnboundedReceiver<WorkerMessage>,
) {
    let mut held: Option<Vec<(HostValue, Option<ClockInstant>)>> = Some(Vec::new());
    let mut performance_held: Option<Option<ClockInstant>> = None;
    #[cfg(feature = "devtools")]
    let mut devtools_held = Vec::new();
    let mut ready = worker.boot_finished.subscribe();
    loop {
        if let Some(queued) = held.take_if(|_| *ready.borrow_and_update()) {
            for (data, enqueued) in queued {
                deliver_post(&worker, data, enqueued);
            }
            #[cfg(feature = "devtools")]
            for message in devtools_held.drain(..) {
                deliver_devtools(&worker, message);
            }
            if let Some(enqueued) = performance_held.take() {
                deliver_performance(&worker, enqueued);
            }
        }
        tokio::select! {
            biased;
            message = messages.recv() => match message {
                None | Some(WorkerMessage::Terminate) => break,
                Some(WorkerMessage::Post { data, enqueued }) => match held.as_mut() {
                    Some(held) => held.push((data, enqueued)),
                    None => deliver_post(&worker, data, enqueued),
                },
                Some(WorkerMessage::Vsync { milliseconds, enqueued }) => {
                    deliver_vsync(&worker, milliseconds, enqueued);
                }
                Some(WorkerMessage::MemorySample { enqueued }) => {
                    deliver_memory_sample(&worker, enqueued);
                }
                Some(WorkerMessage::PerformanceReady { enqueued }) => if held.is_some() {
                    performance_held.get_or_insert(enqueued);
                } else {
                    deliver_performance(&worker, enqueued);
                },
                Some(WorkerMessage::ModuleCallback { call, index, arguments, enqueued }) => {
                    deliver_module_callback(&worker, call, index, arguments, enqueued);
                }
                #[cfg(feature = "devtools")]
                Some(WorkerMessage::Devtools(message)) => if held.is_some() {
                    devtools_held.push(message);
                } else {
                    deliver_devtools(&worker, message);
                },
            },
            changed = ready.changed(), if held.is_some() => if changed.is_err() { return; },
        }
    }
    owner::end(&worker);
}

fn deliver_performance(worker: &Rc<Worker>, enqueued: Option<ClockInstant>) {
    let reporting = Rc::clone(worker);
    owner::queue(worker, JobKind::Performance, move |realm, js| {
        reporting.note_wait(enqueued);
        if realm.closing.get() || !realm.performance_observed.get() {
            return;
        }
        if let Err(error) = realm
            .core
            .engine
            .call_module_export(js, Export::PerformanceReady, &[])
        {
            policy::report(&reporting, Scene::Listener, error);
        }
    });
}

fn deliver_memory_sample(worker: &Rc<Worker>, enqueued: Option<ClockInstant>) {
    drop(run_job(worker, JobKind::Memory, move |worker| {
        worker.note_wait(enqueued);
        if worker.ended() {
            return None;
        }
        let usage = worker.js.borrow().as_ref().ok()?.memory_usage();
        let item =
            MemoryItem::scripting_engine(MemoryCategory::BackgroundThreadScriptingEngine, &usage);
        if worker.host.registry().note_memory(item) {
            worker.host.wake();
        }
        Some(())
    }));
}

#[cfg(feature = "devtools")]
fn deliver_devtools(worker: &Rc<Worker>, message: Box<crate::devtools::worker::WorkerDevtools>) {
    owner::queue(worker, JobKind::Devtools, move |realm, _| {
        crate::devtools::worker::handle(&mut realm.devtools, *message);
    });
}

fn deliver_post(worker: &Rc<Worker>, data: HostValue, enqueued: Option<ClockInstant>) {
    let delivering = Rc::clone(worker);
    owner::queue(worker, JobKind::Post, move |realm, js| {
        delivering.note_wait(enqueued);
        deliver(&delivering, realm, js, &data);
    });
}

fn deliver(
    worker: &Rc<Worker>,
    realm: &mut WorkerRealm,
    js_runtime: &mut ScriptRuntime,
    data: &HostValue,
) {
    if realm.closing.get() {
        return;
    }
    if !realm.scope_installed.get() {
        return;
    }
    let delivered = realm.core.engine.call_module_export(
        js_runtime,
        Export::DeliverWorkerMessage,
        &[data.as_argument()],
    );
    if let Err(error) = delivered {
        policy::report(worker, Scene::Listener, error);
    }
}

#[cfg(test)]
mod tests {

    use super::*;
    use crate::link::ViewNotice;
    use crate::resource::{LoadedSource, SourceRequest};
    use crate::threads::platform_script_error;

    const TURNS: usize = 512;

    const GLOBAL_SCOPE: &str = "import 'bobcat:worker'; import 'bobcat:timers';";

    fn on_a_js_thread<F, B>(body: B)
    where
        F: Future<Output = ()> + 'static,
        B: FnOnce(JsThreadHandle) -> F,
    {
        let thread = JsThread::new();
        let body = body(thread.handle());
        thread.run(body);
    }

    struct Started {
        worker: Rc<Worker>,
        messages: mpsc::UnboundedSender<WorkerMessage>,
        events: mpsc::UnboundedReceiver<WorkerEvent>,
        sources: mpsc::UnboundedReceiver<ViewNotice>,
    }

    async fn start(js: &SharedRuntime, thread: &JsThreadHandle, key: u64) -> Started {
        start_running(js, thread, key, GLOBAL_SCOPE.to_owned()).await
    }

    async fn start_running(
        js: &SharedRuntime,
        thread: &JsThreadHandle,
        key: u64,
        source: String,
    ) -> Started {
        let (events, events_rx) = mpsc::unbounded_channel();
        let (messages, messages_rx) = mpsc::unbounded_channel();
        let url = format!("app:///worker{key}.js");
        let key = WorkerKey::new(key);
        let token = CancellationToken::new();
        let (sources, mut sources_rx) = mpsc::unbounded_channel();
        let worker = Worker::new(
            Rc::clone(js),
            key,
            url.clone(),
            events,
            token.clone(),
            HostOutbox::new(
                sources,
                std::sync::Arc::new(crate::NoWakeup),
                token,
                None,
                crate::link::detached_base(),
                crate::performance::PipelineRegistry::detached(),
            ),
            thread.clone(),
        );
        worker.spawn(boot_worker(Rc::clone(&worker), String::new(), messages_rx));
        let Some(ViewNotice::RequestSource {
            request,
            completion,
        }) = next(&mut sources_rx).await
        else {
            panic!("the worker asked its host for its script");
        };
        assert!(
            matches!(&request, SourceRequest::Module(requested) if *requested == url),
            "the realm's root module is the module at the worker's URL: {request:?}"
        );
        completion.complete(Ok(LoadedSource::Module { source, url }));
        Started {
            worker,
            messages,
            events: events_rx,
            sources: sources_rx,
        }
    }

    fn worker_runtime() -> SharedRuntime {
        let js = build_runtime().expect("the worker runtime builds");
        Rc::new(RefCell::new(Ok(js)))
    }

    #[test]
    fn a_sibling_workers_entry_settles_this_worker() {
        on_a_js_thread(|thread| async move {
            let js = worker_runtime();
            let first = start(&js, &thread, 1).await;
            let second = start(&js, &thread, 2).await;
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

            for _ in 0..8 {
                task::yield_now().await;
            }
            let settled = first.worker.epilogue_count();

            second
                .messages
                .send(WorkerMessage::post(HostValue::String("ping".to_owned())))
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

    #[test]
    fn a_finished_worker_task_makes_a_parked_sibling_settle() {
        on_a_js_thread(|thread| async move {
            let js = worker_runtime();
            let first = start(&js, &thread, 1).await;
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
            let (messages, incoming) = mpsc::unbounded_channel();
            let (events, _events) = mpsc::unbounded_channel();
            let (notices, _notices) = mpsc::unbounded_channel();
            commands
                .send(WorkerCommand::Start(dedicated_start(
                    2, incoming, events, notices,
                )))
                .expect("the loop is serving");

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
            let (_messages, incoming) = mpsc::unbounded_channel();
            let (events, mut reported) = mpsc::unbounded_channel();
            let (notices, mut requested) = mpsc::unbounded_channel();
            commands
                .send(WorkerCommand::Start(dedicated_start(
                    1, incoming, events, notices,
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
                requested.try_recv().is_err(),
                "a worker whose realm was never opened asked its host for nothing"
            );
        });
    }

    fn dedicated_start(
        key: u64,
        messages: mpsc::UnboundedReceiver<WorkerMessage>,
        events: mpsc::UnboundedSender<WorkerEvent>,
        notices: mpsc::UnboundedSender<ViewNotice>,
    ) -> WorkerStart {
        let token = CancellationToken::new();
        let key = WorkerKey::new(key);
        WorkerStart {
            key,
            name: String::new(),
            url: format!("app:///worker{}.js", key.get()),
            messages,
            events,
            token: token.clone(),
            sources: HostOutbox::new(
                notices,
                Arc::new(crate::NoWakeup),
                token,
                None,
                crate::link::detached_base(),
                crate::performance::PipelineRegistry::detached(),
            ),
        }
    }

    impl Started {
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

    fn with_observers(test: impl FnOnce(&mut Started, &mut Started) + 'static) {
        on_a_js_thread(|thread| async move {
            let js = worker_runtime();
            let mut first = start(&js, &thread, 1).await;
            let mut second = start(&js, &thread, 2).await;
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
            assert_eq!(second.messages(), [] as [String; 0]);
        });
    }

    async fn until(mut ready: impl FnMut() -> bool) -> bool {
        for _ in 0..TURNS {
            if ready() {
                return true;
            }
            task::yield_now().await;
        }
        false
    }

    async fn next<T>(channel: &mut mpsc::UnboundedReceiver<T>) -> Option<T> {
        let mut arrived = None;
        until(|| {
            arrived = channel.try_recv().ok();
            arrived.is_some()
        })
        .await;
        arrived
    }

    #[test]
    fn an_in_band_terminate_ends_a_worker_whose_job_is_waiting_and_discards_what_is_behind_it() {
        on_a_js_thread(|thread| async move {
            let js = worker_runtime();
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
            )
            .await;
            assert!(
                until(|| started.worker.is_live()).await,
                "the worker booted"
            );

            started
                .messages
                .send(WorkerMessage::post(HostValue::String("first".to_owned())))
                .expect("the worker is serving");
            let Some(ViewNotice::RequestSource {
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

            started
                .messages
                .send(WorkerMessage::post(HostValue::String(
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

            let posted = next(&mut started.events).await;
            let Some(WorkerPayload::Message(message)) = posted.map(|event| event.payload) else {
                panic!("the handler posted what its require did");
            };
            let caught = super::super::wire_json(&message);
            assert!(
                caught.starts_with("\"first: ") && caught.contains("app:///never.cjs"),
                "the require threw into the handler's catch: {caught}"
            );

            assert!(
                next(&mut started.events).await.is_none(),
                "a post queued behind a terminate is discarded, not delivered"
            );
        });
    }

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
        )
        .await;
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
