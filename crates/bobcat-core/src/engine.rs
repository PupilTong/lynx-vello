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
use std::collections::{HashMap, VecDeque};
use std::fmt;
use std::marker::PhantomData;
use std::ops::{Deref, DerefMut};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::rc::Rc;
#[cfg(target_arch = "wasm32")]
use std::sync::OnceLock;
use std::sync::{Arc, Mutex, MutexGuard, TryLockError, mpsc};
#[cfg(not(target_arch = "wasm32"))]
use std::thread::Builder as ThreadBuilder;

use dom::FontBlob;
use dom::event::EventSteps;
use dom::input::InputEvent;
use dom::render::gpu::Headless;
use dom::scroll::ScrollAxes;
use dom::vello::peniko::Color;
#[cfg(target_arch = "wasm32")]
use wasm_thread::Builder as ThreadBuilder;

use self::graphics::WindowGraphics;
pub use self::graphics::WindowTarget;
use crate::clock::{AnimationClock, SystemClock};
use crate::gesture::{EmitEvent, GestureRouter, InputDecision, RouterHost};
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
static WASM_STYLE_POOL: OnceLock<Result<(), String>> = OnceLock::new();
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
    #[error("this view has already started its entry MTS module")]
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
/// remain private to the engine. The caller becomes index zero of the
/// process-wide Stylo pool and must outlive every view that uses it;
/// `style_thread_count` must be at least two so the pool also has one managed
/// Worker. Each view's separate Lynx-main Worker is outside this budget.
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

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ScriptRunError {
    #[error("could not initialize the main-thread runtime: {0}")]
    Initialization(#[source] crate::script::ScriptError),
    #[error("main-thread module failed: {0}")]
    Script(#[source] crate::script::ScriptError),
    #[error("could not initialize the script thread: {0}")]
    Platform(String),
}

/// A message crossing from an engine-owned thread.
enum EngineMessage {
    /// The entry MTS module and Bobcat boot completed (or failed) on their thread.
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
    /// that it has over its entry module; one that ignores it loses nothing
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

/// The event names the realm currently has listeners for, shared between the
/// script thread that learns about registrations and the presenting thread
/// that synthesizes gestures.
///
/// Maintained by the native `enableEventListener`/`disableEventListener` ESM
/// exports — the realm already reports only empty↔occupied transitions, so
/// the script side touches this on registration edges, never per event. The
/// presenting side reads it when a long-press deadline resolves. Neither
/// touch goes anywhere near the tree slot, so the locks-twice-per-batch law is
/// untouched.
///
/// The set is name-level over the whole document. For gesture synthesis that
/// is a recorded approximation: a `longpress` listener anywhere suppresses a
/// sequence's `tap` even when the fired chain has none. Per-chain precision
/// needs a presenting-side per-node index, which is future work shared with
/// the event-path filtering.
#[derive(Debug, Default)]
pub(crate) struct SharedListenerNames {
    counts: Mutex<HashMap<Arc<str>, usize>>,
}

impl SharedListenerNames {
    fn lock(&self) -> MutexGuard<'_, HashMap<Arc<str>, usize>> {
        self.counts
            .lock()
            .unwrap_or_else(|error| panic!("the listener-name table is poisoned: {error}"))
    }

    /// Records one new `(node, capture)` registration for `name`.
    pub(crate) fn note_enabled(&self, name: &str) {
        let mut counts = self.lock();
        if let Some(count) = counts.get_mut(name) {
            *count += 1;
        } else {
            counts.insert(Arc::from(name), 1);
        }
    }

    /// Records one removed registration for `name`.
    pub(crate) fn note_disabled(&self, name: &str) {
        let mut counts = self.lock();
        if let Some(count) = counts.get_mut(name) {
            *count -= 1;
            if *count == 0 {
                counts.remove(name);
            }
        }
    }

    /// Whether any listener for `name` exists anywhere in the document.
    pub(crate) fn contains(&self, name: &str) -> bool {
        self.lock().contains_key(name)
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
pub(crate) type OffscreenEngine<C = SystemClock> = Engine<'static, NoWindow, C>;

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

/// The device facts the realm turns into a Lynx event object's `detail`,
/// serialized from a router decision.
///
/// Viewport CSS px, which in this engine is also document space: there is no
/// document scrolling area, so the standard's `clientX`/`pageX` pair has one
/// value here.
fn emit_detail(event: &EmitEvent) -> String {
    let position = event.position;
    match event.wheel {
        Some(delta) => format!(
            r#"{{"x":{},"y":{},"deltaX":{},"deltaY":{}}}"#,
            position.x, position.y, delta.x, delta.y
        ),
        None => format!(r#"{{"x":{},"y":{}}}"#, position.x, position.y),
    }
}

/// The document facts the router asks for while deciding, answered from the
/// borrowed tree and the shared listener-name table.
struct EngineRouterHost<'a> {
    tree: &'a LynxDocument,
    listener_names: &'a SharedListenerNames,
}

impl RouterHost for EngineRouterHost<'_> {
    fn nearest_user_scrollable(&self, node: dom::NodeId, axes: ScrollAxes) -> Option<dom::NodeId> {
        self.tree.nearest_user_scrollable(node, axes)
    }

    fn contains_node(&self, node: dom::NodeId) -> bool {
        self.tree.get(node).is_some()
    }

    fn scrollport_size(&self, node: dom::NodeId) -> Option<(f32, f32)> {
        self.tree
            .scroll_box(node)
            .map(|scroll_box| (scroll_box.scrollport.width, scroll_box.scrollport.height))
    }

    fn has_listener(&self, name: &str) -> bool {
        self.listener_names.contains(name)
    }
}

/// What the presenting side asks one view's script thread to do after the
/// entry module has finished booting.
///
/// Only plain data crosses: node ids, an event name, and a JSON payload. The
/// realm and document both stay where they are.
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
pub(crate) struct Engine<'window, W: Window, C: AnimationClock = SystemClock> {
    /// The only `Sender`, dropped first so this view's detached script thread
    /// wakes and releases its owner-thread-bound realm. It must never be cloned
    /// anywhere the script thread can reach: a surviving clone would leave the
    /// receiver parked with a live realm forever.
    script_commands: Option<mpsc::Sender<ScriptCommand>>,
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
    /// Buffered input, each event stamped with the clock reading at arrival —
    /// so a sequence drained late, behind a busy document, keeps its real
    /// duration.
    pending_input: VecDeque<(InputEvent, f64)>,
    pending_resize: Option<(f32, f32, f32)>,
    /// The gesture recognizer: turns routed pointer sequences into Lynx's
    /// `tap`/`longpress` beside the raw pointer events.
    gesture: GestureRouter,
    /// Which event names the realm has listeners for; written by the script
    /// thread's registration members, read when gestures resolve.
    listener_names: Arc<SharedListenerNames>,
    /// The animation timeline, named by the host at construction. Every
    /// reading is a direct call; there is no trait object and no way to swap
    /// one in after the fact.
    clock: C,
    /// Whether the last frame left an animation running. Read without the
    /// document, because the frame request has to be made whether or not the
    /// slot was free this frame.
    animating: bool,
    thread_bound: PhantomData<Rc<()>>,
}

impl<W: Window, C: AnimationClock> fmt::Debug for Engine<'_, W, C> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Engine")
            .field("viewport", &self.viewport)
            .field("frame_size", &self.frame_size)
            .field("pending_input", &self.pending_input.len())
            .finish_non_exhaustive()
    }
}

impl<'window, W: Window, C: AnimationClock> Engine<'window, W, C> {
    /// Builds the engine and its element tree at the given CSS viewport, on
    /// the supplied animation timeline.
    pub(crate) fn new(
        config: PageConfig,
        event_requester: Arc<dyn EventRequester>,
        width: f32,
        height: f32,
        device_pixel_ratio: f32,
        clock: C,
    ) -> Result<Self, EngineError> {
        let frame_size = frame_size(width, height, device_pixel_ratio)?;
        let viewport = Viewport::new(width, height).with_device_pixel_ratio(device_pixel_ratio);
        let (message_sender, messages) = mpsc::channel();
        let elements = SharedTree::new(new_document(viewport, config));
        Ok(Self {
            script_commands: None,
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
            gesture: GestureRouter::default(),
            listener_names: Arc::new(SharedListenerNames::default()),
            clock,
            animating: false,
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

    /// Whether the engine owes the timeline another frame: the last produced
    /// frame left an animation running, or a gesture deadline is armed and
    /// waiting on the clock.
    ///
    /// This is the one continuation signal an offscreen embedder has — no
    /// [`FrameRequester`] exists on that output, so a host that idles its
    /// tick loop must keep ticking while this reports `true` or an armed
    /// long-press never resolves.
    #[must_use]
    pub(crate) fn is_animating(&self) -> bool {
        self.animating || self.gesture.needs_frame()
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
    fn advance_animations(clock: &C, tree: &mut LynxDocument) -> bool {
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

    /// Selects a registered family as the embedder-provided platform default.
    pub(crate) fn set_default_font_family(&mut self, family: &str) -> Result<bool, EngineError> {
        let Some(mut tree) = self.elements.try_tree() else {
            return Err(EngineError::ResourceUpdateBusy);
        };
        let configured = tree.set_default_font_family(family);
        if configured {
            tree.layout();
            drop(tree);
            self.refresh();
        }
        Ok(configured)
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
        pending_input: &mut VecDeque<(InputEvent, f64)>,
        gesture: &mut GestureRouter,
        listener_names: &SharedListenerNames,
        tree: &mut LynxDocument,
        commands: Option<&mpsc::Sender<ScriptCommand>>,
    ) {
        if let Some((width, height, ratio)) = pending_resize.take() {
            tree.set_viewport(width, height);
            tree.set_device_pixel_ratio(ratio);
        }
        let mut decisions = Vec::new();
        while let Some((event, at)) = pending_input.pop_front() {
            // Routing is a pure read; `dom` has no default-action machinery.
            // Deciding the user-agent scroll belongs to the router, and the
            // event's `default_prevented` — the embedder's suppression seam —
            // is honored there by deciding no scroll.
            let target = tree.route_input(event);
            gesture.on_input(
                &event,
                target,
                at,
                &EngineRouterHost {
                    tree,
                    listener_names,
                },
                &mut decisions,
            );
            Self::execute_decisions(&mut decisions, gesture, listener_names, tree, commands);
        }
    }

    /// Executes the router's decisions in order — which is the delivery
    /// order, because the command channel is ordered.
    ///
    /// A scroll decision drives the document's scroll chain, reporting real
    /// consumption back so the router can claim the sequence. An emit
    /// decision goes to path construction and the channel; a target freed
    /// since the decision formed resolves to nothing rather than a path —
    /// checked here because `event_steps` asserts liveness.
    fn execute_decisions(
        decisions: &mut Vec<InputDecision>,
        gesture: &mut GestureRouter,
        listener_names: &SharedListenerNames,
        tree: &mut LynxDocument,
        commands: Option<&mpsc::Sender<ScriptCommand>>,
    ) {
        for decision in decisions.drain(..) {
            match decision {
                InputDecision::Scroll {
                    pointer,
                    from,
                    delta,
                } => {
                    if tree.get(from).is_none() {
                        continue;
                    }
                    if tree.scroll_chain(from, delta).is_some()
                        && let Some(pointer) = pointer
                    {
                        gesture.note_scroll_consumed(pointer);
                    }
                }
                InputDecision::Emit(event) => {
                    let Some(commands) = commands else {
                        continue;
                    };
                    // Asked before anything is built. The shared name table
                    // the gesture router already consults answers "does the
                    // realm want this at all?", so an event no card listens
                    // for — every `pointermove` of every card that does not
                    // track the pointer — costs one lookup instead of a path
                    // walk, two allocations and a cross-thread wakeup.
                    //
                    // The table is what the realm has registered *so far*, and
                    // the two threads run: a listener that registers a new
                    // name while this event is being routed does not receive
                    // it, where an unfiltered channel would have queued the
                    // event behind the registration and delivered it. The
                    // window is one routing pass wide and closes as soon as
                    // the registering call returns. This is the same
                    // presenting-side staleness `long_press_consumed` already
                    // reads the table under; see `docs/tracking/dom-events.md`.
                    if !listener_names.contains(event.name) {
                        continue;
                    }
                    if tree.get(event.target).is_none() {
                        continue;
                    }
                    // Built here, where the document is already borrowed, so
                    // the thread that owns the realm never has to take it to
                    // find out who an event reaches. The router decided the
                    // type and the target; the chain is this module's.
                    let steps = tree.event_steps(event.target, true, true);
                    let _ = commands.send(ScriptCommand::DispatchEvent {
                        steps,
                        name: Arc::from(event.name),
                        detail: Arc::from(emit_detail(&event)),
                    });
                }
            }
        }
    }

    /// Resolves gesture deadlines against the frame clock — the per-frame
    /// half of the router, beside the per-event half in
    /// [`Self::drain_deferred`]. While a deadline is armed the router's
    /// [`GestureRouter::needs_frame`] keeps frames coming, the same
    /// continuation contract running animations use.
    fn service_gesture_clock(
        gesture: &mut GestureRouter,
        listener_names: &SharedListenerNames,
        tree: &mut LynxDocument,
        commands: Option<&mpsc::Sender<ScriptCommand>>,
        now: f64,
    ) {
        let mut decisions = Vec::new();
        gesture.on_tick(
            now,
            &EngineRouterHost {
                tree,
                listener_names,
            },
            &mut decisions,
        );
        Self::execute_decisions(&mut decisions, gesture, listener_names, tree, commands);
    }

    /// Routes one host input event on the presenting side.
    pub(crate) fn dispatch_input(&mut self, event: InputEvent) {
        let at = self.clock.now_seconds();
        self.pending_input.push_back((event, at));
        let needs_frame = match self.elements.try_tree() {
            Some(mut tree) => {
                Self::drain_deferred(
                    &mut self.pending_resize,
                    &mut self.pending_input,
                    &mut self.gesture,
                    &self.listener_names,
                    &mut tree,
                    self.script_commands.as_ref(),
                );
                tree.needs_render()
            }
            None => false,
        };
        // An armed long-press deadline needs the frame clock even when the
        // document is visually clean — the frame is what resolves it.
        if needs_frame || self.gesture.needs_frame() {
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
                    &mut self.gesture,
                    &self.listener_names,
                    &mut tree,
                    self.script_commands.as_ref(),
                );
                Self::service_gesture_clock(
                    &mut self.gesture,
                    &self.listener_names,
                    &mut tree,
                    self.script_commands.as_ref(),
                    self.clock.now_seconds(),
                );
                // Input and resize first, then the timeline, so a scroll and
                // an animation compose in one defined order under one truth;
                // then render, so an animation that just ended relayouts in
                // the same frame it ended.
                self.animating = Self::advance_animations(&self.clock, &mut tree);
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
        if tree_was_busy || self.animating || self.gesture.needs_frame() {
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
            &mut self.gesture,
            &self.listener_names,
            &mut tree,
            self.script_commands.as_ref(),
        );
        Self::service_gesture_clock(
            &mut self.gesture,
            &self.listener_names,
            &mut tree,
            self.script_commands.as_ref(),
            self.clock.now_seconds(),
        );
        self.animating = Self::advance_animations(&self.clock, &mut tree);
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
    /// that owner thread before registering `source` at its resolved entry URL
    /// and running Bobcat's ESM boot module.
    pub(crate) fn spawn_script(
        &mut self,
        source: String,
        source_name: String,
        factory: Arc<dyn crate::script::ScriptEngineFactory>,
    ) -> Result<(), EngineError> {
        let elements = self.take_script_tree()?;
        let events = self.event_sender.clone();
        let frame_requesters = Arc::clone(&self.frames);
        let listener_names = Arc::clone(&self.listener_names);
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
                        let mut runtime = crate::runtime::MainThreadRuntime::new(
                            factory.as_ref(),
                            elements,
                            listener_names,
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

                if let Some(runtime) = runtime {
                    serve_script_commands(runtime, &commands, &events, &frame_requesters);
                }
                #[cfg(all(target_arch = "wasm32", panic = "abort"))]
                set_script_panic_reporter(None);
            });
        let script_thread = spawn.map_err(|error| self.script_spawn_error(&error))?;
        self.script_commands = Some(command_sender);
        drop(script_thread);
        Ok(())
    }

    fn script_spawn_error(&mut self, error: &std::io::Error) -> EngineError {
        self.script_owner_available = true;
        EngineError::Thread {
            name: "script",
            message: error.to_string(),
        }
    }
}

/// Serves the command channel for as long as the engine holds its sender.
///
/// The realm now outlives its entry module. The channel closing is the only
/// shutdown signal this thread needs or gets, and every command queued is
/// delivered, in order.
fn serve_script_commands<F: FrameRequester>(
    mut runtime: crate::runtime::MainThreadRuntime,
    commands: &mpsc::Receiver<ScriptCommand>,
    events: &EngineEventSender,
    frames: &Mutex<Option<Arc<F>>>,
) {
    // Matched exhaustively rather than destructured in the `while let`: a
    // second `ScriptCommand` variant must fail to compile here, not quietly
    // end the loop.
    while let Ok(command) = commands.recv() {
        match command {
            ScriptCommand::DispatchEvent {
                steps,
                name,
                detail,
            } => {
                // A panicking listener must not take the realm with it: the
                // next event still has to arrive.
                let delivered = catch_unwind(AssertUnwindSafe(|| {
                    runtime.dispatch_event(&steps, &name, &detail)
                }));
                match delivered {
                    // A listener may have changed the tree without flushing,
                    // and the presenting thread asked `needs_render` before
                    // any of them ran — so nothing else will notice.
                    Ok(Ok(true)) => request_current_frame(frames),
                    Ok(Err(error)) => {
                        events.send(EngineMessage::ListenerFailed(error.into_script_error()));
                    }
                    // A panic is already the crate's unspecified-state
                    // contract, and the unwind carries no `ScriptError` to
                    // report; the realm survives it, which is what the
                    // `catch_unwind` is for.
                    Ok(Ok(false)) | Err(_) => {}
                }
            }
        }
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
fn create_wasm_style_pool(thread_count: usize) -> Result<(), String> {
    let pool = rayon::ThreadPoolBuilder::new()
        // The persistent presenting Worker is index zero; every LynxView's
        // owner-thread-bound VM runs on a separate, transient Worker.
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

    use super::{OffscreenEngine, ScriptCommand, frame_size};
    use crate::tree::LynxDocument;

    fn engine_with_events(events: Arc<dyn super::EventRequester>) -> super::OffscreenEngine {
        super::OffscreenEngine::new(
            crate::tree::PageConfig::default(),
            events,
            393.0,
            727.0,
            1.0,
            crate::clock::SystemClock::new(),
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

    /// An emit decision whose target was freed before execution must be
    /// skipped, because `Document::event_steps` asserts liveness — the guard
    /// is the only thing between a mid-sequence release of a detached element
    /// and a presenting-thread panic.
    #[test]
    fn an_emit_decision_for_a_freed_target_is_skipped_not_delivered() {
        use crate::gesture::{EmitEvent, GestureRouter, InputDecision, TAP_EVENT};

        let engine = engine();
        let (sender, receiver) = std::sync::mpsc::channel::<super::ScriptCommand>();
        let mut tree = engine.elements.tree();
        let doomed = tree.create_element("view", ());
        tree.release(doomed);
        tree.collect_unheld();

        let mut decisions = vec![InputDecision::Emit(EmitEvent {
            name: TAP_EVENT,
            target: doomed,
            position: dom::Point2D::new(1.0, 1.0),
            wheel: None,
        })];
        let mut gesture = GestureRouter::default();
        let names = super::SharedListenerNames::default();
        names.note_enabled(TAP_EVENT);
        super::OffscreenEngine::<crate::clock::SystemClock>::execute_decisions(
            &mut decisions,
            &mut gesture,
            &names,
            &mut tree,
            Some(&sender),
        );
        assert!(decisions.is_empty(), "the queue is always drained");
        assert!(
            receiver.try_recv().is_err(),
            "a freed target reaches no one rather than panicking the walk"
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
        assert!(matches!(
            engine.set_default_font_family("Ahem"),
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

    /// An emit decision for a name nobody listens to costs a lookup and stops
    /// there: no path is walked and nothing is queued for the thread that
    /// owns the realm.
    #[test]
    fn an_event_no_listener_wants_never_crosses_to_the_script_thread() {
        use std::sync::mpsc;

        use crate::gesture::{EmitEvent, GestureRouter, InputDecision, TAP_EVENT};

        let engine = engine();
        let mut tree = engine.elements.try_tree().expect("the slot is free");
        let page = tree.document_element().id();
        let view = tree.create_element("view", ());
        tree.insert_before(page, view, None);
        tree.layout();

        let (sender, receiver) = mpsc::channel();
        let mut gesture = GestureRouter::default();
        let names = super::SharedListenerNames::default();
        let mut emit = |names: &super::SharedListenerNames, tree: &mut LynxDocument| {
            let mut decisions = vec![InputDecision::Emit(EmitEvent {
                name: TAP_EVENT,
                target: view,
                position: dom::Point2D::new(1.0, 1.0),
                wheel: None,
            })];
            OffscreenEngine::<crate::clock::SystemClock>::execute_decisions(
                &mut decisions,
                &mut gesture,
                names,
                tree,
                Some(&sender),
            );
        };

        emit(&names, &mut tree);
        assert!(
            receiver.try_recv().is_err(),
            "an empty listener table sends nothing"
        );

        names.note_enabled("pointerup");
        emit(&names, &mut tree);
        assert!(
            receiver.try_recv().is_err(),
            "a listener on another name sends nothing"
        );

        names.note_enabled(TAP_EVENT);
        emit(&names, &mut tree);
        let ScriptCommand::DispatchEvent { name, .. } =
            receiver.try_recv().expect("the listened-for name crosses");
        assert_eq!(name.as_ref(), TAP_EVENT);

        // And the count is a count: the last removal is what closes the name.
        names.note_disabled(TAP_EVENT);
        emit(&names, &mut tree);
        assert!(
            receiver.try_recv().is_err(),
            "the removed registration stops the crossing"
        );
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
    use crate::clock::AnimationClock;

    /// The handle a packed id names, the way script spells one.
    fn node_id(bits: u64) -> dom::NodeId {
        dom::NodeId::from_bits(bits).expect("a well-formed packed handle")
    }

    /// Boots a script and waits for it to finish, leaving the script thread
    /// parked on its command channel.
    fn booted(source: &str) -> OffscreenEngine {
        booted_with(source, crate::clock::SystemClock::new())
    }

    /// [`booted`], on a caller-supplied timeline — how the gesture tests hold
    /// a [`crate::clock::ManualClock`] the engine also reads.
    fn booted_with<C: crate::clock::AnimationClock>(source: &str, clock: C) -> OffscreenEngine<C> {
        let mut engine = OffscreenEngine::<C>::new(
            crate::tree::PageConfig::default(),
            Arc::new(|| {}),
            393.0,
            727.0,
            1.0,
            clock,
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
            assert!(Instant::now() < deadline, "the entry module did not finish");
            std::thread::yield_now();
        }
    }

    #[test]
    fn independent_views_can_own_live_script_threads_in_one_process() {
        let source = r"
            globalThis.renderPage = function () {
              const page = __CreatePage('card', 0);
              __AppendElement(page, __CreateView(0));
              __FlushElementTree();
            };
        ";

        let first = booted(source);
        let second = booted(source);

        for engine in [&first, &second] {
            let tree = engine
                .elements
                .try_tree()
                .expect("each live view retains its own document");
            assert_eq!(tree.document_element().child_ids().len(), 1);
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

    /// The gesture suite's page: one 200×200 view whose listeners append
    /// `type:x` to a `log` attribute. The placeholder line opts a variant
    /// into a `longpress` registration.
    const GESTURE_PAGE: &str = r"
        globalThis.renderPage = function () {
          const page = __CreatePage('card', 0);
          const view = __CreateView(0);
          __AppendElement(page, view);
          globalThis.held = [page, view];
          globalThis.entries = [];
          __SetInlineStyles(view, 'width:200px;height:200px');
          const note = (event) => {
            entries.push(event.type + ':' + event.detail.x);
            __SetAttribute(view, 'log', entries.join());
          };
          __AddEventListener(view, 'tap', note, {});
          //LONGPRESS
          __FlushElementTree();
        };
        ";

    fn gesture_page(with_longpress: bool) -> String {
        if with_longpress {
            GESTURE_PAGE.replace(
                "//LONGPRESS",
                "__AddEventListener(view, 'longpress', note, {});",
            )
        } else {
            GESTURE_PAGE.to_owned()
        }
    }

    fn touch(id: u32, phase: PointerPhase, x: f32) -> InputEvent {
        InputEvent::pointer(Point2D::new(x, 10.0), id, PointerKind::Touch, phase)
    }

    /// Polls until the view's `log` attribute equals `expected` — equality,
    /// not containment, so an event that should have been suppressed fails
    /// the wait by showing up in the actual value. The deadline is generous
    /// because the whole suite's realm boots share the machine with this
    /// spin.
    fn wait_for_log<W: super::Window, C: crate::clock::AnimationClock>(
        engine: &mut super::Engine<'_, W, C>,
        expected: &str,
    ) {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let log = engine.elements.try_tree().and_then(|tree| {
                tree.get(node_id(3))
                    .and_then(|node| node.attribute("log").map(str::to_owned))
            });
            if log.as_deref() == Some(expected) {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "expected log {expected:?}, last saw {log:?}"
            );
            std::thread::yield_now();
        }
    }

    /// A press released within the slop synthesizes `tap` at the release
    /// point, delivered through the same path as the raw pointer events.
    #[test]
    fn a_quick_release_delivers_tap_to_the_realm() {
        let mut engine = booted(&gesture_page(false));
        engine
            .elements
            .try_tree()
            .expect("the script thread is parked")
            .render();

        engine.dispatch_input(touch(1, PointerPhase::Down, 10.0));
        engine.dispatch_input(touch(1, PointerPhase::Up, 12.0));
        wait_for_log(&mut engine, "tap:12");
    }

    /// Travel beyond the 50px tap slop disqualifies the sequence; the later
    /// fence tap proves the suppressed one was never sent, because the
    /// command channel is ordered.
    #[test]
    fn travel_beyond_the_tap_slop_suppresses_the_tap() {
        let mut engine = booted(&gesture_page(false));
        engine
            .elements
            .try_tree()
            .expect("the script thread is parked")
            .render();

        engine.dispatch_input(touch(1, PointerPhase::Down, 10.0));
        engine.dispatch_input(touch(1, PointerPhase::Move, 100.0));
        engine.dispatch_input(touch(1, PointerPhase::Up, 100.0));
        engine.dispatch_input(touch(1, PointerPhase::Down, 150.0));
        engine.dispatch_input(touch(1, PointerPhase::Up, 150.0));
        wait_for_log(&mut engine, "tap:150");
    }

    /// Holding past the deadline delivers `longpress` on the engine's own
    /// timeline, and the sequence's release is then not a tap — Lynx's
    /// `long_press_consumed` rule. The fence tap pins the suppression.
    #[test]
    fn a_held_pointer_delivers_longpress_and_suppresses_the_tap() {
        let clock = Arc::new(crate::clock::ManualClock::new());
        let mut engine = booted_with(&gesture_page(true), Arc::clone(&clock));
        engine
            .elements
            .try_tree()
            .expect("the script thread is parked")
            .render();

        engine.dispatch_input(touch(1, PointerPhase::Down, 10.0));
        clock.set(0.6);
        engine.dispatch_input(touch(1, PointerPhase::Move, 10.0));
        wait_for_log(&mut engine, "longpress:10");

        engine.dispatch_input(touch(1, PointerPhase::Up, 10.0));
        engine.dispatch_input(touch(1, PointerPhase::Down, 30.0));
        engine.dispatch_input(touch(1, PointerPhase::Up, 30.0));
        wait_for_log(&mut engine, "longpress:10,tap:30");
    }

    /// With no `longpress` listener anywhere, the deadline lapses silently
    /// and a slow release is still a tap — the listener-presence gate read
    /// through the shared name table.
    #[test]
    fn a_long_hold_without_longpress_listener_still_taps() {
        let clock = Arc::new(crate::clock::ManualClock::new());
        let mut engine = booted_with(&gesture_page(false), Arc::clone(&clock));
        engine
            .elements
            .try_tree()
            .expect("the script thread is parked")
            .render();

        engine.dispatch_input(touch(1, PointerPhase::Down, 10.0));
        clock.set(0.6);
        engine.dispatch_input(touch(1, PointerPhase::Up, 10.0));
        wait_for_log(&mut engine, "tap:10");
    }

    /// A scrollable page: the 200×200 view scrolls a 1000px-tall child, and
    /// its `tap` listener logs `type:x` exactly as the gesture page does.
    const SCROLLING_GESTURE_PAGE: &str = r"
        globalThis.renderPage = function () {
          const page = __CreatePage('card', 0);
          const view = __CreateView(0);
          const filler = __CreateView(0);
          __AppendElement(page, view);
          __AppendElement(view, filler);
          globalThis.held = [page, view, filler];
          globalThis.entries = [];
          __SetInlineStyles(view, 'display:flex;overflow:scroll;width:200px;height:200px');
          __SetInlineStyles(filler, 'flex-shrink:0;width:200px;height:1000px');
          const note = (event) => {
            entries.push(event.type + ':' + event.detail.x);
            __SetAttribute(view, 'log', entries.join());
          };
          __AddEventListener(view, 'tap', note, {});
          __FlushElementTree();
        };
        ";

    /// A drag the user-agent scroll consumed is the claim that suppresses
    /// `tap` — end to end, through the real drag recognizer's
    /// real consumption rather than an injected flag. The drag travels
    /// 30px: past `dom`'s 8px drag slop so it scrolls, inside the 50px tap
    /// slop so the claim is the only suppressor. The fence tap at another x
    /// pins that the suppressed one never crossed the channel.
    #[test]
    fn a_scroll_consuming_drag_suppresses_the_tap() {
        let mut engine = booted(SCROLLING_GESTURE_PAGE);
        engine
            .elements
            .try_tree()
            .expect("the script thread is parked")
            .render();

        engine.dispatch_input(InputEvent::pointer(
            Point2D::new(100.0, 100.0),
            1,
            PointerKind::Touch,
            PointerPhase::Down,
        ));
        engine.dispatch_input(InputEvent::pointer(
            Point2D::new(100.0, 70.0),
            1,
            PointerKind::Touch,
            PointerPhase::Move,
        ));
        engine.dispatch_input(InputEvent::pointer(
            Point2D::new(100.0, 70.0),
            1,
            PointerKind::Touch,
            PointerPhase::Up,
        ));
        engine.dispatch_input(touch(1, PointerPhase::Down, 150.0));
        engine.dispatch_input(touch(1, PointerPhase::Up, 150.0));
        wait_for_log(&mut engine, "tap:150");

        // The router's scroll decision drove the document: 30px of travel
        // minus the 8px drag slop moved the scroller 22px.
        let offset = engine
            .elements
            .try_tree()
            .expect("the script thread is parked")
            .scroll_offset(node_id(3));
        assert!(
            (offset.y - 22.0).abs() < 0.5,
            "the drag scrolled the view, got {offset:?}"
        );
    }

    /// A wheel over scrollable content scrolls it (the router's decision,
    /// executed against the document) and dispatches `wheel` with its delta
    /// in the detail — in that order.
    #[test]
    fn a_wheel_scrolls_and_reaches_a_wheel_listener() {
        let page = SCROLLING_GESTURE_PAGE.replace(
            "__AddEventListener(view, 'tap', note, {});",
            "__AddEventListener(view, 'wheel', (event) => {
               entries.push(event.type + ':' + event.detail.deltaY);
               __SetAttribute(view, 'log', entries.join());
             }, {});",
        );
        let mut engine = booted(&page);
        engine
            .elements
            .try_tree()
            .expect("the script thread is parked")
            .render();

        engine.dispatch_input(InputEvent::wheel(
            Point2D::new(100.0, 100.0),
            dom::Vector2D::new(0.0, 30.0),
        ));
        wait_for_log(&mut engine, "wheel:30");
        let offset = engine
            .elements
            .try_tree()
            .expect("the script thread is parked")
            .scroll_offset(node_id(3));
        assert!(
            (offset.y - 30.0).abs() < 0.5,
            "the wheel scrolled the view, got {offset:?}"
        );
    }

    /// A stationary hold produces no further input, so only the frame half
    /// — `service_gesture_clock` plus the `needs_frame` continuation — can
    /// resolve it. This drives that half exactly as `notify_redraw`/`tick`
    /// do, without needing a GPU output.
    #[test]
    fn a_stationary_hold_longpresses_on_the_frame_clock() {
        let clock = Arc::new(crate::clock::ManualClock::new());
        let mut engine = booted_with(&gesture_page(true), Arc::clone(&clock));
        engine
            .elements
            .try_tree()
            .expect("the script thread is parked")
            .render();

        engine.dispatch_input(touch(1, PointerPhase::Down, 10.0));
        assert!(
            engine.gesture.needs_frame(),
            "the down arms a deadline, which is what keeps frames coming"
        );

        clock.set(0.6);
        {
            let mut tree = engine
                .elements
                .try_tree()
                .expect("the script thread is parked");
            OffscreenEngine::<Arc<crate::clock::ManualClock>>::service_gesture_clock(
                &mut engine.gesture,
                &engine.listener_names,
                &mut tree,
                engine.script_commands.as_ref(),
                engine.clock.now_seconds(),
            );
        }
        wait_for_log(&mut engine, "longpress:10");
        assert!(
            !engine.gesture.needs_frame(),
            "a resolved deadline stops asking for frames"
        );
    }

    /// Input buffered behind an open batch keeps its arrival time: a hold
    /// whose down and release both waited out a busy document still spans
    /// the deadline, so the drain delivers `longpress` first and suppresses
    /// the tap — drain-time stamping would deliver a plain tap instead.
    #[test]
    fn buffered_input_keeps_its_arrival_time_across_a_busy_batch() {
        let clock = Arc::new(crate::clock::ManualClock::new());
        let mut engine = booted_with(&gesture_page(true), Arc::clone(&clock));
        engine
            .elements
            .try_tree()
            .expect("the script thread is parked")
            .render();

        // Open the batch state by hand: the slot is empty, input buffers.
        let tree = engine.elements.take();
        engine.dispatch_input(touch(1, PointerPhase::Down, 10.0));
        clock.set(0.6);
        engine.dispatch_input(touch(1, PointerPhase::Up, 10.0));
        engine.elements.put(tree);

        // The next event drains the buffer under the returned document.
        clock.set(0.61);
        engine.dispatch_input(touch(1, PointerPhase::Down, 30.0));
        engine.dispatch_input(touch(1, PointerPhase::Up, 30.0));
        wait_for_log(&mut engine, "longpress:10,tap:30");
    }
}
