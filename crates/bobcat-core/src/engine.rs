//! The engine half of a running Lynx view, behind the embedder boundary.
//!
//! An embedder provides exactly five things: user input, device metrics,
//! OS initialization (its event loop and window), a draw target, and IO
//! primitives (resource bytes in, pixels out). It never starts or steers the
//! internal pipeline. Its event handlers are relays — they hand the engine
//! an OS fact (`dispatch_input`, `resize`, `notify_redraw`, `pump`) and the
//! engine decides what the pipeline does with it, requesting frames itself
//! through the [`Window`] capabilities supplied at attach time.
//!
//! # Two threads, one hand-off slot
//!
//! The document has exactly one holder at any instant; [`SharedTree`] is
//! the slot it changes hands through:
//!
//! - **The Lynx main thread** (engine-owned, spawned by [`Engine::spawn_script`]): the injected
//!   JavaScript VM and its job loop. A batch's first `bobcat` call takes the document out of the
//!   slot; every call after that is a plain `&mut` mutation with no synchronization at all;
//!   `__FlushElementTree` runs the style + layout commit on the taken document, puts it back, and
//!   asks for a frame. Locks are touched twice per batch, not per call.
//! - **The presenting side** (the thread the embedder calls the engine from — its OS event loop):
//!   input routing, scrolling, frame production (paint-order build + scene encode), GPU submission,
//!   and present. It borrows the document from the slot non-blockingly: an empty slot (a batch is
//!   open) or a busy slot lock means re-present the retained target, buffer the input, and retry
//!   next frame.
//!
//! The slot is occupied while the script merely computes, which is the
//! point: a long JavaScript task between batches does not stop the
//! presenting side from scrolling — target resolution reads the retained
//! paint order, the offset lands in the document, and the next frame is
//! produced and presented without the script's cooperation. A half-applied
//! batch is unobservable while the slot is empty; once an evaluation ends
//! the document is back, and whatever state it holds may present — the
//! same visibility web-core has, where the browser paints the live DOM on
//! its own schedule regardless of `__FlushElementTree`.
//!
//! The law: the main thread waits only on its own batch boundaries; the
//! presenting side never waits on the main thread; present's vsync wait
//! happens outside any borrow, so it blocks no one.

mod graphics;

use std::collections::VecDeque;
use std::fmt;
use std::marker::PhantomData;
use std::ops::{Deref, DerefMut};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::rc::Rc;
#[cfg(target_arch = "wasm32")]
use std::sync::OnceLock;
#[cfg(target_arch = "wasm32")]
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, TryLockError, mpsc};
#[cfg(not(target_arch = "wasm32"))]
use std::thread::Builder as ThreadBuilder;

#[cfg(test)]
use dom::FontBlob;
use dom::input::InputEvent;
use dom::render::gpu::Headless;
use dom::vello::peniko::Color;
#[cfg(target_arch = "wasm32")]
use wasm_thread::Builder as ThreadBuilder;

use self::graphics::WindowGraphics;
pub use self::graphics::WindowTarget;
use crate::tree::{LynxDocument, PageConfig, Viewport, new_document};

/// The physical pixel size of the render target.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameSize {
    pub width: u32,
    pub height: u32,
}

const MAX_RENDER_DIMENSION: u32 = 16_384;

#[cfg(target_arch = "wasm32")]
static WASM_STYLE_THREAD_COUNT: AtomicUsize = AtomicUsize::new(1);
#[cfg(target_arch = "wasm32")]
static WASM_STYLE_POOL: OnceLock<Result<(), String>> = OnceLock::new();
#[cfg(target_arch = "wasm32")]
static WASM_SCRIPT_OWNER_CLAIMED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);
#[cfg(all(target_arch = "wasm32", panic = "abort"))]
static WASM_SCRIPT_PANIC_HOOK: OnceLock<()> = OnceLock::new();

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum EngineError {
    #[error("invalid viewport: {0}")]
    Viewport(String),
    #[error("GPU operation failed: {0}")]
    Gpu(String),
    #[error("rendering failed: {0}")]
    Render(String),
    #[error("could not start the {name} thread: {message}")]
    Thread { name: &'static str, message: String },
    #[error("this view has already started its entry script")]
    ScriptAlreadyStarted,
    #[error("the engine has no draw target attached")]
    NoDrawTarget,
}

/// Configures the Worker bootstrap shared by the engine-owned Lynx main
/// thread and Stylo workers in a Wasm build.
///
/// This is an OS bootstrap capability only; the spawned task and document
/// remain private to the engine. `style_thread_count` must be at least two:
/// index zero is the entry-task owner and at least one managed Rayon worker
/// must remain after that synchronous task exits.
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
    WASM_STYLE_THREAD_COUNT.store(style_thread_count, Ordering::Release);
    wasm_thread::Builder::empty()
        .worker_script_url(worker_script_url)
        .set_default();
    Ok(())
}

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ScriptRunError {
    #[error("could not initialize the main-thread runtime: {0}")]
    Initialization(#[source] crate::script::ScriptError),
    #[error("main-thread script failed: {0}")]
    Script(#[source] crate::script::ScriptError),
    #[error("could not initialize the script thread: {0}")]
    Platform(String),
}

/// A message crossing from an engine-owned thread.
enum EngineMessage {
    /// The main-thread script ran to completion (or failed) on its thread.
    ScriptDone(Result<(), ScriptRunError>),
}

#[derive(Debug)]
#[non_exhaustive]
pub enum EngineEvent {
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

/// The embedder's window: the draw target it lends and the detachable frame
/// capability the engine schedules through. The embedder provides the
/// mechanisms; the engine decides when to invoke them.
///
/// The engine is generic over this trait, so every call here is a direct one
/// — a window is a type, not a set of boxed closures. The draw target is a
/// GAT, which lets native surfaces borrow an embedder-owned window. Browser
/// embedders can instead attach an owned canvas target through
/// [`crate::LynxView::attach_target`].
pub trait Window {
    type Target<'window>: Into<WindowTarget<'window>>
    where
        Self: 'window;

    type Frames: FrameRequester;

    fn target(&self) -> Self::Target<'_>;

    fn frames(&self) -> Self::Frames;
}

/// A window's scheduling capability, held apart from the draw target because
/// frame requests travel to engine-owned threads while pre-present callbacks
/// stay on the presenting side.
pub trait FrameRequester: Send + Sync + 'static {
    fn request_frame(&self);

    /// Called on the presenting side immediately before presenting. Native
    /// window handles use this for mechanisms such as winit's
    /// `Window::pre_present_notify`; browser frame signals can keep the
    /// default no-op.
    fn pre_present(&self) {}
}

#[derive(Debug)]
pub enum NoWindow {}

impl Window for NoWindow {
    type Target<'window> = WindowTarget<'window>;
    type Frames = Self;

    fn target(&self) -> Self::Target<'_> {
        match *self {}
    }

    fn frames(&self) -> Self::Frames {
        match *self {}
    }
}

impl FrameRequester for NoWindow {
    fn request_frame(&self) {
        match *self {}
    }
}

#[cfg(test)]
pub(crate) type OffscreenEngine = Engine<'static, NoWindow>;

/// The hand-off slot for the one document.
#[derive(Clone)]
pub(crate) struct SharedTree {
    slot: Arc<Mutex<Option<LynxDocument>>>,
}

impl fmt::Debug for SharedTree {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("SharedTree").finish_non_exhaustive()
    }
}

impl SharedTree {
    #[must_use]
    pub(crate) fn new(tree: LynxDocument) -> Self {
        Self {
            slot: Arc::new(Mutex::new(Some(tree))),
        }
    }

    /// Blocking borrow for setup and observation.
    #[must_use]
    #[cfg(test)]
    pub(crate) fn tree(&self) -> TreeGuard<'_> {
        let guard = self.lock();
        assert!(
            guard.is_some(),
            "a PAPI batch is open: the Lynx main thread holds the tree"
        );
        TreeGuard(guard)
    }

    pub(crate) fn try_tree(&self) -> Option<TreeGuard<'_>> {
        match self.slot.try_lock() {
            Ok(guard) if guard.is_some() => Some(TreeGuard(guard)),
            Ok(_) | Err(TryLockError::WouldBlock) => None,
            Err(TryLockError::Poisoned(error)) => {
                panic!("the tree slot is poisoned: {error}")
            }
        }
    }

    /// Takes the tree out to open a batch. Blocks only for the presenting
    /// side's brief borrows — the script may wait on the engine.
    ///
    /// # Panics
    ///
    /// Panics if a batch is already open: there is one main thread.
    pub(crate) fn take(&self) -> LynxDocument {
        self.lock()
            .take()
            .expect("the tree was already taken: only one batch can be open")
    }

    /// Puts the tree back at a batch boundary.
    ///
    /// # Panics
    ///
    /// Panics if the slot is occupied: the tree cannot be returned twice.
    pub(crate) fn put(&self, tree: LynxDocument) {
        let mut guard = self.lock();
        assert!(
            guard.is_none(),
            "the slot is occupied: the tree was returned twice"
        );
        *guard = Some(tree);
    }

    fn lock(&self) -> MutexGuard<'_, Option<LynxDocument>> {
        self.slot
            .lock()
            .unwrap_or_else(|error| panic!("the tree slot is poisoned: {error}"))
    }
}

/// A borrow of the document from its slot.
#[derive(Debug)]
pub(crate) struct TreeGuard<'a>(MutexGuard<'a, Option<LynxDocument>>);

impl Deref for TreeGuard<'_> {
    type Target = LynxDocument;
    fn deref(&self) -> &LynxDocument {
        self.0
            .as_ref()
            .expect("a TreeGuard is only built over an occupied slot")
    }
}

impl DerefMut for TreeGuard<'_> {
    fn deref_mut(&mut self) -> &mut LynxDocument {
        self.0
            .as_mut()
            .expect("a TreeGuard is only built over an occupied slot")
    }
}

/// The attached output, if any.
enum Output<'window> {
    None,
    Offscreen(Box<Headless>),
    /// A window: the presentation stack lives here, on the thread the
    /// embedder calls the engine from, and its surface borrows the
    /// embedder's window for exactly as long as it does.
    Window(Box<WindowGraphics<'window>>),
}

/// The engine half of a Lynx view: the shared element tree, input routing,
/// frame production, presentation, and the engine-owned script thread.
///
/// Generic over the embedder's [`Window`] capability types. A native surface
/// may borrow its window for the life of the engine; an owned browser canvas
/// target does not. The public windowless composition is
/// [`crate::OffscreenLynxView`].
///
/// Deliberately `!Send`: it lives on the thread the embedder calls it from.
pub(crate) struct Engine<'window, W: Window> {
    elements: SharedTree,
    script_owner_available: bool,
    viewport: Viewport,
    frame_size: FrameSize,
    messages: mpsc::Receiver<EngineMessage>,
    message_sender: mpsc::Sender<EngineMessage>,
    output: Output<'window>,
    /// The window's frame-request handle, behind `Arc` so the Lynx main
    /// thread always observes the currently attached target rather than a
    /// startup-time snapshot.
    frames: Arc<Mutex<Option<Arc<W::Frames>>>>,
    pending_input: VecDeque<InputEvent>,
    pending_resize: Option<(f32, f32, f32)>,
    thread_bound: PhantomData<Rc<()>>,
}

impl<W: Window> fmt::Debug for Engine<'_, W> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Engine")
            .field("viewport", &self.viewport)
            .field("frame_size", &self.frame_size)
            .field("pending_input", &self.pending_input.len())
            .finish_non_exhaustive()
    }
}

impl<'window, W: Window> Engine<'window, W> {
    /// Builds the engine and its element tree at the given CSS viewport.
    pub(crate) fn new(
        config: PageConfig,
        width: f32,
        height: f32,
        device_pixel_ratio: f32,
    ) -> Result<Self, EngineError> {
        let frame_size = frame_size(width, height, device_pixel_ratio)?;
        let viewport = Viewport::new(width, height).with_device_pixel_ratio(device_pixel_ratio);
        let (message_sender, messages) = mpsc::channel();
        let elements = SharedTree::new(new_document(viewport, config));
        Ok(Self {
            script_owner_available: true,
            elements,
            viewport,
            frame_size,
            messages,
            message_sender,
            output: Output::None,
            frames: Arc::new(Mutex::new(None)),
            pending_input: VecDeque::new(),
            pending_resize: None,
            thread_bound: PhantomData,
        })
    }

    /// A blocking borrow of the document, for observation and setup.
    #[must_use]
    #[cfg(test)]
    pub(crate) fn elements(&self) -> TreeGuard<'_> {
        self.elements.tree()
    }

    /// Hands the private slot to the one engine-owned script task. A second
    /// task could otherwise open an overlapping Element-PAPI batch.
    fn take_script_tree(&mut self) -> Result<SharedTree, EngineError> {
        if !self.script_owner_available {
            return Err(EngineError::ScriptAlreadyStarted);
        }
        self.script_owner_available = false;
        Ok(self.elements.clone())
    }

    /// The current physical render-target size in device pixels.
    #[must_use]
    pub(crate) const fn frame_size(&self) -> FrameSize {
        self.frame_size
    }

    /// Registers shared font data for text measurement without copying it.
    #[cfg(test)]
    pub(crate) fn register_fonts(&mut self, data: FontBlob) -> usize {
        let registered = self.elements.tree().register_fonts(data);
        if registered > 0 {
            self.refresh();
        }
        registered
    }

    fn drain_deferred(
        pending_resize: &mut Option<(f32, f32, f32)>,
        pending_input: &mut VecDeque<InputEvent>,
        tree: &mut LynxDocument,
    ) {
        if let Some((width, height, ratio)) = pending_resize.take() {
            tree.set_viewport(width, height);
            tree.set_device_pixel_ratio(ratio);
        }
        while let Some(event) = pending_input.pop_front() {
            tree.handle_input(event);
        }
    }

    /// Routes one host input event on the presenting side.
    pub(crate) fn dispatch_input(&mut self, event: InputEvent) {
        self.pending_input.push_back(event);
        let needs_frame = match self.elements.try_tree() {
            Some(mut tree) => {
                Self::drain_deferred(&mut self.pending_resize, &mut self.pending_input, &mut tree);
                tree.needs_render()
            }
            None => false,
        };
        if needs_frame {
            self.refresh();
        }
    }

    /// Applies new device metrics from the embedder.
    pub(crate) fn resize(
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
        match self.elements.try_tree() {
            Some(mut tree) => {
                if size_changed {
                    tree.set_viewport(width, height);
                }
                if scale_changed {
                    tree.set_device_pixel_ratio(device_pixel_ratio);
                }
            }
            None => self.pending_resize = Some((width, height, device_pixel_ratio)),
        }
        self.viewport = Viewport::new(width, height).with_device_pixel_ratio(device_pixel_ratio);
        self.frame_size = next_size;
        self.refresh();
        Ok(())
    }

    /// Asks the OS for a frame through the window's frame-request handle.
    pub(crate) fn refresh(&self) {
        if let Some(frames) = self.frame_requester() {
            frames.request_frame();
        }
    }

    fn frame_requester(&self) -> Option<Arc<W::Frames>> {
        self.frames
            .lock()
            .unwrap_or_else(|error| panic!("the frame-requester slot is poisoned: {error}"))
            .clone()
    }

    /// Drains lifecycle messages from engine-owned threads.
    pub(crate) fn pump(&mut self) -> Vec<EngineEvent> {
        let mut events = Vec::new();
        while let Ok(message) = self.messages.try_recv() {
            match message {
                EngineMessage::ScriptDone(result) => {
                    events.push(EngineEvent::ScriptFinished(result));
                }
            }
        }
        events
    }

    /// Attaches an offscreen GPU target.
    pub(crate) fn attach_offscreen(&mut self) -> Result<(), EngineError> {
        let gpu = Headless::new().map_err(|error| EngineError::Gpu(error.to_string()))?;
        self.output = Output::Offscreen(Box::new(gpu));
        Ok(())
    }

    /// Attaches the embedder's window as the draw target: the whole
    /// presentation stack is created here, on the calling thread, and stays
    /// here — presentation and vsync interact with the OS only on this
    /// thread. The surface borrows the window, which therefore outlives the
    /// engine.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn attach_window(
        &mut self,
        window: &'window W,
        size: FrameSize,
    ) -> Result<(), EngineError> {
        pollster::block_on(self.attach_window_async(window, size))
    }

    /// Asynchronously attaches the embedder's window as the draw target.
    ///
    /// Native embedders may use this internally when they already have an
    /// async initialization context; the public synchronous convenience is
    /// [`crate::LynxView::attach_window`]. Browser embedders attach an owned
    /// canvas through [`crate::LynxView::attach_target`] instead.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) async fn attach_window_async(
        &mut self,
        window: &'window W,
        size: FrameSize,
    ) -> Result<(), EngineError> {
        self.attach_target(window.target(), window.frames(), size)
            .await
    }

    /// Attaches an already-owned surface target and its frame capability.
    ///
    /// This is the browser-friendly form: `SurfaceTarget::Canvas` owns a
    /// JavaScript canvas reference, so the Wasm wrapper does not need a
    /// self-referential Rust struct merely to keep a `Window` borrow alive.
    pub(crate) async fn attach_target(
        &mut self,
        target: impl Into<WindowTarget<'window>>,
        frames: W::Frames,
        size: FrameSize,
    ) -> Result<(), EngineError> {
        let graphics = WindowGraphics::new(target, size).await?;
        self.output = Output::Window(Box::new(graphics));
        *self
            .frames
            .lock()
            .unwrap_or_else(|error| panic!("the frame-requester slot is poisoned: {error}")) =
            Some(Arc::new(frames));
        self.refresh();
        Ok(())
    }

    /// Relays the OS's "the window wants a frame" fact.
    pub(crate) fn notify_redraw(&mut self) -> Result<(), EngineError> {
        let frames = self.frame_requester();
        let Output::Window(graphics) = &mut self.output else {
            return Ok(());
        };
        let size = self.frame_size;
        let tree_was_busy = match self.elements.try_tree() {
            Some(mut tree) => {
                Self::drain_deferred(&mut self.pending_resize, &mut self.pending_input, &mut tree);
                let produced = tree.render();
                if produced || !graphics.rendered_at(size) {
                    graphics.render_to_target(&tree.scene(), size)?;
                }
                false
            }
            None => true,
        };
        let frames = frames
            .as_deref()
            .expect("a window output always installs its frame capability");
        // A script/DOM batch can take the slot after requesting this frame.
        // Keep the request alive so event-driven embedders retry instead of
        // losing the only wakeup while presenting the retained target.
        if tree_was_busy {
            frames.request_frame();
        }
        if graphics.rendered_at(size) {
            graphics.present(frames)?;
        }
        Ok(())
    }

    /// Renders one frame to the offscreen target if the document changed (or unconditionally with
    /// `force`), returning whether a frame was submitted.
    pub(crate) fn tick(&mut self, force: bool) -> Result<bool, EngineError> {
        let Output::Offscreen(gpu) = &mut self.output else {
            return Err(EngineError::NoDrawTarget);
        };
        let Some(mut tree) = self.elements.try_tree() else {
            return Ok(false);
        };
        Self::drain_deferred(&mut self.pending_resize, &mut self.pending_input, &mut tree);
        let changed = tree.render();
        if !changed && !force {
            return Ok(false);
        }
        gpu.render_frame(
            &tree.scene(),
            self.frame_size.width,
            self.frame_size.height,
            Color::WHITE,
        )
        .map_err(|error| EngineError::Gpu(error.to_string()))?;
        gpu.wait_idle()
            .map_err(|error| EngineError::Gpu(error.to_string()))?;
        Ok(true)
    }

    /// Captures the current frame as pixels — synchronously, from whichever
    /// target is attached. Renders first if the document changed and the
    /// tree is available; a tree busy mid-commit (window mode) captures the
    /// retained frame, which is what the window is showing.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn capture(&mut self) -> Result<Screenshot, EngineError> {
        let size = self.frame_size;
        match &mut self.output {
            Output::None => Err(EngineError::NoDrawTarget),
            Output::Offscreen(gpu) => {
                if let Some(mut tree) = self.elements.try_tree()
                    && tree.render()
                {
                    gpu.render_frame(&tree.scene(), size.width, size.height, Color::WHITE)
                        .map_err(|error| EngineError::Gpu(error.to_string()))?;
                }
                let pixels = gpu
                    .read_pixels()
                    .map_err(|error| EngineError::Gpu(error.to_string()))?;
                Ok(Screenshot { size, pixels })
            }
            Output::Window(graphics) => {
                if let Some(mut tree) = self.elements.try_tree() {
                    let produced = tree.render();
                    if produced || !graphics.rendered_at(size) {
                        graphics.render_to_target(&tree.scene(), size)?;
                    }
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

    /// Spawns the Lynx main thread and creates its injected JavaScript VM on
    /// that owner thread before running `source`.
    pub(crate) fn spawn_script(
        &mut self,
        source: String,
        source_name: String,
        factory: Arc<dyn crate::script::ScriptEngineFactory>,
    ) -> Result<(), EngineError> {
        let elements = self.take_script_tree()?;
        #[cfg(target_arch = "wasm32")]
        if WASM_SCRIPT_OWNER_CLAIMED.swap(true, Ordering::AcqRel) {
            self.script_owner_available = true;
            return Err(EngineError::Thread {
                name: "script",
                message: "one Wasm instance supports one Lynx view; create each view in its own Render Worker"
                    .to_owned(),
            });
        }
        let sender = self.message_sender.clone();
        let frame_requesters = Arc::clone(&self.frames);
        let spawn = ThreadBuilder::new()
            .name("bobcat-main".to_owned())
            .spawn(move || {
                #[cfg(all(target_arch = "wasm32", panic = "abort"))]
                install_script_panic_hook(sender.clone(), Arc::clone(&frame_requesters));
                let on_flush = Arc::clone(&frame_requesters);
                let result = catch_unwind(AssertUnwindSafe(|| {
                    (|| {
                        #[cfg(target_arch = "wasm32")]
                        prepare_script_thread()?;
                        let mut runtime = crate::runtime::MainThreadRuntime::new(
                            factory.as_ref(),
                            elements,
                            move || {
                                request_current_frame(&on_flush);
                            },
                        )
                        .map_err(|error| {
                            ScriptRunError::Initialization(error.into_script_error())
                        })?;
                        runtime
                            .run_main_thread_script(&source, &source_name)
                            .map_err(|error| ScriptRunError::Script(error.into_script_error()))
                    })()
                }))
                .unwrap_or_else(|payload| {
                    Err(ScriptRunError::Platform(format!(
                        "the injected VM panicked: {}",
                        panic_payload(payload.as_ref())
                    )))
                });
                let _ = sender.send(EngineMessage::ScriptDone(result));
                request_current_frame(&frame_requesters);
            });
        if let Err(error) = spawn {
            self.script_owner_available = true;
            #[cfg(target_arch = "wasm32")]
            WASM_SCRIPT_OWNER_CLAIMED.store(false, Ordering::Release);
            return Err(EngineError::Thread {
                name: "script",
                message: error.to_string(),
            });
        }
        Ok(())
    }
}

fn request_current_frame<F: FrameRequester>(frames: &Mutex<Option<Arc<F>>>) {
    let current = frames
        .lock()
        .unwrap_or_else(|error| panic!("the frame-requester slot is poisoned: {error}"))
        .clone();
    if let Some(frames) = current {
        frames.request_frame();
    }
}

#[cfg(all(target_arch = "wasm32", panic = "abort"))]
fn install_script_panic_hook<F: FrameRequester>(
    sender: mpsc::Sender<EngineMessage>,
    frames: Arc<Mutex<Option<Arc<F>>>>,
) {
    WASM_SCRIPT_PANIC_HOOK.get_or_init(|| {
        std::panic::set_hook(Box::new(move |info| {
            let location = info
                .location()
                .map_or_else(String::new, |location| format!(" at {location}"));
            let _ = sender.send(EngineMessage::ScriptDone(Err(ScriptRunError::Platform(
                format!(
                    "the script Worker aborted after a panic{location}: {}",
                    panic_payload(info.payload())
                ),
            ))));
            request_current_frame(&frames);
        }));
    });
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
fn prepare_script_thread() -> Result<(), ScriptRunError> {
    WASM_STYLE_POOL
        .get_or_init(|| {
            let thread_count = WASM_STYLE_THREAD_COUNT.load(Ordering::Acquire);
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
                .map_err(|_| "Stylo's embedder thread pool was installed twice".to_owned())?;
            Ok(())
        })
        .clone()
        .map_err(ScriptRunError::Platform)
}

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

    #[test]
    fn resource_bytes_reach_font_registration_without_copying() {
        use bytes::Bytes;
        use dom::FontBlob;

        use super::OffscreenEngine;
        use crate::tree::PageConfig;

        let bytes = Bytes::from_static(b"not a font");
        let original = bytes.as_ptr();
        let blob = FontBlob::new(bytes);
        assert_eq!(blob.as_ref().as_ptr(), original);

        let mut engine =
            OffscreenEngine::new(PageConfig::default(), 393.0, 727.0, 1.0).expect("engine");
        assert_eq!(engine.register_fonts(blob), 0);
    }

    #[test]
    fn the_script_slot_is_unique_and_hides_an_open_batch() {
        use super::OffscreenEngine;
        use crate::tree::PageConfig;

        let mut engine =
            OffscreenEngine::new(PageConfig::default(), 393.0, 727.0, 1.0).expect("engine");
        let script_tree = engine
            .take_script_tree()
            .expect("the engine creates one script owner");
        assert!(
            engine.take_script_tree().is_err(),
            "the engine cannot create a second mutation owner"
        );

        let mut tree = script_tree.take();
        let page = tree.document_element().id();
        let view = tree.create_element("view", ());
        tree.insert_before(page, view, None);
        assert!(
            engine.elements.try_tree().is_none(),
            "the presenting side cannot observe a half-applied batch"
        );

        tree.layout();
        script_tree.put(tree);
        assert!(engine.elements().is_connected(view));
    }

    #[cfg(feature = "quickjs")]
    #[test]
    fn a_spawned_script_mutates_the_shared_tree() {
        use std::time::{Duration, Instant};

        use super::{EngineEvent, OffscreenEngine};
        use crate::tree::PageConfig;

        let mut engine =
            OffscreenEngine::new(PageConfig::default(), 393.0, 727.0, 1.0).expect("engine");
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
                "test-entry.js".to_owned(),
                crate::quickjs::engine_factory(),
            )
            .expect("script thread");

        let deadline = Instant::now() + Duration::from_secs(5);
        let finished = loop {
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
            assert!(Instant::now() < deadline, "script thread timed out");
            std::thread::yield_now();
        };
        finished.expect("the script must boot");

        let elements = engine.elements();
        assert!(elements.is_connected(2), "the first view is attached");
        assert!(elements.is_connected(3), "the second view is attached");
        assert!(
            elements.rounded_layout(1).is_some(),
            "the boot's final flush laid the page out"
        );
    }
}
