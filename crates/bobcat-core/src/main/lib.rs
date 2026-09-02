//! Lynx main-thread ownership, startup, and command rounds.
//!
//! The embedder's own thread starts this owner from `LynxView::new` and keeps
//! the painter itself. This thread creates the document, awaits and applies
//! every startup resource, boots the realm, and then owns both document and
//! realm until shutdown.

mod quickjs;
#[path = "runtime/lib.rs"]
pub(crate) mod runtime;
#[path = "tree/lib.rs"]
pub(crate) mod tree;

#[cfg(all(target_arch = "wasm32", panic = "abort"))]
use std::cell::RefCell;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::str;
use std::sync::Arc;
#[cfg(target_arch = "wasm32")]
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(not(target_arch = "wasm32"))]
use std::thread::Builder as ThreadBuilder;

use dom::CommittedFrame;
#[cfg(target_arch = "wasm32")]
use wasm_thread::Builder as ThreadBuilder;

#[cfg(test)]
use self::runtime::MainThreadError;
use self::runtime::MainThreadRuntime;
use self::tree::{LynxDocument, new_document};
use crate::script::{ScriptError, ScriptErrorKind, ScriptErrorPhase};
use crate::view::{
    EngineError, EngineEvent, EventRequester, FrameHub, LoadedSource, LynxViewError, SourceSlot,
    StyleSheetSource, ToMain, ToPainter, ViewSources, Viewport, frame_slot,
};

pub(crate) struct EntryModule {
    pub(crate) source: String,
    pub(crate) url: String,
}

/// The construction guard's cancellation flag.
///
/// It only prevents work that has not entered synchronous JavaScript yet.
/// Once `QuickJS` is executing, teardown waits for that call and joins the
/// owner thread without trying to interrupt it.
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
static WASM_STYLE_POOL: OnceLock<Result<(), String>> = OnceLock::new();
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

#[cfg(target_arch = "wasm32")]
pub fn configure_wasm_workers(
    worker_script_url: String,
    style_thread_count: usize,
) -> Result<(), EngineError> {
    if worker_script_url.is_empty() {
        return Err(EngineError::Thread {
            name: "wasm worker configuration",
            message: "the worker script URL must not be empty".to_owned(),
        });
    }
    if style_thread_count < 2 {
        return Err(EngineError::Thread {
            name: "wasm worker configuration",
            message: "the style thread count must be at least two so one managed worker remains after the entry task"
                .to_owned(),
        });
    }
    if WASM_STYLE_POOL.get().is_some() {
        return Err(EngineError::Thread {
            name: "wasm worker configuration",
            message: "the style thread pool was already initialized".to_owned(),
        });
    }
    wasm_thread::Builder::empty()
        .worker_script_url(worker_script_url)
        .set_default();
    WASM_STYLE_POOL
        .get_or_init(|| create_wasm_style_pool(style_thread_count))
        .clone()
        .map_err(|message| EngineError::Thread {
            name: "wasm style pool",
            message,
        })
}

/// Starts the Lynx main thread, which creates and initializes its own document
/// before holding the main end of the view's link for the rest of its life.
///
/// Nothing announces its exit: dropping the last `ToPainterSender` closes
/// the notification FIFO, which is the same fact — and the one a painter
/// blocked on a `BeginFrame` is already waiting on.
pub(crate) fn spawn_main_thread<R: EventRequester>(
    viewport: Viewport,
    sources: ViewSources,
    link: MainLink<R>,
) -> Result<MainThreadHome, EngineError> {
    let control = Arc::new(StartupControl::default());
    let main_control = Arc::clone(&control);
    let thread = ThreadBuilder::new()
        .name("bobcat-main".to_owned())
        .spawn(move || run_main_thread(viewport, sources, link, &main_control))
        .map_err(|error| EngineError::Thread {
            name: "script",
            message: error.to_string(),
        })?;
    Ok(MainThreadHome {
        thread: Some(thread),
        control,
    })
}

/// Unit-test seam for paint tests that exercise a prebuilt document. The
/// production startup path above is the only non-test caller and creates the
/// document on `bobcat-main`.
#[cfg(test)]
pub(crate) fn spawn_test_main_thread<R: EventRequester>(
    document: LynxDocument,
    entry: EntryModule,
    link: MainLink<R>,
) -> Result<MainThreadHome, EngineError> {
    let control = Arc::new(StartupControl::default());
    let thread = ThreadBuilder::new()
        .name("bobcat-main".to_owned())
        .spawn(move || {
            let MainLink { commands, notify } = link;
            let result = catch_unwind(AssertUnwindSafe(|| {
                let mut runtime = MainThreadRuntime::new(document, notify.clone())
                    .map_err(MainThreadError::into_script_error)?;
                runtime
                    .run_main_thread_script(&entry.source, &entry.url)
                    .map_err(MainThreadError::into_script_error)?;
                Ok(runtime)
            }))
            .unwrap_or_else(|payload| {
                Err(platform_script_error(format!(
                    "the script realm panicked: {}",
                    panic_payload(payload.as_ref())
                )))
            });
            match result {
                Ok(runtime) => {
                    notify.send(ToPainter::Engine(EngineEvent::ScriptFinished));
                    notify.send(ToPainter::FrameChanged);
                    serve_main_commands(runtime, &commands, &notify);
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

/// What boot is still waiting for.
///
/// One slot per stylesheet in cascade order, so an answer arriving out of
/// order cannot reorder the cascade.
struct PendingSources {
    sheets: Vec<Option<StyleSheetSource>>,
    /// How far the ready prefix has been mounted — also the cascade cursor.
    mounted: usize,
    entry: Option<EntryModule>,
}

impl PendingSources {
    /// Whether every source has arrived. `mount_ready_prefix` runs after each
    /// placement, so this is the fact itself rather than a counter kept
    /// alongside it.
    fn complete(&self) -> bool {
        self.mounted == self.sheets.len() && self.entry.is_some()
    }

    fn place(&mut self, slot: SourceSlot, source: LoadedSource) {
        match (slot, source) {
            (SourceSlot::StyleSheet(index), LoadedSource::StyleSheet(sheet)) => {
                self.sheets[index] = Some(sheet);
            }
            (SourceSlot::Entry, LoadedSource::Entry { source, url }) => {
                self.entry = Some(EntryModule { source, url });
            }
            (slot, _) => unreachable!("the painter answers {slot:?} in kind"),
        }
    }

    /// Mounts every sheet whose turn has come. Parsing CSS here overlaps the
    /// fetches still in flight on the painter.
    fn mount_ready_prefix(&mut self, document: &mut LynxDocument) {
        while let Some(slot) = self.sheets.get_mut(self.mounted)
            && let Some(sheet) = slot.take()
        {
            match sheet {
                StyleSheetSource::Preparsed(sheet) => {
                    crate::style::add_preparsed_style_sheet(document, &sheet);
                }
                StyleSheetSource::Text(css) => crate::style::add_style_sheet_text(document, &css),
            }
            self.mounted += 1;
        }
    }
}

/// Everything between this thread starting and its first served command.
///
/// Synchronous top to bottom. Its only wait is `commands.recv()` — the same
/// one it spends the rest of its life in — so there is no future here and no
/// executor to drive one. Every source it will ever need is asked for before
/// that first park, and afterwards it holds no specifier at all, which is why
/// no later fetch is even constructible.
///
/// `None` means the view was cancelled or the painter is already gone: in
/// both cases nobody is listening for an outcome.
fn boot<R: EventRequester>(
    viewport: Viewport,
    sources: ViewSources,
    commands: &flume::Receiver<ToMain>,
    notify: &ToPainterSender<R>,
    control: &StartupControl,
) -> Option<Result<MainThreadRuntime<R>, LynxViewError>> {
    let ViewSources {
        config,
        fonts,
        default_font_family,
        style_sheets,
        entry,
    } = sources;

    let mut document = new_document(viewport, config);
    for font in fonts {
        document.register_fonts(font);
    }
    if let Some(family) = default_font_family
        && !document.set_default_font_family(&family)
    {
        return Some(Err(EngineError::UnknownFontFamily(family).into()));
    }

    let mut pending = PendingSources {
        sheets: (0..style_sheets.len()).map(|_| None).collect(),
        mounted: 0,
        entry: None,
    };
    for (index, specifier) in style_sheets.into_iter().enumerate() {
        notify.send(ToPainter::FetchSource {
            slot: SourceSlot::StyleSheet(index),
            specifier,
        });
    }
    notify.send(ToPainter::FetchSource {
        slot: SourceSlot::Entry,
        specifier: entry,
    });

    while !pending.complete() {
        // The one park, satisfied by *any* message rather than a particular
        // one — which is what keeps this from being half of a wait cycle.
        let Ok(command) = commands.recv() else {
            return None;
        };
        match command {
            ToMain::Shutdown => return None,
            ToMain::SourceLoaded { slot, source } => {
                // A failed fetch never reaches here: the painter holds that
                // failure and returns from construction with it.
                pending.place(slot, source?);
                pending.mount_ready_prefix(&mut document);
            }
            _ => unreachable!("a view that has not booted has nothing else to apply"),
        }
        if control.is_cancelled() {
            return None;
        }
    }

    let entry = pending
        .entry
        .take()
        .expect("the entry is one of the awaited sources");
    if control.is_cancelled() {
        return None;
    }
    let mut runtime = match MainThreadRuntime::new(document, notify.clone()) {
        Ok(runtime) => runtime,
        Err(error) => return Some(Err(error.into_script_error().into())),
    };
    if control.is_cancelled() {
        return None;
    }
    if let Err(error) = runtime.run_main_thread_script(&entry.source, &entry.url) {
        if control.is_cancelled() {
            return None;
        }
        return Some(Err(error.into_script_error().into()));
    }
    if control.is_cancelled() {
        return None;
    }
    Some(Ok(runtime))
}

fn run_main_thread<R: EventRequester>(
    viewport: Viewport,
    sources: ViewSources,
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

    let booted = catch_unwind(AssertUnwindSafe(|| {
        boot(viewport, sources, &commands, &notify, control)
    }))
    .unwrap_or_else(|payload| {
        Some(Err(EngineError::Thread {
            name: "script",
            message: format!(
                "the Lynx main thread panicked during startup: {}",
                panic_payload(payload.as_ref())
            ),
        }
        .into()))
    });

    match booted {
        Some(Ok(runtime)) => {
            // Boot's outcome and boot's pixels ride one FIFO, in this order:
            // whatever the entry committed reaches the target on the turn
            // that reports the ending, with nobody left to ask for another.
            notify.send(ToPainter::Engine(EngineEvent::ScriptFinished));
            notify.send(ToPainter::FrameChanged);
            notify.send(ToPainter::Started(Ok(())));
            let served = catch_unwind(AssertUnwindSafe(|| {
                serve_main_commands(runtime, &commands, &notify);
            }));
            if let Err(payload) = served {
                notify.send(ToPainter::Engine(EngineEvent::ScriptRunError(
                    platform_script_error(format!(
                        "the Lynx main thread panicked while serving commands: {}",
                        panic_payload(payload.as_ref())
                    )),
                )));
            }
        }
        Some(Err(error)) => notify.send(ToPainter::Started(Err(error))),
        // Cancelled, or the painter is already gone: nobody is listening.
        None => {}
    }

    #[cfg(all(target_arch = "wasm32", panic = "abort"))]
    set_script_panic_reporter(None);
}

fn serve_main_commands<R: EventRequester>(
    mut runtime: MainThreadRuntime<R>,
    commands: &flume::Receiver<ToMain>,
    notify: &ToPainterSender<R>,
) {
    while let Ok(first) = commands.recv() {
        let mut serviced_begin_frame = None;
        for command in std::iter::once(first).chain(commands.drain()) {
            match command {
                ToMain::Shutdown => return,
                command => {
                    apply_main_command(&mut runtime, command, notify, &mut serviced_begin_frame);
                }
            }
        }
        runtime.commit_if_dirty();
        if let Some(seq) = serviced_begin_frame {
            notify.send(ToPainter::BeginFrameServiced(seq));
        }
    }
}

fn apply_main_command<R: EventRequester>(
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
                runtime.dispatch_event(target, name, &detail)
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

#[cfg(target_arch = "wasm32")]
fn create_wasm_style_pool(thread_count: usize) -> Result<(), String> {
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(thread_count)
        .use_current_thread()
        .thread_name(|index| format!("StyleThread#{index}"))
        .start_handler(|_| {
            dom::stylo::thread_state::initialize_layout_worker_thread();
        })
        .stack_size(dom::stylo::parallel::STYLE_THREAD_STACK_SIZE_KB * 1024)
        .spawn_handler(|thread| {
            let mut builder = wasm_thread::Builder::new();
            if let Some(name) = thread.name() {
                builder = builder.name(name.to_owned());
            }
            if let Some(stack_size) = thread.stack_size() {
                builder = builder.stack_size(stack_size);
            }
            builder.spawn(move || thread.run()).map(|_| ())
        })
        .build()
        .map_err(|error| error.to_string())?;
    dom::install_style_thread_pool(pool)
        .map_err(|_| "Stylo's embedder thread pool was installed twice".to_owned())
}
