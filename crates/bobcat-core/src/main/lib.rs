//! Lynx main-thread ownership, startup, and command rounds.
//!
//! The embedder's own thread starts this owner from `LynxGroup::new` and keeps
//! every painter itself. This thread builds the script runtime and the style
//! pool the group shares, then adopts one view at a time: it creates each
//! document, requests and mounts its startup sources, boots each realm, and
//! owns document and realm until that view is released or the group is.

pub(crate) mod quickjs;
#[path = "runtime/lib.rs"]
pub(crate) mod runtime;
#[path = "tree/lib.rs"]
pub(crate) mod tree;
mod workers;

#[cfg(all(target_arch = "wasm32", panic = "abort"))]
use std::cell::RefCell;
use std::num::NonZeroUsize;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::rc::Rc;
use std::str;
use std::sync::Arc;
#[cfg(target_arch = "wasm32")]
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(not(target_arch = "wasm32"))]
use std::thread::Builder as ThreadBuilder;

use dom::{CommittedFrame, StylePool};
#[cfg(target_arch = "wasm32")]
use wasm_thread::Builder as ThreadBuilder;

use self::quickjs::ScriptRuntime;
#[cfg(test)]
use self::runtime::MainThreadError;
use self::runtime::{MainThreadRuntime, install_shared_modules};
use self::tree::{LynxDocument, new_document};
pub(crate) use self::workers::WorkerFactory;
use crate::background::WorkerCommand;
use crate::clock::ClockInstant;
use crate::mailbox::{Mailbox, Sender};
use crate::resource::{LoadedSource, SourceRequest, StyleSheetSource};
use crate::script::{ScriptError, ScriptErrorKind, ScriptErrorPhase};
use crate::view::{
    Attachment, EngineError, EngineEvent, EventRequester, FrameHub, LynxViewError, MainSources,
    StyleThreads, ToMain, ToPainter, ViewId, Viewport, frame_slot,
};
#[cfg(test)]
use crate::view::{DETACHED_VIEW, DetachedLink};

#[cfg(test)]
pub(crate) struct EntryModule {
    pub(crate) source: String,
    pub(crate) url: String,
}

/// One view's construction and boot cancellation flag.
///
/// It only prevents work that has not entered synchronous JavaScript yet.
/// Once `QuickJS` is executing, cancellation takes effect when that call
/// returns; nothing interrupts a realm mid-call, and the group's other views
/// are untouched either way.
#[derive(Default)]
pub(crate) struct StartupControl {
    cancelled: AtomicBool,
}

impl StartupControl {
    pub(crate) fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }

    pub(crate) fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }
}

#[cfg(not(target_arch = "wasm32"))]
type MainJoinHandle = std::thread::JoinHandle<()>;
#[cfg(target_arch = "wasm32")]
type MainJoinHandle = wasm_thread::JoinHandle<()>;

/// The group-owned right to join `bobcat-main`.
///
/// One per group rather than one per view: the thread outlives any single
/// view on it, and the join is the last thing that happens once the group and
/// the last view built from it are both gone.
pub(crate) struct GroupHome {
    thread: Option<MainJoinHandle>,
}

impl GroupHome {
    /// Waits for `bobcat-main` to return, once the goodbye that ends it has
    /// already been sent.
    pub(crate) fn join(&mut self) {
        let Some(thread) = self.thread.take() else {
            return;
        };
        // Under `panic = "abort"` a trapped `bobcat-main` runs no
        // destructors and never signals its join handle, and no check
        // can outrun a trap that lands between the check and the wait —
        // so wasm teardown never joins. The goodbye is already sent: a
        // healthy main thread exits on its own, and a trapped one is
        // already gone.
        #[cfg(all(target_arch = "wasm32", panic = "abort"))]
        drop(thread);
        #[cfg(not(all(target_arch = "wasm32", panic = "abort")))]
        {
            let _ = thread.join();
        }
    }
}

/// The main thread's sending end: notification FIFO, newest-frame mailbox,
/// and the wakeup that announces both to the thread that paints.
pub(crate) struct ToPainterSender<R: EventRequester> {
    view: ViewId,
    notifications: Sender<ToPainter>,
    frames: Arc<FrameHub>,
    requester: Arc<R>,
}

// Hand-written: `derive(Clone)` would demand `R: Clone`, while a requester is
// shared through its `Arc` and is never cloned itself.
impl<R: EventRequester> Clone for ToPainterSender<R> {
    fn clone(&self) -> Self {
        Self {
            view: self.view,
            notifications: self.notifications.clone(),
            frames: Arc::clone(&self.frames),
            requester: Arc::clone(&self.requester),
        }
    }
}

impl<R: EventRequester> ToPainterSender<R> {
    pub(crate) fn new(
        view: ViewId,
        notifications: Sender<ToPainter>,
        frames: Arc<FrameHub>,
        requester: Arc<R>,
    ) -> Self {
        Self {
            view,
            notifications,
            frames,
            requester,
        }
    }

    /// Announces one notification, then wakes the thread that paints.
    ///
    /// Enqueue before requesting a host turn, so its pump observes the
    /// notification. Startup and running views use this same path.
    pub(crate) fn send(&self, notification: ToPainter) {
        if self
            .notifications
            .send((Some(self.view), notification))
            .is_ok()
        {
            self.requester.request_event();
        }
    }

    /// Replaces the newest-frame mailbox, then announces it.
    pub(crate) fn publish_frame(&self, frame: Arc<CommittedFrame>) {
        *frame_slot(&self.frames) = Some(frame);
        self.send(ToPainter::FrameChanged);
    }

    /// Asks the painter's store to name these sources and start loading them.
    pub(crate) fn request_images(&self, sources: Vec<Arc<str>>) {
        self.send(ToPainter::RequestImages(sources));
    }

    /// The event loop this view wakes, which in a group is every view's.
    #[cfg(test)]
    fn requester(&self) -> &Arc<R> {
        &self.requester
    }
}

/// The main thread's end of its group's link.
pub(crate) struct GroupLink<R: EventRequester> {
    pub(crate) workers: Sender<WorkerCommand>,
    /// Every view's commands, and every attachment, in the order they were
    /// sent.
    pub(crate) commands: Mailbox<ToMain>,
    pub(crate) notifications: Sender<ToPainter>,
    /// The one event loop every view in this group wakes.
    pub(crate) requester: Arc<R>,
    /// How this thread's own startup went, answered exactly once.
    pub(crate) ready: flume::Sender<Result<(), LynxViewError>>,
}

#[cfg(target_arch = "wasm32")]
static WASM_WORKER_BOOTSTRAP: OnceLock<()> = OnceLock::new();
#[cfg(all(target_arch = "wasm32", panic = "abort"))]
static WASM_SCRIPT_PANIC_HOOK: OnceLock<()> = OnceLock::new();

/// Reports a panic on the thread that installed it, over whatever link that
/// thread holds. Erased to a closure because a `thread_local!` static cannot
/// be generic — and the hook it feeds is process-global anyway.
#[cfg(all(target_arch = "wasm32", panic = "abort"))]
type ScriptPanicReporter = Box<dyn Fn(ScriptError)>;

#[cfg(all(target_arch = "wasm32", panic = "abort"))]
thread_local! {
    /// One reporter per view this thread has ever carried. Append-only: a
    /// view that is gone has a closed FIFO, and sending onto one is already a
    /// no-op, so nothing has to be pruned on a path that only runs as the
    /// Worker traps.
    static WASM_SCRIPT_PANIC_REPORTERS: RefCell<Vec<ScriptPanicReporter>> = const {
        RefCell::new(Vec::new())
    };
}

/// Tells `wasm_thread` which script boots a Worker, which is what every
/// thread a view spawns — `bobcat-main` and each of its style workers — is
/// made of.
///
/// Process-wide because the bootstrap is: one module, one script URL. The
/// thread *counts* are not, and belong to each view's
/// [`ViewSources::style_threads`](crate::ViewSources::style_threads).
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
/// style pool its views share before adopting the first of them.
///
/// Nothing announces its exit: dropping every `ToPainterSender` closes the
/// notification FIFOs, which is the same fact — and the one a painter blocked
/// on a `BeginFrame` is already waiting on.
pub(crate) fn spawn_group<R: EventRequester>(
    style_threads: StyleThreads,
    link: GroupLink<R>,
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

/// Unit-test seam for paint tests that supply their own document, skipping
/// the IO half of startup.
///
/// It takes a *builder* rather than a document for the same reason the
/// production path above creates one itself: a `LynxDocument` owns a stylo
/// `Device`, whose `Box<dyn FontMetricsProvider>` is `Sync` but not `Send`,
/// so a document cannot cross a thread boundary. The builder runs on
/// `bobcat-main`, which is the thread that owns the document for the rest of
/// its life.
///
/// The thread it starts is a group of exactly one view, with no style pool:
/// these tests are about the link and the painter, not about traversal.
#[cfg(test)]
pub(crate) fn spawn_test_main_thread<R: EventRequester>(
    build_document: impl FnOnce() -> LynxDocument + Send + 'static,
    entry: EntryModule,
    link: DetachedLink<R>,
) -> Result<GroupHome, EngineError> {
    let thread = ThreadBuilder::new()
        .name("bobcat-main".to_owned())
        .spawn(move || {
            let DetachedLink { commands, notify } = link;
            let requester = Arc::clone(notify.requester());
            let notifications = notify.notifications.clone();
            // Painter-only tests retain the command boundary but do not run
            // BTS. runtime::worker_tests drives the actual second runtime.
            let (worker_commands, _worker_inbox) = Mailbox::channel();
            let workers = WorkerFactory::new(worker_commands);
            let result = catch_unwind(AssertUnwindSafe(|| {
                let mut js_runtime = ScriptRuntime::new()?;
                install_shared_modules(&mut js_runtime)
                    .map_err(MainThreadError::into_script_error)?;
                let mut runtime =
                    MainThreadRuntime::new(&mut js_runtime, build_document(), notify.clone())
                        .map_err(MainThreadError::into_script_error)?;
                runtime
                    .install_workers(
                        &mut js_runtime,
                        &workers,
                        notify.clone(),
                        &entry.url,
                        None,
                        Arc::new(StartupControl::default()),
                    )
                    .map_err(MainThreadError::into_script_error)?;
                runtime
                    .run_main_thread_script(&mut js_runtime, &entry.source, &entry.url)
                    .map_err(MainThreadError::into_script_error)?;
                Ok((js_runtime, runtime))
            }))
            .unwrap_or_else(|payload| {
                Err(platform_script_error(format!(
                    "the script realm panicked: {}",
                    panic_payload(payload.as_ref())
                )))
            });
            match result {
                Ok((mut js_runtime, runtime)) => {
                    notify.send(ToPainter::Engine(EngineEvent::ScriptFinished));
                    notify.send(ToPainter::FrameChanged);
                    let mut views = vec![CarriedView::new(
                        DETACHED_VIEW,
                        ViewSlot::Running(runtime),
                        notify,
                        Arc::new(StartupControl::default()),
                    )];
                    serve_group(
                        &mut js_runtime,
                        None,
                        &requester,
                        &commands,
                        &notifications,
                        &workers,
                        &mut views,
                    );
                }
                Err(error) => {
                    notify.send(ToPainter::Engine(EngineEvent::ScriptRunError(error)));
                    notify.send(ToPainter::FrameChanged);
                }
            }
        })
        .map_err(|error| EngineError::Thread {
            name: "script",
            message: error.to_string(),
        })?;
    Ok(GroupHome {
        thread: Some(thread),
    })
}

/// One view on this thread: still mounting its sources, or booted and
/// serving.
///
/// Boot is a state machine rather than a loop because the thread may carry
/// several views: a view waiting for its entry must not consume a sibling's
/// commands, so each source is applied as it arrives and the wait belongs to
/// the thread rather than to any one view.
enum ViewSlot<R: EventRequester> {
    /// Boxed: a booting view holds its whole document inline, where a running
    /// one keeps it behind an `Rc` its host closures share.
    Booting(Box<Booting<R>>),
    Running(MainThreadRuntime<R>),
}

/// A view's document between its first source and its entry module.
struct Booting<R: EventRequester> {
    workers: WorkerFactory,
    background_entry: Option<String>,
    requests: std::vec::IntoIter<SourceRequest>,
    document: LynxDocument,
    notify: ToPainterSender<R>,
    init_data: Option<serde_json::Value>,
    global_props: Option<serde_json::Value>,
}

/// What applying one source did to a booting view.
enum Booted<R: EventRequester> {
    /// Still waiting for the entry.
    Waiting(Box<Booting<R>>),
    /// The entry arrived and ran; the view is serving now.
    Running(MainThreadRuntime<R>),
    /// Cancelled, or the painter is gone: nobody is listening for an outcome.
    Gone,
    Failed(LynxViewError),
}

impl<R: EventRequester> Booting<R> {
    /// Builds the document every source will be mounted on.
    ///
    /// `Err` is a source the document itself refuses — a default family
    /// neither the containers nor the platform has — which is a failure to
    /// build the view rather than to run it.
    fn new(
        viewport: Viewport,
        sources: MainSources,
        workers: WorkerFactory,
        style_pool: Option<&Rc<StylePool>>,
        notify: ToPainterSender<R>,
    ) -> Result<Self, LynxViewError> {
        let MainSources {
            config,
            fonts,
            default_font_family,
            style_sheets,
            entry,
            background_entry,
            init_data,
            global_props,
        } = sources;
        let mut document = new_document(viewport, config);
        if let Some(pool) = style_pool {
            document.set_style_pool(Rc::clone(pool));
        }
        for font in fonts {
            document.register_fonts(font);
        }
        if let Some(family) = default_font_family
            && !document.set_default_font_family(&family)
        {
            return Err(EngineError::UnknownFontFamily(family).into());
        }
        let requests = style_sheets
            .into_iter()
            .map(SourceRequest::StyleSheet)
            .chain(std::iter::once(SourceRequest::Entry(entry)))
            .collect::<Vec<_>>()
            .into_iter();
        Ok(Self {
            workers,
            background_entry,
            requests,
            document,
            notify,
            init_data,
            global_props,
        })
    }

    fn request_next(&mut self) {
        if let Some(request) = self.requests.next() {
            self.notify.send(ToPainter::RequestSource(request));
        }
    }

    /// Mounts the requested source, then requests the next. The entry is
    /// requested only after every author sheet has been mounted.
    fn apply(
        mut self: Box<Self>,
        js_runtime: &mut ScriptRuntime,
        source: LoadedSource,
        control: &Arc<StartupControl>,
    ) -> Booted<R> {
        match source {
            LoadedSource::StyleSheet(StyleSheetSource::Preparsed(sheet)) => {
                crate::style::add_preparsed_style_sheet(&mut self.document, &sheet);
            }
            LoadedSource::StyleSheet(StyleSheetSource::Text(css)) => {
                crate::style::add_style_sheet_text(&mut self.document, &css);
            }
            LoadedSource::Entry { source, url } => {
                return self.run_entry(js_runtime, &source, &url, control);
            }
        }
        if control.is_cancelled() {
            return Booted::Gone;
        }
        self.request_next();
        Booted::Waiting(self)
    }

    fn run_entry(
        self: Box<Self>,
        js_runtime: &mut ScriptRuntime,
        source: &str,
        url: &str,
        control: &Arc<StartupControl>,
    ) -> Booted<R> {
        if control.is_cancelled() {
            return Booted::Gone;
        }
        let Self {
            document,
            notify,
            workers,
            background_entry,
            init_data,
            global_props,
            ..
        } = *self;
        let mut runtime = match MainThreadRuntime::new(js_runtime, document, notify.clone()) {
            Ok(runtime) => runtime,
            Err(error) => return Booted::Failed(error.into_script_error().into()),
        };
        if control.is_cancelled() {
            return Booted::Gone;
        }
        if let Err(error) = runtime.prepare_initial_data(init_data.as_ref(), global_props.as_ref())
        {
            return Booted::Failed(error.into_script_error().into());
        }
        if let Err(error) = runtime.install_workers(
            js_runtime,
            &workers,
            notify,
            url,
            background_entry,
            Arc::clone(control),
        ) {
            return Booted::Failed(error.into_script_error().into());
        }
        if let Err(error) = runtime.run_main_thread_script(js_runtime, source, url) {
            if control.is_cancelled() {
                return Booted::Gone;
            }
            return Booted::Failed(error.into_script_error().into());
        }
        if control.is_cancelled() {
            return Booted::Gone;
        }
        Booted::Running(runtime)
    }
}

fn run_group<R: EventRequester>(style_threads: StyleThreads, link: GroupLink<R>) {
    let GroupLink {
        workers,
        commands,
        notifications,
        requester,
        ready,
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
    // Both are ready before group construction returns and any view attaches.
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
    let (mut js_runtime, style_pool) = match started {
        Ok(started) => started,
        Err(error) => {
            let _ = ready.send(Err(error));
            return;
        }
    };
    if ready.send(Ok(())).is_err() {
        return;
    }

    // The table lives outside the guard so a panic here can still be reported
    // to every view it ends — which is all of them, this thread being what
    // they share.
    let workers = WorkerFactory::new(workers);
    let mut views = Vec::new();
    let served = catch_unwind(AssertUnwindSafe(|| {
        serve_group(
            &mut js_runtime,
            style_pool.as_ref(),
            &requester,
            &commands,
            &notifications,
            &workers,
            &mut views,
        );
    }));
    if let Err(payload) = served {
        let error = platform_script_error(format!(
            "the Lynx main thread panicked: {}",
            panic_payload(payload.as_ref())
        ));
        for view in &views {
            view.notify
                .send(ToPainter::Engine(EngineEvent::ScriptRunError(
                    error.clone(),
                )));
        }
    }
}

/// One view a group's thread carries: its slot, the link it reports through,
/// and the flag its embedder cancels when dropping the view.
struct CarriedView<R: EventRequester> {
    id: ViewId,
    /// `None` only while a source is being mounted: mounting one consumes the
    /// booting document and hands back whatever it became. A view that
    /// panicked mid-mount is left this way, still in the table, so the report
    /// its group makes on the way out still reaches its painter.
    slot: Option<ViewSlot<R>>,
    notify: ToPainterSender<R>,
    control: Arc<StartupControl>,
    /// The newest `BeginFrame` this round has serviced, acknowledged in the
    /// round's tail.
    serviced_begin_frame: Option<u64>,
    announced_deadline: Option<ClockInstant>,
    boot_reported: bool,
}

impl<R: EventRequester> Drop for CarriedView<R> {
    fn drop(&mut self) {
        self.control.cancel();
    }
}

impl<R: EventRequester> CarriedView<R> {
    fn new(
        id: ViewId,
        slot: ViewSlot<R>,
        notify: ToPainterSender<R>,
        control: Arc<StartupControl>,
    ) -> Self {
        Self {
            id,
            boot_reported: matches!(slot, ViewSlot::Running(_)),
            slot: Some(slot),
            notify,
            control,
            serviced_begin_frame: None,
            announced_deadline: None,
        }
    }

    /// A booting view has no realm, so nothing can have armed a timer on it;
    /// only a running one can have a deadline at all.
    fn next_timer_deadline(&mut self) -> Option<ClockInstant> {
        match self.slot.as_mut()? {
            ViewSlot::Booting(_) => None,
            ViewSlot::Running(runtime) => runtime.next_timer_deadline(),
        }
    }

    /// Applies one command. `false` ends the view: it was released, its boot
    /// failed, or its construction was cancelled and nobody is listening for
    /// an outcome.
    fn apply(&mut self, js_runtime: &mut ScriptRuntime, command: ToMain) -> bool {
        if self.control.is_cancelled() || matches!(command, ToMain::Shutdown) {
            return false;
        }
        // A running view is served in place: everything after boot is the
        // common case, and nothing about it needs the slot by value.
        if let Some(ViewSlot::Running(runtime)) = self.slot.as_mut() {
            if let ToMain::SourceLoaded {
                module: Some(name),
                source,
            } = command
            {
                if let Err(error) = runtime.complete_module(js_runtime, &name, source) {
                    let error = error.into_script_error();
                    let event = if self.boot_reported {
                        EngineEvent::ScriptRunError(error)
                    } else {
                        EngineEvent::StartupFailed(error.into())
                    };
                    self.notify.send(ToPainter::Engine(event));
                    return false;
                }
                return true;
            }
            apply_main_command(
                js_runtime,
                runtime,
                command,
                &self.notify,
                &mut self.serviced_begin_frame,
            );
            return true;
        }
        let Some(ViewSlot::Booting(mut booting)) = self.slot.take() else {
            unreachable!("a carried view holds its slot between commands")
        };
        let outcome = match command {
            ToMain::SourceLoaded {
                module: None,
                source: Ok(source),
            } => booting.apply(js_runtime, source, &self.control),
            ToMain::SourceLoaded {
                module: None,
                source: Err(error),
            } => Booted::Failed(error),
            other => {
                match other {
                    ToMain::Resize {
                        width,
                        height,
                        device_pixel_ratio,
                    } => {
                        booting.document.set_viewport(width, height);
                        booting.document.set_device_pixel_ratio(device_pixel_ratio);
                    }
                    ToMain::ImageEvents(events) => {
                        booting.document.apply_image_events(&events);
                    }
                    ToMain::BeginFrame { seq, .. } => {
                        self.notify.send(ToPainter::BeginFrameServiced(seq));
                    }
                    // No committed tree or realm exists to route these to
                    // yet. A worker's news is reachable here — a view can be
                    // booting while a realm it created still speaks — and is
                    // dropped for the same reason: nothing can hear it.
                    ToMain::DispatchEvent { .. }
                    | ToMain::Refill { .. }
                    | ToMain::TimersDue
                    | ToMain::Worker { .. } => {}
                    #[cfg(test)]
                    ToMain::Probe(probe) => probe(&mut booting.document),
                    ToMain::Attach(_)
                    | ToMain::Close
                    | ToMain::SourceLoaded { .. }
                    | ToMain::Shutdown => unreachable!(),
                }
                Booted::Waiting(booting)
            }
        };
        match outcome {
            Booted::Waiting(booting) => self.slot = Some(ViewSlot::Booting(booting)),
            Booted::Running(runtime) => {
                self.slot = Some(ViewSlot::Running(runtime));
            }
            Booted::Failed(error) => {
                self.notify
                    .send(ToPainter::Engine(EngineEvent::StartupFailed(error)));
                return false;
            }
            Booted::Gone => return false,
        }
        true
    }

    /// The tail of one round. Only a running view has one: a booting view has
    /// published nothing and armed nothing.
    fn finish_round(&mut self, js_runtime: &mut ScriptRuntime) {
        let Some(ViewSlot::Running(runtime)) = self.slot.as_mut() else {
            return;
        };
        // After this round's commands, because a listener one of them
        // delivered may have cleared a timer that is already due, and on
        // every round rather than only the ones a nudge opened, because a
        // command can arrive while a deadline is already behind us.
        for failure in runtime.run_due_timers(js_runtime) {
            self.notify
                .send(ToPainter::Engine(EngineEvent::TimerFailed(failure)));
        }
        runtime.commit_if_dirty();
        let host_deadline = runtime
            .next_timer_deadline()
            .filter(|deadline| *deadline > ClockInstant::now());
        if host_deadline != self.announced_deadline {
            self.announced_deadline = host_deadline;
            self.notify.send(ToPainter::TimerDeadline(host_deadline));
        }
        if let Some(seq) = self.serviced_begin_frame.take() {
            self.notify.send(ToPainter::BeginFrameServiced(seq));
        }
    }

    /// Inspect all realms after the shared job queue has finished its turns.
    fn finish_module_loads(&mut self) -> bool {
        let Some(ViewSlot::Running(runtime)) = self.slot.as_mut() else {
            return true;
        };
        if !self.boot_reported {
            match runtime.main_module_finished() {
                Ok(false) => {}
                Ok(true) => {
                    self.boot_reported = true;
                    self.notify
                        .send(ToPainter::Engine(EngineEvent::ScriptFinished));
                    self.notify.send(ToPainter::FrameChanged);
                }
                Err(error) => {
                    self.notify
                        .send(ToPainter::Engine(EngineEvent::StartupFailed(
                            error.into_script_error().into(),
                        )));
                    return false;
                }
            }
        }
        runtime.request_modules();
        true
    }
}

/// Adopts one view, on the runtime and pool this thread already holds.
fn attach<R: EventRequester>(
    views: &mut Vec<CarriedView<R>>,
    style_pool: Option<&Rc<StylePool>>,
    requester: &Arc<R>,
    notifications: &Sender<ToPainter>,
    view: ViewId,
    attachment: Attachment,
    workers: &WorkerFactory,
) {
    let Attachment {
        viewport,
        sources,
        frames,
        control,
    } = attachment;
    let notify = ToPainterSender::new(view, notifications.clone(), frames, Arc::clone(requester));
    #[cfg(all(target_arch = "wasm32", panic = "abort"))]
    add_script_panic_reporter({
        let notify = notify.clone();
        Box::new(move |error| {
            notify.send(ToPainter::Engine(EngineEvent::ScriptRunError(error)));
        })
    });
    match Booting::new(
        viewport,
        sources,
        workers.clone(),
        style_pool,
        notify.clone(),
    ) {
        Ok(mut booting) => {
            booting.request_next();
            views.push(CarriedView::new(
                view,
                ViewSlot::Booting(Box::new(booting)),
                notify,
                control,
            ));
        }
        Err(error) => notify.send(ToPainter::Engine(EngineEvent::StartupFailed(error))),
    }
}

/// Drives every view on one group's thread, from its first source to the end
/// of its life.
///
/// One park, `commands.recv()`, for the whole group rather than one per view:
/// every view's commands arrive on one FIFO, each naming the view it is for.
/// That is also what boot being a state machine buys — a view still waiting
/// for its entry would otherwise park on a channel its siblings' commands are
/// already queued on.
fn serve_group<R: EventRequester>(
    js_runtime: &mut ScriptRuntime,
    style_pool: Option<&Rc<StylePool>>,
    requester: &Arc<R>,
    commands: &Mailbox<ToMain>,
    notifications: &Sender<ToPainter>,
    workers: &WorkerFactory,
    views: &mut Vec<CarriedView<R>>,
) {
    loop {
        for view in &mut *views {
            view.finish_round(js_runtime);
        }
        // A shared checkpoint can discover or finish a sibling's import.
        views.retain_mut(CarriedView::finish_module_loads);
        let first = if any_timer_due(views) {
            None
        } else {
            match commands.recv(None) {
                Ok(first) => Some(first),
                Err(_) => return,
            }
        };
        for (view, command) in first.into_iter().chain(commands.drain()) {
            match command {
                ToMain::Close => return,
                ToMain::Attach(attachment) => {
                    attach(
                        views,
                        style_pool,
                        requester,
                        notifications,
                        view.expect("an attachment addresses its view"),
                        *attachment,
                        workers,
                    );
                }
                command => {
                    let Some(index) = views.iter().position(|carried| Some(carried.id) == view)
                    else {
                        // A command for a view already released: its
                        // goodbye won the race with whatever its
                        // painter sent last.
                        continue;
                    };
                    if !views[index].apply(js_runtime, command) {
                        views.swap_remove(index);
                    }
                }
            }
        }
    }
}

fn any_timer_due<R: EventRequester>(views: &mut [CarriedView<R>]) -> bool {
    let now = ClockInstant::now();
    views.iter_mut().any(|view| {
        view.next_timer_deadline()
            .is_some_and(|deadline| deadline <= now)
    })
}

fn apply_main_command<R: EventRequester>(
    js_runtime: &mut ScriptRuntime,
    runtime: &mut MainThreadRuntime<R>,
    command: ToMain,
    notify: &ToPainterSender<R>,
    serviced_begin_frame: &mut Option<u64>,
) {
    match command {
        ToMain::SourceLoaded { .. } => {
            unreachable!("module completions are handled by the carried view")
        }
        ToMain::Worker { key, payload } => {
            if let Err(error) = runtime.dispatch_worker_event(js_runtime, key, payload) {
                notify.send(ToPainter::Engine(EngineEvent::ListenerFailed(
                    error.into_script_error(),
                )));
            }
        }
        ToMain::DispatchEvent {
            target,
            name,
            detail,
        } => {
            let delivered = catch_unwind(AssertUnwindSafe(|| {
                runtime.dispatch_event(js_runtime, target, name, &detail)
            }));
            if let Ok(Err(error)) = delivered {
                notify.send(ToPainter::Engine(EngineEvent::ListenerFailed(
                    error.into_script_error(),
                )));
            }
        }
        ToMain::Resize {
            width,
            height,
            device_pixel_ratio,
        } => runtime.apply_resize(width, height, device_pixel_ratio),
        ToMain::BeginFrame { now, seq } => {
            runtime.begin_frame(now);
            *serviced_begin_frame = Some(seq.max(serviced_begin_frame.unwrap_or(0)));
        }
        ToMain::TimersDue => {}
        ToMain::Refill { offsets } => runtime.refill_scroll_windows(&offsets),
        ToMain::ImageEvents(events) => runtime.apply_image_events(&events),
        ToMain::Attach(_) | ToMain::Close | ToMain::Shutdown => {
            unreachable!("lifecycle ends or attaches before dispatch")
        }
        #[cfg(test)]
        ToMain::Probe(probe) => runtime.with_document(probe),
    }
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
                let error = platform_script_error(format!(
                    "the script Worker aborted after a panic{location}: {}",
                    panic_payload(info.payload())
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

fn platform_script_error(message: String) -> ScriptError {
    ScriptError {
        kind: ScriptErrorKind::Other,
        phase: ScriptErrorPhase::Execute,
        message: Arc::from(message),
        location: None,
    }
}

pub(super) fn panic_payload(payload: &(dyn std::any::Any + Send)) -> &str {
    if let Some(message) = payload.downcast_ref::<String>() {
        message
    } else if let Some(message) = payload.downcast_ref::<&'static str>() {
        message
    } else {
        "non-string panic payload"
    }
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
