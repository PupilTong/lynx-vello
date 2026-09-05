//! Lynx main-thread ownership, startup, and command rounds.
//!
//! The embedder's own thread starts this owner from `LynxGroup::new` and keeps
//! every painter itself. This thread builds the script runtime and the style
//! pool the group shares, then adopts one view at a time: it creates each
//! document, applies every startup source pushed to it, boots each realm, and
//! owns document and realm until that view is released or the group is.

pub(crate) mod quickjs;
#[path = "runtime/lib.rs"]
pub(crate) mod runtime;
#[path = "tree/lib.rs"]
pub(crate) mod tree;

#[cfg(all(target_arch = "wasm32", panic = "abort"))]
use std::cell::RefCell;
use std::future::Future;
use std::num::NonZeroUsize;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::pin::Pin;
use std::rc::Rc;
use std::sync::Arc;
#[cfg(target_arch = "wasm32")]
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::{Context, Poll, Wake, Waker};
#[cfg(not(target_arch = "wasm32"))]
use std::thread::Builder as ThreadBuilder;
use std::{str, thread};

use dom::{CommittedFrame, StylePool};
#[cfg(target_arch = "wasm32")]
use wasm_thread::Builder as ThreadBuilder;

use self::quickjs::ScriptRuntime;
#[cfg(test)]
use self::runtime::MainThreadError;
use self::runtime::{ClockInstant, MainThreadRuntime, install_shared_modules};
use self::tree::{LynxDocument, new_document};
use crate::script::{ScriptError, ScriptErrorKind, ScriptErrorPhase};
use crate::view::{
    Attachment, EngineError, EngineEvent, EventRequester, FrameHub, GroupCommand, LoadedSource,
    LynxViewError, MainSources, StyleSheetSource, StyleThreads, ToMain, ToPainter, ViewId,
    Viewport, frame_slot,
};
#[cfg(test)]
use crate::view::{DETACHED_VIEW, DetachedLink};

#[cfg(test)]
pub(crate) struct EntryModule {
    pub(crate) source: String,
    pub(crate) url: String,
}

/// One view's construction cancellation flag.
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

    fn is_cancelled(&self) -> bool {
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
    notifications: flume::Sender<ToPainter>,
    frames: Arc<FrameHub>,
    requester: Arc<R>,
}

// Hand-written: `derive(Clone)` would demand `R: Clone`, while a requester is
// shared through its `Arc` and is never cloned itself.
impl<R: EventRequester> Clone for ToPainterSender<R> {
    fn clone(&self) -> Self {
        Self {
            notifications: self.notifications.clone(),
            frames: Arc::clone(&self.frames),
            requester: Arc::clone(&self.requester),
        }
    }
}

impl<R: EventRequester> ToPainterSender<R> {
    pub(crate) fn new(
        notifications: flume::Sender<ToPainter>,
        frames: Arc<FrameHub>,
        requester: Arc<R>,
    ) -> Self {
        Self {
            notifications,
            frames,
            requester,
        }
    }

    /// Announces one notification, then wakes the thread that paints.
    ///
    /// One wake, not two: the notification is already queued, and the painter
    /// waits on that queue directly — during construction by awaiting it, and
    /// afterwards on the host's own turns, which this requester asks for.
    pub(crate) fn send(&self, notification: ToPainter) {
        if self.notifications.send(notification).is_ok() {
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
    pub(crate) fn requester(&self) -> &Arc<R> {
        &self.requester
    }
}

/// The main thread's end of its group's link.
pub(crate) struct GroupLink<R: EventRequester> {
    /// Every view's commands, and every attachment, in the order they were
    /// sent.
    pub(crate) commands: flume::Receiver<GroupCommand>,
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
            let result = catch_unwind(AssertUnwindSafe(|| {
                let mut js_runtime = ScriptRuntime::new()?;
                install_shared_modules(&mut js_runtime)
                    .map_err(MainThreadError::into_script_error)?;
                let mut runtime =
                    MainThreadRuntime::new(&mut js_runtime, build_document(), notify.clone())
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
                    serve_group(&mut js_runtime, None, &requester, &commands, &mut views);
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
    document: LynxDocument,
    notify: ToPainterSender<R>,
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
        style_pool: Option<&Rc<StylePool>>,
        notify: ToPainterSender<R>,
    ) -> Result<Self, LynxViewError> {
        let MainSources {
            config,
            fonts,
            default_font_family,
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
        Ok(Self { document, notify })
    }

    /// Applies one pushed source. The entry's arrival is what ends the wait:
    /// the painter sends stylesheets in cascade order with the entry last, so
    /// mounting in arrival order *is* the cascade.
    fn apply(
        mut self: Box<Self>,
        js_runtime: &mut ScriptRuntime,
        source: LoadedSource,
        control: &StartupControl,
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
        Booted::Waiting(self)
    }

    fn run_entry(
        self: Box<Self>,
        js_runtime: &mut ScriptRuntime,
        source: &str,
        url: &str,
        control: &StartupControl,
    ) -> Booted<R> {
        if control.is_cancelled() {
            return Booted::Gone;
        }
        let Self { document, notify } = *self;
        let mut runtime = match MainThreadRuntime::new(js_runtime, document, notify) {
            Ok(runtime) => runtime,
            Err(error) => return Booted::Failed(error.into_script_error().into()),
        };
        if control.is_cancelled() {
            return Booted::Gone;
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
        commands,
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
    // Both are built before the first view attaches, so starting the workers
    // overlaps the fetches that view's painter already has in flight.
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
    let mut views = Vec::new();
    let served = catch_unwind(AssertUnwindSafe(|| {
        serve_group(
            &mut js_runtime,
            style_pool.as_ref(),
            &requester,
            &commands,
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
/// and the flag its embedder can still cancel its construction with.
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
            slot: Some(slot),
            notify,
            control,
            serviced_begin_frame: None,
        }
    }

    /// A booting view has no realm, so nothing can have armed a timer on it;
    /// only a running one can shorten its group's wait.
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
        if matches!(command, ToMain::Shutdown) {
            return false;
        }
        // A running view is served in place: everything after boot is the
        // common case, and nothing about it needs the slot by value.
        if let Some(ViewSlot::Running(runtime)) = self.slot.as_mut() {
            apply_main_command(
                js_runtime,
                runtime,
                command,
                &self.notify,
                &mut self.serviced_begin_frame,
            );
            return true;
        }
        let Some(ViewSlot::Booting(booting)) = self.slot.take() else {
            unreachable!("a carried view holds its slot between commands")
        };
        let ToMain::SourceLoaded { source } = command else {
            unreachable!("a view that has not booted has nothing else to apply")
        };
        match booting.apply(js_runtime, source, &self.control) {
            Booted::Waiting(booting) => self.slot = Some(ViewSlot::Booting(booting)),
            Booted::Running(runtime) => {
                // Boot's outcome and boot's pixels ride one FIFO, in this
                // order: whatever the entry committed reaches the target on
                // the turn that reports the ending, with nobody left to ask
                // for another.
                self.notify
                    .send(ToPainter::Engine(EngineEvent::ScriptFinished));
                self.notify.send(ToPainter::FrameChanged);
                self.notify.send(ToPainter::Started(Ok(())));
                self.slot = Some(ViewSlot::Running(runtime));
            }
            Booted::Failed(error) => {
                self.notify.send(ToPainter::Started(Err(error)));
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
        // every round rather than only the ones a deadline woke, because a
        // command can arrive while a deadline is already behind us.
        for failure in runtime.run_due_timers(js_runtime) {
            self.notify
                .send(ToPainter::Engine(EngineEvent::TimerFailed(failure)));
        }
        runtime.commit_if_dirty();
        if let Some(seq) = self.serviced_begin_frame.take() {
            self.notify.send(ToPainter::BeginFrameServiced(seq));
        }
    }
}

/// Adopts one view, on the runtime and pool this thread already holds.
fn attach<R: EventRequester>(
    views: &mut Vec<CarriedView<R>>,
    style_pool: Option<&Rc<StylePool>>,
    requester: &Arc<R>,
    attachment: Attachment,
) {
    let Attachment {
        view,
        viewport,
        sources,
        notifications,
        frames,
        control,
    } = attachment;
    let notify = ToPainterSender::new(notifications, frames, Arc::clone(requester));
    #[cfg(all(target_arch = "wasm32", panic = "abort"))]
    add_script_panic_reporter({
        let notify = notify.clone();
        Box::new(move |error| {
            notify.send(ToPainter::Engine(EngineEvent::ScriptRunError(error)));
        })
    });
    match Booting::new(viewport, sources, style_pool, notify.clone()) {
        Ok(booting) => views.push(CarriedView::new(
            view,
            ViewSlot::Booting(Box::new(booting)),
            notify,
            control,
        )),
        Err(error) => notify.send(ToPainter::Started(Err(error))),
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
    commands: &flume::Receiver<GroupCommand>,
    views: &mut Vec<CarriedView<R>>,
) {
    loop {
        // The earliest deadline any view armed: the thread wakes for whichever
        // realm needs it first, and the round's tail runs every view's timers.
        let deadline = views
            .iter_mut()
            .filter_map(CarriedView::next_timer_deadline)
            .min();
        match wait_for_command(commands, deadline) {
            Woken::Command(first) => {
                for message in std::iter::once(first).chain(commands.drain()) {
                    match message {
                        GroupCommand::Close => return,
                        GroupCommand::Attach(attachment) => {
                            attach(views, style_pool, requester, *attachment);
                        }
                        GroupCommand::View { view, command } => {
                            let Some(index) = views.iter().position(|carried| carried.id == view)
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
            // A deadline a realm asked for, and nothing else to serve.
            Woken::Deadline => {}
            Woken::Disconnected => return,
        }
        for view in &mut *views {
            view.finish_round(js_runtime);
        }
    }
}

/// What ended one round's wait.
enum Woken {
    /// A message arrived; more may be queued behind it.
    Command(GroupCommand),
    /// The earliest armed timer came due with no command to serve.
    Deadline,
    /// Every painter and the group handle are gone, and nothing more will be
    /// asked of this thread.
    Disconnected,
}

/// Waits for the next command, or until `deadline` when a timer names one.
///
/// `flume`'s own timed receive reads the standard library's clock, which
/// wasm32 does not implement, so the wait is assembled here out of the two
/// pieces both targets do have: the receiver's future, and `park_timeout` —
/// which is exactly what `flume` blocks on itself. Nothing drives the future
/// but this loop, and the waker only unparks this thread, so this is a
/// blocking wait spelled with a future rather than an executor.
fn wait_for_command(
    commands: &flume::Receiver<GroupCommand>,
    deadline: Option<ClockInstant>,
) -> Woken {
    let Some(deadline) = deadline else {
        return commands.recv().map_or(Woken::Disconnected, Woken::Command);
    };
    let waker = Waker::from(Arc::new(UnparkWaker(thread::current())));
    let mut context = Context::from_waker(&waker);
    let mut receiving = commands.recv_async();
    loop {
        match Pin::new(&mut receiving).poll(&mut context) {
            Poll::Ready(Ok(command)) => return Woken::Command(command),
            Poll::Ready(Err(flume::RecvError::Disconnected)) => return Woken::Disconnected,
            Poll::Pending => {}
        }
        let Some(remaining) = deadline.checked_duration_since(ClockInstant::now()) else {
            return Woken::Deadline;
        };
        // A spurious wake just polls again; a real one has already queued the
        // command the poll will find.
        thread::park_timeout(remaining);
    }
}

/// The waker [`wait_for_command`] hands the receiver: the only thing a send
/// has to do is end this thread's park.
struct UnparkWaker(thread::Thread);

impl Wake for UnparkWaker {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.0.unpark();
    }
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
            unreachable!("sources are answered once, before boot returns")
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
        ToMain::Refill { offsets } => runtime.refill_scroll_windows(&offsets),
        ToMain::ImageEvents(events) => runtime.apply_image_events(&events),
        ToMain::Shutdown => unreachable!("shutdown ends the command loop before dispatch"),
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
