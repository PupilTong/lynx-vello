//! Lynx main-thread ownership: the group's thread, and the views on it.
//!
//! The embedder's own thread starts this owner from `LynxGroup::new` and keeps
//! every painter itself. This thread builds the script runtime and the style
//! pool the group shares, and then adopts each view onto a task set of its
//! own: [`page`] is where a view's boot, its command stream, its workers, its
//! timers and the one boundary into JavaScript live.
//!
//! `bobcat-workers` is not this thread's. The group starts it beside this one
//! and joins it after it; what arrives here is one sender and the flag that
//! thread sets when it traps, and the three messages a realm sends on it —
//! start a context, post to one, stop one — are the whole of what this
//! thread does to it.
//!
//! Tasks rather than one state machine for all of them. A view waiting for its
//! entry parks on its own channels, so nothing it is waiting for can hold up a
//! sibling and no wait has to be spelled as a state. What the tasks share is
//! the thread they take turns on: one `QuickJS` runtime, one style pool, and —
//! because the promise-job queue is the runtime's — one checkpoint generation,
//! which is how a view learns that a sibling's entry into JavaScript may have
//! finished its own imports.
//!
//! This thread is one engine thread of [`crate::jobs`], and [`group_task`] is
//! the `main` future of its loop — a task like any other, so a view can attach
//! and a finished one be joined while a sibling view's job is parked on a
//! synchronous stylesheet.

mod exposure;
mod layout_change;
mod page;
pub(crate) mod quickjs;
pub(crate) mod record;
#[path = "runtime/lib.rs"]
pub(crate) mod runtime;
#[path = "tree/lib.rs"]
pub(crate) mod tree;
mod workers;

use std::cell::RefCell;
use std::num::NonZeroUsize;
use std::rc::Rc;
use std::sync::Arc;
#[cfg(target_arch = "wasm32")]
use std::sync::OnceLock;
use std::sync::atomic::AtomicBool;
#[cfg(not(target_arch = "wasm32"))]
use std::thread::Builder as ThreadBuilder;

use rustc_hash::FxHashMap;
use terminus::link::StylePool;
use terminus::link::clock::SpawnedClock;
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::{self, JoinError, JoinSet};
use tokio_util::sync::CancellationToken;
#[cfg(target_arch = "wasm32")]
use wasm_thread::Builder as ThreadBuilder;

use self::quickjs::{SharedRuntime, mark_checkpoint_later};
pub(crate) use self::workers::WorkerFactory;
use crate::background::WorkerCommand;
use crate::esm::build_main_runtime;
use crate::jobs::{JsThread, JsThreadHandle};
use crate::link::{ToMain, ViewOutbox};
use crate::realm::policy;
use crate::threads::ThreadJoin;
use crate::view::{
    EngineError, EventRequester, GroupCommand, StartupSources, StyleThreads, ViewAttachment,
    ViewSources, Viewport,
};

pub(crate) struct GroupLink {
    pub(crate) attach: mpsc::UnboundedReceiver<GroupCommand>,
    pub(crate) requester: Arc<dyn EventRequester>,
    pub(crate) ready: oneshot::Sender<Result<(), EngineError>>,
    pub(crate) workers: mpsc::UnboundedSender<WorkerCommand>,
    pub(crate) workers_trapped: Arc<AtomicBool>,
    pub(crate) clock: SpawnedClock,
}

struct GroupContext {
    js: SharedRuntime,
    style_pool: Option<Rc<StylePool>>,
    requester: Arc<dyn EventRequester>,
    workers: WorkerFactory,
    thread: JsThreadHandle,
}

#[cfg(target_arch = "wasm32")]
static WASM_WORKER_BOOTSTRAP: OnceLock<()> = OnceLock::new();

#[cfg(target_arch = "wasm32")]
pub fn configure_wasm_workers(worker_script_url: String) -> Result<(), EngineError> {
    if worker_script_url.is_empty() {
        return Err(EngineError::Thread {
            name: "wasm worker configuration",
            message: "the worker script URL must not be empty".to_owned(),
        });
    }
    if WASM_WORKER_BOOTSTRAP.set(()).is_err() {
        return Err(EngineError::Thread {
            name: "wasm worker configuration",
            message: "the Worker bootstrap was already configured".to_owned(),
        });
    }
    wasm_thread::Builder::empty()
        .worker_script_url(worker_script_url)
        .set_default();
    Ok(())
}

pub(crate) fn spawn_group(
    style_threads: StyleThreads,
    link: GroupLink,
) -> Result<ThreadJoin, EngineError> {
    let thread = ThreadBuilder::new()
        .name("bobcat-main".to_owned())
        .spawn(move || run_group(style_threads, link))
        .map_err(|error| EngineError::Thread {
            name: "script",
            message: error.to_string(),
        })?;
    Ok(ThreadJoin::new(thread))
}

fn run_group(style_threads: StyleThreads, link: GroupLink) {
    let GroupLink {
        attach,
        requester,
        ready,
        workers,
        workers_trapped,
        clock,
    } = link;
    clock.align_to_spawner();
    #[cfg(all(target_arch = "wasm32", panic = "abort"))]
    crate::threads::install_script_panic_hook();

    let js_runtime = build_main_runtime();
    let style_pool = match build_style_pool(style_threads.resolve()) {
        Ok(pool) => pool.map(Rc::new),
        Err(error) => {
            let _ = ready.send(Err(error));
            return;
        }
    };
    if ready.send(Ok(())).is_err() {
        return;
    }

    let thread = JsThread::new();
    let context = Rc::new(GroupContext {
        js: Rc::new(RefCell::new(js_runtime)),
        style_pool,
        requester,
        workers: WorkerFactory::new(workers, workers_trapped),
        thread: thread.handle(),
    });
    thread.run(group_task(context, attach));
    drop(thread);
}

async fn group_task(context: Rc<GroupContext>, mut attach: mpsc::UnboundedReceiver<GroupCommand>) {
    let mut views = JoinSet::new();
    let mut outboxes: FxHashMap<task::Id, ViewOutbox> = FxHashMap::default();
    loop {
        tokio::select! {
            command = attach.recv() => match command {
                Some(GroupCommand::Attach(attachment)) => {
                    let ViewAttachment {
                        viewport, sources, text_context, worker_fonts, startup, native_modules,
                        native_esms,
                        commands, metrics, scroll, notices, frames, cancel, fetch_probe,
                        style_sheet_probe, base_url, registry, canvases, to_main,
                    } = *attachment;
                    let outbox = ViewOutbox::new(
                        notices,
                        frames,
                        Arc::clone(&context.requester),
                        cancel.clone(),
                        fetch_probe,
                        style_sheet_probe,
                        base_url,
                        registry,
                    )
                    .with_native_esms(native_esms)
                    .with_worker_fonts(worker_fonts)
                    .with_canvases(canvases, to_main);
                    #[cfg(all(target_arch = "wasm32", panic = "abort"))]
                    crate::threads::add_script_panic_reporter({
                        let outbox = outbox.clone();
                        Box::new(move |detail| {
                            outbox.engine_event((policy::main_thread_panic().event)(
                                crate::threads::platform_script_error(format!(
                                    "the Lynx main thread {detail}"
                                )),
                            ));
                        })
                    });
                    let view = AttachedView {
                        viewport, sources, text_context, startup, native_modules, commands,
                        metrics, scroll, cancel,
                    };
                    let handle = views.spawn_local(page::serve_view(
                        Rc::clone(&context),
                        view,
                        outbox.clone(),
                    ));
                    outboxes.insert(handle.id(), outbox);
                }
                None => break,
            },
            Some(finished) = views.join_next_with_id(), if !views.is_empty() => {
                finish_view(&context, finished, &mut outboxes);
            }
        }
    }
    while let Some(finished) = views.join_next_with_id().await {
        finish_view(&context, finished, &mut outboxes);
    }
}

fn finish_view(
    context: &GroupContext,
    finished: Result<(task::Id, ()), JoinError>,
    outboxes: &mut FxHashMap<task::Id, ViewOutbox>,
) {
    let trapped = match finished {
        Ok((id, ())) => {
            outboxes.remove(&id);
            None
        }
        Err(error) => match (outboxes.remove(&error.id()), error.is_panic()) {
            (Some(outbox), true) => Some((outbox, error)),
            _ => None,
        },
    };
    if let Some((outbox, error)) = trapped {
        outbox
            .engine_event(policy::main_thread_panic().event_for_panic(error.into_panic().as_ref()));
    }
    mark_checkpoint_later(&context.js, &context.thread);
}

struct AttachedView {
    viewport: Viewport,
    sources: ViewSources,
    text_context: terminus::link::TextContext,
    startup: StartupSources,
    native_modules: String,
    commands: mpsc::UnboundedReceiver<ToMain>,
    metrics: watch::Receiver<Option<Viewport>>,
    scroll: Arc<crate::link::ScrollMailbox>,
    cancel: CancellationToken,
}

fn build_style_pool(threads: Option<NonZeroUsize>) -> Result<Option<StylePool>, EngineError> {
    let Some(threads) = threads else {
        return Ok(None);
    };
    #[cfg(target_arch = "wasm32")]
    let pool = StylePool::with_spawn_handler(threads, |worker| {
        let mut builder = wasm_thread::Builder::new();
        if let Some(name) = worker.name() {
            builder = builder.name(name.to_owned());
        }
        if let Some(stack_size) = worker.stack_size() {
            builder = builder.stack_size(stack_size);
        }
        builder.spawn(move || worker.run()).map(|_| ())
    });
    #[cfg(not(target_arch = "wasm32"))]
    let pool = StylePool::with_threads(threads);
    pool.map(Some).map_err(|error| EngineError::Thread {
        name: "style pool",
        message: error.to_string(),
    })
}
