//! The engine half of a running Lynx view, behind the embedder boundary.
//!
//! An embedder provides exactly five things: user input, device metrics,
//! OS initialization (its event loop and window), a draw target, and IO
//! primitives (bundle bytes in, pixels out). It never starts or steers the
//! internal pipeline. Its event handlers are relays — they hand the engine
//! an OS fact (`dispatch_input`, `resize`, `notify_redraw`, `pump`) and the
//! engine decides what the pipeline does with it, requesting frames itself
//! through the capabilities the embedder handed over at attach time.
//!
//! # Two threads, one lock
//!
//! The element tree is shared behind one `Mutex` between two roles:
//!
//! - **The Lynx main thread** (engine-owned, spawned by [`Engine::spawn_script`]): the `QuickJS`
//!   realm and its event loop. Each Element PAPI call locks the tree for the duration of one call
//!   and mutates it directly; `__FlushElementTree` runs the style + layout commit under the lock
//!   and then asks for a frame.
//! - **The presenting side** (the thread the embedder calls the engine from — its OS event loop):
//!   input routing, scrolling, frame production (paint-order build + scene encode), GPU submission,
//!   and present. Everything here acquires the lock with `try_lock` and never blocks: if the main
//!   thread is mid-commit, the work is retried at the next frame, and the retained target
//!   re-presents in the meantime.
//!
//! The lock is idle while the script computes, which is the point: a long
//! JavaScript task does not stop the presenting side from scrolling —
//! target resolution reads the retained paint order, the offset lands in
//! the shared tree, and the next frame is produced and presented without
//! the script's cooperation. The presenting side never produces a frame
//! while a PAPI batch is open
//! ([`ElementTree::has_uncommitted_mutations`]) — it re-presents the last
//! committed frame instead, so a half-applied batch is never observable.
//!
//! The law: the main thread waits only on its own commits; the presenting
//! side never waits on the main thread; present's vsync wait happens
//! outside the lock, so it blocks no one.

mod graphics;

use std::collections::VecDeque;
use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard, TryLockError, mpsc};

use lynx_element::dom::ImageStore;
use lynx_element::dom::input::InputEvent;
use lynx_element::dom::render::gpu::{GpuError, Headless};
use lynx_element::dom::vello::peniko::Color;
use lynx_element::{ElementTree, PageConfig, Viewport};

use self::graphics::WindowGraphics;
pub use self::graphics::WindowTarget;

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

/// A message crossing from an engine-owned thread.
enum EngineMessage {
    /// The main-thread script ran to completion (or failed) on its thread.
    #[cfg(feature = "quickjs")]
    ScriptDone(Result<(), ScriptRunError>),
}

/// A lifecycle outcome the embedder must react to, drained by
/// [`Engine::pump`].
#[derive(Debug)]
#[non_exhaustive]
pub enum EngineEvent {
    /// The spawned main-thread script finished.
    #[cfg(feature = "quickjs")]
    ScriptFinished(Result<(), ScriptRunError>),
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

/// OS capabilities the embedder lends the engine with a window target.
/// The embedder provides the mechanisms; the engine decides when to invoke
/// them.
pub struct WindowHooks {
    /// Asks the OS for a redraw of the window (`Window::request_redraw`).
    /// `Send + Sync` because the engine's main thread asks for a frame
    /// after every committed flush.
    pub request_frame: Box<dyn Fn() + Send + Sync>,
    /// Called on the presenting side immediately before presenting
    /// (`Window::pre_present_notify`).
    pub pre_present: Box<dyn Fn()>,
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
    /// An offscreen GPU target: `tick` renders, `capture` reads back.
    /// Boxed to keep the idle variants small.
    Offscreen(Box<Headless>),
    /// A window: the presentation stack lives here, on the thread the
    /// embedder calls the engine from.
    Window {
        graphics: Box<WindowGraphics>,
        pre_present: Box<dyn Fn()>,
    },
}

/// The engine half of a Lynx view: the shared element tree, input routing,
/// frame production, presentation, and the engine-owned script thread.
///
/// Deliberately `!Send`: it lives on the thread the embedder calls it from.
pub struct Engine {
    elements: Arc<Mutex<ElementTree>>,
    viewport: Viewport,
    frame_size: FrameSize,
    messages: mpsc::Receiver<EngineMessage>,
    #[cfg_attr(
        not(feature = "quickjs"),
        allow(dead_code, reason = "only engine-owned script threads send messages")
    )]
    message_sender: mpsc::Sender<EngineMessage>,
    output: Output,
    request_frame: Option<Arc<dyn Fn() + Send + Sync>>,
    /// Input deferred by a missed `try_lock`, drained in arrival order at
    /// the next lock acquisition so gesture sequences stay coherent.
    pending_input: VecDeque<InputEvent>,
}

impl fmt::Debug for Engine {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Engine")
            .field("viewport", &self.viewport)
            .field("frame_size", &self.frame_size)
            .field("pending_input", &self.pending_input.len())
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
            elements: Arc::new(Mutex::new(ElementTree::new(viewport, config))),
            viewport,
            frame_size,
            messages,
            message_sender,
            output: Output::None,
            request_frame: None,
            pending_input: VecDeque::new(),
        })
    }

    /// A blocking borrow of the element tree, for observation and setup.
    ///
    /// # Panics
    ///
    /// Panics if the other side crashed while holding the lock.
    pub fn elements(&self) -> MutexGuard<'_, ElementTree> {
        lock(&self.elements)
    }

    /// Mounts author CSS.
    pub fn add_author_stylesheet(&mut self, css: &str) {
        lock(&self.elements).add_author_stylesheet(css);
    }

    /// Registers font data for text measurement.
    pub fn register_fonts(&mut self, bytes: &[u8]) -> usize {
        lock(&self.elements).register_fonts(bytes)
    }

    /// Registers or updates decoded images, then keeps the next frame fresh.
    pub fn with_images<R>(&mut self, update: impl FnOnce(&mut ImageStore) -> R) -> R {
        let result = update(lock(&self.elements).images_mut());
        self.refresh();
        result
    }

    /// Routes one host input event on the presenting side.
    ///
    /// Never blocks: if the main thread is mid-commit the event is buffered
    /// and drained, in order, at the next lock acquisition (the very next
    /// event or redraw). Scroll target resolution and the offset write
    /// happen under the lock; a long script task leaves the lock idle, so
    /// scrolling proceeds without the script's cooperation.
    pub fn dispatch_input(&mut self, event: InputEvent) {
        self.pending_input.push_back(event);
        match self.elements.try_lock() {
            Ok(mut elements) => {
                while let Some(event) = self.pending_input.pop_front() {
                    elements.handle_input(event);
                }
                let needs_frame = elements.needs_render();
                drop(elements);
                if needs_frame {
                    self.refresh();
                }
            }
            Err(TryLockError::WouldBlock) => {
                // A commit is running; it ends with a frame request, whose
                // redraw drains the buffer.
            }
            Err(TryLockError::Poisoned(error)) => poisoned(&error),
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
            let mut elements = lock(&self.elements);
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
    /// Harmless when nothing changed: the redraw re-presents the retained
    /// target without re-rendering.
    pub fn refresh(&self) {
        if let Some(request_frame) = &self.request_frame {
            request_frame();
        }
    }

    /// Drains lifecycle messages from engine-owned threads. Called from the
    /// embedder's event loop whenever the engine's wakeup capability fired.
    pub fn pump(&mut self) -> Vec<EngineEvent> {
        #[cfg_attr(
            not(feature = "quickjs"),
            allow(unused_mut, reason = "no message variant exists to push")
        )]
        let mut events = Vec::new();
        while let Ok(message) = self.messages.try_recv() {
            match message {
                #[cfg(feature = "quickjs")]
                EngineMessage::ScriptDone(result) => {
                    events.push(EngineEvent::ScriptFinished(result));
                }
            }
        }
        events
    }

    /// Attaches an offscreen GPU target. The headless composition:
    /// [`Self::tick`] renders, [`Self::capture`] reads back.
    pub fn attach_offscreen(&mut self) -> Result<(), EngineError> {
        let gpu = Headless::new().map_err(EngineError::Gpu)?;
        self.output = Output::Offscreen(Box::new(gpu));
        Ok(())
    }

    /// Attaches a window draw target: the whole presentation stack is
    /// created here, on the calling thread, and stays here — presentation
    /// and vsync interact with the OS only on this thread.
    pub fn attach_window(
        &mut self,
        target: impl Into<WindowTarget>,
        size: FrameSize,
        hooks: WindowHooks,
    ) -> Result<(), EngineError> {
        let WindowHooks {
            request_frame,
            pre_present,
            wakeup: _,
        } = hooks;
        let graphics = WindowGraphics::new(target.into(), size)?;
        self.output = Output::Window {
            graphics: Box::new(graphics),
            pre_present,
        };
        self.request_frame = Some(Arc::from(request_frame));
        self.refresh();
        Ok(())
    }

    /// Relays the OS's "the window wants a frame" fact.
    ///
    /// Produces a new frame only when there is one to produce: the tree is
    /// available (`try_lock` — a running commit retries next frame), no
    /// PAPI batch is open, and the document changed since the retained
    /// target was rendered. Everything else — re-exposure, a resize with
    /// unchanged content, a retry — re-presents the retained target with a
    /// blit alone. The present itself (the vsync wait) runs after the lock
    /// is released.
    pub fn notify_redraw(&mut self) -> Result<(), EngineError> {
        let Output::Window {
            graphics,
            pre_present,
        } = &mut self.output
        else {
            return Ok(());
        };
        let size = self.frame_size;
        match self.elements.try_lock() {
            Ok(mut elements) => {
                while let Some(event) = self.pending_input.pop_front() {
                    elements.handle_input(event);
                }
                if !elements.has_uncommitted_mutations() {
                    let produced = elements.render();
                    if produced || !graphics.rendered_at(size) {
                        graphics.render_to_target(&elements.scene(), size)?;
                    }
                }
            }
            Err(TryLockError::WouldBlock) => {
                // A commit is running; it ends with a frame request. Present
                // the retained target now so exposure never waits on it.
            }
            Err(TryLockError::Poisoned(error)) => poisoned(&error),
        }
        // The vsync wait happens here, after the lock is released.
        if graphics.rendered_at(size) {
            graphics.present(pre_present.as_ref())?;
        }
        Ok(())
    }

    /// Renders one frame to the offscreen target if the document changed
    /// (or unconditionally with `force`), returning whether a frame was
    /// submitted. The embedder's clock relays ticks; the engine decides
    /// whether a tick becomes work.
    pub fn tick(&mut self, force: bool) -> Result<bool, EngineError> {
        let Output::Offscreen(gpu) = &mut self.output else {
            return Err(EngineError::NoDrawTarget);
        };
        let mut elements = lock(&self.elements);
        if elements.has_uncommitted_mutations() {
            return Ok(false);
        }
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

    /// Captures the current frame as pixels — synchronously, from whichever
    /// target is attached. Renders first if the document changed and the
    /// tree is available; a tree busy mid-commit (window mode) captures the
    /// retained frame, which is what the window is showing.
    pub fn capture(&mut self) -> Result<Screenshot, EngineError> {
        let size = self.frame_size;
        match &mut self.output {
            Output::None => Err(EngineError::NoDrawTarget),
            Output::Offscreen(gpu) => {
                let mut elements = lock(&self.elements);
                if !elements.has_uncommitted_mutations() && elements.render() {
                    gpu.render_frame(&elements.scene(), size.width, size.height, Color::WHITE)
                        .map_err(EngineError::Gpu)?;
                }
                // The retained target holds the current frame; read it back
                // rather than re-rendering a scene that has not changed.
                let pixels = gpu.read_pixels().map_err(EngineError::Gpu)?;
                Ok(Screenshot { size, pixels })
            }
            Output::Window { graphics, .. } => {
                match self.elements.try_lock() {
                    Ok(mut elements) => {
                        if !elements.has_uncommitted_mutations() {
                            let produced = elements.render();
                            if produced || !graphics.rendered_at(size) {
                                graphics.render_to_target(&elements.scene(), size)?;
                            }
                        }
                    }
                    Err(TryLockError::WouldBlock) => {}
                    Err(TryLockError::Poisoned(error)) => poisoned(&error),
                }
                if !graphics.rendered_at(size) {
                    return Err(EngineError::Render(
                        "no frame has been rendered to capture".to_owned(),
                    ));
                }
                let pixels = graphics.capture_frame(size)?;
                Ok(Screenshot { size, pixels })
            }
        }
    }

    /// Runs a main-thread script to completion on the calling thread over
    /// the shared tree. The headless composition.
    #[cfg(feature = "quickjs")]
    pub fn run_script(&mut self, source: &str) -> Result<(), ScriptRunError> {
        let mut runtime = crate::quickjs::MainThreadRuntime::new(Arc::clone(&self.elements), || {})
            .map_err(ScriptRunError::Initialization)?;
        let result = runtime
            .run_main_thread_script(source)
            .map_err(ScriptRunError::Script);
        self.refresh();
        result
    }

    /// Spawns the Lynx main thread: the `QuickJS` realm running `source`
    /// over the shared tree. Every committed `__FlushElementTree` asks the
    /// presenting side for a frame; completion arrives as
    /// [`EngineEvent::ScriptFinished`] after `wakeup` fires.
    #[cfg(feature = "quickjs")]
    pub fn spawn_script(
        &mut self,
        source: String,
        wakeup: impl Fn() + Send + Sync + 'static,
    ) -> Result<(), EngineError> {
        let elements = Arc::clone(&self.elements);
        let sender = self.message_sender.clone();
        let on_flush = self.request_frame.clone();
        std::thread::Builder::new()
            .name("bobcat-main".to_owned())
            .spawn(move || {
                let result = (|| {
                    let mut runtime = crate::quickjs::MainThreadRuntime::new(elements, move || {
                        if let Some(request_frame) = &on_flush {
                            request_frame();
                        }
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

/// Blocking lock with the crate's poison policy.
fn lock(elements: &Mutex<ElementTree>) -> MutexGuard<'_, ElementTree> {
    elements.lock().unwrap_or_else(|error| poisoned(&error))
}

/// A poisoned tree lock means the other thread crashed mid-mutation;
/// nothing can be trusted, so crash loudly (let-it-crash).
fn poisoned<G>(error: &dyn fmt::Display) -> G {
    panic!("the element tree lock is poisoned: {error}")
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

    /// The two-thread composition, windowless: the script mutates the
    /// shared tree from the engine-owned main thread, and the embedder side
    /// of the loop pumps lifecycle events after each wakeup.
    #[cfg(feature = "quickjs")]
    #[test]
    fn a_spawned_script_mutates_the_shared_tree() {
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

        let finished = loop {
            wakeups.recv().expect("the script thread wakes the loop");
            let done = engine
                .pump()
                .into_iter()
                .map(|event| match event {
                    EngineEvent::ScriptFinished(result) => result,
                })
                .next();
            if let Some(result) = done {
                break result;
            }
        };
        finished.expect("the script must boot");

        let elements = engine.elements();
        assert!(elements.page().is_some(), "the page was created");
        assert!(elements.element(2).is_some(), "the first view is live");
        assert!(elements.element(3).is_some(), "the second view is live");
        assert!(
            !elements.has_uncommitted_mutations(),
            "the boot's final flush closed the batch"
        );
    }
}
