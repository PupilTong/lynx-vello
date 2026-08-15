//! The engine half of a running Lynx view, behind the embedder boundary.
//!
//! An embedder provides exactly five things: user input, device metrics,
//! OS initialization (its event loop and window), a draw target, and IO
//! primitives (bundle bytes in, pixels out). It never starts or steers the
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
//! - **The Lynx main thread** (engine-owned, spawned by [`Engine::spawn_script`]): the `QuickJS`
//!   realm and its event loop. A batch's first `bobcat` call takes the document out of the slot;
//!   every call after that is a plain `&mut` mutation with no synchronization at all;
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
use std::rc::Rc;
#[cfg(feature = "quickjs")]
use std::sync::mpsc;
use std::sync::{Arc, Mutex, MutexGuard, TryLockError};

use dom::input::InputEvent;
use dom::render::gpu::{GpuError, Headless};
use dom::vello::peniko::Color;
use dom::{FontBlob, ImageStore, StylesheetOrigin};

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
    #[error("the engine's Lynx main-thread document owner was already taken")]
    MainThreadAlreadyTaken,
    #[error("the engine has no draw target attached")]
    NoDrawTarget,
}

#[cfg(feature = "quickjs")]
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ScriptRunError {
    #[error("could not initialize the main-thread runtime: {0}")]
    Initialization(#[source] crate::quickjs::QuickJsInitializationError),
    #[error("the engine's Lynx main-thread document owner was already taken")]
    MainThreadAlreadyTaken,
    #[error(transparent)]
    Script(crate::quickjs::MainThreadError),
}

/// A message crossing from an engine-owned thread.
#[cfg(feature = "quickjs")]
enum EngineMessage {
    /// The main-thread script ran to completion (or failed) on its thread.
    ScriptDone(Result<(), ScriptRunError>),
}

#[derive(Debug)]
#[non_exhaustive]
pub enum EngineEvent {
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

/// The embedder's window: the draw target it lends and the detachable frame
/// capability the engine schedules through. The embedder provides the
/// mechanisms; the engine decides when to invoke them.
///
/// The engine is generic over this trait, so every call here is a direct one
/// — a window is a type, not a set of boxed closures. The draw target is a
/// GAT, which lets native surfaces borrow an embedder-owned window. Browser
/// embedders can instead attach an owned canvas target through
/// [`Engine::attach_target`].
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

pub type OffscreenEngine = Engine<'static, NoWindow>;

/// The hand-off slot for the one document.
#[derive(Clone)]
pub struct SharedTree {
    slot: Arc<Mutex<Option<LynxDocument>>>,
}

impl fmt::Debug for SharedTree {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("SharedTree").finish_non_exhaustive()
    }
}

impl SharedTree {
    #[must_use]
    pub fn new(tree: LynxDocument) -> Self {
        Self {
            slot: Arc::new(Mutex::new(Some(tree))),
        }
    }

    /// Blocking borrow for setup and observation.
    #[must_use]
    pub fn tree(&self) -> TreeGuard<'_> {
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

/// The engine-owned Lynx main thread's side of the document hand-off.
///
/// An [`Engine`] creates the document and can transfer this owner exactly
/// once to the runtime that permanently runs its Element PAPI. The first
/// mutation in a batch takes the document out of [`SharedTree`]; subsequent
/// mutations are ordinary `&mut` accesses, and [`Self::flush`] lays it out and
/// returns it to the presenting side. Dropping the owner also returns an open
/// batch, without turning runtime teardown into an implicit flush.
///
/// This type is `Send` and contains no browser or GPU handles, so a browser
/// embedder can move it into a shared-memory Worker through `wasm_thread` while
/// keeping the `Engine` and its draw target on the presenting Worker.
pub struct MainThreadDocument {
    slot: SharedTree,
    taken: Option<LynxDocument>,
}

impl fmt::Debug for MainThreadDocument {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MainThreadDocument")
            .field("batch_open", &self.taken.is_some())
            .finish_non_exhaustive()
    }
}

impl MainThreadDocument {
    fn new(slot: SharedTree) -> Self {
        Self { slot, taken: None }
    }

    /// Opens the current mutation batch if needed and returns its document.
    pub fn document(&mut self) -> &mut LynxDocument {
        if self.taken.is_none() {
            self.taken = Some(self.slot.take());
        }
        self.taken
            .as_mut()
            .expect("the main thread just took the document")
    }

    /// Commits the current batch through style and layout and returns the
    /// document to the presenting side.
    pub fn flush(&mut self) {
        let mut tree = match self.taken.take() {
            Some(tree) => tree,
            None => self.slot.take(),
        };
        tree.layout();
        self.slot.put(tree);
    }

    fn release(&mut self) {
        if let Some(tree) = self.taken.take() {
            self.slot.put(tree);
        }
    }

    #[cfg(feature = "quickjs")]
    fn shared_tree(&self) -> SharedTree {
        self.slot.clone()
    }
}

impl Drop for MainThreadDocument {
    fn drop(&mut self) {
        self.release();
    }
}

/// A borrow of the document from its slot.
#[derive(Debug)]
pub struct TreeGuard<'a>(MutexGuard<'a, Option<LynxDocument>>);

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
/// target does not. [`OffscreenEngine`] is the composition with no window at
/// all.
///
/// Deliberately `!Send`: it lives on the thread the embedder calls it from.
pub struct Engine<'window, W: Window> {
    elements: SharedTree,
    main_thread: Option<MainThreadDocument>,
    viewport: Viewport,
    frame_size: FrameSize,
    #[cfg(feature = "quickjs")]
    messages: mpsc::Receiver<EngineMessage>,
    #[cfg(feature = "quickjs")]
    message_sender: mpsc::Sender<EngineMessage>,
    output: Output<'window>,
    /// The window's frame-request handle, behind `Arc` so the Lynx main
    /// thread keeps one of its own.
    frames: Option<Arc<W::Frames>>,
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
    pub fn new(
        config: PageConfig,
        width: f32,
        height: f32,
        device_pixel_ratio: f32,
    ) -> Result<Self, EngineError> {
        let frame_size = frame_size(width, height, device_pixel_ratio)?;
        let viewport = Viewport::new(width, height).with_device_pixel_ratio(device_pixel_ratio);
        #[cfg(feature = "quickjs")]
        let (message_sender, messages) = mpsc::channel();
        let elements = SharedTree::new(new_document(viewport, config));
        Ok(Self {
            main_thread: Some(MainThreadDocument::new(elements.clone())),
            elements,
            viewport,
            frame_size,
            #[cfg(feature = "quickjs")]
            messages,
            #[cfg(feature = "quickjs")]
            message_sender,
            output: Output::None,
            frames: None,
            pending_input: VecDeque::new(),
            pending_resize: None,
            thread_bound: PhantomData,
        })
    }

    /// A blocking borrow of the document, for observation and setup.
    #[must_use]
    pub fn elements(&self) -> TreeGuard<'_> {
        self.elements.tree()
    }

    /// Transfers the document's unique mutation owner to an external Lynx
    /// main-thread runtime.
    ///
    /// The presenting [`Engine`] keeps only the non-blocking [`SharedTree`]
    /// side. A second call fails because two main threads could otherwise open
    /// overlapping Element-PAPI batches.
    pub fn take_main_thread_document(&mut self) -> Result<MainThreadDocument, EngineError> {
        self.main_thread
            .take()
            .ok_or(EngineError::MainThreadAlreadyTaken)
    }

    /// The current physical render-target size in device pixels.
    #[must_use]
    pub const fn frame_size(&self) -> FrameSize {
        self.frame_size
    }

    /// Mounts author CSS.
    pub fn add_author_stylesheet(&mut self, css: &str) {
        self.elements
            .tree()
            .add_stylesheet(css, StylesheetOrigin::Author);
        self.refresh();
    }

    /// Registers shared font data for text measurement without copying it.
    pub fn register_fonts(&mut self, data: FontBlob) -> usize {
        let registered = self.elements.tree().register_fonts(data);
        if registered > 0 {
            self.refresh();
        }
        registered
    }

    /// Registers or updates decoded images, then keeps the next frame fresh.
    pub fn with_images<R>(&mut self, update: impl FnOnce(&mut ImageStore) -> R) -> R {
        let result = update(self.elements.tree().images_mut());
        self.refresh();
        result
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
    pub fn dispatch_input(&mut self, event: InputEvent) {
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
    pub fn refresh(&self) {
        if let Some(frames) = &self.frames {
            frames.request_frame();
        }
    }

    /// Drains lifecycle messages from engine-owned threads.
    pub fn pump(&mut self) -> Vec<EngineEvent> {
        #[cfg(feature = "quickjs")]
        {
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

        #[cfg(not(feature = "quickjs"))]
        {
            Vec::new()
        }
    }

    /// Attaches an offscreen GPU target.
    pub fn attach_offscreen(&mut self) -> Result<(), EngineError> {
        let gpu = Headless::new().map_err(EngineError::Gpu)?;
        self.output = Output::Offscreen(Box::new(gpu));
        Ok(())
    }

    /// Attaches the embedder's window as the draw target: the whole
    /// presentation stack is created here, on the calling thread, and stays
    /// here — presentation and vsync interact with the OS only on this
    /// thread. The surface borrows the window, which therefore outlives the
    /// engine.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn attach_window(
        &mut self,
        window: &'window W,
        size: FrameSize,
    ) -> Result<(), EngineError> {
        pollster::block_on(self.attach_window_async(window, size))
    }

    /// Asynchronously attaches the embedder's window as the draw target.
    ///
    /// Browser WebGPU obtains its adapter and device through JavaScript
    /// promises, so embedders targeting Wasm must use this entry rather than
    /// blocking the browser thread. Native embedders may use this directly or
    /// the synchronous [`Self::attach_window`] convenience wrapper.
    pub async fn attach_window_async(
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
    pub async fn attach_target(
        &mut self,
        target: impl Into<WindowTarget<'window>>,
        frames: W::Frames,
        size: FrameSize,
    ) -> Result<(), EngineError> {
        let graphics = WindowGraphics::new(target, size).await?;
        self.output = Output::Window(Box::new(graphics));
        self.frames = Some(Arc::new(frames));
        self.refresh();
        Ok(())
    }

    /// Relays the OS's "the window wants a frame" fact.
    pub fn notify_redraw(&mut self) -> Result<(), EngineError> {
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
        let frames = self
            .frames
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
    pub fn tick(&mut self, force: bool) -> Result<bool, EngineError> {
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
        .map_err(EngineError::Gpu)?;
        gpu.wait_idle().map_err(EngineError::Gpu)?;
        Ok(true)
    }

    /// Captures the current frame as pixels — synchronously, from whichever
    /// target is attached. Renders first if the document changed and the
    /// tree is available; a tree busy mid-commit (window mode) captures the
    /// retained frame, which is what the window is showing.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn capture(&mut self) -> Result<Screenshot, EngineError> {
        let size = self.frame_size;
        match &mut self.output {
            Output::None => Err(EngineError::NoDrawTarget),
            Output::Offscreen(gpu) => {
                if let Some(mut tree) = self.elements.try_tree()
                    && tree.render()
                {
                    gpu.render_frame(&tree.scene(), size.width, size.height, Color::WHITE)
                        .map_err(EngineError::Gpu)?;
                }
                let pixels = gpu.read_pixels().map_err(EngineError::Gpu)?;
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

    /// Runs a main-thread script to completion on the calling thread over the shared tree.
    #[cfg(feature = "quickjs")]
    pub fn run_script(&mut self, source: &str) -> Result<(), ScriptRunError> {
        let main_thread = self
            .main_thread
            .as_ref()
            .ok_or(ScriptRunError::MainThreadAlreadyTaken)?;
        let mut runtime = crate::quickjs::MainThreadRuntime::new(main_thread.shared_tree(), || {})
            .map_err(ScriptRunError::Initialization)?;
        let result = runtime
            .run_main_thread_script(source)
            .map_err(ScriptRunError::Script);
        self.refresh();
        result
    }

    /// Spawns the Lynx main thread: the `QuickJS` realm running `source` over the shared tree.
    #[cfg(feature = "quickjs")]
    pub fn spawn_script(
        &mut self,
        source: String,
        wakeup: impl Fn() + Send + Sync + 'static,
    ) -> Result<(), EngineError> {
        let main_thread = self.take_main_thread_document()?;
        let elements = main_thread.shared_tree();
        let sender = self.message_sender.clone();
        let on_flush = self.frames.clone();
        let spawn = std::thread::Builder::new()
            .name("bobcat-main".to_owned())
            .spawn(move || {
                let result = (|| {
                    let mut runtime = crate::quickjs::MainThreadRuntime::new(elements, move || {
                        if let Some(frames) = &on_flush {
                            frames.request_frame();
                        }
                    })
                    .map_err(ScriptRunError::Initialization)?;
                    runtime
                        .run_main_thread_script(&source)
                        .map_err(ScriptRunError::Script)
                })();
                let _ = sender.send(EngineMessage::ScriptDone(result));
                wakeup();
            });
        if let Err(error) = spawn {
            self.main_thread = Some(main_thread);
            return Err(EngineError::Thread {
                name: "script",
                message: error.to_string(),
            });
        }
        Ok(())
    }
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
    fn main_thread_document_is_unique_and_hides_an_open_batch() {
        use super::OffscreenEngine;
        use crate::tree::PageConfig;

        let mut engine =
            OffscreenEngine::new(PageConfig::default(), 393.0, 727.0, 1.0).expect("engine");
        let mut main_thread = engine
            .take_main_thread_document()
            .expect("the engine creates one main-thread owner");
        assert!(
            engine.take_main_thread_document().is_err(),
            "the engine cannot create a second mutation owner"
        );

        let page = main_thread.document().document_element().id();
        let view = main_thread.document().create_element("view", ());
        main_thread.document().insert_before(page, view, None);
        assert!(
            engine.elements.try_tree().is_none(),
            "the presenting side cannot observe a half-applied batch"
        );

        main_thread.flush();
        assert!(engine.elements().is_connected(view));
    }

    #[cfg(feature = "quickjs")]
    #[test]
    fn a_spawned_script_mutates_the_shared_tree() {
        use std::sync::mpsc;

        use super::{EngineEvent, OffscreenEngine};
        use crate::tree::PageConfig;

        let mut engine =
            OffscreenEngine::new(PageConfig::default(), 393.0, 727.0, 1.0).expect("engine");
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
        assert!(elements.is_connected(2), "the first view is attached");
        assert!(elements.is_connected(3), "the second view is attached");
        assert!(
            elements.rounded_layout(1).is_some(),
            "the boot's final flush laid the page out"
        );
    }
}
