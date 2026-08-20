//! The engine half of a running Lynx view, behind the embedder boundary.
//!
//! An embedder provides exactly five things: user input, device metrics,
//! OS initialization (its event loop and window), a draw target, and IO
//! primitives (resource bytes in, pixels out). It never starts or steers the
//! internal pipeline. Its event handlers are relays — they hand the engine
//! an OS fact (`dispatch_input`, `resize`, `notify_redraw`, `pump`) and the
//! engine decides what the pipeline does with it, requesting frames itself
//! through the [`Window`] capabilities supplied at attach time, while
//! lifecycle completion wakes the host event loop through [`EventRequester`].
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

#[cfg(all(target_arch = "wasm32", panic = "abort"))]
use std::cell::RefCell;
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

use dom::FontBlob;
use dom::event::EventSteps;
use dom::input::{InputEvent, InputKind, PointerPhase};
use dom::render::gpu::Headless;
use dom::vello::peniko::Color;
#[cfg(target_arch = "wasm32")]
use wasm_thread::Builder as ThreadBuilder;

use self::graphics::WindowGraphics;
pub use self::graphics::WindowTarget;
use crate::image::DecodedImage;
use crate::script::ScriptError;
use crate::style::PreparsedStyleSheet;
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

#[cfg(all(target_arch = "wasm32", panic = "abort"))]
thread_local! {
    /// The event sink for the script task on this Worker only. The panic
    /// hook itself is process-global, so it must not retain or attribute
    /// panics from render/style Workers to one particular view.
    static WASM_SCRIPT_PANIC_REPORTER: RefCell<Option<EngineEventSender>> = const {
        RefCell::new(None)
    };
}

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
    #[error(
        "the document is busy in a script batch; retry the resource update after the next engine event"
    )]
    ResourceUpdateBusy,
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
    /// A listener failed while an event was being delivered.
    ListenerFailed(ScriptError),
}

#[derive(Debug)]
#[non_exhaustive]
pub enum EngineEvent {
    ScriptFinished(Result<(), ScriptRunError>),
    /// A listener threw while an event was being delivered to it.
    ///
    /// Reported rather than swallowed, and separate from
    /// [`Self::ScriptFinished`] because it is not fatal: the walk goes on, the
    /// realm stays usable, and every later event is delivered as normal. An
    /// embedder that logs it gets the same visibility over its own handlers
    /// that it has over its entry script; one that ignores it loses nothing
    /// but the message.
    ListenerFailed(ScriptError),
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

/// The host event-loop capability used to wake [`crate::LynxView::pump`].
///
/// Engine-owned threads enqueue their event before invoking this callback, so
/// an embedder may immediately call `pump` when its event loop receives the
/// wakeup. This capability is separate from [`FrameRequester`]: lifecycle
/// progress must not depend on a visible or attached draw target.
pub trait EventRequester: Send + Sync + 'static {
    fn request_event(&self);
}

impl<F> EventRequester for F
where
    F: Fn() + Send + Sync + 'static,
{
    fn request_event(&self) {
        self();
    }
}

#[derive(Clone)]
struct EngineEventSender {
    sender: mpsc::Sender<EngineMessage>,
    requester: Arc<dyn EventRequester>,
}

impl EngineEventSender {
    fn send(&self, message: EngineMessage) {
        if self.sender.send(message).is_ok() {
            // Enqueue first: after this wakeup, pump must be able to observe
            // the event without a polling race.
            self.requester.request_event();
        }
    }
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

/// The event one routed input becomes.
///
/// Deliberately the W3C pointer names rather than Lynx's `tap`/`longpress`:
/// those are *synthesized* from a pointer sequence by a gesture layer that
/// does not exist yet, and naming a single `pointerup` `tap` would be a guess
/// at the synthesis rather than an implementation of it.
fn event_name(event: &InputEvent) -> Option<&'static str> {
    match event.kind {
        InputKind::Pointer { phase, .. } => match phase {
            PointerPhase::Down => Some("pointerdown"),
            PointerPhase::Move => Some("pointermove"),
            PointerPhase::Up => Some("pointerup"),
            PointerPhase::Cancel => Some("pointercancel"),
            // `InputKind` and its enums are `#[non_exhaustive]` so keyboard
            // and focus can arrive without a break; an unnamed one dispatches
            // nothing rather than guessing at a name.
            _ => None,
        },
        InputKind::Wheel { .. } => Some("wheel"),
        _ => None,
    }
}

/// The device facts the realm turns into a Lynx event object's `detail`.
///
/// Viewport CSS px, which in this engine is also document space: there is no
/// document scrolling area, so the standard's `clientX`/`pageX` pair has one
/// value here.
fn event_detail(event: &InputEvent) -> String {
    let position = event.position;
    match event.kind {
        InputKind::Wheel { delta, .. } => format!(
            r#"{{"x":{},"y":{},"deltaX":{},"deltaY":{}}}"#,
            position.x, position.y, delta.x, delta.y
        ),
        _ => format!(r#"{{"x":{},"y":{}}}"#, position.x, position.y),
    }
}

/// What the presenting side asks the script thread to do after the entry
/// script has finished.
///
/// Only plain data crosses: node ids, an event name, a JSON payload. The realm
/// and the document both stay where they are.
enum ScriptCommand {
    /// Deliver one already-computed event path to the realm's listeners.
    DispatchEvent {
        steps: EventSteps,
        name: Arc<str>,
        detail: Arc<str>,
    },
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
    event_sender: EngineEventSender,
    output: Output<'window>,
    /// The window's frame-request handle, behind `Arc` so the Lynx main
    /// thread always observes the currently attached target rather than a
    /// startup-time snapshot.
    frames: Arc<Mutex<Option<Arc<W::Frames>>>>,
    pending_input: VecDeque<InputEvent>,
    pending_resize: Option<(f32, f32, f32)>,
    /// The host's animation timeline. Absent until an embedder installs one,
    /// and a view without one never animates.
    clock: Option<Arc<dyn crate::clock::AnimationClock>>,
    /// Whether the last frame left an animation running. Read without the
    /// document, because the frame request has to be made whether or not the
    /// slot was free this frame.
    animating: bool,
    /// The only `Sender`. It must never be cloned anywhere the script thread
    /// can reach: the channel closing is what ends that thread's loop, and a
    /// surviving clone would leave it parked with a live realm forever.
    script_commands: Option<mpsc::Sender<ScriptCommand>>,
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
        event_requester: Arc<dyn EventRequester>,
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
            event_sender: EngineEventSender {
                sender: message_sender,
                requester: event_requester,
            },
            output: Output::None,
            frames: Arc::new(Mutex::new(None)),
            pending_input: VecDeque::new(),
            pending_resize: None,
            clock: None,
            animating: false,
            script_commands: None,
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

    /// Installs the host's animation timeline. Without one, `@keyframes` and
    /// transitions resolve to their start values and never advance.
    pub(crate) fn set_animation_clock(&mut self, clock: Arc<dyn crate::clock::AnimationClock>) {
        self.clock = Some(clock);
        self.refresh();
    }

    /// Whether the last produced frame left an animation running, and so owes
    /// the timeline another frame.
    #[must_use]
    pub(crate) const fn is_animating(&self) -> bool {
        self.animating
    }

    /// Samples the clock once and advances the document's animations to it.
    ///
    /// Runs on the presenting thread, inside the borrow that is about to
    /// produce the frame: no script, no DOM mutation, and no hand-off to the
    /// Lynx main thread. An animation of a property that does not affect
    /// geometry re-cascades only the elements it touches and never reaches
    /// layout.
    ///
    /// Takes the clock rather than `&self` so it can run while the attached
    /// output is mutably borrowed.
    fn advance_animations(
        clock: Option<&Arc<dyn crate::clock::AnimationClock>>,
        tree: &mut LynxDocument,
    ) -> bool {
        let Some(clock) = clock else {
            return false;
        };
        tree.advance_animations(clock.now_seconds())
            .needs_next_frame
    }

    /// The current physical render-target size in device pixels.
    #[must_use]
    pub(crate) const fn frame_size(&self) -> FrameSize {
        self.frame_size
    }

    /// Registers shared font data without exposing the document to the host.
    pub(crate) fn register_fonts(&mut self, data: FontBlob) -> Result<usize, EngineError> {
        let Some(mut tree) = self.elements.try_tree() else {
            return Err(EngineError::ResourceUpdateBusy);
        };
        let registered = tree.register_fonts(data);
        if registered > 0 {
            tree.layout();
            drop(tree);
            self.refresh();
        }
        Ok(registered)
    }

    /// Mounts an author stylesheet the host had already parsed.
    ///
    /// Mount order is cascade order: a sheet mounted later wins ties against
    /// one mounted earlier, exactly as later sheets do in a document.
    pub(crate) fn add_preparsed_style_sheet(
        &mut self,
        sheet: &PreparsedStyleSheet,
    ) -> Result<(), EngineError> {
        let Some(mut tree) = self.elements.try_tree() else {
            return Err(EngineError::ResourceUpdateBusy);
        };
        crate::style::add_preparsed_style_sheet(&mut tree, sheet);
        drop(tree);
        self.refresh();
        Ok(())
    }

    /// Mounts an author stylesheet supplied as CSS text.
    pub(crate) fn add_style_sheet_text(&mut self, css: &str) -> Result<(), EngineError> {
        let Some(mut tree) = self.elements.try_tree() else {
            return Err(EngineError::ResourceUpdateBusy);
        };
        crate::style::add_style_sheet_text(&mut tree, css);
        drop(tree);
        self.refresh();
        Ok(())
    }

    /// Installs decoded pixels under their CSS URL without publishing the
    /// paint registry itself.
    pub(crate) fn register_image_url(
        &mut self,
        url: impl Into<String>,
        image: &DecodedImage,
    ) -> Result<(), EngineError> {
        let Some(mut tree) = self.elements.try_tree() else {
            return Err(EngineError::ResourceUpdateBusy);
        };
        tree.images_mut().insert_url(url, image.to_image_data());
        drop(tree);
        self.refresh();
        Ok(())
    }

    fn drain_deferred(
        pending_resize: &mut Option<(f32, f32, f32)>,
        pending_input: &mut VecDeque<InputEvent>,
        tree: &mut LynxDocument,
        commands: Option<&mpsc::Sender<ScriptCommand>>,
    ) {
        if let Some((width, height, ratio)) = pending_resize.take() {
            tree.set_viewport(width, height);
            tree.set_device_pixel_ratio(ratio);
        }
        while let Some(event) = pending_input.pop_front() {
            // Routing performs the user-agent default action, and reports the
            // node it routed to — so the event path costs no second hit test.
            // The default action runs first and unconditionally: no listener
            // can suppress one, because Lynx has no cancelable event.
            let response = tree.handle_input(event);
            let (Some(commands), Some(target)) = (commands, response.target) else {
                continue;
            };
            let Some(name) = event_name(&event) else {
                continue;
            };
            // Built here, where the document is already borrowed, so the
            // thread that owns the realm never has to take it to find out who
            // an event reaches.
            let steps = tree.event_steps(target, true, true);
            let _ = commands.send(ScriptCommand::DispatchEvent {
                steps,
                name: Arc::from(name),
                detail: Arc::from(event_detail(&event)),
            });
        }
    }

    /// Routes one host input event on the presenting side.
    pub(crate) fn dispatch_input(&mut self, event: InputEvent) {
        self.pending_input.push_back(event);
        let needs_frame = match self.elements.try_tree() {
            Some(mut tree) => {
                Self::drain_deferred(
                    &mut self.pending_resize,
                    &mut self.pending_input,
                    &mut tree,
                    self.script_commands.as_ref(),
                );
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
                EngineMessage::ListenerFailed(error) => {
                    events.push(EngineEvent::ListenerFailed(error));
                }
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
                Self::drain_deferred(
                    &mut self.pending_resize,
                    &mut self.pending_input,
                    &mut tree,
                    self.script_commands.as_ref(),
                );
                // Input and resize first, then the timeline, so a scroll and
                // an animation compose in one defined order under one truth;
                // then render, so an animation that just ended relayouts in
                // the same frame it ended.
                self.animating = Self::advance_animations(self.clock.as_ref(), &mut tree);
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
        if tree_was_busy || self.animating {
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
        Self::drain_deferred(
            &mut self.pending_resize,
            &mut self.pending_input,
            &mut tree,
            self.script_commands.as_ref(),
        );
        self.animating = Self::advance_animations(self.clock.as_ref(), &mut tree);
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
        let events = self.event_sender.clone();
        let frame_requesters = Arc::clone(&self.frames);
        let (command_sender, commands) = mpsc::channel::<ScriptCommand>();
        let spawn = ThreadBuilder::new()
            .name("bobcat-main".to_owned())
            .spawn(move || {
                #[cfg(all(target_arch = "wasm32", panic = "abort"))]
                install_script_panic_hook();
                #[cfg(all(target_arch = "wasm32", panic = "abort"))]
                set_script_panic_reporter(Some(events.clone()));
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
                            .map_err(|error| ScriptRunError::Script(error.into_script_error()))?;
                        Ok(runtime)
                    })()
                }))
                .unwrap_or_else(|payload| {
                    Err(ScriptRunError::Platform(format!(
                        "the injected VM panicked: {}",
                        panic_payload(payload.as_ref())
                    )))
                });
                let runtime = match result {
                    Ok(runtime) => {
                        events.send(EngineMessage::ScriptDone(Ok(())));
                        Some(runtime)
                    }
                    Err(error) => {
                        events.send(EngineMessage::ScriptDone(Err(error)));
                        None
                    }
                };
                request_current_frame(&frame_requesters);

                // The realm now outlives its entry script. `recv` returns
                // `Err` when the engine drops its `Sender`, which is the only
                // shutdown signal this thread needs or gets.
                if let Some(mut runtime) = runtime {
                    while let Ok(command) = commands.recv() {
                        match command {
                            ScriptCommand::DispatchEvent {
                                steps,
                                name,
                                detail,
                            } => {
                                // A panicking listener must not take the realm
                                // with it: the next event still has to arrive.
                                let delivered = catch_unwind(AssertUnwindSafe(|| {
                                    runtime.dispatch_event(&steps, &name, &detail)
                                }));
                                match delivered {
                                    Ok(Ok(true)) => {
                                        // A listener may have changed the tree
                                        // without flushing, and the presenting
                                        // thread asked `needs_render` before
                                        // any of them ran — so nothing else
                                        // will notice.
                                        request_current_frame(&frame_requesters);
                                    }
                                    Ok(Err(error)) => {
                                        events.send(EngineMessage::ListenerFailed(
                                            error.into_script_error(),
                                        ));
                                    }
                                    // A panic is already the crate's
                                    // unspecified-state contract, and the
                                    // unwind carries no `ScriptError` to
                                    // report; the realm survives it, which is
                                    // what the `catch_unwind` is for.
                                    Ok(Ok(false)) | Err(_) => {}
                                }
                            }
                        }
                    }
                }
                #[cfg(all(target_arch = "wasm32", panic = "abort"))]
                set_script_panic_reporter(None);
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
        self.script_commands = Some(command_sender);
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
fn install_script_panic_hook() {
    WASM_SCRIPT_PANIC_HOOK.get_or_init(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            WASM_SCRIPT_PANIC_REPORTER.with(|reporter| {
                if let Some(reporter) = reporter.borrow().as_ref() {
                    let location = info
                        .location()
                        .map_or_else(String::new, |location| format!(" at {location}"));
                    reporter.send(EngineMessage::ScriptDone(Err(ScriptRunError::Platform(
                        format!(
                            "the script Worker aborted after a panic{location}: {}",
                            panic_payload(info.payload())
                        ),
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
    use std::sync::Arc;

    use super::frame_size;

    fn engine_with_events(events: Arc<dyn super::EventRequester>) -> super::OffscreenEngine {
        super::OffscreenEngine::new(
            crate::tree::PageConfig::default(),
            events,
            393.0,
            727.0,
            1.0,
        )
        .expect("engine")
    }

    fn engine() -> super::OffscreenEngine {
        engine_with_events(Arc::new(|| {}))
    }

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

        let bytes = Bytes::from_static(b"not a font");
        let original = bytes.as_ptr();
        let blob = FontBlob::new(bytes);
        assert_eq!(blob.as_ref().as_ptr(), original);

        let mut engine = engine();
        assert_eq!(engine.register_fonts(blob).expect("available tree"), 0);
    }

    #[test]
    fn decoded_image_registration_reaches_the_private_url_registry() {
        use crate::image::{AlphaType, DecodedImage, ImageFormat};

        let mut engine = engine();
        let image = DecodedImage::from_rgba8(
            1,
            1,
            AlphaType::Straight,
            vec![1, 2, 3, 255],
            ImageFormat::Png,
        )
        .expect("image");
        let image_id = image.id();
        engine
            .register_image_url("app:///pixel.png", &image)
            .expect("available tree");

        let mut tree = engine.elements();
        assert_eq!(
            tree.images_mut()
                .url("app:///pixel.png")
                .expect("registered URL")
                .data
                .id(),
            image_id
        );
    }

    #[test]
    fn resource_updates_report_a_busy_script_batch() {
        use bytes::Bytes;
        use dom::FontBlob;

        let mut engine = engine();
        let script_tree = engine
            .take_script_tree()
            .expect("the engine creates one script owner");
        let tree = script_tree.take();

        assert!(matches!(
            engine.register_fonts(FontBlob::new(Bytes::from_static(b"font"))),
            Err(super::EngineError::ResourceUpdateBusy)
        ));

        script_tree.put(tree);
    }

    #[test]
    fn the_script_slot_is_unique_and_hides_an_open_batch() {
        let mut engine = engine();
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
        use std::sync::mpsc;
        use std::time::Duration;

        use super::EngineEvent;

        let (wake_sender, wake_receiver) = mpsc::channel();
        let mut engine = engine_with_events(Arc::new(move || {
            let _ = wake_sender.send(());
        }));
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

        wake_receiver
            .recv_timeout(Duration::from_secs(5))
            .expect("script completion must wake the host event loop");
        let finished = engine
            .pump()
            .into_iter()
            .find_map(|event| match event {
                EngineEvent::ScriptFinished(result) => Some(result),
                EngineEvent::ListenerFailed(_) => None,
            })
            .expect("the event must be enqueued before the wakeup");
        finished.expect("the script must boot");

        let elements = engine.elements();
        let page = elements.document_element().id();
        let views = elements
            .get(page)
            .expect("the page is live")
            .child_ids()
            .to_vec();
        assert_eq!(views.len(), 2, "the boot script appends two views");
        assert!(
            views.iter().all(|&view| elements.is_connected(view)),
            "both views are attached"
        );
        assert!(
            elements.rounded_layout(page).is_some(),
            "the boot's final flush laid the page out"
        );
    }
}

#[cfg(all(test, feature = "quickjs"))]
mod event_loop_tests {
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use dom::Point2D;
    use dom::input::{InputEvent, PointerKind, PointerPhase};

    use super::OffscreenEngine;

    /// The handle a packed id names, the way script spells one.
    fn node_id(bits: u64) -> dom::NodeId {
        dom::NodeId::from_bits(bits).expect("a well-formed packed handle")
    }

    /// Boots a script and waits for it to finish, leaving the script thread
    /// parked on its command channel.
    fn booted(source: &str) -> OffscreenEngine {
        let mut engine = OffscreenEngine::new(
            crate::tree::PageConfig::default(),
            Arc::new(|| {}),
            393.0,
            727.0,
            1.0,
        )
        .expect("engine");
        engine
            .spawn_script(
                source.to_owned(),
                "app:///main.js".to_owned(),
                crate::quickjs::engine_factory(),
            )
            .expect("spawn");

        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if engine.pump().into_iter().any(|event| {
                matches!(event, crate::EngineEvent::ScriptFinished(result) if result.is_ok())
            }) {
                return engine;
            }
            assert!(Instant::now() < deadline, "the entry script did not finish");
            std::thread::yield_now();
        }
    }

    /// The whole loop: input arrives on this thread, is routed and given its
    /// default action here, and its path is delivered to a listener on the
    /// thread that owns the realm.
    #[test]
    fn a_host_input_event_reaches_a_listener_in_the_realm() {
        let mut engine = booted(
            r"
            globalThis.renderPage = function () {
              const page = __CreatePage('card', 0);
              const view = __CreateView(0);
              __AppendElement(page, view);
              globalThis.held = [page, view];
              __SetInlineStyles(view, 'width:200px;height:200px');
              __AddEventListener(view, 'pointerdown', (event) => {
                // Observable from the presenting side, and proof the document
                // was free while a listener ran.
                __SetAttribute(view, 'seen', event.type + ':' + event.detail.x);
              }, {});
              __FlushElementTree();
            };
            ",
        );

        // Routing reads the rendered frame, so one has to exist first.
        engine
            .elements
            .try_tree()
            .expect("the script thread is parked")
            .render();
        engine.dispatch_input(InputEvent::pointer(
            Point2D::new(10.0, 10.0),
            1,
            PointerKind::Touch,
            PointerPhase::Down,
        ));

        // Delivery is asynchronous by construction: this thread queued a path
        // and moved on.
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            // Non-blocking, like the presenting side's own borrow: an empty
            // slot means the script thread is mid-delivery, not an error.
            let seen = engine.elements.try_tree().and_then(|tree| {
                tree.get(node_id(3))
                    .and_then(|node| node.attribute("seen").map(str::to_owned))
            });
            if seen.as_deref() == Some("pointerdown:10") {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "the listener never ran, attribute was {seen:?}"
            );
            std::thread::yield_now();
        }
    }

    /// A listener that throws must not take the document with it. Building the
    /// event object alone takes it — the realm reads two ids to fill in
    /// `target` and `currentTarget` — so a bare `throw` used to strand it in
    /// the hand-off slot with nothing able to put it back.
    #[test]
    fn a_throwing_listener_leaves_the_document_where_the_presenter_can_find_it() {
        let mut engine = booted(
            r"
            globalThis.renderPage = function () {
              const page = __CreatePage('card', 0);
              const view = __CreateView(0);
              __AppendElement(page, view);
              globalThis.held = [page, view];
              __SetInlineStyles(view, 'width:200px;height:200px');
              __AddEventListener(view, 'pointerdown', () => {
                __SetAttribute(view, 'seen', 'yes');
                throw new Error('a listener may fail');
              }, {});
              __FlushElementTree();
            };
            ",
        );

        engine
            .elements
            .try_tree()
            .expect("the script thread is parked")
            .render();
        engine.dispatch_input(InputEvent::pointer(
            Point2D::new(10.0, 10.0),
            1,
            PointerKind::Touch,
            PointerPhase::Down,
        ));

        // The listener ran, threw, and the document still came back — and the
        // failure is reported rather than swallowed, which is the only way an
        // embedder can see its own handler fail.
        let mut reported = false;
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            reported |= engine.pump().into_iter().any(|event| {
                matches!(event, crate::EngineEvent::ListenerFailed(error)
                    if error.message.contains("a listener may fail"))
            });
            let seen = engine.elements.try_tree().and_then(|tree| {
                tree.get(node_id(3))
                    .and_then(|node| node.attribute("seen").map(str::to_owned))
            });
            if seen.as_deref() == Some("yes") && reported {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "listener ran: {seen:?}, failure reported: {reported}"
            );
            std::thread::yield_now();
        }

        // And the view still works: a second event routes and is delivered.
        engine
            .elements
            .try_tree()
            .expect("a thrown listener must not wedge the view")
            .render();
    }

    /// A node with no registration must not cost a trip into the realm, and a
    /// script that registered nothing must not keep the loop from working.
    #[test]
    fn an_event_with_no_listener_changes_nothing() {
        let mut engine = booted(
            r"
            globalThis.renderPage = function () {
              const page = __CreatePage('card', 0);
              const view = __CreateView(0);
              __AppendElement(page, view);
              globalThis.held = [page, view];
              __SetInlineStyles(view, 'width:200px;height:200px');
              __FlushElementTree();
            };
            ",
        );

        engine
            .elements
            .try_tree()
            .expect("the script thread is parked")
            .render();
        engine.dispatch_input(InputEvent::pointer(
            Point2D::new(10.0, 10.0),
            1,
            PointerKind::Touch,
            PointerPhase::Down,
        ));

        std::thread::sleep(Duration::from_millis(50));
        assert!(
            engine
                .elements
                .try_tree()
                .expect("nothing was delivered, so the slot is free")
                .get(node_id(3))
                .expect("the view is live")
                .attribute("seen")
                .is_none()
        );
    }
}
