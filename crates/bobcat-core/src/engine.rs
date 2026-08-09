//! The engine half of a running Lynx view, behind the embedder boundary.
//!
//! An embedder provides exactly five things: user input, device metrics,
//! OS initialization (its event loop and window), a draw target, and IO
//! primitives (bundle bytes in, pixels out). It never starts or steers the
//! internal pipeline. Its event handlers are relays — they hand the engine
//! an OS fact (`dispatch_input`, `resize`, `notify_redraw`, `pump`) and the
//! engine decides what the pipeline does with it, requesting frames itself
//! through the [`Window`] it borrowed at attach time.
//!
//! # Two threads, one hand-off slot
//!
//! The element tree has exactly one holder at any instant; [`SharedTree`]
//! is the slot it changes hands through:
//!
//! - **The Lynx main thread** (engine-owned, spawned by [`Engine::spawn_script`]): the `QuickJS`
//!   realm and its event loop. A batch's first Element PAPI mutation takes the tree out of the
//!   slot; every call after that is a plain `&mut` mutation with no synchronization at all;
//!   `__FlushElementTree` runs the style + layout commit on the taken tree, puts it back, and asks
//!   for a frame. Locks are touched twice per batch, not per call.
//! - **The presenting side** (the thread the embedder calls the engine from — its OS event loop):
//!   input routing, scrolling, frame production (paint-order build + scene encode), GPU submission,
//!   and present. It borrows the tree from the slot non-blockingly: an empty slot (a batch is open)
//!   or a busy slot lock means re-present the retained target, buffer the input, and retry next
//!   frame.
//!
//! The slot is occupied while the script merely computes, which is the
//! point: a long JavaScript task between batches does not stop the
//! presenting side from scrolling — target resolution reads the retained
//! paint order, the offset lands in the tree, and the next frame is
//! produced and presented without the script's cooperation. A half-applied
//! batch is unobservable by construction (the tree is simply absent), and
//! [`ElementTree::has_uncommitted_mutations`] guards the one edge where an
//! abandoned batch comes back uncommitted at the end of an evaluation.
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

/// The embedder's window: the draw target it lends, and the OS mechanisms
/// the engine schedules through. The embedder provides the mechanisms; the
/// engine decides when to invoke them.
///
/// The engine is generic over this trait, so every call here is a direct
/// one — a window is a type, not a set of boxed closures. The draw target
/// is a GAT, which is what lets the surface the engine builds borrow the
/// embedder's window: an embedder lends a borrow of the window it owns
/// instead of handing over a `'static` refcounted handle.
pub trait Window {
    /// What this window lends wgpu to build a surface on — a borrow of the
    /// window itself, a refcounted handle, a raw handle pair, … — valid for
    /// as long as the engine borrows the window.
    type Target<'window>: Into<WindowTarget<'window>>
    where
        Self: 'window;

    /// The frame-request handle this window hands out. It is separate from
    /// the window because engine-owned threads keep one: the Lynx main
    /// thread asks for a frame after every committed flush.
    type Frames: FrameRequester;

    /// Lends the draw target for the engine's borrow of this window.
    fn target(&self) -> Self::Target<'_>;

    /// Hands out one frame-request handle.
    fn frames(&self) -> Self::Frames;

    /// Called on the presenting side immediately before presenting (winit's
    /// `Window::pre_present_notify`).
    fn pre_present(&self);
}

/// A window's frame-request capability, held apart from the window itself
/// because it travels to engine-owned threads.
pub trait FrameRequester: Send + Sync + 'static {
    /// Asks the OS for a redraw of the window (winit's
    /// `Window::request_redraw`).
    fn request_frame(&self);
}

/// The window of an engine that has none. Uninhabited: an
/// [`OffscreenEngine`] cannot reach a window path at all, and no embedder
/// can construct one.
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

    fn pre_present(&self) {
        match *self {}
    }
}

impl FrameRequester for NoWindow {
    fn request_frame(&self) {
        match *self {}
    }
}

/// The headless composition: an engine that renders to an offscreen target
/// and never attaches a window.
pub type OffscreenEngine = Engine<'static, NoWindow>;

/// The hand-off slot for the one element tree.
///
/// The tree has exactly one holder at any instant. Between batches it sits
/// here, and the presenting side borrows it briefly (production, input,
/// setup, observation). The Lynx main thread takes it at a batch's first
/// mutation and puts it back at the flush that commits the batch — so PAPI
/// calls in between are plain `&mut` mutations with no synchronization,
/// and the presenting side can never observe a half-applied batch.
#[derive(Clone)]
pub struct SharedTree {
    slot: Arc<Mutex<Option<ElementTree>>>,
}

impl fmt::Debug for SharedTree {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("SharedTree").finish_non_exhaustive()
    }
}

impl SharedTree {
    #[must_use]
    pub fn new(tree: ElementTree) -> Self {
        Self {
            slot: Arc::new(Mutex::new(Some(tree))),
        }
    }

    /// Blocking borrow for setup and observation.
    ///
    /// # Panics
    ///
    /// Panics while a batch is open — setup runs before the script starts,
    /// observation after it finishes.
    #[must_use]
    pub fn tree(&self) -> TreeGuard<'_> {
        let guard = self.lock();
        assert!(
            guard.is_some(),
            "a PAPI batch is open: the Lynx main thread holds the tree"
        );
        TreeGuard(guard)
    }

    /// Non-blocking borrow for the presenting side. `None` both while the
    /// main thread holds the tree (a batch is open) and while the slot lock
    /// itself is momentarily busy — either way: work from the retained
    /// frame and retry.
    pub(crate) fn try_tree(&self) -> Option<TreeGuard<'_>> {
        match self.slot.try_lock() {
            Ok(guard) if guard.is_some() => Some(TreeGuard(guard)),
            // An empty slot (a batch is open) and a momentarily busy slot
            // lock answer the same way: work from the retained frame.
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
    pub(crate) fn take(&self) -> ElementTree {
        self.lock()
            .take()
            .expect("the tree was already taken: only one batch can be open")
    }

    /// Puts the tree back at a batch boundary.
    ///
    /// # Panics
    ///
    /// Panics if the slot is occupied: the tree cannot be returned twice.
    pub(crate) fn put(&self, tree: ElementTree) {
        let mut guard = self.lock();
        assert!(
            guard.is_none(),
            "the slot is occupied: the tree was returned twice"
        );
        *guard = Some(tree);
    }

    fn lock(&self) -> MutexGuard<'_, Option<ElementTree>> {
        // A poisoned slot means the other thread crashed mid-borrow;
        // nothing can be trusted, so crash loudly (let-it-crash).
        self.slot
            .lock()
            .unwrap_or_else(|error| panic!("the tree slot is poisoned: {error}"))
    }
}

/// A borrow of the element tree from its slot.
#[derive(Debug)]
pub struct TreeGuard<'a>(MutexGuard<'a, Option<ElementTree>>);

impl Deref for TreeGuard<'_> {
    type Target = ElementTree;
    fn deref(&self) -> &ElementTree {
        self.0
            .as_ref()
            .expect("a TreeGuard is only built over an occupied slot")
    }
}

impl DerefMut for TreeGuard<'_> {
    fn deref_mut(&mut self) -> &mut ElementTree {
        self.0
            .as_mut()
            .expect("a TreeGuard is only built over an occupied slot")
    }
}

/// The attached output, if any.
enum Output<'window, W> {
    None,
    /// An offscreen GPU target: `tick` renders, `capture` reads back.
    /// Boxed to keep the idle variants small.
    Offscreen(Box<Headless>),
    /// A window: the presentation stack lives here, on the thread the
    /// embedder calls the engine from, and its surface borrows the
    /// embedder's window for exactly as long as it does.
    Window {
        graphics: Box<WindowGraphics<'window>>,
        window: &'window W,
    },
}

/// The engine half of a Lynx view: the shared element tree, input routing,
/// frame production, presentation, and the engine-owned script thread.
///
/// Generic over the embedder's [`Window`], which it borrows for the life of
/// the surface built from it; [`OffscreenEngine`] is the composition with no
/// window at all.
///
/// Deliberately `!Send`: it lives on the thread the embedder calls it from.
pub struct Engine<'window, W: Window> {
    elements: SharedTree,
    viewport: Viewport,
    frame_size: FrameSize,
    messages: mpsc::Receiver<EngineMessage>,
    #[cfg_attr(
        not(feature = "quickjs"),
        allow(dead_code, reason = "only engine-owned script threads send messages")
    )]
    message_sender: mpsc::Sender<EngineMessage>,
    output: Output<'window, W>,
    /// The window's frame-request handle, behind `Arc` so the Lynx main
    /// thread keeps one of its own.
    frames: Option<Arc<W::Frames>>,
    /// Input deferred while the tree was away, drained in arrival order at
    /// the next acquisition so gesture sequences stay coherent.
    pending_input: VecDeque<InputEvent>,
    /// The latest viewport metrics not yet applied to the tree, for a
    /// resize that arrived while a batch was open.
    pending_resize: Option<(f32, f32, f32)>,
    /// Presentation and vsync interact with the OS only on the thread the
    /// embedder calls the engine from, so the engine may not cross threads.
    /// `Rc` is the marker that says so; nothing is stored.
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
        let (message_sender, messages) = mpsc::channel();
        Ok(Self {
            elements: SharedTree::new(ElementTree::new(viewport, config)),
            viewport,
            frame_size,
            messages,
            message_sender,
            output: Output::None,
            frames: None,
            pending_input: VecDeque::new(),
            pending_resize: None,
            thread_bound: PhantomData,
        })
    }

    /// A blocking borrow of the element tree, for observation and setup.
    ///
    /// # Panics
    ///
    /// Panics while a batch is open — setup runs before the script starts,
    /// observation after it finishes.
    #[must_use]
    pub fn elements(&self) -> TreeGuard<'_> {
        self.elements.tree()
    }

    /// Mounts author CSS.
    pub fn add_author_stylesheet(&mut self, css: &str) {
        self.elements.tree().add_author_stylesheet(css);
    }

    /// Registers font data for text measurement.
    pub fn register_fonts(&mut self, bytes: &[u8]) -> usize {
        self.elements.tree().register_fonts(bytes)
    }

    /// Registers or updates decoded images, then keeps the next frame fresh.
    pub fn with_images<R>(&mut self, update: impl FnOnce(&mut ImageStore) -> R) -> R {
        let result = update(self.elements.tree().images_mut());
        self.refresh();
        result
    }

    /// Applies deferred embedder facts — the latest resize and buffered
    /// input, in arrival order — at a successful tree acquisition.
    fn drain_deferred(
        pending_resize: &mut Option<(f32, f32, f32)>,
        pending_input: &mut VecDeque<InputEvent>,
        tree: &mut ElementTree,
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
    ///
    /// Never blocks: while a batch is open the event is buffered and
    /// drained, in order, at the next acquisition (the very next event or
    /// redraw). Scroll target resolution and the offset write happen on the
    /// borrowed tree; a long script task between batches leaves the tree in
    /// its slot, so scrolling proceeds without the script's cooperation.
    pub fn dispatch_input(&mut self, event: InputEvent) {
        self.pending_input.push_back(event);
        let needs_frame = match self.elements.try_tree() {
            Some(mut tree) => {
                Self::drain_deferred(&mut self.pending_resize, &mut self.pending_input, &mut tree);
                tree.needs_render()
            }
            // The batch in progress ends with a frame request, whose redraw
            // drains the buffer.
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
            // A batch is open: apply the newest metrics when the tree is
            // next acquired.
            None => self.pending_resize = Some((width, height, device_pixel_ratio)),
        }
        self.viewport = Viewport::new(width, height).with_device_pixel_ratio(device_pixel_ratio);
        self.frame_size = next_size;
        self.refresh();
        Ok(())
    }

    /// Asks the OS for a frame through the window's frame-request handle.
    /// Harmless when nothing changed: the redraw re-presents the retained
    /// target without re-rendering.
    pub fn refresh(&self) {
        if let Some(frames) = &self.frames {
            frames.request_frame();
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

    /// Attaches the embedder's window as the draw target: the whole
    /// presentation stack is created here, on the calling thread, and stays
    /// here — presentation and vsync interact with the OS only on this
    /// thread. The surface borrows the window, which therefore outlives the
    /// engine.
    pub fn attach_window(
        &mut self,
        window: &'window W,
        size: FrameSize,
    ) -> Result<(), EngineError> {
        let graphics = WindowGraphics::new(window.target(), size)?;
        self.output = Output::Window {
            graphics: Box::new(graphics),
            window,
        };
        self.frames = Some(Arc::new(window.frames()));
        self.refresh();
        Ok(())
    }

    /// Relays the OS's "the window wants a frame" fact.
    ///
    /// Produces a new frame only when there is one to produce: the tree is
    /// in its slot (an open batch retries next frame), the batch that last
    /// returned it was committed, and the document changed since the
    /// retained target was rendered. Everything else — re-exposure, a
    /// resize with unchanged content, a retry — re-presents the retained
    /// target with a blit alone. The present itself (the vsync wait) runs
    /// after the borrow ends.
    pub fn notify_redraw(&mut self) -> Result<(), EngineError> {
        let Output::Window { graphics, window } = &mut self.output else {
            return Ok(());
        };
        let size = self.frame_size;
        if let Some(mut tree) = self.elements.try_tree() {
            Self::drain_deferred(&mut self.pending_resize, &mut self.pending_input, &mut tree);
            if !tree.has_uncommitted_mutations() {
                let produced = tree.render();
                if produced || !graphics.rendered_at(size) {
                    graphics.render_to_target(&tree.scene(), size)?;
                }
            }
        }
        // The batch in progress, if any, ends with a frame request; present
        // the retained target now so exposure never waits on it. The vsync
        // wait happens here, after the borrow ends.
        if graphics.rendered_at(size) {
            graphics.present(*window)?;
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
        let Some(mut tree) = self.elements.try_tree() else {
            // A batch is open; its flush asks for a frame anyway.
            return Ok(false);
        };
        Self::drain_deferred(&mut self.pending_resize, &mut self.pending_input, &mut tree);
        if tree.has_uncommitted_mutations() {
            return Ok(false);
        }
        let changed = tree.render();
        if !changed && !force {
            // The retained target already holds this exact frame;
            // re-submitting it would burn a full GPU pass per tick on a
            // static scene.
            return Ok(false);
        }
        gpu.render_frame(
            &tree.scene(),
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
                if let Some(mut tree) = self.elements.try_tree()
                    && !tree.has_uncommitted_mutations()
                    && tree.render()
                {
                    gpu.render_frame(&tree.scene(), size.width, size.height, Color::WHITE)
                        .map_err(EngineError::Gpu)?;
                }
                // The retained target holds the current frame; read it back
                // rather than re-rendering a scene that has not changed.
                let pixels = gpu.read_pixels().map_err(EngineError::Gpu)?;
                Ok(Screenshot { size, pixels })
            }
            Output::Window { graphics, .. } => {
                if let Some(mut tree) = self.elements.try_tree()
                    && !tree.has_uncommitted_mutations()
                {
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

    /// Runs a main-thread script to completion on the calling thread over
    /// the shared tree. The headless composition.
    #[cfg(feature = "quickjs")]
    pub fn run_script(&mut self, source: &str) -> Result<(), ScriptRunError> {
        let mut runtime = crate::quickjs::MainThreadRuntime::new(self.elements.clone(), || {})
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
        let elements = self.elements.clone();
        let sender = self.message_sender.clone();
        let on_flush = self.frames.clone();
        std::thread::Builder::new()
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

    /// The two-thread composition, windowless: the script mutates the
    /// shared tree from the engine-owned main thread, and the embedder side
    /// of the loop pumps lifecycle events after each wakeup.
    #[cfg(feature = "quickjs")]
    #[test]
    fn a_spawned_script_mutates_the_shared_tree() {
        use std::sync::mpsc;

        use lynx_element::PageConfig;

        use super::{EngineEvent, OffscreenEngine};

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
        assert!(elements.page().is_some(), "the page was created");
        assert!(elements.element(2).is_some(), "the first view is live");
        assert!(elements.element(3).is_some(), "the second view is live");
        assert!(
            !elements.has_uncommitted_mutations(),
            "the boot's final flush closed the batch"
        );
    }
}
