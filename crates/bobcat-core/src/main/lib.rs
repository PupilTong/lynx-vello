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
//! start a context with its script, post to one, stop one — are the whole of
//! what this thread does to it.
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

mod page;
pub(crate) mod quickjs;
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

use dom::StylePool;
use rustc_hash::FxHashMap;
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::{self, JoinError, JoinSet};
use tokio_util::sync::CancellationToken;
#[cfg(target_arch = "wasm32")]
use wasm_thread::Builder as ThreadBuilder;

use self::quickjs::{SharedRuntime, mark_checkpoint_later};
pub(crate) use self::workers::WorkerFactory;
use crate::background::WorkerCommand;
use crate::esm::build_runtime;
use crate::jobs::{JsThread, JsThreadHandle};
use crate::link::{ToMain, ViewOutbox};
use crate::threads::{self, ThreadJoin};
use crate::view::{
    EngineError, EngineEvent, EventRequester, GroupCommand, StartupSources, StyleThreads,
    ViewAttachment, ViewSources, Viewport,
};

/// The main thread's end of its group's link.
pub(crate) struct GroupLink {
    /// Every view this group is ever asked to serve.
    pub(crate) attach: mpsc::UnboundedReceiver<GroupCommand>,
    /// The one event loop every view in this group wakes.
    pub(crate) requester: Arc<dyn EventRequester>,
    /// How this thread's own startup went, answered exactly once. Only the
    /// style pool can fail it: a script runtime that could not be built is
    /// each view's startup failure rather than the group's.
    pub(crate) ready: oneshot::Sender<Result<(), EngineError>>,
    /// What this thread is given of `bobcat-workers`: the right to send it
    /// messages, and the flag below. The thread itself is the group's,
    /// started before this one and joined after it.
    pub(crate) workers: mpsc::UnboundedSender<WorkerCommand>,
    /// Set by `bobcat-workers` once it has trapped. A `Worker` constructed
    /// after that fails at once instead of being sent to a thread that will
    /// never read it.
    pub(crate) workers_trapped: Arc<AtomicBool>,
}

/// What every view on this thread shares.
struct GroupContext {
    /// The runtime every view's realm is opened on, or why it could not be
    /// built. In that case each view that attaches fails its startup with
    /// that error, and the group itself stays up.
    js: SharedRuntime,
    style_pool: Option<Rc<StylePool>>,
    requester: Arc<dyn EventRequester>,
    workers: WorkerFactory,
    /// The thread itself: where a view's tasks are spawned, and where every
    /// entry into a realm is queued. A `Weak`, because the thread's own top
    /// loop owns everything this is reachable from.
    thread: JsThreadHandle,
}

#[cfg(target_arch = "wasm32")]
static WASM_WORKER_BOOTSTRAP: OnceLock<()> = OnceLock::new();

/// Tells `wasm_thread` which script boots a Worker, which is what every
/// thread a view spawns — `bobcat-main` and each of its style workers — is
/// made of.
///
/// Process-wide because the bootstrap is: one module, one script URL. The
/// thread *counts* are not, and belong to each group's
/// [`StyleThreads`](crate::StyleThreads).
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

/// Starts one group's Lynx main thread, which builds the script runtime and
/// the style pool its views share before adopting the first of them. Only the
/// pool failing fails the group: a runtime that could not be built is kept,
/// and fails each view that attaches.
///
/// Nothing announces its exit: dropping every view's notice sender closes
/// those channels, which is the same fact — and the one a painter blocked on
/// a `BeginFrame` is already waiting on.
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

/// The thread's whole body: build what the group shares, then run its views.
fn run_group(style_threads: StyleThreads, link: GroupLink) {
    let GroupLink {
        attach,
        requester,
        ready,
        workers,
        workers_trapped,
    } = link;
    #[cfg(all(target_arch = "wasm32", panic = "abort"))]
    threads::install_script_panic_hook();

    // Both the runtime and the pool are the group's, not any view's. A group
    // opens one realm per view on that runtime, which is why the modules its
    // views share are registered here, once.
    //
    // The pool is the group's for a harder reason than the runtime is: rayon
    // takes this thread over as index zero and never gives it back, so a
    // second pool built here would be refused outright. Views may share the
    // one pool because they cannot traverse at once — the single thread that
    // drives them both is already inside whichever traversal is running.
    //
    // Both are built before group construction returns and any view
    // attaches, and only the pool can fail it: a runtime that could not be
    // built is kept as its error, which every view that attaches reports as
    // its own startup failure — as `bobcat-workers` keeps its own runtime's
    // error for each of its workers.
    let js_runtime = build_runtime();
    let style_pool = match build_style_pool(style_threads.resolve()) {
        Ok(pool) => pool.map(Rc::new),
        Err(error) => {
            let _ = ready.send(Err(error));
            return;
        }
    };
    // The one thread this one does start, and for the same reason the group
    // starts its two eagerly: a card's first `setTimeout` must wait out its
    // own delay rather than a Worker's cold start. Natively tokio's timer is
    // the engine thread's own and there is nothing to start.
    #[cfg(target_arch = "wasm32")]
    crate::alarm::start();
    if ready.send(Ok(())).is_err() {
        return;
    }

    // The thread this group's views run on: a tokio scheduler for their waits,
    // and the FIFO of jobs every entry into a realm is queued on. `run` below
    // is its whole body.
    let thread = JsThread::new();
    let context = Rc::new(GroupContext {
        js: Rc::new(RefCell::new(js_runtime)),
        style_pool,
        requester,
        workers: WorkerFactory::new(workers, workers_trapped),
        thread: thread.handle(),
    });
    // By value: the context — and with it this thread's one sender to
    // `bobcat-workers` — is dropped with the loop, which drops the queue, then
    // the tasks, then the runtime.
    thread.run(group_task(context, attach));
    drop(thread);
}

/// Serves one group: adopts each view onto its own task, and reports a task
/// that trapped to the view it was serving.
async fn group_task(context: Rc<GroupContext>, mut attach: mpsc::UnboundedReceiver<GroupCommand>) {
    let mut views = JoinSet::new();
    let mut outboxes: FxHashMap<task::Id, ViewOutbox> = FxHashMap::default();
    loop {
        tokio::select! {
            command = attach.recv() => match command {
                Some(GroupCommand::Attach(attachment)) => {
                    let ViewAttachment {
                        viewport, sources, text_context, startup, native_modules, commands,
                        metrics, notices, frames, cancel, fetch_probe,
                    } = *attachment;
                    let outbox = ViewOutbox::new(
                        notices,
                        frames,
                        Arc::clone(&context.requester),
                        cancel.clone(),
                        fetch_probe,
                    );
                    #[cfg(all(target_arch = "wasm32", panic = "abort"))]
                    threads::add_script_panic_reporter({
                        let outbox = outbox.clone();
                        Box::new(move |detail| {
                            outbox.engine_event(EngineEvent::ScriptRunError(
                                threads::platform_script_error(format!(
                                    "the Lynx main thread {detail}"
                                )),
                            ));
                        })
                    });
                    let view = AttachedView {
                        viewport, sources, text_context, startup, native_modules, commands,
                        metrics, cancel,
                    };
                    let handle = views.spawn_local(page::serve_view(
                        Rc::clone(&context),
                        view,
                        outbox.clone(),
                    ));
                    outboxes.insert(handle.id(), outbox);
                }
                // The group's goodbye. Its views are already gone, or will
                // end as their own channels close.
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

/// A view task ended. If it trapped, its painter hears about it; either way
/// its siblings settle what their own realms owe, because a task that ended
/// part-way through may have left the shared job queue with work in it.
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
        outbox.engine_event(EngineEvent::ScriptRunError(threads::panicked(
            "the Lynx main thread panicked",
            error.into_panic().as_ref(),
        )));
    }
    mark_checkpoint_later(&context.js, &context.thread);
}

/// One view, minus the ends its outbox already took.
struct AttachedView {
    viewport: Viewport,
    sources: ViewSources,
    /// This view's fonts and default family, validated on the embedder's
    /// thread before anything was requested.
    text_context: Option<dom::TextContext>,
    /// The answers to the startup requests `create_lynx_view` already made:
    /// the author sheets in the order the view listed them, and the entry.
    startup: StartupSources,
    /// The embedder's native modules, as the realm is told about them: the
    /// record `create_lynx_view` encoded out of their names and methods.
    native_modules: String,
    commands: mpsc::UnboundedReceiver<ToMain>,
    /// The metrics an attached painter names, `None` until one binds. Not a
    /// command: an unbound `__FlushElementTree` parks the job it runs in on
    /// this, and no other job runs while one is parked.
    metrics: watch::Receiver<Option<Viewport>>,
    /// This view's end signal, minted on the embedder's thread. It is what the
    /// view's owner waits on and what its own end cancels.
    cancel: CancellationToken,
}

/// Builds one group's style pool, on `bobcat-main` — the thread that owns
/// every document in that group, is the only one that will ever flush them,
/// and is index zero of the pool this returns.
///
/// Call it nowhere else. Rayon takes the calling thread over in place, which
/// is what makes a lone view restyle on exactly the threads and with exactly
/// the parallelism it did when every document shared Stylo's global pool: the
/// root closure runs inline on `bobcat-main` and the managed members take over
/// only where a level is wider than the traversal's work unit.
///
/// The takeover is permanent — rayon leaks about 25 KB per pool and refuses a
/// second one on the same thread forever — which is the reason the pool is the
/// group's rather than any view's: one thread can only ever build one, so
/// every view on it shares that one or has none.
///
/// `None` asks for no pool at all, and answers `None`: those documents
/// traverse on `bobcat-main` alone, which is what a machine gets when the pool
/// would have held `bobcat-main` and nothing else.
fn build_style_pool(threads: Option<NonZeroUsize>) -> Result<Option<StylePool>, EngineError> {
    let Some(threads) = threads else {
        return Ok(None);
    };
    // A managed style thread is a Worker here, which rayon cannot start
    // itself — and this Worker spawns them, being itself one the Render
    // Worker spawned, and being index zero of the pool it is spawning for.
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
