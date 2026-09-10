//! Lynx main-thread ownership: the group's thread, and the views on it.
//!
//! The embedder's own thread starts this owner from `LynxGroup::new` and keeps
//! every painter itself. This thread builds the script runtime and the style
//! pool the group shares, and then adopts each view onto a task set of its
//! own: [`page`] is where a view's boot, its command stream, its workers, its
//! timers and the one boundary into JavaScript live.
//!
//! `bobcat-workers` is not this thread's. The group starts it beside this one
//! and joins it after it; what arrives here is one sender, and the three
//! messages a realm sends on it — start a context with its script, post to
//! one, stop one — are the whole of what this thread does to it.
//!
//! Tasks rather than one state machine for all of them. A view waiting for its
//! entry parks on its own channels, so nothing it is waiting for can hold up a
//! sibling and no wait has to be spelled as a state. What the tasks share is
//! the thread they take turns on: one `QuickJS` runtime, one style pool, and —
//! because the promise-job queue is the runtime's — one checkpoint generation,
//! which is how a view learns that a sibling's entry into JavaScript may have
//! finished its own imports.

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
#[cfg(not(target_arch = "wasm32"))]
use std::thread::Builder as ThreadBuilder;

use dom::StylePool;
use rustc_hash::FxHashMap;
use tokio::sync::{mpsc, oneshot};
use tokio::task::{self, JoinError, JoinSet, LocalSet};
use tokio_util::sync::CancellationToken;
#[cfg(target_arch = "wasm32")]
use wasm_thread::Builder as ThreadBuilder;

use self::quickjs::ScriptRuntime;
use self::runtime::install_shared_modules;
pub(crate) use self::workers::WorkerFactory;
use crate::background::WorkerCommand;
use crate::link::{ToMain, ViewOutbox};
use crate::threads::{self, JoinHandle};
use crate::view::{
    EngineError, EngineEvent, EventRequester, GroupCommand, LynxViewError, MainSources,
    StyleThreads, ViewAttachment, Viewport,
};

/// The group-owned right to join `bobcat-main`.
///
/// One per group rather than one per view: the thread outlives any single
/// view on it, and the join is the last thing that happens once the group and
/// the last view built from it are both gone.
pub(crate) struct GroupHome {
    thread: Option<JoinHandle>,
}

impl GroupHome {
    /// Waits for `bobcat-main` to return, once the goodbye that ends it has
    /// already been sent.
    pub(crate) fn join(&mut self) {
        if let Some(thread) = self.thread.take() {
            threads::join(thread);
        }
    }
}

/// The main thread's end of its group's link.
pub(crate) struct GroupLink {
    /// Every view this group is ever asked to serve.
    pub(crate) attach: mpsc::UnboundedReceiver<GroupCommand>,
    /// The one event loop every view in this group wakes.
    pub(crate) requester: Arc<dyn EventRequester>,
    /// How this thread's own startup went, answered exactly once.
    pub(crate) ready: oneshot::Sender<Result<(), LynxViewError>>,
    /// The one thing this thread is given of `bobcat-workers`: the right to
    /// send it messages. The thread itself is the group's, started before
    /// this one and joined after it.
    pub(crate) workers: mpsc::UnboundedSender<WorkerCommand>,
}

/// What every view on this thread shares.
struct GroupContext {
    /// Shared rather than owned by the group task: each view task enters it
    /// synchronously, inside one poll, and never holds the borrow across an
    /// await.
    js: Rc<RefCell<ScriptRuntime>>,
    style_pool: Option<Rc<StylePool>>,
    requester: Arc<dyn EventRequester>,
    workers: WorkerFactory,
}

#[cfg(target_arch = "wasm32")]
static WASM_WORKER_BOOTSTRAP: OnceLock<()> = OnceLock::new();
#[cfg(all(target_arch = "wasm32", panic = "abort"))]
static WASM_SCRIPT_PANIC_HOOK: OnceLock<()> = OnceLock::new();

/// Reports a panic on the thread that installed it, over whatever link that
/// thread holds. Erased to a closure because a `thread_local!` static cannot
/// be generic — and the hook it feeds is process-global anyway.
#[cfg(all(target_arch = "wasm32", panic = "abort"))]
type ScriptPanicReporter = Box<dyn Fn(crate::script::ScriptError)>;

#[cfg(all(target_arch = "wasm32", panic = "abort"))]
thread_local! {
    /// One reporter per view this thread has ever carried. Append-only: a
    /// view that is gone has a closed channel, and sending onto one is
    /// already a no-op, so nothing has to be pruned on a path that only runs
    /// as the Worker traps.
    static WASM_SCRIPT_PANIC_REPORTERS: RefCell<Vec<ScriptPanicReporter>> = const {
        RefCell::new(Vec::new())
    };
}

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
/// the style pool its views share before adopting the first of them.
///
/// Nothing announces its exit: dropping every view's notice sender closes
/// those channels, which is the same fact — and the one a painter blocked on
/// a `BeginFrame` is already waiting on.
pub(crate) fn spawn_group(
    style_threads: StyleThreads,
    link: GroupLink,
) -> Result<GroupHome, EngineError> {
    let thread = ThreadBuilder::new()
        .name("bobcat-main".to_owned())
        .spawn(move || run_group(style_threads, link))
        .map_err(|error| EngineError::Thread {
            name: "script",
            message: error.to_string(),
        })?;
    Ok(GroupHome {
        thread: Some(thread),
    })
}

/// The thread's whole body: build what the group shares, then run its views.
fn run_group(style_threads: StyleThreads, link: GroupLink) {
    let GroupLink {
        attach,
        requester,
        ready,
        workers,
    } = link;
    #[cfg(all(target_arch = "wasm32", panic = "abort"))]
    install_script_panic_hook();

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
    // Both are ready before group construction returns and any view
    // attaches.
    let started = ScriptRuntime::new()
        .map_err(LynxViewError::from)
        .and_then(|mut runtime| {
            install_shared_modules(&mut runtime)
                .map_err(|error| error.into_script_error().into())
                .map(|()| runtime)
        })
        .and_then(|runtime| {
            build_style_pool(style_threads.resolve())
                .map_err(LynxViewError::from)
                .map(|pool| (runtime, pool.map(Rc::new)))
        });
    let (js_runtime, style_pool) = match started {
        Ok(started) => started,
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

    let context = Rc::new(GroupContext {
        js: Rc::new(RefCell::new(js_runtime)),
        style_pool,
        requester,
        workers: WorkerFactory::new(workers),
    });
    let mut builder = tokio::runtime::Builder::new_current_thread();
    // Natively the engine waits out its own realms' timers on tokio's timer.
    // On wasm32 that timer reads `std::time::Instant`, which panics there, so
    // `crate::clock::sleep_until` serves the same waits through a thread of
    // this crate's own instead.
    #[cfg(not(target_arch = "wasm32"))]
    builder.enable_time();
    let runtime = builder
        .build()
        .expect("a current-thread runtime asks the platform for nothing");
    let local = LocalSet::new();
    // By value: the context — and with it this thread's one sender to
    // `bobcat-workers` — is dropped with the loop.
    local.block_on(&runtime, group_task(context, attach));
    drop(local);
    drop(runtime);
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
                    let ViewAttachment { viewport, sources, commands, notices, frames, cancel } =
                        *attachment;
                    let outbox = ViewOutbox::new(
                        notices,
                        frames,
                        Arc::clone(&context.requester),
                        cancel.clone(),
                    );
                    #[cfg(all(target_arch = "wasm32", panic = "abort"))]
                    add_script_panic_reporter({
                        let outbox = outbox.clone();
                        Box::new(move |error| {
                            outbox.engine_event(EngineEvent::ScriptRunError(error));
                        })
                    });
                    let view = AttachedView { viewport, sources, commands, cancel };
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
    context.js.borrow().mark_checkpoint();
}

/// One view, minus the ends its outbox already took.
struct AttachedView {
    viewport: Viewport,
    sources: MainSources,
    commands: mpsc::UnboundedReceiver<ToMain>,
    /// This view's end signal, minted on the embedder's thread. It is what the
    /// view's owner waits on, what its own end cancels, and the parent of the
    /// token every worker its realm creates carries.
    cancel: CancellationToken,
}

/// Registers a view's fonts and selects its default family, before any
/// document exists and before anything has been fetched.
///
/// Neither needs a document: fonts and the default family are a
/// [`TextContext`](dom::TextContext)'s business, and a document only ever
/// adopts a finished one. That is what keeps a family nothing provides a
/// zero-fetch failure — the check happens here, ahead of the first source
/// request, rather than inside the document that would have been built for it.
///
/// `None` is a view that named neither, which leaves the document's own lazy
/// context alone. `Err` is a default family neither the containers nor the
/// platform has, which is a failure to build the view rather than to run it.
fn stage_text_context(
    fonts: Vec<dom::FontBlob>,
    default_font_family: Option<&str>,
) -> Result<Option<dom::TextContext>, LynxViewError> {
    if fonts.is_empty() && default_font_family.is_none() {
        return Ok(None);
    }
    let mut text = dom::TextContext::new();
    for font in fonts {
        text.register_fonts(font);
    }
    if let Some(family) = default_font_family
        && !text.set_default_font_family(family)
    {
        return Err(EngineError::UnknownFontFamily(family.to_owned()).into());
    }
    Ok(Some(text))
}

#[cfg(all(target_arch = "wasm32", panic = "abort"))]
fn install_script_panic_hook() {
    WASM_SCRIPT_PANIC_HOOK.get_or_init(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            WASM_SCRIPT_PANIC_REPORTERS.with(|reporters| {
                let location = info
                    .location()
                    .map_or_else(String::new, |location| format!(" at {location}"));
                let error = threads::platform_script_error(format!(
                    "the script Worker aborted after a panic{location}: {}",
                    threads::panic_message(info.payload())
                ));
                for reporter in reporters.borrow().iter() {
                    reporter(error.clone());
                }
            });
            previous(info);
        }));
    });
}

#[cfg(all(target_arch = "wasm32", panic = "abort"))]
fn add_script_panic_reporter(reporter: ScriptPanicReporter) {
    WASM_SCRIPT_PANIC_REPORTERS.with(|reporters| reporters.borrow_mut().push(reporter));
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
