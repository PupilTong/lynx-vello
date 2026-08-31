//! Lynx main-thread startup and its ordered command protocol.

#[cfg(all(target_arch = "wasm32", panic = "abort"))]
use std::cell::RefCell;
use std::marker::PhantomData;
use std::panic::{AssertUnwindSafe, catch_unwind};
#[cfg(target_arch = "wasm32")]
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
#[cfg(not(target_arch = "wasm32"))]
use std::thread::Builder as ThreadBuilder;
#[cfg(test)]
use std::time::Duration;

#[cfg(test)]
use dom::CommittedFrame;
use dom::vello::Scene;
use dom::{ImageStore, NodeId, Vector2D};
#[cfg(target_arch = "wasm32")]
use wasm_thread::Builder as ThreadBuilder;

use super::loading::EntryModule;
use super::presenter::ScrollIntents;
use super::{EngineError, EngineEvent, EventRequester, FrameSize, LynxView, Output, Window};
use crate::clock::FrameClock;
use crate::gesture::GestureRouter;
use crate::pipeline::{FrameHub, ListenerNames, ListenerUpdate};
use crate::runtime::{MainThreadError, MainThreadRuntime};
use crate::script::{ScriptError, ScriptErrorKind, ScriptErrorPhase};
use crate::tree::{LynxDocument, Viewport};

#[cfg(target_arch = "wasm32")]
static WASM_STYLE_POOL: OnceLock<Result<(), String>> = OnceLock::new();
#[cfg(all(target_arch = "wasm32", panic = "abort"))]
static WASM_SCRIPT_PANIC_HOOK: OnceLock<()> = OnceLock::new();

#[cfg(all(target_arch = "wasm32", panic = "abort"))]
thread_local! {
    static WASM_SCRIPT_PANIC_REPORTER: RefCell<Option<EngineEventSender>> = const {
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

#[derive(Clone)]
pub(super) struct FrameWakeup {
    pending: Arc<AtomicBool>,
    requester: Arc<dyn EventRequester>,
}

impl FrameWakeup {
    fn new(requester: Arc<dyn EventRequester>) -> Self {
        Self {
            pending: Arc::new(AtomicBool::new(false)),
            requester,
        }
    }

    pub(super) fn request(&self) {
        // The event-loop wake must never overtake the state it announces.
        self.pending.store(true, Ordering::Release);
        self.requester.request_event();
    }

    pub(super) fn take(&self) -> bool {
        self.pending.swap(false, Ordering::AcqRel)
    }
}

#[derive(Clone)]
pub(super) struct EngineEventSender {
    sender: mpsc::Sender<EngineEvent>,
    requester: Arc<dyn EventRequester>,
}

impl EngineEventSender {
    fn send(&self, event: EngineEvent) {
        // Enqueue before waking so pump observes the announced event.
        if self.sender.send(event).is_ok() {
            self.requester.request_event();
        }
    }
}

pub(crate) enum MainCommand {
    DispatchEvent {
        target: NodeId,
        name: &'static str,
        detail: String,
    },
    Resize {
        width: f32,
        height: f32,
        device_pixel_ratio: f32,
    },
    BeginFrame {
        now: f64,
        seq: u64,
    },
    Refill {
        offsets: Vec<(NodeId, Vector2D<f32>)>,
    },
    NoteImagesChanged,
    #[cfg(test)]
    Probe(Box<dyn FnOnce(&mut LynxDocument) + Send>),
}

impl<W: Window> LynxView<'_, W> {
    pub(super) fn start(
        document: LynxDocument,
        viewport: Viewport,
        frame_size: FrameSize,
        event_requester: Arc<dyn EventRequester>,
        entry: EntryModule,
    ) -> Result<Self, EngineError> {
        let image_store = Arc::clone(document.image_store());
        let (view, receiver, events, listener_updates) =
            Self::with_channel(image_store, viewport, frame_size, event_requester);
        spawn_main_thread(
            document,
            entry,
            receiver,
            Arc::clone(&view.hub),
            listener_updates,
            view.frames.clone(),
            events,
        )?;
        #[cfg(test)]
        let view = {
            let mut view = view;
            view.detached = false;
            view
        };
        Ok(view)
    }

    pub(super) fn with_channel(
        image_store: Arc<dyn ImageStore>,
        viewport: Viewport,
        frame_size: FrameSize,
        event_requester: Arc<dyn EventRequester>,
    ) -> (
        Self,
        mpsc::Receiver<MainCommand>,
        EngineEventSender,
        mpsc::Sender<ListenerUpdate>,
    ) {
        let (message_sender, messages) = mpsc::channel();
        let (commands, command_receiver) = mpsc::channel();
        let (listener_updates, listener_names) = mpsc::channel();
        let frames = FrameWakeup::new(Arc::clone(&event_requester));
        let view = Self {
            commands,
            #[cfg(test)]
            detached: true,
            hub: Arc::new(FrameHub::default()),
            image_store,
            viewport,
            frame_size,
            messages,
            output: Output::None,
            frames,
            gesture: GestureRouter::default(),
            listener_names: ListenerNames::new(listener_names),
            clock: FrameClock::new(),
            scroll_intents: ScrollIntents::default(),
            composed: None,
            composed_scene: Scene::new(),
            refill_requested_for: None,
            begin_frames_sent: 0,
            window: PhantomData,
            thread_bound: PhantomData,
        };
        let events = EngineEventSender {
            sender: message_sender,
            requester: event_requester,
        };
        (view, command_receiver, events, listener_updates)
    }

    #[cfg(test)]
    pub(crate) fn probe_document<R: Send + 'static>(
        &mut self,
        probe: impl FnOnce(&mut LynxDocument) -> R + Send + 'static,
    ) -> Option<R> {
        if self.detached {
            return None;
        }
        let (sender, receiver) = mpsc::channel();
        self.commands
            .send(MainCommand::Probe(Box::new(move |document| {
                let _ = sender.send(probe(document));
            })))
            .ok()?;
        receiver.recv_timeout(Duration::from_secs(10)).ok()
    }

    #[cfg(test)]
    pub(crate) fn published_frame(&self) -> Option<Arc<CommittedFrame>> {
        self.hub.latest()
    }
}

fn spawn_main_thread(
    document: LynxDocument,
    entry: EntryModule,
    commands: mpsc::Receiver<MainCommand>,
    hub: Arc<FrameHub>,
    listener_updates: mpsc::Sender<ListenerUpdate>,
    frames: FrameWakeup,
    events: EngineEventSender,
) -> Result<(), EngineError> {
    ThreadBuilder::new()
        .name("bobcat-main".to_owned())
        .spawn(move || {
            #[cfg(all(target_arch = "wasm32", panic = "abort"))]
            install_script_panic_hook();
            #[cfg(all(target_arch = "wasm32", panic = "abort"))]
            set_script_panic_reporter(Some(events.clone()));
            let publish_hub = Arc::clone(&hub);
            let wake_frames = frames.clone();
            let result = catch_unwind(AssertUnwindSafe(|| {
                let mut runtime =
                    MainThreadRuntime::new(document, listener_updates, publish_hub, move || {
                        wake_frames.request();
                    })
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
                    events.send(EngineEvent::ScriptFinished);
                    Some(runtime)
                }
                Err(error) => {
                    events.send(EngineEvent::ScriptRunError(error));
                    None
                }
            };
            frames.request();

            if let Some(runtime) = runtime {
                let served = catch_unwind(AssertUnwindSafe(|| {
                    serve_main_commands(runtime, &commands, &events, &hub);
                }));
                if let Err(payload) = served {
                    events.send(EngineEvent::ScriptRunError(platform_script_error(format!(
                        "the Lynx main thread panicked while serving commands: {}",
                        panic_payload(payload.as_ref())
                    ))));
                }
            }
            hub.note_committer_gone();
            #[cfg(all(target_arch = "wasm32", panic = "abort"))]
            set_script_panic_reporter(None);
        })
        .map(|_thread| ())
        .map_err(|error| EngineError::Thread {
            name: "script",
            message: error.to_string(),
        })
}

fn serve_main_commands(
    mut runtime: MainThreadRuntime,
    commands: &mpsc::Receiver<MainCommand>,
    events: &EngineEventSender,
    hub: &FrameHub,
) {
    while let Ok(first) = commands.recv() {
        let mut serviced_begin_frame = None;
        let mut command = Some(first);
        while let Some(current) = command.take() {
            apply_main_command(&mut runtime, current, events, &mut serviced_begin_frame);
            command = commands.try_recv().ok();
        }
        runtime.commit_if_dirty();
        if let Some(seq) = serviced_begin_frame {
            hub.note_begin_frame_serviced(seq);
        }
    }
}

fn apply_main_command(
    runtime: &mut MainThreadRuntime,
    command: MainCommand,
    events: &EngineEventSender,
    serviced_begin_frame: &mut Option<u64>,
) {
    match command {
        MainCommand::DispatchEvent {
            target,
            name,
            detail,
        } => {
            let delivered = catch_unwind(AssertUnwindSafe(|| {
                runtime.dispatch_event(target, name, &detail)
            }));
            if let Ok(Err(error)) = delivered {
                events.send(EngineEvent::ListenerFailed(error.into_script_error()));
            }
        }
        MainCommand::Resize {
            width,
            height,
            device_pixel_ratio,
        } => runtime.apply_resize(width, height, device_pixel_ratio),
        MainCommand::BeginFrame { now, seq } => {
            runtime.begin_frame(now);
            *serviced_begin_frame = Some(seq.max(serviced_begin_frame.unwrap_or(0)));
        }
        MainCommand::Refill { offsets } => runtime.refill_scroll_windows(&offsets),
        MainCommand::NoteImagesChanged => runtime.note_images_changed(),
        #[cfg(test)]
        MainCommand::Probe(probe) => runtime.with_document(probe),
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
                    reporter.send(EngineEvent::ScriptRunError(platform_script_error(format!(
                        "the script Worker aborted after a panic{location}: {}",
                        panic_payload(info.payload())
                    ))));
                }
            });
            previous(info);
        }));
    });
}

#[cfg(all(target_arch = "wasm32", panic = "abort"))]
fn set_script_panic_reporter(reporter: Option<EngineEventSender>) {
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

fn panic_payload(payload: &(dyn std::any::Any + Send)) -> &str {
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
