//! Lynx main-thread ownership, startup, and command rounds.
//!
//! The embedder's own thread starts this owner from `LynxView::new` and keeps
//! the painter itself. This thread creates the document, awaits and applies
//! every startup resource, boots the realm, and then owns both document and
//! realm until shutdown.

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
    EngineError, EngineEvent, EventRequester, FrameHub, LoadedSource, LynxViewError, MainSources,
    StyleSheetSource, StyleThreads, ToMain, ToPainter, Viewport, frame_slot,
};

#[cfg(test)]
pub(crate) struct EntryModule {
    pub(crate) source: String,
    pub(crate) url: String,
}

/// The construction guard's cancellation flag.
///
/// It only prevents work that has not entered synchronous JavaScript yet.
/// Once `QuickJS` is executing, teardown waits for that call and joins the
/// owner thread without trying to interrupt it — on the targets that join
/// at all; see [`MainThreadHome::shutdown`].
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

/// The view-owned right to join `bobcat-main`.
pub(crate) struct MainThreadHome {
    thread: Option<MainJoinHandle>,
    control: Arc<StartupControl>,
}

impl MainThreadHome {
    pub(crate) fn cancel(&self) {
        self.control.cancel();
    }

    pub(crate) fn shutdown(&mut self) {
        if let Some(thread) = self.thread.take() {
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
}

/// The main thread's receiving end of its link to the painter.
pub(crate) struct MainLink<R: EventRequester> {
    /// Commands from the painter, in the order it sent them.
    pub(crate) commands: flume::Receiver<ToMain>,
    /// Everything this thread has to say back.
    pub(crate) notify: ToPainterSender<R>,
}

impl<R: EventRequester> MainLink<R> {
    pub(crate) fn new(commands: flume::Receiver<ToMain>, notify: ToPainterSender<R>) -> Self {
        Self { commands, notify }
    }
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
    static WASM_SCRIPT_PANIC_REPORTER: RefCell<Option<ScriptPanicReporter>> = const {
        RefCell::new(None)
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

/// Starts the Lynx main thread, which creates and initializes its own document
/// before holding the main end of the view's link for the rest of its life.
///
/// Nothing announces its exit: dropping the last `ToPainterSender` closes
/// the notification FIFO, which is the same fact — and the one a painter
/// blocked on a `BeginFrame` is already waiting on.
pub(crate) fn spawn_main_thread<R: EventRequester>(
    viewport: Viewport,
    style_threads: StyleThreads,
    sources: MainSources,
    link: MainLink<R>,
) -> Result<MainThreadHome, EngineError> {
    let control = Arc::new(StartupControl::default());
    let main_control = Arc::clone(&control);
    let thread = ThreadBuilder::new()
        .name("bobcat-main".to_owned())
        .spawn(move || run_main_thread(viewport, style_threads, sources, link, &main_control))
        .map_err(|error| EngineError::Thread {
            name: "script",
            message: error.to_string(),
        })?;
    Ok(MainThreadHome {
        thread: Some(thread),
        control,
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
#[cfg(test)]
pub(crate) fn spawn_test_main_thread<R: EventRequester>(
    build_document: impl FnOnce() -> LynxDocument + Send + 'static,
    entry: EntryModule,
    link: MainLink<R>,
) -> Result<MainThreadHome, EngineError> {
    let control = Arc::new(StartupControl::default());
    let thread = ThreadBuilder::new()
        .name("bobcat-main".to_owned())
        .spawn(move || {
            let MainLink { commands, notify } = link;
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
                    serve_view(
                        &mut js_runtime,
                        ViewSlot::Running(runtime),
                        &commands,
                        &notify,
                        &StartupControl::default(),
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
    Ok(MainThreadHome {
        thread: Some(thread),
        control,
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

fn run_main_thread<R: EventRequester>(
    viewport: Viewport,
    style_threads: StyleThreads,
    sources: MainSources,
    link: MainLink<R>,
    control: &StartupControl,
) {
    let MainLink { commands, notify } = link;
    #[cfg(all(target_arch = "wasm32", panic = "abort"))]
    install_script_panic_hook();
    #[cfg(all(target_arch = "wasm32", panic = "abort"))]
    set_script_panic_reporter(Some({
        let notify = notify.clone();
        Box::new(move |error| {
            notify.send(ToPainter::Engine(EngineEvent::ScriptRunError(error)));
        })
    }));

    // The runtime is the thread's, not the view's: a group opens one realm on
    // it per view, which is why the modules they share are registered here.
    let mut js_runtime =
        match ScriptRuntime::new()
            .map_err(LynxViewError::from)
            .and_then(|mut runtime| {
                install_shared_modules(&mut runtime)
                    .map_err(|error| error.into_script_error().into())
                    .map(|()| runtime)
            }) {
            Ok(runtime) => runtime,
            Err(error) => {
                notify.send(ToPainter::Started(Err(error)));
                return;
            }
        };

    // The pool is the thread's for a harder reason than the runtime is:
    // rayon takes this thread over as index zero and never gives it back, so
    // a second pool built here would be refused outright. Every document this
    // thread carries therefore shares this one — which they may, being unable
    // to traverse at once on the single thread that drives them both. Built
    // before the first source arrives, so starting the workers overlaps the
    // fetches the painter already has in flight.
    let style_pool = match build_style_pool(style_threads.resolve()) {
        Ok(pool) => pool.map(Rc::new),
        Err(error) => {
            notify.send(ToPainter::Started(Err(error.into())));
            return;
        }
    };

    let slot = match Booting::new(viewport, sources, style_pool.as_ref(), notify.clone()) {
        Ok(booting) => ViewSlot::Booting(Box::new(booting)),
        Err(error) => {
            notify.send(ToPainter::Started(Err(error)));
            return;
        }
    };
    let served = catch_unwind(AssertUnwindSafe(|| {
        serve_view(&mut js_runtime, slot, &commands, &notify, control);
    }));
    if let Err(payload) = served {
        notify.send(ToPainter::Engine(EngineEvent::ScriptRunError(
            platform_script_error(format!(
                "the Lynx main thread panicked: {}",
                panic_payload(payload.as_ref())
            )),
        )));
    }
}

/// Drives one view from its first source to the end of its life.
///
/// One park, `commands.recv()`, whether the view is still mounting sources or
/// already serving — which is what lets a thread carrying several views wait
/// once for all of them instead of once per view.
fn serve_view<R: EventRequester>(
    js_runtime: &mut ScriptRuntime,
    slot: ViewSlot<R>,
    commands: &flume::Receiver<ToMain>,
    notify: &ToPainterSender<R>,
    control: &StartupControl,
) {
    let mut slot = slot;
    loop {
        let mut serviced_begin_frame = None;
        // A booting view has no realm, so nothing can have armed a timer on
        // it; only a running one can shorten this wait.
        let deadline = match &mut slot {
            ViewSlot::Booting(_) => None,
            ViewSlot::Running(runtime) => runtime.next_timer_deadline(),
        };
        match wait_for_command(commands, deadline) {
            Woken::Command(first) => {
                for command in std::iter::once(first).chain(commands.drain()) {
                    if matches!(command, ToMain::Shutdown) {
                        return;
                    }
                    slot = match slot {
                        ViewSlot::Booting(booting) => {
                            let ToMain::SourceLoaded { source } = command else {
                                unreachable!("a view that has not booted has nothing else to apply")
                            };
                            match booting.apply(js_runtime, source, control) {
                                Booted::Waiting(booting) => ViewSlot::Booting(booting),
                                Booted::Running(runtime) => {
                                    // Boot's outcome and boot's pixels ride one
                                    // FIFO, in this order: whatever the entry
                                    // committed reaches the target on the turn
                                    // that reports the ending, with nobody left
                                    // to ask for another.
                                    notify.send(ToPainter::Engine(EngineEvent::ScriptFinished));
                                    notify.send(ToPainter::FrameChanged);
                                    notify.send(ToPainter::Started(Ok(())));
                                    ViewSlot::Running(runtime)
                                }
                                Booted::Failed(error) => {
                                    notify.send(ToPainter::Started(Err(error)));
                                    return;
                                }
                                Booted::Gone => return,
                            }
                        }
                        ViewSlot::Running(mut runtime) => {
                            apply_main_command(
                                js_runtime,
                                &mut runtime,
                                command,
                                notify,
                                &mut serviced_begin_frame,
                            );
                            ViewSlot::Running(runtime)
                        }
                    };
                }
            }
            // A deadline the realm asked for, and nothing else to serve.
            Woken::Deadline => {}
            Woken::Disconnected => return,
        }
        // Only a running view has a round tail: a booting one has published
        // nothing and armed nothing.
        if let ViewSlot::Running(runtime) = &mut slot {
            // After this round's commands, because a listener one of them
            // delivered may have cleared a timer that is already due, and on
            // every round rather than only the ones a deadline woke, because a
            // command can arrive while a deadline is already behind us.
            for failure in runtime.run_due_timers(js_runtime) {
                notify.send(ToPainter::Engine(EngineEvent::TimerFailed(failure)));
            }
            runtime.commit_if_dirty();
            if let Some(seq) = serviced_begin_frame {
                notify.send(ToPainter::BeginFrameServiced(seq));
            }
        }
    }
}

/// What ended one round's wait.
enum Woken {
    /// A command arrived; more may be queued behind it.
    Command(ToMain),
    /// The earliest armed timer came due with no command to serve.
    Deadline,
    /// The painter is gone, and nothing more will be asked of this thread.
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
fn wait_for_command(commands: &flume::Receiver<ToMain>, deadline: Option<ClockInstant>) -> Woken {
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
            WASM_SCRIPT_PANIC_REPORTER.with(|reporter| {
                if let Some(reporter) = reporter.borrow().as_ref() {
                    let location = info
                        .location()
                        .map_or_else(String::new, |location| format!(" at {location}"));
                    reporter(platform_script_error(format!(
                        "the script Worker aborted after a panic{location}: {}",
                        panic_payload(info.payload())
                    )));
                }
            });
            previous(info);
        }));
    });
}

#[cfg(all(target_arch = "wasm32", panic = "abort"))]
fn set_script_panic_reporter(reporter: Option<ScriptPanicReporter>) {
    WASM_SCRIPT_PANIC_REPORTER.with(|slot| *slot.borrow_mut() = reporter);
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

/// Builds one document's style pool, on `bobcat-main` — the thread that owns
/// that document, is the only one that will ever flush it, and is index zero
/// of the pool this returns.
///
/// Call it nowhere else. Rayon takes the calling thread over in place, which
/// is what makes a lone view restyle on exactly the threads and with exactly
/// the parallelism it did when every document shared Stylo's global pool: the
/// root closure runs inline on `bobcat-main` and the managed members take over
/// only where a level is wider than the traversal's work unit.
///
/// The takeover is permanent — rayon leaks about 25 KB per pool and refuses a
/// second one on the same thread forever — which is affordable only because
/// `bobcat-main` is created for one view and dies with it.
///
/// `None` asks for no pool at all, and answers `None`: that document traverses
/// on `bobcat-main` with no pool, which is what a machine gets when the pool
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
