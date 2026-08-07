//! The engine half of a running Lynx view, behind the embedder boundary.
//!
//! An embedder provides exactly five things: user input, device metrics,
//! OS initialization (its event loop and window), a draw target, and IO
//! primitives (bundle bytes in, pixels out). It never starts or steers the
//! internal pipeline: commit application, style, layout, paint, frame
//! scheduling, and the script and render threads all live inside [`Engine`].
//! The embedder's event handlers are relays — they hand the engine an OS
//! fact (`dispatch_input`, `resize`, `notify_redraw`, `pump`) and the engine
//! decides what the pipeline does with it, requesting frames itself through
//! the capabilities the embedder handed over at attach time.
//!
//! Thread placement is intentionally invisible in this contract. Today the
//! engine's logic runs inline on whichever thread the embedder calls from
//! (its OS event loop), while the engine owns a script thread and a render
//! thread; a dedicated engine thread would change no signature here.
//!
//! The element tree is never shared across threads. Every crossing is a
//! plain value: recorded [`ElementOp`] batches inward over the message
//! channel, cloned [`Scene`]s outward to the render thread, acknowledged
//! over one-shot channels — the script may wait on the engine, never the
//! reverse.

mod graphics;

use std::cell::{Ref, RefCell};
use std::fmt;
use std::rc::Rc;
use std::sync::mpsc;

use lynx_element::dom::ImageStore;
use lynx_element::dom::input::InputEvent;
use lynx_element::dom::render::gpu::{GpuError, Headless};
use lynx_element::dom::vello::peniko::Color;
use lynx_element::{ElementOp, ElementTree, PageConfig, PapiError, Viewport};

pub use self::graphics::WindowTarget;
use self::graphics::{FrameJob, WindowGraphics};

/// The physical pixel size of the render target.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameSize {
    pub width: u32,
    pub height: u32,
}

/// The largest physical render target the engine accepts, chosen to stay
/// inside common GPU texture limits.
const MAX_RENDER_DIMENSION: u32 = 16_384;

/// Why an engine operation failed.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum EngineError {
    #[error("invalid viewport: {0}")]
    Viewport(String),
    #[error("{0}")]
    Gpu(#[source] GpuError),
    #[error("rendering failed: {0}")]
    Render(String),
    #[error("could not start the {name} thread: {message}")]
    Thread { name: &'static str, message: String },
    #[error("the engine has no draw target attached")]
    NoDrawTarget,
}

/// Why a committed Element PAPI batch was not applied.
#[derive(Debug)]
#[non_exhaustive]
pub enum CommitError {
    /// The engine rejected the batch: it diverged from the script-side
    /// recorder's shadow and must not be half-trusted.
    Rejected(PapiError),
    /// The engine side is gone; no further commit can ever succeed.
    Disconnected,
}

impl fmt::Display for CommitError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Rejected(error) => write!(formatter, "the engine rejected the batch: {error}"),
            Self::Disconnected => write!(formatter, "the engine side is disconnected"),
        }
    }
}

impl std::error::Error for CommitError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Rejected(error) => Some(error),
            Self::Disconnected => None,
        }
    }
}

/// Applies one flushed Element PAPI batch and runs the style + layout
/// commit. This is the single commit-application point every composition
/// shares: the in-process sink, the engine's own message pump, and tests.
pub(crate) fn apply_batch(
    elements: &mut ElementTree,
    ops: &[ElementOp],
) -> Result<(), CommitError> {
    for op in ops {
        elements.apply(op).map_err(CommitError::Rejected)?;
    }
    elements.flush_element_tree();
    Ok(())
}

/// How a main-thread script run ended in failure.
#[cfg(feature = "quickjs")]
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ScriptRunError {
    #[error("could not initialize the main-thread runtime: {0}")]
    Initialization(#[source] crate::quickjs::QuickJsInitializationError),
    #[error(transparent)]
    Script(crate::quickjs::MainThreadError),
}

/// A message crossing into the engine from one of its own threads.
enum EngineMessage {
    /// One `__FlushElementTree` batch; `ack` unblocks the flush once the
    /// engine has applied and committed it. Only the engine-owned script
    /// thread constructs this.
    #[cfg(feature = "quickjs")]
    Commit {
        ops: Vec<ElementOp>,
        ack: mpsc::Sender<Result<(), CommitError>>,
    },
    /// The main-thread script ran to completion (or failed) on its thread.
    #[cfg(feature = "quickjs")]
    ScriptDone(Result<(), ScriptRunError>),
    /// The render thread died; the session cannot continue.
    RenderFailed(EngineError),
}

/// A lifecycle outcome the embedder must react to, drained by
/// [`Engine::pump`].
#[derive(Debug)]
#[non_exhaustive]
pub enum EngineEvent {
    /// The spawned main-thread script finished.
    #[cfg(feature = "quickjs")]
    ScriptFinished(Result<(), ScriptRunError>),
    /// The render thread failed; the view cannot continue.
    RenderFailed(EngineError),
}

/// One captured frame: tightly packed RGBA8 pixels at `size`.
#[derive(Clone)]
pub struct Screenshot {
    pub size: FrameSize,
    pub pixels: Vec<u8>,
}

impl fmt::Debug for Screenshot {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Screenshot")
            .field("size", &self.size)
            .finish_non_exhaustive()
    }
}

/// Where a screenshot lands once the frame that carries it is captured.
/// Runs on the engine's render thread in window mode; persisting the pixels
/// (an IO primitive) is the embedder's half.
type ScreenshotSink = Box<dyn FnOnce(Result<Screenshot, EngineError>) + Send>;

/// OS capabilities the embedder lends the engine with a window target.
/// The embedder provides the mechanisms; the engine decides when to invoke
/// them.
pub struct WindowHooks {
    /// Asks the OS for a redraw of the window (`Window::request_redraw`).
    /// Called on the engine's own thread whenever the pipeline has something
    /// new to show.
    pub request_frame: Box<dyn Fn()>,
    /// Called by the render thread immediately before presenting
    /// (`Window::pre_present_notify`).
    pub pre_present: Box<dyn Fn() + Send>,
    /// Posts a wakeup to the embedder's event loop so it calls
    /// [`Engine::pump`]. Called from engine-owned threads.
    pub wakeup: Box<dyn Fn() + Send + Sync>,
}

impl fmt::Debug for WindowHooks {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("WindowHooks")
            .finish_non_exhaustive()
    }
}

/// The attached output, if any.
enum Output {
    None,
    /// An offscreen GPU target on the calling thread: `tick` renders,
    /// `capture` reads back. Boxed to keep the idle variants small.
    Offscreen(Box<Headless>),
    /// A window: prepared frames cross to the engine-owned render thread
    /// through a latest-wins mailbox.
    Window {
        frames: mpsc::Sender<FrameJob>,
    },
}

/// The engine half of a Lynx view: the element tree, commit application,
/// input routing, frame scheduling, and the script/render threads.
///
/// Deliberately `!Send`: it lives on the thread the embedder calls it from.
pub struct Engine {
    elements: Rc<RefCell<ElementTree>>,
    viewport: Viewport,
    frame_size: FrameSize,
    messages: mpsc::Receiver<EngineMessage>,
    message_sender: mpsc::Sender<EngineMessage>,
    output: Output,
    request_frame: Option<Box<dyn Fn()>>,
    pending_screenshots: Vec<ScreenshotSink>,
}

impl fmt::Debug for Engine {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Engine")
            .field("viewport", &self.viewport)
            .field("frame_size", &self.frame_size)
            .finish_non_exhaustive()
    }
}

impl Engine {
    /// Builds the engine and its element tree at the given CSS viewport.
    pub fn new(
        config: PageConfig,
        width: f32,
        height: f32,
        device_pixel_ratio: f32,
    ) -> Result<Self, EngineError> {
        let frame_size = frame_size(width, height, device_pixel_ratio)?;
        let viewport = Viewport::new(width, height).with_device_pixel_ratio(device_pixel_ratio);
        let (message_sender, messages) = mpsc::channel();
        Ok(Self {
            elements: Rc::new(RefCell::new(ElementTree::new(viewport, config))),
            viewport,
            frame_size,
            messages,
            message_sender,
            output: Output::None,
            request_frame: None,
            pending_screenshots: Vec::new(),
        })
    }

    /// A read-only borrow of the element tree, for observation.
    ///
    /// # Panics
    ///
    /// Panics if called while the engine itself holds the tree mutably —
    /// impossible from embedder code, which only ever runs between engine
    /// calls.
    #[must_use]
    pub fn elements(&self) -> Ref<'_, ElementTree> {
        self.elements.borrow()
    }

    /// The commit sink for a script running on the engine's own thread:
    /// every flushed batch applies to the tree on the spot.
    pub fn commit_sink(&self) -> impl FnMut(Vec<ElementOp>) -> Result<(), CommitError> + 'static {
        let elements = Rc::clone(&self.elements);
        move |ops| apply_batch(&mut elements.borrow_mut(), &ops)
    }

    /// Mounts author CSS.
    pub fn add_author_stylesheet(&mut self, css: &str) {
        self.elements.borrow_mut().add_author_stylesheet(css);
    }

    /// Registers font data for text measurement.
    pub fn register_fonts(&mut self, bytes: &[u8]) -> usize {
        self.elements.borrow_mut().register_fonts(bytes)
    }

    /// Registers or updates decoded images, then keeps the next frame fresh.
    pub fn with_images<R>(&mut self, update: impl FnOnce(&mut ImageStore) -> R) -> R {
        let result = update(self.elements.borrow_mut().images_mut());
        self.refresh();
        result
    }

    /// Routes one host input event; the engine schedules a frame itself if
    /// the default action changed anything on screen.
    pub fn dispatch_input(&mut self, event: InputEvent) {
        let needs_frame = {
            let mut elements = self.elements.borrow_mut();
            elements.handle_input(event);
            elements.needs_render()
        };
        if needs_frame {
            self.refresh();
        }
    }

    /// Applies new device metrics from the embedder.
    pub fn resize(
        &mut self,
        width: f32,
        height: f32,
        device_pixel_ratio: f32,
    ) -> Result<(), EngineError> {
        let next_size = frame_size(width, height, device_pixel_ratio)?;
        let size_changed = self.viewport.width.to_bits() != width.to_bits()
            || self.viewport.height.to_bits() != height.to_bits();
        let scale_changed =
            self.viewport.device_pixel_ratio.to_bits() != device_pixel_ratio.to_bits();
        if !size_changed && !scale_changed {
            return Ok(());
        }
        {
            let mut elements = self.elements.borrow_mut();
            if size_changed {
                elements.set_viewport(width, height);
            }
            if scale_changed {
                elements.set_device_pixel_ratio(device_pixel_ratio);
            }
        }
        self.viewport = Viewport::new(width, height).with_device_pixel_ratio(device_pixel_ratio);
        self.frame_size = next_size;
        self.refresh();
        Ok(())
    }

    /// Asks the OS for a frame through the embedder-provided capability.
    /// Harmless when nothing changed: the redraw dedupes against the
    /// retained scene.
    pub fn refresh(&self) {
        if let Some(request_frame) = &self.request_frame {
            request_frame();
        }
    }

    /// Drains engine-thread messages: applies committed batches, and returns
    /// the lifecycle events the embedder must react to. Called from the
    /// embedder's event loop whenever the engine's wakeup capability fired.
    pub fn pump(&mut self) -> Vec<EngineEvent> {
        let mut events = Vec::new();
        while let Ok(message) = self.messages.try_recv() {
            match message {
                #[cfg(feature = "quickjs")]
                EngineMessage::Commit { ops, ack } => {
                    let result = apply_batch(&mut self.elements.borrow_mut(), &ops);
                    // A dead receiver only means the script thread gave up;
                    // the tree has already applied whatever was valid.
                    let _ = ack.send(result);
                    self.refresh();
                }
                #[cfg(feature = "quickjs")]
                EngineMessage::ScriptDone(result) => {
                    events.push(EngineEvent::ScriptFinished(result));
                }
                EngineMessage::RenderFailed(error) => {
                    events.push(EngineEvent::RenderFailed(error));
                }
            }
        }
        events
    }

    /// Attaches an offscreen GPU target on the calling thread. The headless
    /// composition: [`Self::tick`] renders, [`Self::capture`] reads back.
    pub fn attach_offscreen(&mut self) -> Result<(), EngineError> {
        let gpu = Headless::new().map_err(EngineError::Gpu)?;
        self.output = Output::Offscreen(Box::new(gpu));
        Ok(())
    }

    /// Attaches a window draw target and takes over presentation: creates
    /// the GPU surface on the calling thread (the one place macOS guarantees
    /// layer setup works), then moves the whole GPU stack to an engine-owned
    /// render thread behind a latest-wins mailbox.
    pub fn attach_window(
        &mut self,
        target: impl Into<WindowTarget>,
        size: FrameSize,
        hooks: WindowHooks,
    ) -> Result<(), EngineError> {
        let WindowHooks {
            request_frame,
            pre_present,
            wakeup,
        } = hooks;
        let graphics = WindowGraphics::new(target.into(), size)?;
        let (frames, frame_jobs) = mpsc::channel();
        let sender = self.message_sender.clone();
        std::thread::Builder::new()
            .name("bobcat-render".to_owned())
            .spawn(move || {
                graphics::render_thread(graphics, &frame_jobs, pre_present.as_ref(), &|error| {
                    let _ = sender.send(EngineMessage::RenderFailed(error));
                    wakeup();
                });
            })
            .map_err(|error| EngineError::Thread {
                name: "render",
                message: error.to_string(),
            })?;
        self.output = Output::Window { frames };
        self.request_frame = Some(request_frame);
        self.refresh();
        Ok(())
    }

    /// Relays the OS's "the window wants a frame" fact: prepares the current
    /// frame (rendering only if the retained scene is stale) and mails it to
    /// the render thread, carrying any pending screenshot requests.
    pub fn notify_redraw(&mut self) {
        let Output::Window { frames } = &self.output else {
            return;
        };
        let mut elements = self.elements.borrow_mut();
        elements.render();
        let job = FrameJob {
            scene: elements.scene().clone(),
            size: self.frame_size,
            screenshots: std::mem::take(&mut self.pending_screenshots),
        };
        drop(elements);
        // A closed mailbox means the render thread already posted its
        // failure.
        let _ = frames.send(job);
    }

    /// Renders one frame to the offscreen target if the document changed
    /// (or unconditionally with `force`), returning whether a frame was
    /// submitted. The embedder's clock relays ticks; the engine decides
    /// whether a tick becomes work.
    pub fn tick(&mut self, force: bool) -> Result<bool, EngineError> {
        let Output::Offscreen(gpu) = &mut self.output else {
            return Err(EngineError::NoDrawTarget);
        };
        let mut elements = self.elements.borrow_mut();
        let changed = elements.render();
        if !changed && !force {
            // The retained target already holds this exact frame;
            // re-submitting it would burn a full GPU pass per tick on a
            // static scene.
            return Ok(false);
        }
        gpu.render_frame(
            &elements.scene(),
            self.frame_size.width,
            self.frame_size.height,
            Color::WHITE,
        )
        .map_err(EngineError::Gpu)?;
        // Keep at most one frame in flight: nothing else synchronizes with
        // the GPU, so a clock that outpaces it would otherwise pile up
        // submissions without bound.
        gpu.wait_idle().map_err(EngineError::Gpu)?;
        Ok(true)
    }

    /// Captures the current frame from the offscreen target, rendering
    /// first if the document changed since the last [`Self::tick`].
    pub fn capture(&mut self) -> Result<Screenshot, EngineError> {
        let Output::Offscreen(gpu) = &mut self.output else {
            return Err(EngineError::NoDrawTarget);
        };
        let mut elements = self.elements.borrow_mut();
        if elements.render() {
            gpu.render_frame(
                &elements.scene(),
                self.frame_size.width,
                self.frame_size.height,
                Color::WHITE,
            )
            .map_err(EngineError::Gpu)?;
        }
        // The retained target holds the current frame; read it back rather
        // than re-rendering a scene that has not changed.
        let pixels = gpu.read_pixels().map_err(EngineError::Gpu)?;
        Ok(Screenshot {
            size: self.frame_size,
            pixels,
        })
    }

    /// Requests one screenshot of the current frame. In window mode the
    /// pixels are captured on the render thread from the next presented
    /// frame; offscreen they are captured immediately. `deliver` receives
    /// the result — persisting it is the embedder's IO.
    pub fn request_screenshot(
        &mut self,
        deliver: impl FnOnce(Result<Screenshot, EngineError>) + Send + 'static,
    ) {
        match &mut self.output {
            Output::Window { .. } => {
                self.pending_screenshots.push(Box::new(deliver));
                self.refresh();
            }
            Output::Offscreen(_) => deliver(self.capture()),
            Output::None => deliver(Err(EngineError::NoDrawTarget)),
        }
    }

    /// Runs a main-thread script to completion on the calling thread,
    /// applying every flushed batch as it commits. The headless composition.
    #[cfg(feature = "quickjs")]
    pub fn run_script(&mut self, source: &str) -> Result<(), ScriptRunError> {
        let mut runtime = crate::quickjs::MainThreadRuntime::new(self.commit_sink())
            .map_err(ScriptRunError::Initialization)?;
        let result = runtime
            .run_main_thread_script(source)
            .map_err(ScriptRunError::Script);
        self.refresh();
        result
    }

    /// Spawns the main-thread script on an engine-owned thread. Every
    /// `__FlushElementTree` batch crosses the message channel and blocks the
    /// script until [`Self::pump`] applies and acknowledges it; completion
    /// arrives as [`EngineEvent::ScriptFinished`]. `wakeup` posts to the
    /// embedder's event loop so it knows to pump.
    #[cfg(feature = "quickjs")]
    pub fn spawn_script(
        &mut self,
        source: String,
        wakeup: impl Fn() + Send + Sync + 'static,
    ) -> Result<(), EngineError> {
        use std::sync::Arc;
        let sender = self.message_sender.clone();
        let wakeup = Arc::new(wakeup);
        std::thread::Builder::new()
            .name("bobcat-js".to_owned())
            .spawn(move || {
                let commit_sender = sender.clone();
                let commit_wakeup = Arc::clone(&wakeup);
                let result = (|| {
                    let mut runtime = crate::quickjs::MainThreadRuntime::new(move |ops| {
                        let (ack, ack_receiver) = mpsc::channel();
                        commit_sender
                            .send(EngineMessage::Commit { ops, ack })
                            .map_err(|_| CommitError::Disconnected)?;
                        commit_wakeup();
                        ack_receiver.recv().map_err(|_| CommitError::Disconnected)?
                    })
                    .map_err(ScriptRunError::Initialization)?;
                    runtime
                        .run_main_thread_script(&source)
                        .map_err(ScriptRunError::Script)
                })();
                let _ = sender.send(EngineMessage::ScriptDone(result));
                wakeup();
            })
            .map_err(|error| EngineError::Thread {
                name: "script",
                message: error.to_string(),
            })?;
        Ok(())
    }
}

/// Validates CSS viewport metrics and derives the physical target size.
fn frame_size(width: f32, height: f32, device_pixel_ratio: f32) -> Result<FrameSize, EngineError> {
    if !width.is_finite()
        || !height.is_finite()
        || !device_pixel_ratio.is_finite()
        || width <= 0.0
        || height <= 0.0
        || device_pixel_ratio <= 0.0
    {
        return Err(EngineError::Viewport(format!(
            "CSS size and device-pixel ratio must be finite and positive, got \
             {width}\u{d7}{height} at {device_pixel_ratio}\u{d7}"
        )));
    }

    let physical_width = f64::from(width) * f64::from(device_pixel_ratio);
    let physical_height = f64::from(height) * f64::from(device_pixel_ratio);
    if physical_width > f64::from(MAX_RENDER_DIMENSION)
        || physical_height > f64::from(MAX_RENDER_DIMENSION)
    {
        return Err(EngineError::Viewport(format!(
            "the physical render target may not exceed \
             {MAX_RENDER_DIMENSION}\u{d7}{MAX_RENDER_DIMENSION}, got \
             {physical_width:.0}\u{d7}{physical_height:.0}"
        )));
    }

    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "finite positive values were bounded to 16384 immediately above"
    )]
    let size = FrameSize {
        width: physical_width.round().max(1.0) as u32,
        height: physical_height.round().max(1.0) as u32,
    };
    Ok(size)
}

#[cfg(test)]
mod tests {
    use super::frame_size;

    #[test]
    fn frame_size_applies_the_device_scale_once() {
        let size = frame_size(393.0, 727.0, 2.0).unwrap();
        assert_eq!((size.width, size.height), (786, 1_454));
    }

    #[test]
    fn frame_size_rejects_unbounded_targets() {
        let error = frame_size(20_000.0, 100.0, 1.0).unwrap_err();
        assert!(error.to_string().contains("16384"));
    }

    /// The windowed commit protocol, minus the window: the script runs on
    /// the engine-owned thread, every `__FlushElementTree` batch crosses the
    /// message channel, and [`super::Engine::pump`] applies and acknowledges
    /// it — the exact wiring an embedder's event loop drives through its
    /// wakeup relay.
    #[cfg(feature = "quickjs")]
    #[test]
    fn a_spawned_script_commits_across_threads() {
        use std::sync::mpsc;

        use lynx_element::PageConfig;

        use super::{Engine, EngineEvent};

        let mut engine = Engine::new(PageConfig::default(), 393.0, 727.0, 1.0).expect("engine");
        let (wake_sender, wakeups) = mpsc::channel();
        engine
            .spawn_script(
                r"
                globalThis.renderPage = function () {
                  const page = __CreatePage('card', 0);
                  __AppendElement(page, __CreateView(0));
                  __FlushElementTree();
                  __AppendElement(page, __CreateView(0));
                };
                "
                .to_owned(),
                move || {
                    let _ = wake_sender.send(());
                },
            )
            .expect("script thread");

        // The embedder side of the loop: block on the wakeup relay, pump,
        // react to lifecycle events, until the script finishes.
        let finished = loop {
            wakeups.recv().expect("the script thread wakes the loop");
            let done = engine
                .pump()
                .into_iter()
                .map(|event| match event {
                    EngineEvent::ScriptFinished(result) => result,
                    EngineEvent::RenderFailed(error) => panic!("render failed: {error}"),
                })
                .next();
            if let Some(result) = done {
                break result;
            }
        };
        finished.expect("the script must boot");

        let elements = engine.elements();
        let page = elements.page().expect("the page was created");
        let page_node = elements.node_id(page).expect("a live page");
        assert_eq!(
            elements
                .document()
                .get(page_node)
                .expect("a live page node")
                .child_ids()
                .len(),
            2,
            "both views landed, one per committed batch"
        );
    }
}
