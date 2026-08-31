//! Lynx main-thread ownership, startup, and command rounds.
//!
//! The user thread starts this owner and the presenter as peers. This thread
//! creates the document, awaits and applies every startup resource, boots the
//! realm, and then owns both document and realm until shutdown.

mod quickjs;
#[path = "runtime/lib.rs"]
pub(crate) mod runtime;
#[path = "tree/lib.rs"]
pub(crate) mod tree;

#[cfg(all(target_arch = "wasm32", panic = "abort"))]
use std::cell::RefCell;
use std::future::{Future, poll_fn};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::pin::pin;
use std::str;
#[cfg(target_arch = "wasm32")]
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, mpsc};
use std::task::Poll;
#[cfg(not(target_arch = "wasm32"))]
use std::thread::Builder as ThreadBuilder;

use dom::{CommittedFrame, ImageStore};
use http::HeaderMap;
use tokio::sync::oneshot;
#[cfg(target_arch = "wasm32")]
use wasm_thread::Builder as ThreadBuilder;

use self::runtime::{MainThreadError, MainThreadRuntime};
use self::tree::{LynxDocument, PageConfig, new_document};
use crate::resource::{
    CachePolicy, RequestContext, RequestId, ResolveRequest, ResourceDescriptor, ResourceFetcher,
    ResourcePriority, ResourceRequest, StyleSheetPayload,
};
use crate::script::{ScriptError, ScriptErrorKind, ScriptErrorPhase};
use crate::user::ViewSources;
#[cfg(not(target_arch = "wasm32"))]
use crate::view::ToPainter;
use crate::view::{
    EngineError, EngineEvent, EventRequester, FrameHub, LynxViewError, ToMain, ToPresenter,
    Viewport, frame_slot,
};

static NEXT_REQUEST_NAMESPACE: AtomicU64 = AtomicU64::new(1);

pub(crate) struct EntryModule {
    pub(crate) source: String,
    pub(crate) url: String,
}

/// Everything `bobcat-main` needs before it can enter its command loop.
pub(crate) struct StartupRequest {
    config: PageConfig,
    viewport: Viewport,
    sources: ViewSources,
}

impl StartupRequest {
    pub(crate) fn new(config: PageConfig, viewport: Viewport, sources: ViewSources) -> Self {
        Self {
            config,
            viewport,
            sources,
        }
    }
}

/// The only owner copied back out of the initialized document. The document
/// itself is born, booted, served, and destroyed on `bobcat-main`.
pub(crate) struct StartupSuccess {
    pub(crate) image_store: Arc<dyn ImageStore>,
}

pub(crate) type StartupResult = Result<StartupSuccess, LynxViewError>;

#[derive(Default)]
struct StartupState {
    cancelled: bool,
    startup_finished: bool,
    interrupt: Option<quickjs_rust_bridge::InterruptHandle>,
}

/// Cancellation shared with the user-thread construction guard.
///
/// Dropping the startup receiver wakes an async resource wait. Once `QuickJS`
/// exists, this control also interrupts synchronous author code, including an
/// entry module that never returns.
#[derive(Default)]
pub(crate) struct StartupControl {
    state: Mutex<StartupState>,
}

impl StartupControl {
    fn state(&self) -> MutexGuard<'_, StartupState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub(crate) fn cancel(&self) {
        let interrupt = {
            let mut state = self.state();
            state.cancelled = true;
            if state.startup_finished {
                return;
            }
            state.interrupt.clone()
        };
        let Some(interrupt) = interrupt else {
            // The main thread will observe `cancelled` before it starts
            // author code; closing the startup receiver wakes resource IO.
            return;
        };
        while !interrupt.request_interrupt_if_running() {
            if self.state().startup_finished {
                break;
            }
            std::thread::yield_now();
        }
    }

    fn install_interrupt(&self, interrupt: quickjs_rust_bridge::InterruptHandle) -> bool {
        let mut state = self.state();
        state.interrupt = Some(interrupt);
        !state.cancelled
    }

    fn is_cancelled(&self) -> bool {
        self.state().cancelled
    }

    fn finish(&self) {
        let mut state = self.state();
        state.startup_finished = true;
    }

    /// Interrupts owner-thread JavaScript if it is running, or waits until
    /// the main thread has consumed its shutdown command and stopped.
    ///
    /// A one-shot request is not enough: the owner may be between commands
    /// when shutdown begins and enter a queued listener immediately after the
    /// request. Retrying closes that idle-to-execute race without leaving an
    /// interrupt armed for unrelated work.
    fn interrupt_until(&self, mut stopped: impl FnMut() -> bool) {
        let interrupt = self.state().interrupt.clone();
        let Some(interrupt) = interrupt else {
            return;
        };
        while !stopped() && !interrupt.request_interrupt_if_running() {
            std::thread::yield_now();
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
type MainJoinHandle = std::thread::JoinHandle<()>;
#[cfg(target_arch = "wasm32")]
type MainJoinHandle = wasm_thread::JoinHandle<()>;

/// The user-thread-owned right to cancel and join `bobcat-main`.
pub(crate) struct MainThreadHome {
    thread: Option<MainJoinHandle>,
    control: Arc<StartupControl>,
}

impl MainThreadHome {
    pub(crate) fn cancel(&self) {
        self.control.cancel();
    }

    pub(crate) fn shutdown(&mut self) {
        self.cancel();
        if let Some(thread) = self.thread.take() {
            self.control.interrupt_until(|| thread.is_finished());
            let _ = thread.join();
        }
    }
}

/// The main thread's sending end: notification FIFO, newest-frame mailbox,
/// and the wakeup that announces both to the paint thread.
pub(crate) struct ToPresenterSender<R: EventRequester> {
    notifications: mpsc::Sender<ToPresenter>,
    frames: Arc<FrameHub>,
    requester: Arc<R>,
}

// Hand-written: `derive(Clone)` would demand `R: Clone`, while a requester is
// shared through its `Arc` and is never cloned itself.
impl<R: EventRequester> Clone for ToPresenterSender<R> {
    fn clone(&self) -> Self {
        Self {
            notifications: self.notifications.clone(),
            frames: Arc::clone(&self.frames),
            requester: Arc::clone(&self.requester),
        }
    }
}

impl<R: EventRequester> ToPresenterSender<R> {
    pub(crate) fn new(
        notifications: mpsc::Sender<ToPresenter>,
        frames: Arc<FrameHub>,
        requester: Arc<R>,
    ) -> Self {
        Self {
            notifications,
            frames,
            requester,
        }
    }

    /// Announces one notification, then wakes the paint thread.
    pub(crate) fn send(&self, notification: ToPresenter) {
        if self.notifications.send(notification).is_ok() {
            self.requester.request_event();
        }
    }

    /// Replaces the newest-frame mailbox, then announces it.
    pub(crate) fn publish_frame(&self, frame: Arc<CommittedFrame>) {
        *frame_slot(&self.frames) = Some(frame);
        self.send(ToPresenter::FrameChanged);
    }
}

/// The main thread's receiving end of its link to the painter.
pub(crate) struct MainLink<R: EventRequester> {
    /// Commands from the paint thread, in the order it sent them.
    pub(crate) commands: mpsc::Receiver<ToMain>,
    /// Everything this thread has to say back.
    pub(crate) notify: ToPresenterSender<R>,
}

impl<R: EventRequester> MainLink<R> {
    pub(crate) fn new(commands: mpsc::Receiver<ToMain>, notify: ToPresenterSender<R>) -> Self {
        Self { commands, notify }
    }
}

/// The wakeup owned by the main thread: one command in the paint inbox.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) struct InboxWakeup(Mutex<mpsc::Sender<ToPainter>>);

#[cfg(not(target_arch = "wasm32"))]
impl InboxWakeup {
    pub(crate) fn new(commands: mpsc::Sender<ToPainter>) -> Self {
        Self(Mutex::new(commands))
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl EventRequester for InboxWakeup {
    fn request_event(&self) {
        let _ = self
            .0
            .lock()
            .unwrap_or_else(|error| panic!("the presenter inbox is poisoned: {error}"))
            .send(ToPainter::MainChanged);
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl std::fmt::Debug for InboxWakeup {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("InboxWakeup")
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
/// Nothing announces its exit: dropping the last `ToPresenterSender` closes
/// the notification FIFO, which is the same fact — and the one a presenting
/// side blocked on a `BeginFrame` is already waiting on.
pub(crate) fn spawn_main_thread<R: EventRequester>(
    startup: StartupRequest,
    link: MainLink<R>,
    result: oneshot::Sender<StartupResult>,
) -> Result<MainThreadHome, EngineError> {
    let control = Arc::new(StartupControl::default());
    let main_control = Arc::clone(&control);
    let thread = ThreadBuilder::new()
        .name("bobcat-main".to_owned())
        .spawn(move || run_main_thread(startup, link, result, &main_control))
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
    let main_control = Arc::clone(&control);
    let thread = ThreadBuilder::new()
        .name("bobcat-main".to_owned())
        .spawn(move || {
            let MainLink { commands, notify } = link;
            let result = catch_unwind(AssertUnwindSafe(|| {
                let mut runtime = MainThreadRuntime::new(document, notify.clone())
                    .map_err(MainThreadError::into_script_error)?;
                if !main_control.install_interrupt(runtime.interrupt_handle()) {
                    return Ok(None);
                }
                runtime
                    .run_main_thread_script(&entry.source, &entry.url)
                    .map_err(MainThreadError::into_script_error)?;
                Ok(Some(runtime))
            }))
            .unwrap_or_else(|payload| {
                Err(platform_script_error(format!(
                    "the script realm panicked: {}",
                    panic_payload(payload.as_ref())
                )))
            });
            main_control.finish();
            match result {
                Ok(Some(runtime)) => {
                    notify.send(ToPresenter::Engine(EngineEvent::ScriptFinished));
                    notify.send(ToPresenter::FrameChanged);
                    serve_main_commands(runtime, &commands, &notify, &main_control);
                }
                Ok(None) => {}
                Err(error) => {
                    notify.send(ToPresenter::Engine(EngineEvent::ScriptRunError(error)));
                    notify.send(ToPresenter::FrameChanged);
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

struct StartupReady<R: EventRequester> {
    runtime: MainThreadRuntime<R>,
    success: StartupSuccess,
}

type StartupOutcome<R> = Option<StartupReady<R>>;

fn run_main_thread<R: EventRequester>(
    startup: StartupRequest,
    link: MainLink<R>,
    mut started: oneshot::Sender<StartupResult>,
    control: &StartupControl,
) {
    let MainLink { commands, notify } = link;
    #[cfg(all(target_arch = "wasm32", panic = "abort"))]
    install_script_panic_hook();
    #[cfg(all(target_arch = "wasm32", panic = "abort"))]
    set_script_panic_reporter(Some({
        let notify = notify.clone();
        Box::new(move |error| {
            notify.send(ToPresenter::Engine(EngineEvent::ScriptRunError(error)));
        })
    }));

    let initialized = catch_unwind(AssertUnwindSafe(|| {
        pollster::block_on(until_startup_cancelled(
            initialize(startup, notify.clone(), control),
            &mut started,
        ))
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
    control.finish();

    if let Some(Ok(Some(StartupReady { runtime, success }))) = initialized {
        notify.send(ToPresenter::Engine(EngineEvent::ScriptFinished));
        // Boot's outcome and boot's pixels ride one wakeup: whatever the
        // entry committed before it ended reaches the target on the turn
        // that reports the ending, with nobody left to ask for another.
        notify.send(ToPresenter::FrameChanged);
        if started.send(Ok(success)).is_ok() {
            let served = catch_unwind(AssertUnwindSafe(|| {
                serve_main_commands(runtime, &commands, &notify, control);
            }));
            if let Err(payload) = served {
                notify.send(ToPresenter::Engine(EngineEvent::ScriptRunError(
                    platform_script_error(format!(
                        "the Lynx main thread panicked while serving commands: {}",
                        panic_payload(payload.as_ref())
                    )),
                )));
            }
        }
    } else if let Some(Err(error)) = initialized {
        let _ = started.send(Err(error));
    }

    #[cfg(all(target_arch = "wasm32", panic = "abort"))]
    set_script_panic_reporter(None);
}

async fn until_startup_cancelled<T>(
    future: impl Future<Output = T>,
    started: &mut oneshot::Sender<StartupResult>,
) -> Option<T> {
    let mut future = pin!(future);
    let mut cancelled = pin!(started.closed());
    poll_fn(|context| {
        if cancelled.as_mut().poll(context).is_ready() {
            Poll::Ready(None)
        } else {
            future.as_mut().poll(context).map(Some)
        }
    })
    .await
}

async fn initialize<R: EventRequester>(
    startup: StartupRequest,
    notify: ToPresenterSender<R>,
    control: &StartupControl,
) -> Result<StartupOutcome<R>, LynxViewError> {
    let StartupRequest {
        config,
        viewport,
        sources,
    } = startup;
    let ViewSources {
        resource_fetcher,
        fonts,
        default_font_family,
        image_store,
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
        return Err(EngineError::UnknownFontFamily(family).into());
    }
    if let Some(store) = image_store {
        document.set_image_store(store);
    }

    let mut requests = RequestId {
        namespace: NEXT_REQUEST_NAMESPACE.fetch_add(1, Ordering::Relaxed),
        sequence: 0,
    };
    for url in &style_sheets {
        mount_style_sheet(resource_fetcher.as_ref(), &mut requests, url, &mut document).await?;
    }
    let entry = fetch_entry(resource_fetcher.as_ref(), &mut requests, &entry).await?;
    let image_store = Arc::clone(document.image_store());
    let mut runtime =
        MainThreadRuntime::new(document, notify).map_err(MainThreadError::into_script_error)?;
    if !control.install_interrupt(runtime.interrupt_handle()) {
        return Ok(None);
    }
    if let Err(error) = runtime.run_main_thread_script(&entry.source, &entry.url) {
        if control.is_cancelled() {
            return Ok(None);
        }
        return Err(error.into_script_error().into());
    }
    if control.is_cancelled() {
        return Ok(None);
    }
    Ok(Some(StartupReady {
        runtime,
        success: StartupSuccess { image_store },
    }))
}

async fn mount_style_sheet(
    fetcher: &dyn ResourceFetcher,
    requests: &mut RequestId,
    url: &str,
    document: &mut LynxDocument,
) -> Result<(), LynxViewError> {
    let (request, source_name) = resolve_for_fetch(fetcher, requests, url).await?;
    match fetcher.fetch_style_sheet(request).await?.payload {
        StyleSheetPayload::Preparsed(sheet) => {
            crate::style::add_preparsed_style_sheet(document, &sheet);
        }
        StyleSheetPayload::Text(bytes) => {
            let css = str::from_utf8(&bytes).map_err(|error| {
                LynxViewError::InvalidStyleSheetEncoding {
                    url: source_name,
                    message: error.to_string(),
                }
            })?;
            crate::style::add_style_sheet_text(document, css);
        }
    }
    Ok(())
}

async fn fetch_entry(
    fetcher: &dyn ResourceFetcher,
    requests: &mut RequestId,
    url: &str,
) -> Result<EntryModule, LynxViewError> {
    let (request, url) = resolve_for_fetch(fetcher, requests, url).await?;
    let response = fetcher.fetch_resource(request).await?;
    let source = str::from_utf8(&response.bytes)
        .map_err(|error| LynxViewError::InvalidScriptEncoding {
            url: url.clone(),
            message: error.to_string(),
        })?
        .to_owned();
    Ok(EntryModule { source, url })
}

async fn resolve_for_fetch(
    fetcher: &dyn ResourceFetcher,
    requests: &mut RequestId,
    url: &str,
) -> Result<(ResourceRequest, String), LynxViewError> {
    let context = RequestContext {
        id: *requests,
        priority: ResourcePriority::Critical,
    };
    requests.sequence += 1;
    let resolved = fetcher
        .resolve_locator(ResolveRequest {
            context: context.clone(),
            resource: ResourceDescriptor {
                specifier: Arc::from(url),
                base_url: None,
            },
            percent_decode: false,
        })
        .await?;
    let source_name = resolved.url.to_string();
    Ok((
        ResourceRequest {
            context,
            resource: resolved,
            headers: HeaderMap::new(),
            cache_policy: CachePolicy::Default,
        },
        source_name,
    ))
}

fn serve_main_commands<R: EventRequester>(
    mut runtime: MainThreadRuntime<R>,
    commands: &mpsc::Receiver<ToMain>,
    notify: &ToPresenterSender<R>,
    control: &StartupControl,
) {
    while let Ok(first) = commands.recv() {
        if control.is_cancelled() {
            return;
        }
        let mut serviced_begin_frame = None;
        let mut command = Some(first);
        while let Some(current) = command.take() {
            if control.is_cancelled() {
                return;
            }
            match current {
                ToMain::Shutdown => return,
                current => {
                    apply_main_command(&mut runtime, current, notify, &mut serviced_begin_frame);
                }
            }
            command = commands.try_recv().ok();
        }
        runtime.commit_if_dirty();
        if let Some(seq) = serviced_begin_frame {
            notify.send(ToPresenter::BeginFrameServiced(seq));
        }
    }
}

fn apply_main_command<R: EventRequester>(
    runtime: &mut MainThreadRuntime<R>,
    command: ToMain,
    notify: &ToPresenterSender<R>,
    serviced_begin_frame: &mut Option<u64>,
) {
    match command {
        ToMain::DispatchEvent {
            target,
            name,
            detail,
        } => {
            let delivered = catch_unwind(AssertUnwindSafe(|| {
                runtime.dispatch_event(target, name, &detail)
            }));
            if let Ok(Err(error)) = delivered {
                notify.send(ToPresenter::Engine(EngineEvent::ListenerFailed(
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
        ToMain::NoteImagesChanged => runtime.note_images_changed(),
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

#[cfg(test)]
mod startup_tests {
    use std::sync::{Arc, mpsc};
    use std::time::Duration;

    use super::StartupControl;
    use crate::main::quickjs::ScriptEngine;

    #[test]
    fn cancellation_interrupts_script_even_in_the_install_to_execute_race() {
        let control = Arc::new(StartupControl::default());
        let main_control = Arc::clone(&control);
        let (installed, ready) = mpsc::channel();
        let (finished, outcome) = mpsc::channel();
        let worker = std::thread::Builder::new()
            .name("bobcat-main-cancel-test".to_owned())
            .spawn(move || {
                let mut engine = ScriptEngine::new().expect("QuickJS starts");
                assert!(main_control.install_interrupt(engine.interrupt_handle()));
                installed.send(()).expect("publish installed handle");
                let result = engine.execute_module("for (;;) {}", "test:///endless.mjs");
                main_control.finish();
                let _ = finished.send(result);
            })
            .expect("test main thread starts");

        ready
            .recv_timeout(Duration::from_secs(10))
            .expect("the interrupt handle is installed");
        control.cancel();
        let error = outcome
            .recv_timeout(Duration::from_secs(10))
            .expect("cancelled JavaScript returns")
            .expect_err("the endless entry is interrupted");
        assert!(
            error.message.contains("interrupted"),
            "cancellation is reported as an interrupt: {error}"
        );
        worker.join().expect("test main thread exits");
    }

    #[test]
    fn shutdown_interrupts_javascript_started_after_startup() {
        let control = Arc::new(StartupControl::default());
        let main_control = Arc::clone(&control);
        let (installed, ready) = mpsc::channel();
        let (finished, outcome) = mpsc::channel();
        let worker = std::thread::Builder::new()
            .name("bobcat-main-shutdown-test".to_owned())
            .spawn(move || {
                let mut engine = ScriptEngine::new().expect("QuickJS starts");
                assert!(main_control.install_interrupt(engine.interrupt_handle()));
                main_control.finish();
                installed.send(()).expect("publish installed handle");
                let result = engine.execute_module("for (;;) {}", "test:///endless.mjs");
                let _ = finished.send(result);
            })
            .expect("test main thread starts");

        ready
            .recv_timeout(Duration::from_secs(10))
            .expect("the post-startup runtime is ready");
        let mut home = super::MainThreadHome {
            thread: Some(worker),
            control,
        };
        home.shutdown();
        let error = outcome
            .recv_timeout(Duration::from_secs(10))
            .expect("shutdown JavaScript returns")
            .expect_err("the endless listener is interrupted");
        assert!(
            error.message.contains("interrupted"),
            "shutdown is reported as an interrupt: {error}"
        );
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
