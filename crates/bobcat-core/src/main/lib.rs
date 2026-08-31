//! Lynx main-thread ownership, startup, and command rounds.
//!
//! The thread is started from the presenter, which owns the other end of its
//! link for the rest of the view's life.

mod quickjs;
#[path = "runtime/lib.rs"]
pub(crate) mod runtime;
#[path = "tree/lib.rs"]
pub(crate) mod tree;

#[cfg(all(target_arch = "wasm32", panic = "abort"))]
use std::cell::RefCell;
use std::panic::{AssertUnwindSafe, catch_unwind};
#[cfg(not(target_arch = "wasm32"))]
use std::sync::Mutex;
#[cfg(target_arch = "wasm32")]
use std::sync::OnceLock;
use std::sync::{Arc, mpsc};
#[cfg(not(target_arch = "wasm32"))]
use std::thread::Builder as ThreadBuilder;

use dom::CommittedFrame;
#[cfg(target_arch = "wasm32")]
use wasm_thread::Builder as ThreadBuilder;

use self::runtime::{MainThreadError, MainThreadRuntime};
use self::tree::LynxDocument;
use crate::script::{ScriptError, ScriptErrorKind, ScriptErrorPhase};
#[cfg(not(target_arch = "wasm32"))]
use crate::view::ToPainter;
use crate::view::{
    EngineError, EngineEvent, EntryModule, EventRequester, FrameHub, ToMain, ToPresenter,
    frame_slot,
};

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

/// Starts the Lynx main thread over `document`, holding the main end of the
/// view's link for the rest of its life.
///
/// Nothing announces its exit: dropping the last `ToPresenterSender` closes
/// the notification FIFO, which is the same fact — and the one a presenting
/// side blocked on a `BeginFrame` is already waiting on.
pub(crate) fn spawn_main_thread<R: EventRequester>(
    document: LynxDocument,
    entry: EntryModule,
    link: MainLink<R>,
) -> Result<(), EngineError> {
    ThreadBuilder::new()
        .name("bobcat-main".to_owned())
        .spawn(move || {
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
            let runtime = match result {
                Ok(runtime) => {
                    notify.send(ToPresenter::Engine(EngineEvent::ScriptFinished));
                    Some(runtime)
                }
                Err(error) => {
                    notify.send(ToPresenter::Engine(EngineEvent::ScriptRunError(error)));
                    None
                }
            };
            // Boot's outcome and boot's pixels ride one wakeup: whatever the
            // entry committed before it ended reaches the target on the turn
            // that reports the ending, with nobody left to ask for another.
            notify.send(ToPresenter::FrameChanged);

            if let Some(runtime) = runtime {
                let served = catch_unwind(AssertUnwindSafe(|| {
                    serve_main_commands(runtime, &commands, &notify);
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
            #[cfg(all(target_arch = "wasm32", panic = "abort"))]
            set_script_panic_reporter(None);
        })
        .map(|_thread| ())
        .map_err(|error| EngineError::Thread {
            name: "script",
            message: error.to_string(),
        })
}

fn serve_main_commands<R: EventRequester>(
    mut runtime: MainThreadRuntime<R>,
    commands: &mpsc::Receiver<ToMain>,
    notify: &ToPresenterSender<R>,
) {
    while let Ok(first) = commands.recv() {
        let mut serviced_begin_frame = None;
        let mut command = Some(first);
        while let Some(current) = command.take() {
            apply_main_command(&mut runtime, current, notify, &mut serviced_begin_frame);
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
